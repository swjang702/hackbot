#!/usr/bin/env python3
"""
export_hackbot.py — Convert a HuggingFace LlamaForCausalLM model to hackbot binary format.

Target: SmolLM2-135M-Instruct (or any Llama-architecture model).

Binary format v1:
  HEADER (56 bytes) — model config
  TOKENIZER — vocab entries with BPE merge scores
  WEIGHTS — INT8 quantized weights with Q16.16 fixed-point scales

Usage:
  python3 export_hackbot.py HuggingFaceTB/SmolLM2-135M-Instruct hackbot-model.bin
  sudo cp hackbot-model.bin /lib/firmware/hackbot-model.bin
"""

import argparse
import json
import struct
import sys
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer, AutoConfig


# === Constants ===

MAGIC = 0x484B4254  # "HKBT" in little-endian
FORMAT_VERSION = 1
Q16_SHIFT = 16  # Q16.16 fixed-point: multiply float by 2^16, cast to int32


# === Quantization ===

def quantize_tensor_q8(tensor: np.ndarray, group_size: int) -> tuple[np.ndarray, np.ndarray]:
    """Quantize a 2D float tensor to INT8 with per-group Q16.16 scales.

    Args:
        tensor: float32 array of shape [rows, cols]
        group_size: number of elements per quantization group

    Returns:
        q_data: int8 array of shape [rows, cols]
        q_scales: int32 array of shape [rows, cols // group_size] (Q16.16 fixed-point)
    """
    assert tensor.ndim == 2, f"Expected 2D tensor, got {tensor.ndim}D"
    rows, cols = tensor.shape
    assert cols % group_size == 0, f"cols={cols} not divisible by group_size={group_size}"

    n_groups = cols // group_size
    # Reshape to [rows, n_groups, group_size]
    reshaped = tensor.reshape(rows, n_groups, group_size)

    # Per-group max absolute value
    max_abs = np.max(np.abs(reshaped), axis=2)  # [rows, n_groups]
    # Avoid division by zero
    max_abs = np.maximum(max_abs, 1e-10)

    # Scale: maps [-max_abs, max_abs] to [-127, 127]
    # scale = max_abs / 127.0
    # To dequantize: float_val = int8_val * scale
    # As Q16.16: scale_q16 = round(scale * 2^16)
    scales_float = max_abs / 127.0
    scales_q16 = np.round(scales_float * (1 << Q16_SHIFT)).astype(np.int32)

    # Quantize: q = round(val / scale) clamped to [-127, 127]
    # Expand scales for broadcasting: [rows, n_groups, 1]
    scales_expanded = scales_float[:, :, np.newaxis]
    q_float = reshaped / scales_expanded
    q_clamped = np.clip(np.round(q_float), -127, 127)
    q_data = q_clamped.astype(np.int8).reshape(rows, cols)

    return q_data, scales_q16


def float_to_q16(arr: np.ndarray) -> np.ndarray:
    """Convert float32 array to Q16.16 fixed-point int32."""
    return np.round(arr * (1 << Q16_SHIFT)).astype(np.int32)


# === Tokenizer Export ===

