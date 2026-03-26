#!/usr/bin/env python3
"""
verify_prefill.py — Run full 57-token ChatML prefill using Python INT8 reference
and compare with kernel output + float32 reference.

This diagnoses why multi-token prefill diverges from single-token correctness.
Uses numpy-vectorized matmul for speed.

Usage:
  python3 verify_prefill.py [hackbot-model.bin]
"""

import struct
import sys
import time
from pathlib import Path
import numpy as np

Q16_SHIFT = 16
Q16_ONE = 1 << 16
MODEL_MAGIC = 0x484B4254
INFERENCE_MAX_SEQ = 256

ROPE_FREQS_64 = [
    65536, 49145, 36854, 27636, 20724, 15541, 11654, 8739,
     6554,  4915,  3685,  2764,  2072,  1554,  1165,  874,
      655,   491,   369,   276,   207,   155,   117,   87,
       66,    49,    37,    28,    21,    16,    12,     9,
]

TWO_PI_Q16 = 411775

EXP_TABLE = [65536, 24109, 8869, 3263, 1200, 442, 162, 60, 22, 8, 3, 1, 0, 0, 0, 0, 0]

SIN_TABLE = [
        0,  1608,  3216,  4821,  6424,  8022,  9616, 11204, 12785, 14359,
    15924, 17479, 19024, 20557, 22078, 23586, 25080, 26558, 28020, 29466,
    30893, 32303, 33692, 35062, 36410, 37736, 39040, 40320, 41576, 42806,
    44011, 45190, 46341, 47464, 48559, 49624, 50660, 51665, 52639, 53581,
    54491, 55368, 56212, 57022, 57798, 58538, 59244, 59914, 60547, 61145,
    61705, 62228, 62714, 63162, 63572, 63944, 64277, 64571, 64827, 65043,
    65220, 65358, 65457, 65516, 65536, 65516, 65457, 65358, 65220, 65043,
    64827, 64571, 64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
    60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368, 54491, 53581,
    52639, 51665, 50660, 49624, 48559, 47464, 46341, 45190, 44011, 42806,
    41576, 40320, 39040, 37736, 36410, 35062, 33692, 32303, 30893, 29466,
    28020, 26558, 25080, 23586, 22078, 20557, 19024, 17479, 15924, 14359,
    12785, 11204,  9616,  8022,  6424,  4821,  3216,  1608,     0, -1608,
    -3216, -4821, -6424, -8022, -9616,-11204,-12785,-14359,-15924,-17479,
   -19024,-20557,-22078,-23586,-25080,-26558,-28020,-29466,-30893,-32303,
   -33692,-35062,-36410,-37736,-39040,-40320,-41576,-42806,-44011,-45190,
   -46341,-47464,-48559,-49624,-50660,-51665,-52639,-53581,-54491,-55368,
   -56212,-57022,-57798,-58538,-59244,-59914,-60547,-61145,-61705,-62228,
   -62714,-63162,-63572,-63944,-64277,-64571,-64827,-65043,-65220,-65358,
   -65457,-65516,-65536,-65516,-65457,-65358,-65220,-65043,-64827,-64571,
   -64277,-63944,-63572,-63162,-62714,-62228,-61705,-61145,-60547,-59914,
   -59244,-58538,-57798,-57022,-56212,-55368,-54491,-53581,-52639,-51665,
   -50660,-49624,-48559,-47464,-46341,-45190,-44011,-42806,-41576,-40320,
   -39040,-37736,-36410,-35062,-33692,-32303,-30893,-29466,-28020,-26558,
   -25080,-23586,-22078,-20557,-19024,-17479,-15924,-14359,-12785,-11204,
    -9616, -8022, -6424, -4821, -3216, -1608,
]


def sin_q16(angle):
    two_pi = TWO_PI_Q16
    a = int(angle) % two_pi
    if a < 0:
        a += two_pi
    idx_fixed = (a << 8) // two_pi
    idx = int(idx_fixed) & 0xFF
    frac = (a << 8) - idx_fixed * two_pi
    s0 = SIN_TABLE[idx % 256]
    s1 = SIN_TABLE[(idx + 1) % 256]
    interp = s0 + ((s1 - s0) * frac) // two_pi
    return int(interp)


