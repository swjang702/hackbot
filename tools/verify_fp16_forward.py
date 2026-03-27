#!/usr/bin/env python3
"""
verify_fp16_forward.py — Verify hackbot FP16 (format v2) forward pass against HuggingFace.

Reads the FP16 binary model file, implements the EXACT same forward pass
as hackbot_fpu.c, and compares every intermediate result against the
HuggingFace float32 reference.

This pinpoints exactly where divergence occurs:
  - Weight reading (offset mismatch)
  - RMSNorm
  - QKV projections
  - RoPE
  - Attention
  - FFN
  - Logits

Usage:
  python3 verify_fp16_forward.py [model_binary] [hf_model_name]
  python3 verify_fp16_forward.py hackbot-model-fp16.bin
"""

import argparse
import math
import struct
import sys
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

MODEL_MAGIC = 0x484B4254


def load_fp16_binary(path: str):
    """Load hackbot FP16 binary and parse header + tokenizer."""
    data = Path(path).read_bytes()
    raw = np.frombuffer(data, dtype=np.uint8)

    # Parse header (56 bytes = 14 x u32)
    header = struct.unpack_from("<14I", data, 0)
    magic, version, dim, hidden_dim, n_layers, n_heads, n_kv_heads, \
        vocab_size, seq_len, weight_type, head_dim, kv_dim, rope_theta, pad = header

    assert magic == MODEL_MAGIC, f"Bad magic: {magic:#x}"
    assert version == 2, f"Expected format v2, got {version}"
    assert weight_type == 0, f"Expected weight_type=0 (FP16), got {weight_type}"

    cfg = {
        'dim': dim, 'hidden_dim': hidden_dim, 'n_layers': n_layers,
        'n_heads': n_heads, 'n_kv_heads': n_kv_heads, 'vocab_size': vocab_size,
        'seq_len': seq_len, 'head_dim': head_dim, 'kv_dim': kv_dim,
        'rope_theta': rope_theta,
    }

    print(f"Config: dim={dim}, hidden_dim={hidden_dim}, n_layers={n_layers}")
    print(f"  n_heads={n_heads}, n_kv_heads={n_kv_heads}, head_dim={head_dim}")
    print(f"  vocab_size={vocab_size}, rope_theta={rope_theta}")

    # Skip tokenizer section
    off = 56
    tok_vocab_size, max_token_len = struct.unpack_from("<II", data, off)
    off += 8
    for i in range(tok_vocab_size):
        off += 4  # score
        tlen = struct.unpack_from("<H", data, off)[0]
        off += 2 + tlen

    weights_start = off
    print(f"Tokenizer: {tok_vocab_size} tokens, weights start at byte {weights_start}")

    return data, cfg, weights_start


def read_fp16_matrix(data: bytes, offset: int, rows: int, cols: int):
    """Read an FP16 weight matrix and convert to float32."""
    n_elements = rows * cols
    fp16_data = np.frombuffer(data, dtype=np.float16, count=n_elements, offset=offset)
    return fp16_data.astype(np.float32).reshape(rows, cols)


def read_f32_vector(data: bytes, offset: int, dim: int):
    """Read a float32 vector."""
    return np.frombuffer(data, dtype=np.float32, count=dim, offset=offset).copy()


def rmsnorm(x, weight, eps=1e-5):
    """RMSNorm matching hackbot_fpu.c."""
    ss = np.sum(x * x)
    rms = math.sqrt(ss / len(x) + eps)
    return x * (1.0 / rms) * weight


