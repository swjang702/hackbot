// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_fpu.c — Float32 transformer forward pass using kernel FPU.
 *
 * Implements the SmolLM2-135M forward pass in float32 arithmetic,
 * wrapped in kernel_fpu_begin()/kernel_fpu_end() guards. This avoids
 * the Q16.16 fixed-point precision loss that causes incorrect generation
 * at >20 token sequences.
 *
 * Weight format: FP16 (format v2) — decoded to float32 on-the-fly.
 * Activations: float32 throughout.
 * KV cache: float32.
 *
 * Called from Rust via extern "C" FFI.
 */

#include <linux/kernel.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/math64.h>
#include <linux/random.h>
#include <linux/sched.h>
#include <asm/fpu/api.h>

#include "hackbot_fpu.h"

/* ===================================================================
 * Configuration constants
 * =================================================================== */

#define HACKBOT_MAX_SEQ     256
#define HACKBOT_MAX_LAYERS  64
#define HACKBOT_MAX_HEADS   32

/* RoPE base frequency (10000.0 for SmolLM2) */
#define ROPE_THETA 10000.0f

/* RMSNorm epsilon */
#define RMS_EPS 1e-5f

/*
 * Sampling parameters for token generation.
 *
 * Pure greedy (argmax) causes repetitive output on small FP16 models.
 * Temperature + top-k sampling breaks repetition while staying coherent.
 *
 * HACKBOT_TEMPERATURE: 0 = greedy (argmax), 70 = 0.70, 100 = 1.0
 *   Lower → more deterministic, higher → more creative.
 * HACKBOT_TOP_K: number of top tokens to consider (0 = full vocab).
 *   Higher → more diverse, lower → more focused.
 */
#define HACKBOT_TEMPERATURE   70    /* 0.70 — good balance for small models */
#define HACKBOT_TOP_K         40    /* consider top 40 tokens */

/*
 * FPU tile budget for the final logits matmul.
 *
 * The whole forward pass is decomposed into tiles, each bracketed by
 * kernel_fpu_begin()/kernel_fpu_end(). The largest single op in the
 * pass is the vocab×dim logits projection (e.g. 49152×576 ≈ 28 M MAC),
 * which would blow the per-tile latency budget if executed in one
 * window. We slice it into chunks of LOGITS_ROWS_PER_TILE output rows.
 *
 * 1024 rows × 576 cols ≈ 0.6 M MAC ≈ ~0.6 ms per tile on x86_64
 * software fp — safely under the ~1 ms target that keeps softirq
 * latency bounded.
 */
#define LOGITS_ROWS_PER_TILE  1024

/*
 * Sampler tile budget: number of vocab logits scanned per FPU window
 * inside hackbot_fpu_get_next_token(). The full top-K scan over
 * vocab_size logits (49152 for SmolLM2-135M) is otherwise a single
 * ~2 ms FPU window — same softirq-stall class as the un-tiled forward
 * pass fixed in R-007. 4096 logits × ~1 compare+conditional swap each
 * runs in ~50 µs on x86_64, comfortably under the ~1 ms target.
 */
#define LOGITS_SAMPLER_CHUNK  4096

/* ===================================================================
 * FP16 → float32 conversion (software, no SSE/AVX needed)
 * =================================================================== */

static inline float fp16_to_f32(u16 h)
{
	u32 sign = (h >> 15) & 1;
	u32 exp  = (h >> 10) & 0x1f;
	u32 mant = h & 0x3ff;
	u32 f;

	if (exp == 0) {
		if (mant == 0) {
			/* ±zero */
			f = sign << 31;
		} else {
			/* denormal → normalize */
			exp = 1;
			while (!(mant & 0x400)) {
				mant <<= 1;
				exp--;
			}
			mant &= 0x3ff;
			f = (sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13);
		}
	} else if (exp == 31) {
		/* inf / nan */
		f = (sign << 31) | (0xffu << 23) | (mant << 13);
	} else {
		/* normal */
		f = (sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13);
	}

	float result;
	memcpy(&result, &f, sizeof(float));
	return result;
}

