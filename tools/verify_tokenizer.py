#!/usr/bin/env python3
"""
verify_tokenizer.py — Verify hackbot's BPE tokenizer against HuggingFace reference.

Replicates the kernel's encode_bpe() exactly, including:
1. GPT-2 byte preprocessing
2. Sorted vocabulary + binary search
3. BPE merge loop with score-based priority

Compares token-by-token with HuggingFace tokenizer output.
"""

import struct
import sys
from pathlib import Path
import numpy as np

MODEL_MAGIC = 0x484B4254

# GPT-2 byte-to-codepoint mapping (matching kernel code)
GPT2_BYTE_TO_CODEPOINT = [
    256, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271,  # 0x00-0x0F
    272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287,  # 0x10-0x1F
    288,  33,  34,  35,  36,  37,  38,  39,  40,  41,  42,  43,  44,  45,  46,  47,  # 0x20-0x2F
     48,  49,  50,  51,  52,  53,  54,  55,  56,  57,  58,  59,  60,  61,  62,  63,  # 0x30-0x3F
     64,  65,  66,  67,  68,  69,  70,  71,  72,  73,  74,  75,  76,  77,  78,  79,  # 0x40-0x4F
     80,  81,  82,  83,  84,  85,  86,  87,  88,  89,  90,  91,  92,  93,  94,  95,  # 0x50-0x5F
     96,  97,  98,  99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111,  # 0x60-0x6F
    112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 289,  # 0x70-0x7F
    290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303, 304, 305,  # 0x80-0x8F
    306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321,  # 0x90-0x9F
    322, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 323, 174, 175,  # 0xA0-0xAF
    176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191,  # 0xB0-0xBF
    192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207,  # 0xC0-0xCF
    208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223,  # 0xD0-0xDF
    224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239,  # 0xE0-0xEF
    240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,  # 0xF0-0xFF
]