def cos_q16(angle):
    return sin_q16(int(angle) + 102944)


def isqrt_u64(n):
    if n < 2:
        return n
    n = int(n)
    bits = n.bit_length()
    x = 1 << ((bits + 1) // 2)
    while True:
        y = (x + n // x) // 2
        if y >= x:
            return int(x)
        x = y


def exp_q16_neg(x):
    x = int(x)
    if x >= 0:
        return Q16_ONE
    x_int = x >> 16
    idx = -x_int
    if idx >= len(EXP_TABLE):
        return 0
    exp_int = EXP_TABLE[idx]
    x_frac = x - (x_int << 16)
    f = x_frac
    f2 = (f * f) >> 16
    f3 = (f2 * f) >> 16
    exp_frac = Q16_ONE + f + (f2 >> 1) + f3 // 6
    return (exp_int * exp_frac) >> 16


class FastHackbotModel:
    """INT8 model with numpy-vectorized matmul for fast prefill."""

    def __init__(self, path):
        self.raw = Path(path).read_bytes()
        self._parse()

    def _parse(self):
        d = self.raw
        header = struct.unpack_from("<14I", d, 0)
        magic, version, dim, hidden_dim, n_layers, n_heads, n_kv_heads, \
            vocab_size, seq_len, group_size, head_dim, kv_dim, rope_theta, pad = header
        assert magic == MODEL_MAGIC
        self.dim = dim
        self.hidden_dim = hidden_dim
        self.n_layers = n_layers
        self.n_heads = n_heads
        self.n_kv_heads = n_kv_heads
        self.vocab_size = vocab_size
        self.group_size = group_size
        self.head_dim = head_dim
        self.kv_dim = kv_dim
        self.heads_per_group = n_heads // n_kv_heads

        # Parse tokenizer
        off = 56
        tok_vocab_size, max_token_len = struct.unpack_from("<II", d, off)
        off += 8
        self.tok_bytes = []
        for i in range(tok_vocab_size):
            off += 4  # skip score
            tlen = struct.unpack_from("<H", d, off)[0]
            off += 2
            self.tok_bytes.append(d[off:off+tlen])
            off += tlen

        # Parse weight offsets
        gs = group_size
        def q8_size(rows, cols):
            return rows * cols + rows * (cols // gs) * 4

        self.embed_off = off
        off += q8_size(vocab_size, dim)

        self.layer_offsets = []
        for l in range(n_layers):
            layer = {}
            layer['rms_att_off'] = off; off += dim * 4
            layer['wq_off'] = off; off += q8_size(n_heads * head_dim, dim)
            layer['wk_off'] = off; off += q8_size(n_kv_heads * head_dim, dim)
            layer['wv_off'] = off; off += q8_size(n_kv_heads * head_dim, dim)
            layer['wo_off'] = off; off += q8_size(dim, n_heads * head_dim)
            layer['rms_ffn_off'] = off; off += dim * 4
            layer['gate_off'] = off; off += q8_size(hidden_dim, dim)
            layer['up_off'] = off; off += q8_size(hidden_dim, dim)
            layer['down_off'] = off; off += q8_size(dim, hidden_dim)
            self.layer_offsets.append(layer)

        self.rms_final_off = off

        # Pre-load weight matrices as numpy arrays for fast matmul
        self._preload_weights()

        # KV cache
        self.kv_cache = np.zeros(
            (n_layers, 2, n_kv_heads, INFERENCE_MAX_SEQ, head_dim), dtype=np.int64
        )

    def _load_q8_matrix(self, offset, rows, cols):
        """Load INT8 weight matrix and scales as numpy arrays."""
        gs = self.group_size
        n_groups = cols // gs
        d = self.raw

        # INT8 data
        w_i8 = np.frombuffer(d, dtype=np.int8, count=rows * cols, offset=offset)
        w_i8 = w_i8.reshape(rows, cols)

        # Scales: [rows * n_groups] i32, little-endian
        scale_off = offset + rows * cols
        scales = np.frombuffer(d, dtype=np.int32, count=rows * n_groups, offset=scale_off)
        scales = scales.reshape(rows, n_groups)

        return w_i8, scales

    def _preload_weights(self):
        """Pre-load all weight matrices for fast numpy matmul."""
        dim = self.dim
        hd = self.hidden_dim
        n_h = self.n_heads
        n_kv = self.n_kv_heads
        head_d = self.head_dim

        # Embedding
        self.embed_w, self.embed_s = self._load_q8_matrix(
            self.embed_off, self.vocab_size, dim)

        # RMS norms and layer weights
        self.rms_att_w = []
        self.rms_ffn_w = []
        self.wq = []
        self.wk = []
        self.wv = []
        self.wo = []
        self.gate = []
        self.up = []
        self.down = []

        for l in range(self.n_layers):
            lo = self.layer_offsets[l]
            # RMS norms (Q16.16 i32 arrays)
            self.rms_att_w.append(
                np.frombuffer(self.raw, dtype=np.int32, count=dim,
                              offset=lo['rms_att_off']))
            self.rms_ffn_w.append(
                np.frombuffer(self.raw, dtype=np.int32, count=dim,
                              offset=lo['rms_ffn_off']))
            # Weight matrices
            self.wq.append(self._load_q8_matrix(lo['wq_off'], n_h * head_d, dim))
            self.wk.append(self._load_q8_matrix(lo['wk_off'], n_kv * head_d, dim))
            self.wv.append(self._load_q8_matrix(lo['wv_off'], n_kv * head_d, dim))
            self.wo.append(self._load_q8_matrix(lo['wo_off'], dim, n_h * head_d))
            self.gate.append(self._load_q8_matrix(lo['gate_off'], hd, dim))
            self.up.append(self._load_q8_matrix(lo['up_off'], hd, dim))
            self.down.append(self._load_q8_matrix(lo['down_off'], dim, hd))

        # Final RMS norm
        self.rms_final_w = np.frombuffer(
            self.raw, dtype=np.int32, count=dim, offset=self.rms_final_off)

    def matmul_q8_fast(self, x_q16, w_i8, scales, rows, cols):
        """Vectorized INT8 × Q16.16 matmul."""
        gs = self.group_size
        n_groups = cols // gs

        # x_q16 is 1D array of int64/int32 with length cols
        x = x_q16.astype(np.int64).reshape(1, cols)

        # w_i8 is [rows, cols] int8
        # scales is [rows, n_groups] int32

        # Reshape for group-wise computation
        x_grouped = x.reshape(1, n_groups, gs)          # [1, n_groups, gs]
        w_grouped = w_i8.reshape(rows, n_groups, gs)     # [rows, n_groups, gs]

        # Group dot products: for each row and group, compute sum(w * x)
        # Use int64 to avoid overflow
        group_dots = np.sum(
            w_grouped.astype(np.int64) * x_grouped.astype(np.int64),
            axis=2
        )  # [rows, n_groups]

        # Multiply by scales and shift
        scaled = (group_dots * scales.astype(np.int64)) >> 16  # [rows, n_groups]

        # Sum across groups
        out = np.sum(scaled, axis=1)  # [rows]

        return out.astype(np.int32)

    def rmsnorm_fast(self, x_q16, weights):
        """Vectorized RMSNorm in Q16.16."""
        x = x_q16.astype(np.int64)
        ss = np.sum(x * x, dtype=np.uint64)
        mean_sq = int(ss) // len(x)
        mean_sq_eps = mean_sq + 42950
        rms = isqrt_u64(mean_sq_eps)
        if rms == 0:
            return np.zeros_like(x)
        rsqrt = (1 << 32) // rms

        x_norm = (x * rsqrt) >> 16
        w = weights.astype(np.int64)
        out = (x_norm * w) >> 16
        return out.astype(np.int32)

    def rope_apply(self, vec, pos, head_dim):
        """RoPE matching kernel code."""
        out = vec.copy()
        n_pairs = head_dim // 2
        for i in range(n_pairs):
            freq = ROPE_FREQS_64[i] if i < len(ROPE_FREQS_64) else 0
            theta = (pos * freq) % TWO_PI_Q16
            cv = cos_q16(theta)
            sv = sin_q16(theta)
            v0 = int(vec[2*i])
            v1 = int(vec[2*i+1])
            out[2*i] = (v0 * cv - v1 * sv) >> 16
            out[2*i+1] = (v0 * sv + v1 * cv) >> 16
        return out

    def softmax_q16(self, att, length):
        """Softmax in Q16.16."""
        if length <= 1:
            return np.array([Q16_ONE], dtype=np.int64)
        vals = att[:length].copy().astype(np.int64)
        max_val = int(np.max(vals))

        total = 0
        for i in range(length):
            e = exp_q16_neg(int(vals[i]) - max_val)
            vals[i] = e
            total += e

        if total == 0:
            vals[0] = Q16_ONE
            return vals

        for i in range(length):
            vals[i] = (int(vals[i]) * Q16_ONE) // total

        return vals

    def sigmoid_q16(self, x):
        x = int(x)
        if x >= 0:
            e = exp_q16_neg(-x)
            return (Q16_ONE * Q16_ONE) // (Q16_ONE + e)
        else:
            e = exp_q16_neg(x)
            return (e * Q16_ONE) // (Q16_ONE + e)

    def silu_q16_vec(self, vec):
        """Vectorized SiLU (element-wise, but Python loop for exp)."""
        out = np.zeros_like(vec, dtype=np.int64)
        for i in range(len(vec)):
            xi = int(vec[i])
            sig = self.sigmoid_q16(xi)
            out[i] = (xi * sig) >> 16
        return out.astype(np.int32)

    def forward_token(self, token_id, pos, verbose=False):
        """Full forward pass, using vectorized matmul."""
        dim = self.dim
        hidden_dim = self.hidden_dim
        n_heads = self.n_heads
        n_kv_heads = self.n_kv_heads
        head_dim = self.head_dim
        gs = self.group_size
        hpg = self.heads_per_group

        # Embedding lookup (vectorized)
        n_groups = dim // gs
        w_row = self.embed_w[token_id]  # [dim] int8
        s_row = self.embed_s[token_id]  # [n_groups] int32

        # Dequantize embedding: x[c] = w[c] * scale[g]
        w_grouped = w_row.reshape(n_groups, gs).astype(np.int64)
        s_expanded = s_row.astype(np.int64).reshape(n_groups, 1)
        x = (w_grouped * s_expanded).reshape(dim).astype(np.int32)

        if verbose and pos == 0:
            print(f"  embed[{token_id}]: x[0..4] = {x[:4].tolist()}")

        # Transformer layers
        for l in range(self.n_layers):
            # Pre-attention RMSNorm
            xb = self.rmsnorm_fast(x, self.rms_att_w[l])

            # QKV projections
            q_buf = self.matmul_q8_fast(xb, *self.wq[l], n_heads * head_dim, dim)
            k_buf = self.matmul_q8_fast(xb, *self.wk[l], n_kv_heads * head_dim, dim)
            v_buf = self.matmul_q8_fast(xb, *self.wv[l], n_kv_heads * head_dim, dim)

            # RoPE
            for h in range(n_heads):
                s = h * head_dim
                q_head = q_buf[s:s+head_dim].astype(np.int64)
                q_buf[s:s+head_dim] = self.rope_apply(q_head, pos, head_dim).astype(np.int32)
            for h in range(n_kv_heads):
                s = h * head_dim
                k_head = k_buf[s:s+head_dim].astype(np.int64)
                k_buf[s:s+head_dim] = self.rope_apply(k_head, pos, head_dim).astype(np.int32)

            # Store K,V in cache
            for h in range(n_kv_heads):
                self.kv_cache[l, 0, h, pos, :] = k_buf[h*head_dim:(h+1)*head_dim]
                self.kv_cache[l, 1, h, pos, :] = v_buf[h*head_dim:(h+1)*head_dim]

            # Multi-head attention with GQA
            xb_out = np.zeros(dim, dtype=np.int64)
            for h in range(n_heads):
                kv_group = h // hpg
                q_head = q_buf[h*head_dim:(h+1)*head_dim].astype(np.int64)

                # Attention scores
                att = np.zeros(INFERENCE_MAX_SEQ, dtype=np.int64)
                for p in range(pos + 1):
                    k_cached = self.kv_cache[l, 0, kv_group, p, :]
                    dot = np.sum(q_head * k_cached)
                    att[p] = int(dot) >> 19

                # Softmax
                att_sm = self.softmax_q16(att.astype(np.int32), pos + 1)

                # Weighted V sum
                for d in range(head_dim):
                    acc = 0
                    for p in range(pos + 1):
                        v_val = int(self.kv_cache[l, 1, kv_group, p, d])
                        acc += int(att_sm[p]) * v_val
                    xb_out[h * head_dim + d] = acc >> 16

            # Output projection
            xb2 = self.matmul_q8_fast(xb_out.astype(np.int32), *self.wo[l], dim, n_heads * head_dim)

            # Residual
            x = np.int32(x.astype(np.int64) + xb2.astype(np.int64))

            # Pre-FFN RMSNorm
            xb = self.rmsnorm_fast(x, self.rms_ffn_w[l])

            # SwiGLU FFN
            hb = self.matmul_q8_fast(xb, *self.gate[l], hidden_dim, dim)
            hb2 = self.matmul_q8_fast(xb, *self.up[l], hidden_dim, dim)

            # silu(gate) * up
            hb = self.silu_q16_vec(hb)
            hb = np.int32((hb.astype(np.int64) * hb2.astype(np.int64)) >> 16)

            # Down projection
            xb2 = self.matmul_q8_fast(hb, *self.down[l], dim, hidden_dim)

            # Residual
            x = np.int32(x.astype(np.int64) + xb2.astype(np.int64))

        if verbose:
            print(f"  after layers: x[0..4] = {x[:4].tolist()}")

        # Final RMSNorm
        xb = self.rmsnorm_fast(x, self.rms_final_w)

        # Logits (tied embeddings)
        logits = self.matmul_q8_fast(xb, self.embed_w, self.embed_s,
                                     self.vocab_size, dim)
        return logits


def main():
    bin_path = sys.argv[1] if len(sys.argv) > 1 else "hackbot-model.bin"

    print(f"Loading model from {bin_path}...")
    t0 = time.time()
    model = FastHackbotModel(bin_path)
    print(f"  Loaded in {time.time()-t0:.1f}s")
    print(f"  dim={model.dim}, layers={model.n_layers}, vocab={model.vocab_size}")

    # Correct HuggingFace ChatML tokens for:
    # system: "You are hackbot, a kernel agent. Answer concisely. For live system data, use: <tool>ps</tool> <tool>mem</tool> <tool>loadavg</tool>"
    # user: "hello"
    # + generation prompt (assistant\n)
    prompt_tokens = [
        1, 9690, 198, 2683, 359, 11042, 9433, 28, 253, 11498,
        7997, 30, 19842, 1700, 9182, 30, 1068, 2330, 817, 940,
        28, 722, 42, 2067, 19324, 46, 851, 9617, 19324, 46,
        2067, 19324, 46, 9808, 9617, 19324, 46, 2067, 19324, 46,
        2386, 20452, 9617, 19324, 46, 2, 198, 1, 4093, 198,
        28120, 2, 198, 1, 520, 9531, 198,
    ]
    print(f"\nPrompt: {len(prompt_tokens)} tokens (from HuggingFace tokenizer)")

    # === Single-token test (verify matches kernel) ===
    print(f"\n=== Single-token test (token 1 at pos 0) ===")
    model.kv_cache[:] = 0
    t0 = time.time()
    logits = model.forward_token(1, 0, verbose=True)
    elapsed = time.time() - t0
    top1 = int(np.argmax(logits))
    top1_logit = int(logits[top1])
    print(f"  top-1: token {top1} (logit {top1_logit})  [{elapsed:.1f}s]")
    print(f"  Expected: token 28 (logit 1304762)")
    if top1 == 28 and top1_logit == 1304762:
        print(f"  ✓ MATCHES kernel exactly!")
    else:
        print(f"  ✗ MISMATCH!")

    # === Full 57-token prefill ===
    print(f"\n=== Full {len(prompt_tokens)}-token prefill ===")
    model.kv_cache[:] = 0

    for i, tid in enumerate(prompt_tokens):
        t0 = time.time()
        logits = model.forward_token(tid, i, verbose=(i == 0))
        elapsed = time.time() - t0

        if i < 3 or i == len(prompt_tokens) - 1:
            top1 = int(np.argmax(logits))
            top1_logit = int(logits[top1])
            tok_str = model.tok_bytes[top1].decode('utf-8', errors='replace')
            print(f"  pos {i}: token {tid} → top-1 = {top1} ({repr(tok_str)}) "
                  f"logit={top1_logit} [{elapsed:.1f}s]")
        elif i == 3:
            print(f"  ... processing tokens 3-{len(prompt_tokens)-2} ...")

    # Final logits analysis
    print(f"\n=== Prefill result (pos {len(prompt_tokens)-1}) ===")
    top5_idx = np.argsort(logits)[-5:][::-1]
    print(f"INT8 Top-5 predictions after full prompt:")
    for rank, tid in enumerate(top5_idx):
        tok_str = model.tok_bytes[tid].decode('utf-8', errors='replace')
        print(f"  {rank+1}. token {tid} ({repr(tok_str)}) logit={int(logits[tid])}")

    print(f"\nKernel output was: top-1 = token 2683 ('You') logit 2033846")
    print(f"Float32 ref was:   top-1 = token 28120 ('hello')")

    if int(top5_idx[0]) == 2683:
        print(f"\n→ Python INT8 ALSO gets token 2683 — this is a quantization precision issue")
    elif int(top5_idx[0]) == 28120:
        print(f"\n→ Python INT8 gets correct token 28120 — kernel has a specific bug")
    else:
        print(f"\n→ Python INT8 gets different token {int(top5_idx[0])} — "
              f"need further analysis")

    # Compare with float32 reference
    try:
        import torch
        from transformers import AutoModelForCausalLM, AutoTokenizer

        print(f"\n=== Float32 Reference ===")
        ref_model = AutoModelForCausalLM.from_pretrained(
            "HuggingFaceTB/SmolLM2-135M-Instruct", torch_dtype=torch.float32)
        ref_model.eval()
        tokenizer = AutoTokenizer.from_pretrained("HuggingFaceTB/SmolLM2-135M-Instruct")

        input_ids = torch.tensor([prompt_tokens])
        with torch.no_grad():
            ref_logits = ref_model(input_ids).logits[0, -1].numpy()

        ref_top5 = np.argsort(ref_logits)[-5:][::-1]
        print(f"Float32 Top-5:")
        for rank, tid in enumerate(ref_top5):
            print(f"  {rank+1}. token {tid} ({repr(tokenizer.decode([tid]))}) "
                  f"logit={ref_logits[tid]:.4f}")

        # Correlation between INT8 and float32 logits
        int8_f = logits.astype(np.float64) / Q16_ONE
        corr = np.corrcoef(int8_f, ref_logits)[0, 1]
        print(f"\nLogit correlation (INT8 vs float32): {corr:.6f}")

    except ImportError:
        print("\n(transformers not available)")


if __name__ == "__main__":
    main()
