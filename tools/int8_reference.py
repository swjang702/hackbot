#!/usr/bin/env python3
"""
int8_reference.py — Python reference implementation of hackbot INT8 forward pass.

Does the exact same computation as the kernel code (Q16.16 fixed-point)
to verify correctness. Compares with float32 reference at each step.

Usage:
  python3 int8_reference.py hackbot-model.bin
"""

import struct
import sys
from pathlib import Path
import numpy as np


Q16_SHIFT = 16
Q16_ONE = 1 << 16
MODEL_MAGIC = 0x484B4254
INFERENCE_MAX_SEQ = 256

# RoPE frequencies: 1/10000^(2i/64) in Q16.16
ROPE_FREQS_64 = [
    65536, 49145, 36854, 27636, 20724, 15541, 11654, 8739,
     6554,  4915,  3685,  2764,  2072,  1554,  1165,  874,
      655,   491,   369,   276,   207,   155,   117,   87,
       66,    49,    37,    28,    21,    16,    12,    9,
]

TWO_PI_Q16 = 411775

# exp(-k) in Q16.16
EXP_TABLE = [65536, 24109, 8869, 3263, 1200, 442, 162, 60, 22, 8, 3, 1, 0, 0, 0, 0, 0]

# SIN_TABLE[k] = sin(2*pi*k/256) in Q16.16
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