/* ===================================================================
 * Inference state
 * =================================================================== */

struct hackbot_fpu_config {
	int dim;
	int hidden_dim;
	int n_layers;
	int n_heads;
	int n_kv_heads;
	int head_dim;
	int vocab_size;
	int max_seq;
	int heads_per_group;
};

/*
 * Weight offsets within the weight data blob (byte offsets from
 * the start of the weights section, after header + tokenizer).
 */
struct hackbot_layer_offsets {
	size_t rms_att;     /* [dim] float32 */
	size_t wq;          /* [n_heads*head_dim, dim] fp16 */
	size_t wk;          /* [n_kv_heads*head_dim, dim] fp16 */
	size_t wv;          /* [n_kv_heads*head_dim, dim] fp16 */
	size_t wo;          /* [dim, n_heads*head_dim] fp16 */
	size_t rms_ffn;     /* [dim] float32 */
	size_t gate;        /* [hidden_dim, dim] fp16 */
	size_t up;          /* [hidden_dim, dim] fp16 */
	size_t down;        /* [dim, hidden_dim] fp16 */
};

struct hackbot_fpu_state {
	struct hackbot_fpu_config cfg;

	/* Weight offsets */
	size_t embed_off;
	struct hackbot_layer_offsets layers[HACKBOT_MAX_LAYERS];
	size_t rms_final_off;

	/* KV cache: [n_layers][2][n_kv_heads][max_seq][head_dim] */
	float *kv_cache;
	size_t kv_cache_bytes;

	/* Activation buffers (all [max_dim] or similar) */
	float *x;           /* [dim] */
	float *xb;          /* [dim] */
	float *xb2;         /* [dim] */
	float *q_buf;       /* [dim] (= n_heads * head_dim) */
	float *k_buf;       /* [n_kv_heads * head_dim] */
	float *v_buf;       /* [n_kv_heads * head_dim] */
	float *att;         /* [max_seq] */
	float *hb;          /* [hidden_dim] */
	float *hb2;         /* [hidden_dim] */
	float *logits;      /* [vocab_size] */
};

/* ===================================================================
 * Math primitives (float32)
 * =================================================================== */

static float sqrtf_approx(float x)
{
	/* Newton's method for sqrt, starting from integer sqrt approximation */
	if (x <= 0.0f)
		return 0.0f;

	/* Initial guess using bit manipulation */
	u32 i;
	float y;
	memcpy(&i, &x, sizeof(u32));
	i = 0x1fbd1df5 + (i >> 1);  /* Quake-style initial guess */
	memcpy(&y, &i, sizeof(float));

	/* 3 Newton iterations for good precision */
	y = 0.5f * (y + x / y);
	y = 0.5f * (y + x / y);
	y = 0.5f * (y + x / y);

	return y;
}

static float expf_approx(float x)
{
	/* Clamp to prevent overflow/underflow */
	if (x > 88.0f) return 3.4028235e+38f;
	if (x < -88.0f) return 0.0f;

	/*
	 * Approximation: exp(x) = 2^(x/ln2) = 2^(n+f)
	 * where n = floor(x/ln2), f = x/ln2 - n
	 * 2^f ≈ polynomial approximation for f in [0, 1)
	 */
	float t = x * 1.4426950408889634f;  /* x / ln(2) */
	int n = (int)t;
	if (t < 0.0f && t != (float)n)
		n--;  /* floor for negative */
	float f = t - (float)n;

	/* 2^f approximation using minimax polynomial for f ∈ [0, 1) */
	float p = 1.0f + f * (0.6931471805599453f +
		f * (0.2402265069591007f +
		f * (0.0555041086648216f +
		f * (0.0096181291076285f +
		f * 0.0013333558146428f))));

	/* Multiply by 2^n via bit manipulation */
	u32 bits;
	memcpy(&bits, &p, sizeof(u32));
	bits += (u32)n << 23;
	float result;
	memcpy(&result, &bits, sizeof(float));

	return result;
}

static inline float sinf_approx(float x);
static inline float cosf_approx(float x);