def export_tokenizer(tokenizer, vocab_size: int) -> bytes:
    """Export tokenizer vocabulary to binary format.

    Format per token:
      score: i32 (merge priority — higher = merge first)
      len: u16
      bytes: [u8; len]
    """
    # Build token id → bytes mapping
    # Use the tokenizer's convert_ids_to_tokens and encode back to bytes
    id_to_bytes = {}
    for token_id in range(vocab_size):
        # Get the token string
        token_str = tokenizer.convert_ids_to_tokens(token_id)
        if token_str is None:
            token_str = ""

        # Convert to bytes. HuggingFace tokenizers may use special Unicode
        # representations for byte-level tokens (e.g., 'Ġ' for space).
        # Use the tokenizer's decode to get actual bytes where possible.
        try:
            # For byte-level BPE: tokens like 'Ä' represent raw bytes
            # The tokenizer's backend handles the conversion
            token_bytes = token_str.encode("utf-8")
        except (UnicodeEncodeError, UnicodeDecodeError):
            token_bytes = token_str.encode("utf-8", errors="replace")

        id_to_bytes[token_id] = token_bytes

    # Build merge scores from the tokenizer's merges list
    # Try to load merges from tokenizer.json
    scores = [0] * vocab_size  # default: 0 (base tokens, no merge priority)

    tokenizer_json_path = None
    if hasattr(tokenizer, "name_or_path"):
        # Try local cache
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

        # Build a lookup: merged_token_bytes → merge rank
        # Each merge "tok_a tok_b" produces a merged token
        # Score = n_merges - index (higher index = later merge = lower priority)
        vocab_str_to_id = tokenizer.get_vocab()

        for merge_idx, merge_str in enumerate(merges):
            # merge_str is "token_a token_b"
            parts = merge_str.split(" ", 1)
            if len(parts) != 2:
                continue
            merged = parts[0] + parts[1]
            if merged in vocab_str_to_id:
                token_id = vocab_str_to_id[merged]
                # Higher score = higher merge priority
                scores[token_id] = n_merges - merge_idx
    else:
        print("  WARNING: Could not find tokenizer.json, using default scores")

    # Build binary
    buf = bytearray()
    max_token_len = max(len(b) for b in id_to_bytes.values()) if id_to_bytes else 0

    # Header: n_vocab, max_token_len
    buf.extend(struct.pack("<II", vocab_size, max_token_len))

    for token_id in range(vocab_size):
        token_bytes = id_to_bytes.get(token_id, b"")
        score = scores[token_id]
        buf.extend(struct.pack("<i", score))        # i32 score
        buf.extend(struct.pack("<H", len(token_bytes)))  # u16 length
        buf.extend(token_bytes)                      # raw bytes

    return bytes(buf)


# === Main Export ===