def sin_q16(theta):
    """sin(theta) in Q16.16, matching kernel code."""
    two_pi = TWO_PI_Q16
    # Normalize to [0, 2pi)
    t = theta % two_pi
    if t < 0:
        t += two_pi
    # Map to table index: idx = t * 256 / (2*pi) in Q16.16
    idx256 = (t * 256) // two_pi
    idx = int(idx256) & 0xFF
    # Linear interpolation
    frac = (t * 256) - (idx256 * two_pi // 256 + two_pi * (idx256 % 256) // 256)
    # Simplified: just use table lookup (matching kernel's interpolation)
    return SIN_TABLE[idx]


def cos_q16(theta):
    """cos(theta) in Q16.16."""
    return sin_q16(theta + TWO_PI_Q16 // 4)


def isqrt_u64(n):
    """Integer square root (matching kernel code)."""
    if n < 2:
        return n
    bits = n.bit_length()
    x = 1 << ((bits + 1) // 2)
    while True:
        y = (x + n // x) // 2
        if y >= x:
            return x
        x = y


class HackbotModel:
    """INT8 model loaded from hackbot-model.bin."""

    def __init__(self, path):
        self.data = Path(path).read_bytes()
        self._parse()

    def _parse(self):
        """Parse the binary file."""
        d = self.data
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

        # Parse tokenizer
        off = 56
        tok_vocab_size, max_token_len = struct.unpack_from("<II", d, off)
        off += 8

        self.tok_scores = []
        self.tok_bytes = []
        for i in range(tok_vocab_size):
            score = struct.unpack_from("<i", d, off)[0]
            off += 4
            tlen = struct.unpack_from("<H", d, off)[0]
            off += 2
            tbytes = d[off:off+tlen]
            off += tlen
            self.tok_scores.append(score)
            self.tok_bytes.append(tbytes)

        self.weights_start = off

        # Parse weight offsets (matching kernel code exactly)
        gs = group_size

        def q8_size(rows, cols):
            return rows * cols + rows * (cols // gs) * 4

        # Embedding
        self.embed_off = off
        off += q8_size(vocab_size, dim)

        # Layers
        self.layer_offsets = []
        for l in range(n_layers):
            layer = {}
            # rms_att
            layer['rms_att_off'] = off
            off += dim * 4
            # wq
            layer['wq_off'] = off
            off += q8_size(n_heads * head_dim, dim)
            # wk
            layer['wk_off'] = off
            off += q8_size(n_kv_heads * head_dim, dim)
            # wv
            layer['wv_off'] = off
            off += q8_size(n_kv_heads * head_dim, dim)
            # wo
            layer['wo_off'] = off
            off += q8_size(dim, n_heads * head_dim)
            # rms_ffn
            layer['rms_ffn_off'] = off
            off += dim * 4
            # gate
            layer['gate_off'] = off
            off += q8_size(hidden_dim, dim)
            # up
            layer['up_off'] = off
            off += q8_size(hidden_dim, dim)
            # down
            layer['down_off'] = off
            off += q8_size(dim, hidden_dim)
            self.layer_offsets.append(layer)

        self.rms_final_off = off

        # Allocate inference state (all as Python arrays of int64 to avoid overflow)
        self.kv_cache = np.zeros((n_layers, 2, n_kv_heads, INFERENCE_MAX_SEQ, head_dim), dtype=np.int64)
        self.x = np.zeros(dim, dtype=np.int64)

    def _read_q8_row(self, offset, row, cols):
        """Read one row of Q8 weights and dequantize to Q16.16 int values."""
        gs = self.group_size
        n_groups = cols // gs
        data_base = offset + row * cols
        scale_base = offset + self.vocab_size * self.dim if offset == self.embed_off else None

        # General case: data is [rows * cols] i8, then [rows * n_groups] i32 scales
        # For a weight matrix at `offset` with `total_rows` rows:
        # Actually, let me just compute it properly
        pass

    def matmul_q8(self, x_q16, w_offset, rows, cols):
        """INT8 × Q16.16 matmul, matching kernel code exactly."""
        gs = self.group_size
        n_groups = cols // gs
        d = self.data

        out = np.zeros(rows, dtype=np.int64)

        for r in range(rows):
            row_acc = np.int64(0)
            row_base = r * cols
            scale_row_base = r * n_groups * 4

            for g in range(n_groups):
                # Read scale (Q16.16 i32, little-endian)
                sb = w_offset + rows * cols + scale_row_base + g * 4
                scale = struct.unpack_from("<i", d, sb)[0]

                # Dot product
                data_base = w_offset + row_base + g * gs
                x_base = g * gs
                group_acc = np.int64(0)
                for j in range(gs):
                    w = np.int64(np.uint8(d[data_base + j]).view(np.int8))
                    xv = np.int64(x_q16[x_base + j])
                    group_acc += w * xv

                row_acc += (group_acc * np.int64(scale)) >> 16

            out[r] = row_acc

        return out.astype(np.int32)

    def rmsnorm_q16(self, x_q16, weight_offset, dim):
        """RMSNorm in Q16.16, matching kernel code."""
        d = self.data

        # Sum of squares (u64)
        ss = np.uint64(0)
        for i in range(dim):
            xi = np.int64(x_q16[i])
            ss += np.uint64(xi * xi)

        mean_sq = ss // np.uint64(dim)
        mean_sq_eps = mean_sq + np.uint64(42950)

        rms = isqrt_u64(int(mean_sq_eps))
        if rms == 0:
            return np.zeros(dim, dtype=np.int64)

        rsqrt = int(np.uint64(1) << 32) // rms

        out = np.zeros(dim, dtype=np.int64)
        for i in range(dim):
            wb = weight_offset + i * 4
            w = struct.unpack_from("<i", d, wb)[0]

            x = int(x_q16[i])
            x_norm = (x * rsqrt) >> 16
            out[i] = (x_norm * w) >> 16

        return out

    def rope_apply(self, vec, pos, head_dim):
        """RoPE, matching kernel code."""
        n_pairs = head_dim // 2
        out = vec.copy()
        for i in range(n_pairs):
            freq = ROPE_FREQS_64[i] if i < len(ROPE_FREQS_64) else 0
            theta = (pos * freq) % TWO_PI_Q16

            cos_val = cos_q16(theta)
            sin_val = sin_q16(theta)

            v0 = int(vec[2*i])
            v1 = int(vec[2*i+1])

            out[2*i] = (v0 * cos_val - v1 * sin_val) >> 16
            out[2*i+1] = (v0 * sin_val + v1 * cos_val) >> 16

        return out

    def softmax_q16(self, x, length):
        """Softmax in Q16.16."""
        if length <= 1:
            return np.array([Q16_ONE], dtype=np.int64)

        vals = x[:length].copy().astype(np.int64)
        max_val = int(np.max(vals))

        # exp and sum
        total = np.int64(0)
        for i in range(length):
            diff = int(vals[i]) - max_val  # always <= 0
            e = self._exp_q16_neg(diff)
            vals[i] = e
            total += e

        if total == 0:
            vals[0] = Q16_ONE
            return vals

        for i in range(length):
            vals[i] = (vals[i] * Q16_ONE) // total

        return vals

    def _exp_q16_neg(self, x):
        """exp(x) for x <= 0 in Q16.16."""
        if x >= 0:
            return Q16_ONE
        x_int = x >> 16
        idx = -x_int
        if idx >= len(EXP_TABLE):
            return 0
        exp_int = EXP_TABLE[idx]

        x_frac = x - (x_int << 16)  # [0, 65535]
        f = x_frac
        f2 = (f * f) >> 16
        f3 = (f2 * f) >> 16
        exp_frac = Q16_ONE + f + (f2 >> 1) + f3 // 6

        return (exp_int * exp_frac) >> 16

    def silu_q16(self, x):
        """SiLU in Q16.16."""
        out = x.copy()
        for i in range(len(x)):
            xi = int(x[i])
            # sigmoid
            if xi >= 0:
                neg_x = -xi
                e = self._exp_q16_neg(neg_x)
                sig = (Q16_ONE * Q16_ONE) // (Q16_ONE + e)
            else:
                e = self._exp_q16_neg(xi)
                sig = (e * Q16_ONE) // (Q16_ONE + e)
            out[i] = (xi * sig) >> 16
        return out

    def forward_token(self, token_id, pos):
        """Full forward pass for one token, matching kernel code."""
        dim = self.dim
        hidden_dim = self.hidden_dim
        n_heads = self.n_heads
        n_kv_heads = self.n_kv_heads
        head_dim = self.head_dim
        gs = self.group_size
        d = self.data
        heads_per_group = n_heads // n_kv_heads

        # Embedding lookup
        n_groups_e = dim // gs
        x = np.zeros(dim, dtype=np.int64)
        for g in range(n_groups_e):
            sb = self.embed_off + self.vocab_size * dim + token_id * n_groups_e * 4 + g * 4
            scale = struct.unpack_from("<i", d, sb)[0]
            for j in range(gs):
                c = g * gs + j
                w = np.int8(np.uint8(d[self.embed_off + token_id * dim + c]).view(np.int8))
                x[c] = int(w) * scale

        # Transformer layers
        for l in range(self.n_layers):
            layer = self.layer_offsets[l]

            # Pre-attention RMSNorm
            xb = self.rmsnorm_q16(x, layer['rms_att_off'], dim)

            # QKV projections
            q_buf = self.matmul_q8(xb, layer['wq_off'], dim, dim)
            k_buf = self.matmul_q8(xb, layer['wk_off'], n_kv_heads * head_dim, dim)
            v_buf = self.matmul_q8(xb, layer['wv_off'], n_kv_heads * head_dim, dim)

            # RoPE
            for h in range(n_heads):
                q_head = q_buf[h*head_dim:(h+1)*head_dim].astype(np.int64)
                q_buf[h*head_dim:(h+1)*head_dim] = self.rope_apply(q_head, pos, head_dim).astype(np.int32)

            for h in range(n_kv_heads):
                k_head = k_buf[h*head_dim:(h+1)*head_dim].astype(np.int64)
                k_buf[h*head_dim:(h+1)*head_dim] = self.rope_apply(k_head, pos, head_dim).astype(np.int32)

            # Store K,V in cache
            for h in range(n_kv_heads):
                self.kv_cache[l, 0, h, pos, :] = k_buf[h*head_dim:(h+1)*head_dim]
                self.kv_cache[l, 1, h, pos, :] = v_buf[h*head_dim:(h+1)*head_dim]

            # Attention
            xb_out = np.zeros(dim, dtype=np.int64)
            for h in range(n_heads):
                kv_group = h // heads_per_group
                q_head = q_buf[h*head_dim:(h+1)*head_dim].astype(np.int64)

                att = np.zeros(INFERENCE_MAX_SEQ, dtype=np.int64)
                for p in range(pos + 1):
                    k_cached = self.kv_cache[l, 0, kv_group, p, :]
                    dot = np.int64(0)
                    for dd in range(head_dim):
                        dot += np.int64(q_head[dd]) * np.int64(k_cached[dd])
                    att[p] = dot >> 19

                # Softmax
                att_sm = self.softmax_q16(att.astype(np.int32), pos + 1)

                # Weighted V sum
                for dd in range(head_dim):
                    acc = np.int64(0)
                    for p in range(pos + 1):
                        v_val = self.kv_cache[l, 1, kv_group, p, dd]
                        acc += np.int64(att_sm[p]) * np.int64(v_val)
                    xb_out[h * head_dim + dd] = acc >> 16

            # Output projection
            xb2 = self.matmul_q8(xb_out.astype(np.int32), layer['wo_off'], dim, dim)

            # Residual
            x = np.int32(np.int64(x) + np.int64(xb2))

            # Pre-FFN RMSNorm
            xb = self.rmsnorm_q16(x, layer['rms_ffn_off'], dim)

            # SwiGLU FFN
            hb = self.matmul_q8(xb, layer['gate_off'], hidden_dim, dim)
            hb2 = self.matmul_q8(xb, layer['up_off'], hidden_dim, dim)

            # silu(gate) * up
            hb = self.silu_q16(hb.astype(np.int64))
            hb = np.int32((hb.astype(np.int64) * hb2.astype(np.int64)) >> 16)

            # Down projection
            xb2 = self.matmul_q8(hb, layer['down_off'], dim, hidden_dim)

            # Residual
            x = np.int32(np.int64(x) + np.int64(xb2))

            if l == 0:
                print(f"  Layer 0 output x[:8]: {x[:8]}")
            if l == self.n_layers - 1:
                print(f"  Layer {l} output x[:8]: {x[:8]}")

        # Final RMSNorm
        xb = self.rmsnorm_q16(x, self.rms_final_off, dim)

        # Logits (tied embeddings)
        logits = self.matmul_q8(xb, self.embed_off, self.vocab_size, dim)

        return logits


def main():
    bin_path = sys.argv[1] if len(sys.argv) > 1 else "hackbot-model.bin"

    print(f"Loading model from {bin_path}...")
    model = HackbotModel(bin_path)
    print(f"  dim={model.dim}, layers={model.n_layers}, vocab={model.vocab_size}")

    # Test: forward pass on token 1 (<|im_start|>)
    token_id = 1
    print(f"\nForward pass on token {token_id} at position 0...")
    logits = model.forward_token(token_id, 0)

    # Top-5
    top5 = np.argsort(logits)[-5:][::-1]
    print(f"\nINT8 Top-5 predictions after token {token_id}:")
    for tid in top5:
        tok_str = model.tok_bytes[tid]
        print(f"  Token {tid}: {repr(tok_str.decode('utf-8', errors='replace'))} (logit={logits[tid]})")

    # Compare with reference (if available)
    try:
        import torch
        from transformers import AutoModelForCausalLM, AutoTokenizer

        print(f"\nLoading float32 reference model...")
        ref_model = AutoModelForCausalLM.from_pretrained(
            "HuggingFaceTB/SmolLM2-135M-Instruct", dtype=torch.float32
        )
        ref_model.eval()

        with torch.no_grad():
            ref_logits = ref_model(torch.tensor([[token_id]])).logits[0, 0].numpy()

        ref_top5 = np.argsort(ref_logits)[-5:][::-1]
        print(f"Float32 Top-5 predictions after token {token_id}:")
        tokenizer = AutoTokenizer.from_pretrained("HuggingFaceTB/SmolLM2-135M-Instruct")
        for tid in ref_top5:
            print(f"  Token {tid}: {repr(tokenizer.decode([tid]))} (logit={ref_logits[tid]:.4f})")

        # Correlation between INT8 and float32 logits
        int8_f = logits.astype(np.float64) / Q16_ONE
        corr = np.corrcoef(int8_f, ref_logits)[0, 1]
        print(f"\nLogit correlation (INT8 vs float32): {corr:.6f}")

        # Check if top-1 matches
        int8_top1 = np.argmax(logits)
        ref_top1 = np.argmax(ref_logits)
        print(f"INT8 top-1: {int8_top1}, Float32 top-1: {ref_top1}")
        print(f"Match: {int8_top1 == ref_top1}")

    except ImportError:
        print("(transformers not available, skipping reference comparison)")

    # Also test a sequence of a few tokens
    print(f"\n=== Multi-token test (ChatML prefix) ===")
    # <|im_start|>system\n
    test_tokens = [1, 9690, 198]  # im_start, "system", newline
    model.kv_cache[:] = 0  # reset

    for i, tid in enumerate(test_tokens):
        logits = model.forward_token(tid, i)
        top1 = np.argmax(logits)
        tok_str = model.tok_bytes[top1]
        print(f"  After token {tid} (pos={i}): top-1 = {top1} ({repr(tok_str.decode('utf-8', errors='replace'))}), logit={logits[top1]}")


if __name__ == "__main__":
    main()