/*
 * Fast sine approximation using Bhaskara I's formula + refinement.
 * Input range: any float. Output: sin(x) with ~1e-4 max error.
 */
static inline float sinf_approx(float x)
{
	/* Reduce to [0, 2π) */
	const float TWO_PI = 6.2831853071795864f;
	const float PI = 3.1415926535897932f;
	const float HALF_PI = 1.5707963267948966f;

	/* Modulo reduction */
	x = x - TWO_PI * (float)(int)(x / TWO_PI);
	if (x < 0.0f) x += TWO_PI;

	/* Use symmetry to reduce to [0, π/2] */
	int sign = 1;
	if (x > PI) { x -= PI; sign = -1; }
	if (x > HALF_PI) x = PI - x;

	/* Polynomial: sin(x) ≈ x - x³/6 + x⁵/120 - x⁷/5040 */
	float x2 = x * x;
	float x3 = x2 * x;
	float x5 = x3 * x2;
	float x7 = x5 * x2;
	float s = x - x3 / 6.0f + x5 / 120.0f - x7 / 5040.0f;

	return sign > 0 ? s : -s;
}

static inline float cosf_approx(float x)
{
	return sinf_approx(x + 1.5707963267948966f);
}

/* ===================================================================
 * Transformer operations
 * =================================================================== */

/*
 * Matrix-vector multiply: out = W × x
 * W is [rows × cols] stored as FP16, x is float32[cols], out is float32[rows]
 */
static void matmul_fp16(float *out, const float *x,
			const u8 *w_fp16, int rows, int cols)
{
	const u16 *w = (const u16 *)w_fp16;
	int r, c;

	for (r = 0; r < rows; r++) {
		float acc = 0.0f;
		const u16 *row = w + (size_t)r * cols;
		for (c = 0; c < cols; c++)
			acc += fp16_to_f32(row[c]) * x[c];
		out[r] = acc;
	}
}

/*
 * RMSNorm: out[i] = x[i] * weight[i] / RMS(x)
 * weight is float32[dim]
 */
static void rmsnorm_f32(float *out, const float *x,
			const u8 *weight_data, int dim)
{
	const float *w = (const float *)weight_data;
	float ss = 0.0f;
	int i;

	for (i = 0; i < dim; i++)
		ss += x[i] * x[i];

	float rms = sqrtf_approx(ss / (float)dim + RMS_EPS);
	float inv_rms = 1.0f / rms;

	for (i = 0; i < dim; i++)
		out[i] = x[i] * inv_rms * w[i];
}

/*
 * RoPE: rotate Q/K head vectors by positional angle.
 */
static void rope_f32(float *vec, int pos, int head_dim)
{
	int i;
	for (i = 0; i < head_dim / 2; i++) {
		float freq = 1.0f / expf_approx((float)(2 * i) / (float)head_dim
						* 9.2103403719761827f);
		/* 9.2103... = ln(10000) for rope_theta=10000 */
		float theta = (float)pos * freq;
		float cos_t = cosf_approx(theta);
		float sin_t = sinf_approx(theta);
		float v0 = vec[2 * i];
		float v1 = vec[2 * i + 1];
		vec[2 * i]     = v0 * cos_t - v1 * sin_t;
		vec[2 * i + 1] = v0 * sin_t + v1 * cos_t;
	}
}

/*
 * Softmax over x[0..len-1], in-place.
 */
static void softmax_f32(float *x, int len)
{
	float max_val, sum;
	int i;

	if (len <= 0) return;
	if (len == 1) { x[0] = 1.0f; return; }

	max_val = x[0];
	for (i = 1; i < len; i++)
		if (x[i] > max_val)
			max_val = x[i];

	sum = 0.0f;
	for (i = 0; i < len; i++) {
		x[i] = expf_approx(x[i] - max_val);
		sum += x[i];
	}

	if (sum > 0.0f) {
		float inv_sum = 1.0f / sum;
		for (i = 0; i < len; i++)
			x[i] *= inv_sum;
	}
}