def rope(vec, pos, head_dim, rope_theta=10000):
    """RoPE matching hackbot_fpu.c."""
    out = vec.copy()
    for i in range(head_dim // 2):
        freq = 1.0 / math.exp((2 * i) / head_dim * math.log(rope_theta))
        theta = pos * freq
        cos_t = math.cos(theta)
        sin_t = math.sin(theta)
        v0 = out[2 * i]
        v1 = out[2 * i + 1]
        out[2 * i]     = v0 * cos_t - v1 * sin_t
        out[2 * i + 1] = v0 * sin_t + v1 * cos_t
    return out


def softmax(x):
    """Softmax matching hackbot_fpu.c."""
    m = np.max(x)
    e = np.exp(x - m)
    return e / np.sum(e)


def silu(x):
    """SiLU matching hackbot_fpu.c."""
    return x / (1.0 + np.exp(-x))


def forward_token(data, cfg, weights_start, token_id, pos, kv_cache):
    """
    Run one token through the transformer, matching hackbot_fpu.c exactly.
    Returns logits and updated kv_cache.
    """
    dim = cfg['dim']
    hidden_dim = cfg['hidden_dim']
    n_layers = cfg['n_layers']
    n_heads = cfg['n_heads']
    n_kv_heads = cfg['n_kv_heads']
    head_dim = cfg['head_dim']
    vocab_size = cfg['vocab_size']
    rope_theta = cfg['rope_theta']
    kv_dim = n_kv_heads * head_dim
    hpg = n_heads // n_kv_heads  # heads per group

    # Compute weight offsets (matching hackbot_fpu.c lines 547-585)
    off = weights_start
    embed_off = off
    off += vocab_size * dim * 2  # FP16

    layer_offsets = []
    for _ in range(n_layers):
        lo = {}
        lo['rms_att'] = off;   off += dim * 4
        lo['wq'] = off;        off += n_heads * head_dim * dim * 2
        lo['wk'] = off;        off += n_kv_heads * head_dim * dim * 2
        lo['wv'] = off;        off += n_kv_heads * head_dim * dim * 2
        lo['wo'] = off;        off += dim * n_heads * head_dim * 2
        lo['rms_ffn'] = off;   off += dim * 4
        lo['gate'] = off;      off += hidden_dim * dim * 2
        lo['up'] = off;        off += hidden_dim * dim * 2
        lo['down'] = off;      off += dim * hidden_dim * 2
        layer_offsets.append(lo)
    rms_final_off = off

    # Step 1: Embedding lookup
    embed = read_fp16_matrix(data, embed_off, vocab_size, dim)
    x = embed[token_id].copy()

    if pos == 0:
        print(f"  embed[{token_id}]: x[0:4] = {x[:4]}")

    # Step 2: Transformer layers
    for l in range(n_layers):
        lo = layer_offsets[l]

        # 2a: Pre-attention RMSNorm
        rms_w = read_f32_vector(data, lo['rms_att'], dim)
        xb = rmsnorm(x, rms_w)

        # 2b: QKV projections (W @ xb where W is [out_dim x dim])
        wq = read_fp16_matrix(data, lo['wq'], n_heads * head_dim, dim)
        wk = read_fp16_matrix(data, lo['wk'], kv_dim, dim)
        wv = read_fp16_matrix(data, lo['wv'], kv_dim, dim)

        q = wq @ xb
        k = wk @ xb
        v = wv @ xb

        # 2c: RoPE
        for h in range(n_heads):
            q[h*head_dim:(h+1)*head_dim] = rope(
                q[h*head_dim:(h+1)*head_dim], pos, head_dim, rope_theta)
        for h in range(n_kv_heads):
            k[h*head_dim:(h+1)*head_dim] = rope(
                k[h*head_dim:(h+1)*head_dim], pos, head_dim, rope_theta)

        # 2d: Store K, V in cache
        for h in range(n_kv_heads):
            kv_cache[l][0][h][pos] = k[h*head_dim:(h+1)*head_dim].copy()
            kv_cache[l][1][h][pos] = v[h*head_dim:(h+1)*head_dim].copy()

        # 2e: Multi-head attention with GQA
        inv_sqrt = 1.0 / math.sqrt(head_dim)
        xb_attn = np.zeros(n_heads * head_dim, dtype=np.float32)

        for h in range(n_heads):
            kv_group = h // hpg
            q_head = q[h*head_dim:(h+1)*head_dim]

            # Attention scores
            scores = np.zeros(pos + 1, dtype=np.float32)
            for p in range(pos + 1):
                k_cached = kv_cache[l][0][kv_group][p]
                scores[p] = np.dot(q_head, k_cached) * inv_sqrt

            # Softmax
            attn = softmax(scores)

            # Weighted V sum
            for d in range(head_dim):
                acc = 0.0
                for p in range(pos + 1):
                    acc += attn[p] * kv_cache[l][1][kv_group][p][d]
                xb_attn[h * head_dim + d] = acc

        # 2f: Output projection
        wo = read_fp16_matrix(data, lo['wo'], dim, n_heads * head_dim)
        xb2 = wo @ xb_attn

        # 2g: Residual
        x = x + xb2

        # 2h: Pre-FFN RMSNorm
        rms_ffn_w = read_f32_vector(data, lo['rms_ffn'], dim)
        xb = rmsnorm(x, rms_ffn_w)

        # 2i: SwiGLU FFN
        gate_w = read_fp16_matrix(data, lo['gate'], hidden_dim, dim)
        up_w = read_fp16_matrix(data, lo['up'], hidden_dim, dim)
        down_w = read_fp16_matrix(data, lo['down'], dim, hidden_dim)

        hb = silu(gate_w @ xb) * (up_w @ xb)
        xb2 = down_w @ hb

        # 2j: Residual
        x = x + xb2

        if pos == 0 and (l == 0 or l == n_layers - 1):
            print(f"  layer {l}: x[0:4] = {x[:4]}")

    # Step 3: Final RMSNorm
    rms_final_w = read_f32_vector(data, rms_final_off, dim)
    xb = rmsnorm(x, rms_final_w)

    # Step 4: Logits (tied embeddings)
    logits = embed @ xb  # [vocab_size x dim] @ [dim] = [vocab_size]

    if pos == 0:
        top5 = np.argsort(logits)[-5:][::-1]
        print(f"  logits top-5: {[(int(t), f'{logits[t]:.2f}') for t in top5]}")

    return logits, kv_cache


def main():
    parser = argparse.ArgumentParser(description="Verify FP16 forward pass")
    parser.add_argument("bin_path", nargs="?",
                        default="hackbot-model-fp16.bin",
                        help="Path to hackbot FP16 model binary")
    parser.add_argument("model_name", nargs="?",
                        default="HuggingFaceTB/SmolLM2-135M-Instruct",
                        help="HuggingFace model name")
    parser.add_argument("--token", type=int, default=1,
                        help="Token ID to test (default: 1 = <|im_start|>)")
    parser.add_argument("--prompt", type=str, default=None,
                        help="Prompt to test (overrides --token)")
    args = parser.parse_args()

    if not Path(args.bin_path).exists():
        print(f"ERROR: {args.bin_path} not found.")
        print(f"Generate it with: python3 tools/export_hackbot_fp16.py")
        sys.exit(1)

    # Load binary model
    print(f"=== Loading binary model: {args.bin_path} ===")
    data, cfg, weights_start = load_fp16_binary(args.bin_path)

    # Load HuggingFace reference
    print(f"\n=== Loading HuggingFace reference: {args.model_name} ===")
    ref_model = AutoModelForCausalLM.from_pretrained(args.model_name, torch_dtype=torch.float32)
    tokenizer = AutoTokenizer.from_pretrained(args.model_name)
    ref_model.eval()

    dim = cfg['dim']
    n_layers = cfg['n_layers']
    n_kv_heads = cfg['n_kv_heads']
    head_dim = cfg['head_dim']
    max_seq = 256

    # Determine tokens to test
    if args.prompt:
        messages = [
            {"role": "system", "content": "You are hackbot, a kernel agent. Answer concisely."},
            {"role": "user", "content": args.prompt},
        ]
        chat_text = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
        test_tokens = tokenizer.encode(chat_text, add_special_tokens=False)
        print(f"\nPrompt tokens ({len(test_tokens)}): {test_tokens[:20]}...")
    else:
        test_tokens = [args.token]
        print(f"\nTest token: {args.token} = {repr(tokenizer.decode([args.token]))}")

    # Initialize KV cache
    kv_cache = [[
        [np.zeros((max_seq, head_dim), dtype=np.float32) for _ in range(n_kv_heads)]
        for _ in range(2)  # K, V
    ] for _ in range(n_layers)]

    # === Run our forward pass ===
    print(f"\n=== Our FP16 forward pass ===")
    for i, tok in enumerate(test_tokens):
        if i < 3 or i == len(test_tokens) - 1:
            print(f"\nToken {i}: {tok} = {repr(tokenizer.decode([tok]))}")
        our_logits, kv_cache = forward_token(data, cfg, weights_start, tok, i, kv_cache)

    # Get our prediction
    our_top1 = int(np.argmax(our_logits))
    print(f"\nOur top-1: token {our_top1} = {repr(tokenizer.decode([our_top1]))} (logit={our_logits[our_top1]:.4f})")

    our_top5 = np.argsort(our_logits)[-5:][::-1]
    print("Our top-5:")
    for t in our_top5:
        print(f"  token {t} = {repr(tokenizer.decode([int(t)]))} (logit={our_logits[t]:.4f})")

    # === Run HuggingFace reference ===
    print(f"\n=== HuggingFace reference ===")
    input_ids = torch.tensor([test_tokens])
    with torch.no_grad():
        ref_output = ref_model(input_ids)
        ref_logits = ref_output.logits[0, -1].numpy()  # last position

    ref_top1 = int(np.argmax(ref_logits))
    print(f"Ref top-1: token {ref_top1} = {repr(tokenizer.decode([ref_top1]))} (logit={ref_logits[ref_top1]:.4f})")

    ref_top5 = np.argsort(ref_logits)[-5:][::-1]
    print("Ref top-5:")
    for t in ref_top5:
        print(f"  token {t} = {repr(tokenizer.decode([int(t)]))} (logit={ref_logits[t]:.4f})")

    # === Compare ===
    print(f"\n=== Comparison ===")
    max_err = np.max(np.abs(our_logits - ref_logits))
    mean_err = np.mean(np.abs(our_logits - ref_logits))
    print(f"Max logits error: {max_err:.6f}")
    print(f"Mean logits error: {mean_err:.6f}")
    print(f"Top-1 match: {'YES' if our_top1 == ref_top1 else 'NO'}")

    # Correlation
    corr = np.corrcoef(our_logits, ref_logits)[0, 1]
    print(f"Logits correlation: {corr:.6f}")

    if corr < 0.9:
        print("\nWARNING: Low correlation suggests a bug in the forward pass!")
        print("Check intermediate values against reference.")
    elif our_top1 != ref_top1:
        print("\nNOTE: Top-1 differs but correlation is high — likely precision difference.")
    else:
        print("\nForward pass looks correct!")

    # === Generate a few tokens ===
    if args.prompt:
        print(f"\n=== Greedy generation (our model) ===")
        gen_tokens = []
        for step in range(20):
            next_tok = int(np.argmax(our_logits))
            if next_tok == 0 or next_tok == 2:  # EOS or IM_END
                break
            gen_tokens.append(next_tok)
            our_logits, kv_cache = forward_token(
                data, cfg, weights_start, next_tok, len(test_tokens) + step, kv_cache)
        print(f"Generated: {repr(tokenizer.decode(gen_tokens))}")

        print(f"\n=== Greedy generation (HuggingFace) ===")
        with torch.no_grad():
            ref_gen = ref_model.generate(input_ids, max_new_tokens=20, do_sample=False)
        ref_text = tokenizer.decode(ref_gen[0][len(test_tokens):], skip_special_tokens=False)
        print(f"Generated: {repr(ref_text)}")


if __name__ == "__main__":
    main()
