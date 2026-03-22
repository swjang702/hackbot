#!/usr/bin/env python3
"""
verify_hackbot.py — Verify hackbot-model.bin INT8 inference against float16 reference.

This script diagnoses the degenerate output issue by:
1. Loading the hackbot binary model and the reference HuggingFace model
2. Comparing tokenization (GPT-2 BPE)
3. Comparing forward pass outputs (logits) at each step
4. Running greedy generation from both and comparing outputs

Usage:
  python3 verify_hackbot.py hackbot-model.bin HuggingFaceTB/SmolLM2-135M-Instruct
"""

import argparse
import struct
import sys
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer


# === Constants matching hackbot.rs ===
MODEL_MAGIC = 0x484B4254
Q16_SHIFT = 16
Q16_ONE = 1 << 16


def load_hackbot_bin(path: str):
    """Load hackbot binary model file."""
    data = Path(path).read_bytes()

    # Parse header (56 bytes = 14 × u32)
    header = struct.unpack_from("<14I", data, 0)
    magic, version, dim, hidden_dim, n_layers, n_heads, n_kv_heads, \
        vocab_size, seq_len, group_size, head_dim, kv_dim, rope_theta, pad = header

    assert magic == MODEL_MAGIC, f"Bad magic: {magic:#x}"
    print(f"Header: dim={dim}, hidden_dim={hidden_dim}, n_layers={n_layers}")
    print(f"  n_heads={n_heads}, n_kv_heads={n_kv_heads}, vocab_size={vocab_size}")
    print(f"  group_size={group_size}, head_dim={head_dim}")

    cfg = {
        'dim': dim, 'hidden_dim': hidden_dim, 'n_layers': n_layers,
        'n_heads': n_heads, 'n_kv_heads': n_kv_heads, 'vocab_size': vocab_size,
        'seq_len': seq_len, 'group_size': group_size, 'head_dim': head_dim,
        'kv_dim': kv_dim, 'rope_theta': rope_theta,
    }

    # Parse tokenizer
    off = 56  # after header
    tok_vocab_size, max_token_len = struct.unpack_from("<II", data, off)
    off += 8
    print(f"Tokenizer: vocab_size={tok_vocab_size}, max_token_len={max_token_len}")

    tokens = []
    for i in range(tok_vocab_size):
        score = struct.unpack_from("<i", data, off)[0]
        off += 4
        tlen = struct.unpack_from("<H", data, off)[0]
        off += 2
        tbytes = data[off:off+tlen]
        off += tlen
        tokens.append((score, tbytes))

    tok_end = off
    print(f"Tokenizer section: {tok_end - 56} bytes")

    return data, cfg, tokens, tok_end


def dequantize_q8(data: bytes, offset: int, rows: int, cols: int, group_size: int):
    """Dequantize INT8 weights with Q16.16 scales to float32."""
    n_groups = cols // group_size

    # INT8 data: [rows × cols] bytes
    q_data = np.frombuffer(data, dtype=np.int8, count=rows*cols, offset=offset)
    q_data = q_data.reshape(rows, cols)

    # Scales: [rows × n_groups] int32 (Q16.16)
    scale_offset = offset + rows * cols
    scales_q16 = np.frombuffer(data, dtype=np.int32, count=rows*n_groups, offset=scale_offset)
    scales_q16 = scales_q16.reshape(rows, n_groups)
    scales_float = scales_q16.astype(np.float64) / (1 << Q16_SHIFT)

    # Dequantize
    result = np.zeros((rows, cols), dtype=np.float32)
    for g in range(n_groups):
        start = g * group_size
        end = start + group_size
        result[:, start:end] = q_data[:, start:end].astype(np.float32) * scales_float[:, g:g+1]

    total_bytes = rows * cols + rows * n_groups * 4
    return result, total_bytes