/*
 * SiLU activation: silu(x) = x * sigmoid(x) = x / (1 + exp(-x))
 */
static inline float silu_f32(float x)
{
	return x / (1.0f + expf_approx(-x));
}

/* ===================================================================
 * Forward pass
 * =================================================================== */

/*
 * Run one transformer forward pass for `token_id` at sequence position
 * `pos`, decomposed into bounded FPU windows.
 *
 * Each `kernel_fpu_begin()/kernel_fpu_end()` pair brackets a "tile" of
 * float work whose runtime is empirically under ~1 ms on x86_64. The
 * outer caller (`hackbot_fpu_forward`) does NOT wrap this function in
 * an FPU window — every float access in the body lives inside one of
 * the per-tile windows opened here.
 *
 * Discipline (must hold for correctness):
 *   1. Between kernel_fpu_end() and the next kernel_fpu_begin(), this
 *      function performs only integer/pointer work. No float loads,
 *      stores, or arithmetic. Activations live in heap-backed buffers
 *      (st->x, st->xb, ...) and are reloaded by the next tile's
 *      helpers from memory.
 *   2. None of the math helpers (matmul_fp16, rmsnorm_f32, rope_f32,
 *      softmax_f32, silu_f32, fp16_to_f32) call kernel_fpu_begin/end
 *      themselves — they just assume the caller has opened a window.
 *   3. cond_resched() is invoked only OUTSIDE FPU windows, since it
 *      can sleep and kernel_fpu_end() has already re-enabled BH.
 */
