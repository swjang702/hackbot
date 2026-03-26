#!/usr/bin/env python3
"""
export_hackbot_fp16.py — Export SmolLM2-135M to hackbot binary format v2 (FP16 weights).

Format v2 uses float16 weights (no quantization) and float32 RMSNorm weights.
This avoids the INT8 quantization precision loss that causes incorrect generation
at >20 token sequences with Q16.16 fixed-point arithmetic.

Binary layout v2:
  HEADER (56 bytes) — model config (version=2)
  TOKENIZER — same as v1
  WEIGHTS:
    embed_tokens: [vocab_size × dim] float16
    per layer:
      rms_att: [dim] float32
      wq: [n_heads*head_dim × dim] float16
      wk: [n_kv*head_dim × dim] float16
      wv: [n_kv*head_dim × dim] float16
      wo: [dim × n_heads*head_dim] float16
      rms_ffn: [dim] float32
      gate: [hidden_dim × dim] float16
      up: [hidden_dim × dim] float16
      down: [dim × hidden_dim] float16
    rms_final: [dim] float32

Usage:
  python3 export_hackbot_fp16.py [model_name] [output_path]
  sudo cp hackbot-model-fp16.bin /lib/firmware/hackbot-model.bin
"""

import argparse
import json
import struct
import sys
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer, AutoConfig


MAGIC = 0x484B4254  # "HKBT"
FORMAT_VERSION = 2


def export_tokenizer(tokenizer, vocab_size: int) -> bytes:
    """Export tokenizer vocabulary to binary format (same as v1)."""
    id_to_bytes = {}
    for token_id in range(vocab_size):
        token_str = tokenizer.convert_ids_to_tokens(token_id)
        if token_str is None:
            token_str = ""
        try:
            token_bytes = token_str.encode("utf-8")
        except (UnicodeEncodeError, UnicodeDecodeError):
            token_bytes = token_str.encode("utf-8", errors="replace")
        id_to_bytes[token_id] = token_bytes

    scores = [0] * vocab_size

    tokenizer_json_path = None
    if hasattr(tokenizer, "name_or_path"):
        from transformers.utils import cached_file
        try:
            tokenizer_json_path = cached_file(
                tokenizer.name_or_path, "tokenizer.json"
            )
        except Exception:
            pass

    if tokenizer_json_path and Path(tokenizer_json_path).exists():
        with open(tokenizer_json_path, "r") as f:
            tok_data = json.load(f)
        merges = tok_data.get("model", {}).get("merges", [])
        n_merges = len(merges)
        print(f"  Found {n_merges} BPE merges")
        vocab_str_to_id = tokenizer.get_vocab()
        for merge_idx, merge_str in enumerate(merges):
            parts = merge_str.split(" ", 1)
            if len(parts) != 2:
                continue
            merged = parts[0] + parts[1]
            if merged in vocab_str_to_id:
                token_id = vocab_str_to_id[merged]
                scores[token_id] = n_merges - merge_idx
    else:
        print("  WARNING: Could not find tokenizer.json, using default scores")

    buf = bytearray()
    max_token_len = max(len(b) for b in id_to_bytes.values()) if id_to_bytes else 0
    buf.extend(struct.pack("<II", vocab_size, max_token_len))

    for token_id in range(vocab_size):
        token_bytes = id_to_bytes.get(token_id, b"")
        score = scores[token_id]
        buf.extend(struct.pack("<i", score))
        buf.extend(struct.pack("<H", len(token_bytes)))
        buf.extend(token_bytes)

    return bytes(buf)