class HackbotTokenizer:
    def __init__(self, bin_path):
        data = Path(bin_path).read_bytes()
        header = struct.unpack_from("<14I", data, 0)
        magic = header[0]
        assert magic == MODEL_MAGIC
        self.vocab_size = header[7]

        # Parse tokenizer
        off = 56
        tok_vocab_size, max_token_len = struct.unpack_from("<II", data, off)
        off += 8

        self.tok_scores = []
        self.tok_bytes = []
        for i in range(tok_vocab_size):
            score = struct.unpack_from("<i", data, off)[0]
            off += 4
            tlen = struct.unpack_from("<H", data, off)[0]
            off += 2
            tbytes = data[off:off+tlen]
            off += tlen
            self.tok_scores.append(score)
            self.tok_bytes.append(tbytes)

        # Build sorted vocab (matching kernel heapsort)
        self.sorted_ids = list(range(self.vocab_size))
        self.sorted_ids.sort(key=lambda i: self.tok_bytes[i])

        # Verify sort
        for i in range(len(self.sorted_ids) - 1):
            a = self.tok_bytes[self.sorted_ids[i]]
            b = self.tok_bytes[self.sorted_ids[i+1]]
            assert a <= b, f"Sort broken at {i}: {a} > {b}"

        # Build byte_to_token (matching kernel)
        self.byte_to_token = [0] * 256  # default: TOKEN_ENDOFTEXT
        n_found = 0
        for b in range(256):
            cp = GPT2_BYTE_TO_CODEPOINT[b]
            if cp < 128:
                utf8 = bytes([cp])
            else:
                utf8 = bytes([0xC0 | (cp >> 6), 0x80 | (cp & 0x3F)])

            tid = self._find_token(utf8)
            if tid is not None:
                self.byte_to_token[b] = tid
                n_found += 1

        print(f"Tokenizer: {n_found}/256 byte tokens found")

        # Verify byte_to_token for key bytes
        for b, name in [(0x0A, "newline"), (0x20, "space"), (ord('h'), "h"),
                         (ord('e'), "e"), (ord('l'), "l"), (ord('o'), "o"),
                         (ord('Y'), "Y")]:
            tid = self.byte_to_token[b]
            tok_str = self.tok_bytes[tid].decode('utf-8', errors='replace')
            print(f"  byte_to_token[0x{b:02x}] ({name}) = token {tid} ({repr(tok_str)})")

    def _find_token(self, query):
        """Binary search matching kernel's find_token_by_bytes."""
        lo, hi = 0, len(self.sorted_ids)
        while lo < hi:
            mid = lo + (hi - lo) // 2
            mid_id = self.sorted_ids[mid]
            mid_bytes = self.tok_bytes[mid_id]
            if query == mid_bytes:
                return mid_id
            elif query < mid_bytes:
                hi = mid
            else:
                lo = mid + 1
        return None

    def preprocess_gpt2(self, input_bytes):
        """GPT-2 byte preprocessing matching kernel code."""
        out = bytearray()
        for b in input_bytes:
            cp = GPT2_BYTE_TO_CODEPOINT[b]
            if cp < 128:
                out.append(cp)
            else:
                out.append(0xC0 | (cp >> 6))
                out.append(0x80 | (cp & 0x3F))
        return bytes(out)

    def encode_bpe(self, input_bytes):
        """BPE encoding matching kernel's encode_bpe exactly."""
        if not input_bytes:
            return []

        # Preprocess
        preproc = self.preprocess_gpt2(input_bytes)

        # Initialize token array
        tokens = []
        pi = 0
        while pi < len(preproc):
            b = preproc[pi]
            if b < 0x80:
                tokens.append(self.byte_to_token[b])
                pi += 1
            elif b >= 0xC0 and b < 0xE0 and pi + 1 < len(preproc):
                tid = self._find_token(preproc[pi:pi+2])
                if tid is not None:
                    tokens.append(tid)
                else:
                    tokens.append(0)  # fallback to endoftext
                pi += 2
            else:
                pi += 1

        # BPE merge loop
        while len(tokens) >= 2:
            best_score = -2**31
            best_idx = 0
            best_token = 0
            found = False

            for i in range(len(tokens) - 1):
                bytes_a = self.tok_bytes[tokens[i]]
                bytes_b = self.tok_bytes[tokens[i+1]]
                concat = bytes_a + bytes_b

                if len(concat) > 128:
                    continue

                merged_id = self._find_token(concat)
                if merged_id is not None:
                    score = self.tok_scores[merged_id]
                    if score > best_score:
                        best_score = score
                        best_idx = i
                        best_token = merged_id
                        found = True

            if not found:
                break

            tokens[best_idx] = best_token
            del tokens[best_idx + 1]

        return tokens

    def decode_tokens(self, token_ids):
        """Decode token IDs to bytes using GPT-2 reverse mapping."""
        # Build reverse mapping
        cp_to_byte = {}
        for b in range(256):
            cp_to_byte[GPT2_BYTE_TO_CODEPOINT[b]] = b

        out = bytearray()
        for tid in token_ids:
            tok_bytes = self.tok_bytes[tid]
            # Parse UTF-8 and map codepoints back
            i = 0
            while i < len(tok_bytes):
                b = tok_bytes[i]
                if b < 0x80:
                    cp = b
                    i += 1
                elif b >= 0xC0 and b < 0xE0 and i + 1 < len(tok_bytes):
                    cp = ((b & 0x1F) << 6) | (tok_bytes[i+1] & 0x3F)
                    i += 2
                else:
                    out.append(ord('?'))
                    i += 1
                    continue
                out.append(cp_to_byte.get(cp, ord('?')))
        return bytes(out)