static void forward_token_impl(struct hackbot_fpu_state *st,
				const u8 *weights, int token_id, int pos)
{
	struct hackbot_fpu_config *c = &st->cfg;
	int dim = c->dim;
	int hidden_dim = c->hidden_dim;
	int n_heads = c->n_heads;
	int n_kv_heads = c->n_kv_heads;
	int head_dim = c->head_dim;
	int kv_dim = n_kv_heads * head_dim;
	int hpg = c->heads_per_group;
	int vocab_size = c->vocab_size;
	int l, h, p, d;
	int row_off, chunk;

	float *x = st->x;
	float *xb = st->xb;
	float *xb2 = st->xb2;
	float *q_buf = st->q_buf;
	float *k_buf = st->k_buf;
	float *v_buf = st->v_buf;
	float *att_buf = st->att;
	float *hb = st->hb;
	float *hb2 = st->hb2;
	float *logits_buf = st->logits;

	/* KV cache strides */
	size_t kv_head_stride  = (size_t)HACKBOT_MAX_SEQ * head_dim;
	size_t kv_type_stride  = (size_t)n_kv_heads * kv_head_stride;
	size_t kv_layer_stride = 2 * kv_type_stride;
	float *kv = st->kv_cache;

	/* === Tile T0: Embedding lookup === */
	kernel_fpu_begin();
	{
		const u16 *embed = (const u16 *)(weights + st->embed_off);
		const u16 *row = embed + (size_t)token_id * dim;
		for (d = 0; d < dim; d++)
			x[d] = fp16_to_f32(row[d]);
	}
	kernel_fpu_end();

	/* === Transformer layers === */
	for (l = 0; l < c->n_layers; l++) {
		struct hackbot_layer_offsets *lo = &st->layers[l];

		/* --- Tile T1: pre-attn RMSNorm + QKV projections --- */
		kernel_fpu_begin();
		rmsnorm_f32(xb, x, weights + lo->rms_att, dim);
		matmul_fp16(q_buf, xb, weights + lo->wq, dim, dim);
		matmul_fp16(k_buf, xb, weights + lo->wk, kv_dim, dim);
		matmul_fp16(v_buf, xb, weights + lo->wv, kv_dim, dim);
		kernel_fpu_end();

		/*
		 * --- Tile T2: RoPE + KV-cache store + attention + wo +
		 *               residual ---
		 *
		 * KV-store walks float values through pointer dereferences
		 * (kv[k_off + d] = k_buf[...]) — those are float loads/stores
		 * and therefore must remain inside the FPU window, even
		 * though the operation is conceptually a memcpy. Keep it
		 * grouped with the attention math.
		 */
		kernel_fpu_begin();
		for (h = 0; h < n_heads; h++)
			rope_f32(q_buf + h * head_dim, pos, head_dim);
		for (h = 0; h < n_kv_heads; h++)
			rope_f32(k_buf + h * head_dim, pos, head_dim);
		{
			size_t base = (size_t)l * kv_layer_stride;
			for (h = 0; h < n_kv_heads; h++) {
				size_t k_off = base + (size_t)h * kv_head_stride
					       + (size_t)pos * head_dim;
				size_t v_off = base + kv_type_stride
					       + (size_t)h * kv_head_stride
					       + (size_t)pos * head_dim;
				for (d = 0; d < head_dim; d++) {
					kv[k_off + d] = k_buf[h * head_dim + d];
					kv[v_off + d] = v_buf[h * head_dim + d];
				}
			}
		}
		{
			float inv_sqrt_hd = 1.0f / sqrtf_approx((float)head_dim);
			size_t base = (size_t)l * kv_layer_stride;

			for (h = 0; h < n_heads; h++) {
				int kv_group = h / hpg;
				float *q_head = q_buf + h * head_dim;

				/* Attention scores */
				for (p = 0; p <= pos; p++) {
					size_t k_off = base
						+ (size_t)kv_group * kv_head_stride
						+ (size_t)p * head_dim;
					float dot = 0.0f;
					for (d = 0; d < head_dim; d++)
						dot += q_head[d] * kv[k_off + d];
					att_buf[p] = dot * inv_sqrt_hd;
				}

				/* Softmax */
				softmax_f32(att_buf, pos + 1);

				/* Weighted V sum */
				{
					size_t v_base = base + kv_type_stride
						+ (size_t)kv_group * kv_head_stride;
					for (d = 0; d < head_dim; d++) {
						float acc = 0.0f;
						for (p = 0; p <= pos; p++)
							acc += att_buf[p]
							     * kv[v_base + (size_t)p * head_dim + d];
						xb[h * head_dim + d] = acc;
					}
				}
			}
		}
		matmul_fp16(xb2, xb, weights + lo->wo, dim, dim);
		for (d = 0; d < dim; d++)
			x[d] += xb2[d];
		kernel_fpu_end();

		/* --- Tile T3: pre-FFN RMSNorm + FFN gate matmul --- */
		kernel_fpu_begin();
		rmsnorm_f32(xb, x, weights + lo->rms_ffn, dim);
		matmul_fp16(hb, xb, weights + lo->gate, hidden_dim, dim);
		kernel_fpu_end();

		/* --- Tile T4: FFN up + SwiGLU fusion --- */
		kernel_fpu_begin();
		matmul_fp16(hb2, xb, weights + lo->up, hidden_dim, dim);
		for (d = 0; d < hidden_dim; d++)
			hb[d] = silu_f32(hb[d]) * hb2[d];
		kernel_fpu_end();

		/* --- Tile T5: FFN down + residual --- */
		kernel_fpu_begin();
		matmul_fp16(xb2, hb, weights + lo->down, dim, hidden_dim);
		for (d = 0; d < dim; d++)
			x[d] += xb2[d];
		kernel_fpu_end();

		cond_resched();
	}

	/* === Tile T6: Final RMSNorm === */
	kernel_fpu_begin();
	rmsnorm_f32(xb, x, weights + st->rms_final_off, dim);
	kernel_fpu_end();

	/*
	 * === Tiles T7..: Logits matmul, sliced by output rows. ===
	 *
	 * We reuse matmul_fp16 with a row-window: each call computes
	 * `chunk` output rows starting at row index `row_off`. The
	 * weight pointer advances by row_off * dim * sizeof(u16).
	 */
	for (row_off = 0; row_off < vocab_size; row_off += LOGITS_ROWS_PER_TILE) {
		chunk = vocab_size - row_off;
		if (chunk > LOGITS_ROWS_PER_TILE)
			chunk = LOGITS_ROWS_PER_TILE;

		kernel_fpu_begin();
		matmul_fp16(logits_buf + row_off, xb,
			    weights + st->embed_off
			    + (size_t)row_off * dim * sizeof(u16),
			    chunk, dim);
		kernel_fpu_end();

		cond_resched();
	}
}