def export_model(model_name: str, output_path: str, group_size: int = 64):
    """Export a HuggingFace Llama model to hackbot binary format."""

    print(f"Loading model: {model_name}")
    config = AutoConfig.from_pretrained(model_name)

    # Validate architecture
    assert config.model_type in ("llama",), \
        f"Unsupported model type: {config.model_type}. Expected 'llama'."

    # Extract config values
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
    print(f"  group_size={group_size}")

    # Validate group size divides all relevant dimensions
    for name, size in [("dim", dim), ("hidden_dim", hidden_dim), ("kv_dim", kv_dim)]:
        if size % group_size != 0:
            print(f"WARNING: {name}={size} not divisible by group_size={group_size}")
            # Find the largest valid group size
            while size % group_size != 0:
                group_size //= 2
            print(f"  Adjusted group_size to {group_size}")
            break

    print(f"\nLoading tokenizer...")
    tokenizer = AutoTokenizer.from_pretrained(model_name)

    # Get special token IDs
    eos_id = tokenizer.eos_token_id
    bos_id = tokenizer.bos_token_id
    print(f"  BOS={bos_id}, EOS={eos_id}")

    print(f"\nLoading model weights (this may download ~270MB)...")
    model = AutoModelForCausalLM.from_pretrained(
        model_name, torch_dtype=torch.float32
    )
    state_dict = model.state_dict()

    # Print all weight names for debugging
    print(f"\nWeight tensors ({len(state_dict)} total):")
    total_params = 0
    for name, tensor in state_dict.items():
        total_params += tensor.numel()
        print(f"  {name}: {list(tensor.shape)}")
    print(f"  Total parameters: {total_params:,}")

    # === Write binary file ===

    print(f"\nExporting to {output_path}...")
    with open(output_path, "wb") as f:
        # --- HEADER (56 bytes) ---
        header = struct.pack(
            "<14I",  # 14 × uint32 = 56 bytes
            MAGIC,
            FORMAT_VERSION,
            dim,
            hidden_dim,
            n_layers,
            n_heads,
            n_kv_heads,
            vocab_size,
            seq_len,
            group_size,
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

        def write_quantized(name: str, key: str, expected_shape: tuple[int, int]):
            """Quantize and write a weight matrix."""
            nonlocal bytes_written
            w = state_dict[key].numpy().astype(np.float32)
            assert w.shape == expected_shape, \
                f"{name}: expected {expected_shape}, got {w.shape}"

            q_data, q_scales = quantize_tensor_q8(w, group_size)

            q_bytes = q_data.tobytes()
            s_bytes = q_scales.tobytes()
            f.write(q_bytes)
            f.write(s_bytes)
            bytes_written += len(q_bytes) + len(s_bytes)

        def write_norm(name: str, key: str, expected_len: int):
            """Convert RMSNorm weight to Q16.16 and write."""
            nonlocal bytes_written
            w = state_dict[key].numpy().astype(np.float32)
            assert w.shape == (expected_len,), \
                f"{name}: expected ({expected_len},), got {w.shape}"

            w_q16 = float_to_q16(w)
            w_bytes = w_q16.tobytes()
            f.write(w_bytes)
            bytes_written += len(w_bytes)

        # 1. Embedding table
        print(f"  Exporting embedding table...")
        write_quantized(
            "embed_tokens",
            "model.embed_tokens.weight",
            (vocab_size, dim),
        )

        # 2. Per-layer weights
        for layer_idx in range(n_layers):
            prefix = f"model.layers.{layer_idx}"
            print(f"  Exporting layer {layer_idx}/{n_layers}...", end="\r")

            # RMSNorm (attention)
            write_norm(
                f"layer{layer_idx}.rms_att",
                f"{prefix}.input_layernorm.weight",
                dim,
            )

            # Attention projections
            write_quantized(
                f"layer{layer_idx}.wq",
                f"{prefix}.self_attn.q_proj.weight",
                (n_heads * head_dim, dim),
            )
            write_quantized(
                f"layer{layer_idx}.wk",
                f"{prefix}.self_attn.k_proj.weight",
                (n_kv_heads * head_dim, dim),
            )
            write_quantized(
                f"layer{layer_idx}.wv",
                f"{prefix}.self_attn.v_proj.weight",
                (n_kv_heads * head_dim, dim),
            )
            write_quantized(
                f"layer{layer_idx}.wo",
                f"{prefix}.self_attn.o_proj.weight",
                (dim, n_heads * head_dim),
            )

            # RMSNorm (FFN)
            write_norm(
                f"layer{layer_idx}.rms_ffn",
                f"{prefix}.post_attention_layernorm.weight",
                dim,
            )

            # FFN projections
            write_quantized(
                f"layer{layer_idx}.gate",
                f"{prefix}.mlp.gate_proj.weight",
                (hidden_dim, dim),
            )
            write_quantized(
                f"layer{layer_idx}.up",
                f"{prefix}.mlp.up_proj.weight",
                (hidden_dim, dim),
            )
            write_quantized(
                f"layer{layer_idx}.down",
                f"{prefix}.mlp.down_proj.weight",
                (dim, hidden_dim),
            )

        print(f"  Exported all {n_layers} layers.          ")

        # 3. Final RMSNorm
        write_norm("rms_final", "model.norm.weight", dim)

        # 4. lm_head (if not tied to embeddings)
        if not tie_word_embeddings and "lm_head.weight" in state_dict:
            print(f"  Exporting separate lm_head...")
            write_quantized(
                "lm_head",
                "lm_head.weight",
                (vocab_size, dim),
            )
        else:
            print(f"  lm_head tied to embed_tokens (no separate weights)")

        print(f"  Weights: {bytes_written:,} bytes")

    # Summary
    file_size = Path(output_path).stat().st_size
    print(f"\nExport complete!")
    print(f"  Output: {output_path}")
    print(f"  File size: {file_size:,} bytes ({file_size / (1024*1024):.1f} MB)")
    print(f"  Header: 56 bytes")
    print(f"  Tokenizer: {len(tok_data):,} bytes")
    print(f"  Weights: {bytes_written:,} bytes")
    print(f"\nTo install:")
    print(f"  sudo cp {output_path} /lib/firmware/hackbot-model.bin")


def main():
    parser = argparse.ArgumentParser(
        description="Export HuggingFace Llama model to hackbot binary format"
    )
    parser.add_argument(
        "model",
        help="HuggingFace model name (e.g., HuggingFaceTB/SmolLM2-135M-Instruct)",
    )
    parser.add_argument(
        "output",
        help="Output binary file path",
    )
    parser.add_argument(
        "--group-size",
        type=int,
        default=64,
        help="Quantization group size (default: 64)",
    )
    args = parser.parse_args()

    export_model(args.model, args.output, args.group_size)


if __name__ == "__main__":
    main()