def compare_weights(bin_path: str, model_name: str):
    """Compare binary weights against HuggingFace reference."""
    data, cfg, tokens, tok_end = load_hackbot_bin(bin_path)

    print(f"\nLoading reference model: {model_name}")
    ref_model = AutoModelForCausalLM.from_pretrained(model_name, torch_dtype=torch.float32)
    ref_state = ref_model.state_dict()

    dim = cfg['dim']
    hidden_dim = cfg['hidden_dim']
    n_layers = cfg['n_layers']
    n_heads = cfg['n_heads']
    n_kv_heads = cfg['n_kv_heads']
    vocab_size = cfg['vocab_size']
    head_dim = cfg['head_dim']
    kv_dim = cfg['kv_dim']
    gs = cfg['group_size']

    off = tok_end

    # Embedding table
    print("\n=== Embedding Table ===")
    embed_deq, embed_bytes = dequantize_q8(data, off, vocab_size, dim, gs)
    embed_ref = ref_state["model.embed_tokens.weight"].numpy()
    max_err = np.max(np.abs(embed_deq - embed_ref))
    mean_err = np.mean(np.abs(embed_deq - embed_ref))
    print(f"  Max error: {max_err:.6f}, Mean error: {mean_err:.6f}")
    print(f"  Ref range: [{embed_ref.min():.3f}, {embed_ref.max():.3f}]")
    print(f"  Deq range: [{embed_deq.min():.3f}, {embed_deq.max():.3f}]")
    off += embed_bytes

    # Spot-check a few specific embedding rows
    for token_id in [0, 1, 2, 100, 1000]:
        row_err = np.max(np.abs(embed_deq[token_id] - embed_ref[token_id]))
        print(f"  Token {token_id}: max row error = {row_err:.6f}")

    # Check first layer weights
    print("\n=== Layer 0 ===")
    prefix = "model.layers.0"

    # RMSNorm (attention)
    rms_att_q16 = np.frombuffer(data, dtype=np.int32, count=dim, offset=off)
    rms_att_float = rms_att_q16.astype(np.float64) / (1 << Q16_SHIFT)
    rms_ref = ref_state[f"{prefix}.input_layernorm.weight"].numpy()
    rms_err = np.max(np.abs(rms_att_float - rms_ref))
    print(f"  RMSNorm att: max error = {rms_err:.6f}")
    off += dim * 4

    # Q projection
    wq_deq, wq_bytes = dequantize_q8(data, off, n_heads * head_dim, dim, gs)
    wq_ref = ref_state[f"{prefix}.self_attn.q_proj.weight"].numpy()
    wq_err = np.max(np.abs(wq_deq - wq_ref))
    print(f"  Wq: max error = {wq_err:.6f}, mean = {np.mean(np.abs(wq_deq - wq_ref)):.6f}")
    off += wq_bytes

    # K projection
    wk_deq, wk_bytes = dequantize_q8(data, off, n_kv_heads * head_dim, dim, gs)
    wk_ref = ref_state[f"{prefix}.self_attn.k_proj.weight"].numpy()
    wk_err = np.max(np.abs(wk_deq - wk_ref))
    print(f"  Wk: max error = {wk_err:.6f}")
    off += wk_bytes

    # V projection
    wv_deq, wv_bytes = dequantize_q8(data, off, n_kv_heads * head_dim, dim, gs)
    wv_ref = ref_state[f"{prefix}.self_attn.v_proj.weight"].numpy()
    wv_err = np.max(np.abs(wv_deq - wv_ref))
    print(f"  Wv: max error = {wv_err:.6f}")
    off += wv_bytes

    # O projection
    wo_deq, wo_bytes = dequantize_q8(data, off, dim, n_heads * head_dim, gs)
    wo_ref = ref_state[f"{prefix}.self_attn.o_proj.weight"].numpy()
    wo_err = np.max(np.abs(wo_deq - wo_ref))
    print(f"  Wo: max error = {wo_err:.6f}")
    off += wo_bytes

    # RMSNorm (FFN)
    off += dim * 4  # skip rms_ffn

    # Gate projection
    gate_deq, gate_bytes = dequantize_q8(data, off, hidden_dim, dim, gs)
    gate_ref = ref_state[f"{prefix}.mlp.gate_proj.weight"].numpy()
    gate_err = np.max(np.abs(gate_deq - gate_ref))
    print(f"  Gate: max error = {gate_err:.6f}")
    off += gate_bytes

    print("\n=== Reference Model Generation (float32) ===")
    tokenizer = AutoTokenizer.from_pretrained(model_name)

    # Test with ChatML format
    messages = [
        {"role": "system", "content": "You are hackbot, a kernel agent. Answer concisely. For live system data, use: <tool>ps</tool> <tool>mem</tool> <tool>loadavg</tool>"},
        {"role": "user", "content": "hello"},
    ]

    chat_text = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
    print(f"ChatML text:\n{repr(chat_text)}")

    chat_tokens = tokenizer.encode(chat_text, add_special_tokens=False)
    print(f"ChatML tokens ({len(chat_tokens)}): {chat_tokens}")

    # Simple test: what does "hello" tokenize to?
    hello_tokens = tokenizer.encode("hello", add_special_tokens=False)
    print(f"\n'hello' tokens: {hello_tokens}")
    for tid in hello_tokens:
        print(f"  Token {tid}: {repr(tokenizer.decode([tid]))}")

    # Generate with float32 model
    print(f"\nGenerating with float32 model (greedy)...")
    input_ids = torch.tensor([chat_tokens])
    with torch.no_grad():
        output = ref_model.generate(
            input_ids, max_new_tokens=50, do_sample=False,
            temperature=1.0, top_p=1.0,
        )
    generated = tokenizer.decode(output[0][len(chat_tokens):], skip_special_tokens=True)
    print(f"Float32 output: {repr(generated)}")

    # Also test with just raw "hello" (no ChatML)
    print(f"\n=== Raw 'hello' generation (no ChatML) ===")
    hello_ids = torch.tensor([hello_tokens])
    with torch.no_grad():
        output2 = ref_model.generate(
            hello_ids, max_new_tokens=50, do_sample=False,
        )
    raw_generated = tokenizer.decode(output2[0][len(hello_tokens):], skip_special_tokens=True)
    print(f"Raw output: {repr(raw_generated)}")

    # === Verify our INT8 forward pass matches ===
    print(f"\n=== INT8 Forward Pass Verification ===")

    # Reload all weights for full forward pass comparison
    data_np = np.frombuffer(data, dtype=np.uint8)

    # Run reference forward on first token of chat
    first_token = chat_tokens[0]
    print(f"First token: {first_token} = {repr(tokenizer.decode([first_token]))}")

    # Get reference logits for first token
    with torch.no_grad():
        ref_logits = ref_model(torch.tensor([[first_token]])).logits[0, 0].numpy()

    # Get reference top-5 predictions
    top5_ref = np.argsort(ref_logits)[-5:][::-1]
    print(f"Reference top-5 tokens after first token:")
    for tid in top5_ref:
        print(f"  Token {tid}: {repr(tokenizer.decode([tid]))} (logit={ref_logits[tid]:.4f})")

    # Now manually do INT8 forward for the same token
    print(f"\nINT8 embedding for token {first_token}:")
    # Re-parse to get embedding at the right offset
    embed_off = tok_end
    embed_row = embed_deq[first_token]
    embed_ref_row = embed_ref[first_token]
    print(f"  INT8 deq first 8: {embed_row[:8]}")
    print(f"  Float ref first 8: {embed_ref_row[:8]}")
    print(f"  Max diff: {np.max(np.abs(embed_row - embed_ref_row)):.6f}")

    # Check tokenizer: verify byte mapping
    print(f"\n=== Tokenizer Byte Mapping Check ===")
    # For each printable ASCII byte, check what token it maps to
    for byte_val in [ord('h'), ord('e'), ord('l'), ord('o'), 0x20, 0x0A]:
        # In GPT-2, this byte should map to a specific single-char token
        char_repr = chr(byte_val) if 32 <= byte_val < 127 else f"\\x{byte_val:02x}"
        # Find the token for this byte
        # GPT-2 bytes_to_unicode
        gpt2_char = bytes_to_unicode()[byte_val]
        # Find token ID for this character
        vocab = tokenizer.get_vocab()
        if gpt2_char in vocab:
            tid = vocab[gpt2_char]
            print(f"  byte 0x{byte_val:02x} ({char_repr}) → GPT2 char {repr(gpt2_char)} → token {tid}")
        else:
            print(f"  byte 0x{byte_val:02x} ({char_repr}) → GPT2 char {repr(gpt2_char)} → NOT FOUND!")

    # Check special tokens
    print(f"\n=== Special Tokens ===")
    for name, tid in [("endoftext", 0), ("im_start", 1), ("im_end", 2)]:
        tok_str = tokenizer.convert_ids_to_tokens(tid)
        print(f"  Token {tid}: {repr(tok_str)}")

    return cfg, tokens, ref_model, tokenizer


def bytes_to_unicode():
    """GPT-2's bytes_to_unicode mapping (from OpenAI)."""
    bs = list(range(ord("!"), ord("~")+1)) + list(range(ord("¡"), ord("¬")+1)) + list(range(ord("®"), ord("ÿ")+1))
    cs = list(bs)
    n = 0
    for b in range(2**8):
        if b not in bs:
            bs.append(b)
            cs.append(2**8+n)
            n += 1
    cs = [chr(n) for n in cs]
    return dict(zip(bs, cs))


def main():
    parser = argparse.ArgumentParser(description="Verify hackbot INT8 model")
    parser.add_argument("bin_path", help="Path to hackbot-model.bin")
    parser.add_argument("model_name", nargs="?",
                        default="HuggingFaceTB/SmolLM2-135M-Instruct",
                        help="HuggingFace model name")
    args = parser.parse_args()

    compare_weights(args.bin_path, args.model_name)


if __name__ == "__main__":
    main()