/* ===================================================================
 * Public API (called from Rust)
 * =================================================================== */

/*
 * Allocate inference state.
 * Returns pointer to state, or NULL on failure.
 */
void *hackbot_fpu_alloc(int dim, int hidden_dim, int n_layers,
			int n_heads, int n_kv_heads, int head_dim,
			int vocab_size, int max_seq)
{
	struct hackbot_fpu_state *st;
	int kv_dim;
	size_t kv_size, buf_size;
	u8 *buf;

	if (n_layers > HACKBOT_MAX_LAYERS || n_heads > HACKBOT_MAX_HEADS)
		return NULL;
	if (max_seq > HACKBOT_MAX_SEQ)
		max_seq = HACKBOT_MAX_SEQ;

	st = kzalloc(sizeof(*st), GFP_KERNEL);
	if (!st)
		return NULL;

	st->cfg.dim = dim;
	st->cfg.hidden_dim = hidden_dim;
	st->cfg.n_layers = n_layers;
	st->cfg.n_heads = n_heads;
	st->cfg.n_kv_heads = n_kv_heads;
	st->cfg.head_dim = head_dim;
	st->cfg.vocab_size = vocab_size;
	st->cfg.max_seq = max_seq;
	st->cfg.heads_per_group = n_heads / n_kv_heads;

	kv_dim = n_kv_heads * head_dim;

	/* KV cache */
	kv_size = (size_t)n_layers * 2 * n_kv_heads * max_seq * head_dim
		  * sizeof(float);
	st->kv_cache = kvmalloc(kv_size, GFP_KERNEL);
	if (!st->kv_cache)
		goto fail;
	memset(st->kv_cache, 0, kv_size);
	st->kv_cache_bytes = kv_size;

	/*
	 * Activation buffers: allocate as one contiguous block.
	 * Sizes: x(dim) + xb(dim) + xb2(dim) + q(dim) + k(kv_dim)
	 *      + v(kv_dim) + att(max_seq) + hb(hidden_dim) + hb2(hidden_dim)
	 *      + logits(vocab_size)
	 */
	buf_size = ((size_t)dim * 4 + (size_t)kv_dim * 2 + max_seq
		    + (size_t)hidden_dim * 2 + vocab_size) * sizeof(float);
	buf = kvmalloc(buf_size, GFP_KERNEL);
	if (!buf)
		goto fail_kv;

	{
		float *p = (float *)buf;
		st->x      = p; p += dim;
		st->xb     = p; p += dim;
		st->xb2    = p; p += dim;
		st->q_buf  = p; p += dim;
		st->k_buf  = p; p += kv_dim;
		st->v_buf  = p; p += kv_dim;
		st->att    = p; p += max_seq;
		st->hb     = p; p += hidden_dim;
		st->hb2    = p; p += hidden_dim;
		st->logits = p;
	}

	/* Compute weight offsets for format v2 */
	{
		size_t off = 0;
		int l;

		st->embed_off = off;
		off += (size_t)vocab_size * dim * 2; /* fp16 */

		for (l = 0; l < n_layers; l++) {
			st->layers[l].rms_att = off;
			off += (size_t)dim * 4; /* float32 */

			st->layers[l].wq = off;
			off += (size_t)(n_heads * head_dim) * dim * 2;

			st->layers[l].wk = off;
			off += (size_t)(n_kv_heads * head_dim) * dim * 2;

			st->layers[l].wv = off;
			off += (size_t)(n_kv_heads * head_dim) * dim * 2;

			st->layers[l].wo = off;
			off += (size_t)dim * (n_heads * head_dim) * 2;

			st->layers[l].rms_ffn = off;
			off += (size_t)dim * 4;

			st->layers[l].gate = off;
			off += (size_t)hidden_dim * dim * 2;

			st->layers[l].up = off;
			off += (size_t)hidden_dim * dim * 2;

			st->layers[l].down = off;
			off += (size_t)dim * hidden_dim * 2;
		}

		st->rms_final_off = off;
	}

	pr_info("hackbot_fpu: allocated state (%zu KB KV, %zu KB act)\n",
		kv_size / 1024, buf_size / 1024);

	return st;

fail_kv:
	kvfree(st->kv_cache);
fail:
	kfree(st);
	return NULL;
}