def main():
    bin_path = sys.argv[1] if len(sys.argv) > 1 else "hackbot-model.bin"

    print("=== Building tokenizer from binary model ===")
    tok = HackbotTokenizer(bin_path)

    # Test basic encoding
    test_cases = [
        b"hello",
        b"You are hackbot",
        b"system",
        b"user",
        b"assistant",
    ]

    print("\n=== Encoding Tests ===")
    for text in test_cases:
        tokens = tok.encode_bpe(text)
        decoded = tok.decode_tokens(tokens)
        tok_strs = [repr(tok.tok_bytes[t].decode('utf-8', errors='replace')) for t in tokens]
        print(f"  {repr(text.decode())} → {tokens}")
        print(f"    token strings: {tok_strs}")
        print(f"    roundtrip decode: {repr(decoded.decode('utf-8', errors='replace'))}")
        if decoded != text:
            print(f"    *** ROUNDTRIP MISMATCH! ***")

    # Compare with HuggingFace tokenizer
    print("\n=== Comparison with HuggingFace Tokenizer ===")
    try:
        from transformers import AutoTokenizer
        hf_tok = AutoTokenizer.from_pretrained("HuggingFaceTB/SmolLM2-135M-Instruct")

        for text in test_cases:
            our_tokens = tok.encode_bpe(text)
            hf_tokens = hf_tok.encode(text.decode(), add_special_tokens=False)
            match = our_tokens == hf_tokens
            status = "✓ MATCH" if match else "✗ MISMATCH"
            print(f"  {repr(text.decode())}:")
            print(f"    Ours: {our_tokens}")
            print(f"    HF:   {hf_tokens}")
            print(f"    {status}")
            if not match:
                # Show where they differ
                for i in range(max(len(our_tokens), len(hf_tokens))):
                    ours = our_tokens[i] if i < len(our_tokens) else None
                    hf = hf_tokens[i] if i < len(hf_tokens) else None
                    if ours != hf:
                        ours_str = repr(tok.tok_bytes[ours].decode('utf-8', errors='replace')) if ours else "N/A"
                        hf_str = repr(hf_tok.decode([hf])) if hf else "N/A"
                        print(f"      Diff at pos {i}: ours={ours}({ours_str}) vs hf={hf}({hf_str})")

        # Test the full ChatML assembly
        print("\n=== ChatML Token Assembly ===")
        system_prompt = b"You are hackbot, a kernel agent. Answer concisely. For live system data, use: <tool>ps</tool> <tool>mem</tool> <tool>loadavg</tool>"
        user_prompt = b"hello"
        nl_token = tok.byte_to_token[0x0A]

        # Build ChatML tokens the same way as append_chat_tokens
        chatml_tokens = []

        # <|im_start|>system\n{system_prompt}<|im_end|>\n
        chatml_tokens.append(1)  # TOKEN_IM_START
        chatml_tokens.extend(tok.encode_bpe(b"system"))
        chatml_tokens.append(nl_token)
        chatml_tokens.extend(tok.encode_bpe(system_prompt))
        chatml_tokens.append(2)  # TOKEN_IM_END
        chatml_tokens.append(nl_token)

        # <|im_start|>user\n{user_prompt}<|im_end|>\n
        chatml_tokens.append(1)  # TOKEN_IM_START
        chatml_tokens.extend(tok.encode_bpe(b"user"))
        chatml_tokens.append(nl_token)
        chatml_tokens.extend(tok.encode_bpe(user_prompt))
        chatml_tokens.append(2)  # TOKEN_IM_END
        chatml_tokens.append(nl_token)

        # <|im_start|>assistant\n
        chatml_tokens.append(1)  # TOKEN_IM_START
        chatml_tokens.extend(tok.encode_bpe(b"assistant"))
        chatml_tokens.append(nl_token)

        print(f"Our ChatML tokens ({len(chatml_tokens)}): {chatml_tokens}")

        # Compare with HuggingFace
        messages = [
            {"role": "system", "content": system_prompt.decode()},
            {"role": "user", "content": "hello"},
        ]
        hf_chat = hf_tok.apply_chat_template(messages, tokenize=True, add_generation_prompt=True)
        print(f"HF  ChatML tokens ({len(hf_chat)}): {hf_chat}")

        if chatml_tokens == hf_chat:
            print("✓ ChatML tokens MATCH perfectly!")
        else:
            print("✗ ChatML tokens DIFFER!")
            # Find first difference
            for i in range(max(len(chatml_tokens), len(hf_chat))):
                ours = chatml_tokens[i] if i < len(chatml_tokens) else None
                hf = hf_chat[i] if i < len(hf_chat) else None
                if ours != hf:
                    ours_str = repr(tok.tok_bytes[ours].decode('utf-8', errors='replace')) if ours is not None else "N/A"
                    hf_str = repr(hf_tok.decode([hf])) if hf is not None else "N/A"
                    print(f"  Diff at pos {i}: ours={ours}({ours_str}) vs hf={hf}({hf_str})")
                    if i >= 5:
                        print(f"  ... (more differences)")
                        break

        # Decode both to text for visual comparison
        our_text = tok.decode_tokens(chatml_tokens)
        hf_text = hf_tok.decode(hf_chat, skip_special_tokens=False)
        print(f"\nOur decoded text:\n{repr(our_text.decode('utf-8', errors='replace'))}")
        print(f"\nHF decoded text:\n{repr(hf_text)}")

    except ImportError:
        print("(transformers not available)")


if __name__ == "__main__":
    main()