def export_model_fp16(model_name: str, output_path: str):
    """Export model with FP16 weights."""

    print(f"Loading model: {model_name}")
    config = AutoConfig.from_pretrained(model_name)

    assert config.model_type in ("llama",), \
        f"Unsupported model type: {config.model_type}"

    dim = config.hidden_size
    hidden_dim = config.intermediate_size
    n_layers = config.num_hidden_layers
    n_heads = config.num_attention_heads
    n_kv_heads = getattr(config, "num_key_value_heads", n_heads)
    vocab_size = config.vocab_size
    seq_len = config.max_position_embeddings
    head_dim = dim // n_heads
    kv_dim = head_dim * n_kv_heads
    rope_theta = int(getattr(config, "rope_theta", 10000))
    tie_word_embeddings = getattr(config, "tie_word_embeddings", True)

    print(f"Config:")
    print(f"  dim={dim}, hidden_dim={hidden_dim}, n_layers={n_layers}")
    print(f"  n_heads={n_heads}, n_kv_heads={n_kv_heads}, head_dim={head_dim}")
    print(f"  vocab_size={vocab_size}, seq_len={seq_len}")
    print(f"  rope_theta={rope_theta}, tie_embeddings={tie_word_embeddings}")

    print(f"\nLoading tokenizer...")
    tokenizer = AutoTokenizer.from_pretrained(model_name)

    print(f"\nLoading model weights...")
    model = AutoModelForCausalLM.from_pretrained(
        model_name, torch_dtype=torch.float16
    )
    state_dict = model.state_dict()

    total_params = sum(t.numel() for t in state_dict.values())
    print(f"  Total parameters: {total_params:,}")

    print(f"\nExporting to {output_path} (format v2, FP16)...")
    with open(output_path, "wb") as f:
        # --- HEADER (56 bytes) ---
        # In v2, the group_size field is repurposed as weight_type:
        #   0 = FP16 weights (this format)
        #   64 = INT8 with group_size=64 (v1 format)
        weight_type = 0  # FP16
        header = struct.pack(
            "<14I",
            MAGIC,
            FORMAT_VERSION,
            dim,
            hidden_dim,
            n_layers,
            n_heads,
            n_kv_heads,
            vocab_size,
            seq_len,
            weight_type,  # was group_size in v1
            head_dim,
            kv_dim,
            rope_theta,
            0,  # padding
        )
        f.write(header)
        print(f"  Header: {len(header)} bytes")

        # --- TOKENIZER ---
        print(f"  Exporting tokenizer ({vocab_size} tokens)...")
        tok_data = export_tokenizer(tokenizer, vocab_size)
        f.write(tok_data)
        print(f"  Tokenizer: {len(tok_data):,} bytes")

        # --- WEIGHTS ---
        bytes_written = 0

        def write_fp16(name: str, key: str, expected_shape: tuple):
            """Write weight matrix as FP16."""
            nonlocal bytes_written
            w = state_dict[key]
            if w.dtype != torch.float16:
                w = w.half()
            w_np = w.numpy()
            assert w_np.shape == expected_shape, \
                f"{name}: expected {expected_shape}, got {w_np.shape}"
            data = w_np.tobytes()
            f.write(data)
            bytes_written += len(data)

        def write_f32(name: str, key: str, expected_len: int):
            """Write RMSNorm weight as float32."""
            nonlocal bytes_written
            w = state_dict[key].float().numpy()
            assert w.shape == (expected_len,), \
                f"{name}: expected ({expected_len},), got {w.shape}"
            data = w.tobytes()
            f.write(data)
            bytes_written += len(data)

        # 1. Embedding table
        print(f"  Exporting embedding table...")
        write_fp16("embed_tokens", "model.embed_tokens.weight", (vocab_size, dim))

        # 2. Per-layer weights
        for layer_idx in range(n_layers):
            prefix = f"model.layers.{layer_idx}"
            print(f"  Exporting layer {layer_idx}/{n_layers}...", end="\r")

            write_f32(f"layer{layer_idx}.rms_att",
                      f"{prefix}.input_layernorm.weight", dim)
            write_fp16(f"layer{layer_idx}.wq",
                       f"{prefix}.self_attn.q_proj.weight",
                       (n_heads * head_dim, dim))
            write_fp16(f"layer{layer_idx}.wk",
                       f"{prefix}.self_attn.k_proj.weight",
                       (n_kv_heads * head_dim, dim))
            write_fp16(f"layer{layer_idx}.wv",
                       f"{prefix}.self_attn.v_proj.weight",
                       (n_kv_heads * head_dim, dim))
            write_fp16(f"layer{layer_idx}.wo",
                       f"{prefix}.self_attn.o_proj.weight",
                       (dim, n_heads * head_dim))
            write_f32(f"layer{layer_idx}.rms_ffn",
                      f"{prefix}.post_attention_layernorm.weight", dim)
            write_fp16(f"layer{layer_idx}.gate",
                       f"{prefix}.mlp.gate_proj.weight",
                       (hidden_dim, dim))
            write_fp16(f"layer{layer_idx}.up",
                       f"{prefix}.mlp.up_proj.weight",
                       (hidden_dim, dim))
            write_fp16(f"layer{layer_idx}.down",
                       f"{prefix}.mlp.down_proj.weight",
                       (dim, hidden_dim))

        print(f"  Exported all {n_layers} layers.          ")

        # 3. Final RMSNorm
        write_f32("rms_final", "model.norm.weight", dim)

        # 4. lm_head (if not tied)
        if not tie_word_embeddings and "lm_head.weight" in state_dict:
            print(f"  Exporting separate lm_head...")
            write_fp16("lm_head", "lm_head.weight", (vocab_size, dim))

        print(f"  Weights: {bytes_written:,} bytes ({bytes_written/1024/1024:.1f} MB)")

    total_size = Path(output_path).stat().st_size
    print(f"\nTotal file size: {total_size:,} bytes ({total_size/1024/1024:.1f} MB)")
    print(f"Done!")


def main():
    parser = argparse.ArgumentParser(description="Export model to hackbot FP16 format")
    parser.add_argument("model", nargs="?",
                        default="HuggingFaceTB/SmolLM2-135M-Instruct",
                        help="HuggingFace model name")
    parser.add_argument("output", nargs="?",
                        default="hackbot-model-fp16.bin",
                        help="Output binary path")
    args = parser.parse_args()

    export_model_fp16(args.model, args.output)


if __name__ == "__main__":
    main()