void hackbot_fpu_free(void *state)
{
	struct hackbot_fpu_state *st = state;
	if (!st)
		return;

	/* Free activation buffer (single allocation starting at x) */
	if (st->x)
		kvfree(st->x);
	if (st->kv_cache)
		kvfree(st->kv_cache);
	kfree(st);
}

void hackbot_fpu_reset(void *state)
{
	struct hackbot_fpu_state *st = state;
	if (st && st->kv_cache)
		memset(st->kv_cache, 0, st->kv_cache_bytes);
}

/*
 * Run one token through the transformer.
 * weights: pointer to start of weight data (after header + tokenizer)
 * weights_len: length of weight data in bytes
 *
 * Must be called from process context (sleepable).
 * Returns 0 on success.
 *
 * NOTE: FPU bracketing (kernel_fpu_begin/end) lives INSIDE
 * forward_token_impl, split into per-tile windows of bounded latency.
 * Wrapping the entire forward pass in a single FPU window held
 * local_bh_disable for ~100 ms per token, causing visible system
 * stutter; the tiled design keeps each window under ~1 ms.
 */
int hackbot_fpu_forward(void *state, const void *weights,
			size_t weights_len, int token_id, int pos)
{
	struct hackbot_fpu_state *st = state;

	if (!st || !weights)
		return -1;
	if (token_id < 0 || token_id >= st->cfg.vocab_size)
		return -2;
	if (pos < 0 || pos >= st->cfg.max_seq)
		return -3;

	forward_token_impl(st, (const u8 *)weights, token_id, pos);

	return 0;
}

/*
 * Sample the next token from the logits buffer using temperature + top-k.
 *
 * If HACKBOT_TEMPERATURE == 0: pure greedy (argmax).
 * Otherwise: apply temperature scaling, find top-K candidates, compute
 * softmax over them, and sample from the distribution.
 *
 * Returns the selected token ID.
 *
 * Tile the vocab-wide scan into LOGITS_SAMPLER_CHUNK-sized FPU windows.
 *
 * Discipline (mirrors forward_token_impl, R-007):
 *   - All float reads / writes (logits[], top_vals[], best_val) happen
 *     INSIDE a kernel_fpu_begin()/kernel_fpu_end() pair.
 *   - Integer state (top_ids[], n_top, min_idx, result) survives across
 *     tiles on the stack.
 *   - cond_resched() is invoked only outside FPU windows.
 *
 * The compile-time switch HACKBOT_TEMPERATURE == 0 selects a greedy
 * tile body (argmax); otherwise it selects a top-K maintenance body.
 * Final softmax + sample is one small additional tile.
 */
int hackbot_fpu_get_next_token(const void *state)
{
	const struct hackbot_fpu_state *st = state;
	int result = 0;
	int row_off, chunk, chunk_end;

	if (!st || !st->logits)
		return 0;

#if HACKBOT_TEMPERATURE == 0
	{
		const int vocab_size = st->cfg.vocab_size;
		const float *logits = st->logits;
		float best_val;

		/* Seed best_val from logits[0] in a tiny FPU tile. */
		kernel_fpu_begin();
		best_val = logits[0];
		kernel_fpu_end();

		for (row_off = 1; row_off < vocab_size;
		     row_off += LOGITS_SAMPLER_CHUNK) {
			chunk_end = row_off + LOGITS_SAMPLER_CHUNK;
			if (chunk_end > vocab_size)
				chunk_end = vocab_size;

			kernel_fpu_begin();
			{
				int i;
				for (i = row_off; i < chunk_end; i++) {
					if (logits[i] > best_val) {
						best_val = logits[i];
						result = i;
					}
				}
			}
			kernel_fpu_end();
			cond_resched();
		}
	}
#else
	{
		const float temperature = (float)HACKBOT_TEMPERATURE / 100.0f;
		const int vocab_size = st->cfg.vocab_size;
		const int top_k = (HACKBOT_TOP_K > 0 && HACKBOT_TOP_K < vocab_size)
				  ? HACKBOT_TOP_K : vocab_size;
		const float *logits = st->logits;

		/* Top-K candidates: (token_id, logit_value).
		 * top_ids/n_top/min_idx are integer state — survive across tiles.
		 * top_vals[] is float and is only read/written inside FPU windows. */
		int   top_ids[HACKBOT_TOP_K];
		float top_vals[HACKBOT_TOP_K];
		int   n_top = 0;
		int   min_idx = 0;

		/*
		 * Step 1: tiled top-K scan over vocab_size logits.
		 *
		 * Each chunk maintains the running top-K under a single FPU
		 * window. The min-of-top-K linear search inside the
		 * replacement branch stays bounded by HACKBOT_TOP_K compares,
		 * which is dwarfed by the chunk's LOGITS_SAMPLER_CHUNK
		 * sequential scan.
		 */
		for (row_off = 0; row_off < vocab_size;
		     row_off += LOGITS_SAMPLER_CHUNK) {
			chunk = LOGITS_SAMPLER_CHUNK;
			chunk_end = row_off + chunk;
			if (chunk_end > vocab_size)
				chunk_end = vocab_size;

			kernel_fpu_begin();
			{
				int i, j;
				for (i = row_off; i < chunk_end; i++) {
					if (n_top < top_k) {
						top_ids[n_top] = i;
						top_vals[n_top] = logits[i];
						if (n_top == 0 ||
						    logits[i] < top_vals[min_idx])
							min_idx = n_top;
						n_top++;
					} else if (logits[i] > top_vals[min_idx]) {
						top_ids[min_idx] = i;
						top_vals[min_idx] = logits[i];
						min_idx = 0;
						for (j = 1; j < n_top; j++) {
							if (top_vals[j] < top_vals[min_idx])
								min_idx = j;
						}
					}
				}
			}
			kernel_fpu_end();
			cond_resched();
		}

		/*
		 * Step 2 + 3: softmax over top-K + cumulative sample, in one
		 * small final FPU window. n_top <= HACKBOT_TOP_K (40), so the
		 * total work is a few hundred FLOPs — well under one tile
		 * budget.
		 *
		 * get_random_u32() takes its own internal locks but is safe
		 * to call inside an FPU window: it doesn't sleep and doesn't
		 * itself use the FPU.
		 */
		kernel_fpu_begin();
		{
			float max_val, sum_exp, cumul, r;
			u32   rand_val;
			int   i;

			max_val = top_vals[0];
			for (i = 1; i < n_top; i++) {
				if (top_vals[i] > max_val)
					max_val = top_vals[i];
			}

			sum_exp = 0.0f;
			for (i = 0; i < n_top; i++) {
				top_vals[i] = expf_approx(
					(top_vals[i] - max_val) / temperature);
				sum_exp += top_vals[i];
			}

			if (sum_exp > 0.0f) {
				for (i = 0; i < n_top; i++)
					top_vals[i] /= sum_exp;
			} else {
				/* Degenerate: uniform distribution */
				for (i = 0; i < n_top; i++)
					top_vals[i] = 1.0f / (float)n_top;
			}

			rand_val = get_random_u32();
			/* Uniform in [0, 1) with 24-bit precision */
			r = (float)(rand_val >> 8) / 16777216.0f;

			cumul = 0.0f;
			result = top_ids[0]; /* fallback */
			for (i = 0; i < n_top; i++) {
				cumul += top_vals[i];
				if (r < cumul) {
					result = top_ids[i];
					break;
				}
			}
		}
		kernel_fpu_end();
	}
#endif

	return result;
}
