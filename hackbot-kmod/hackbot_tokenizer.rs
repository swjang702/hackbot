// SPDX-License-Identifier: GPL-2.0

//! GPT-2 BPE tokenizer and text generation for in-kernel inference.
//!
//! # Unsafe invariants (apply to every `unsafe` block in this file)
//!
//! - **Provenance**: `tok_offsets: *const u32` aliases the `tok_offsets_addr`
//!   region allocated in `parse_and_store_model` for the loaded model.
//!   `sorted: *const u32` aliases `sorted_vocab_addr` allocated in
//!   `build_sorted_vocab`. `data: &[u8]` is constructed from `data_addr` /
//!   `data_len`, the vmalloc'd model blob held alive by the loaded model.
//! - **Lifetime / lock**: every entry point that exposes these pointers runs
//!   while `MODEL.lock()` is held by the caller (see `forward::forward_token`,
//!   `model::load_model_if_needed`); the underlying allocations are freed only
//!   in `free_model_resources`, also under the same lock.
//! - **Bounds**: `tok_offsets` and `sorted` each cover exactly `vocab_size`
//!   `u32` entries (validated at parse time against the model header).
//!   `vocab_size <= MODEL_MAX_VOCAB`. Each `tok_offsets[i]` points at a
//!   6-byte score+length prefix that fits inside `data`, validated by the
//!   `pos + 6 > data.len()` check in `parse_and_store_model`.
//! - **Aliasing**: pointer reads here do not coexist with any `&mut` borrow
//!   over the same region — the offset table and sorted index are immutable
//!   after `build_sorted_vocab` returns; `data` is `&[u8]`.
//!
//! Per-block SAFETY comments below highlight only deviations or tighter
//! sub-range bounds.

use kernel::{bindings, prelude::*};

use crate::config::*;
use crate::forward::{forward_token, reset_kv_cache};
use crate::types::*;

/// GPT-2 byte→Unicode codepoint mapping table.
#[allow(dead_code)]
pub(crate) const GPT2_BYTE_TO_CODEPOINT: [u16; 256] = [
    256, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271,
    272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287,
    288,  33,  34,  35,  36,  37,  38,  39,  40,  41,  42,  43,  44,  45,  46,  47,
     48,  49,  50,  51,  52,  53,  54,  55,  56,  57,  58,  59,  60,  61,  62,  63,
     64,  65,  66,  67,  68,  69,  70,  71,  72,  73,  74,  75,  76,  77,  78,  79,
     80,  81,  82,  83,  84,  85,  86,  87,  88,  89,  90,  91,  92,  93,  94,  95,
     96,  97,  98,  99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111,
    112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 289,
    290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303, 304, 305,
    306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321,
    322, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 323, 174, 175,
    176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191,
    192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207,
    208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223,
    224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239,
    240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,
];

/// GPT-2 Unicode codepoint→raw byte reverse mapping.
#[allow(dead_code)]
pub(crate) const GPT2_CODEPOINT_TO_BYTE: [u8; 324] = {
    let mut table = [0u8; 324];
    let mut b: u16 = 0;
    while b < 256 {
        table[GPT2_BYTE_TO_CODEPOINT[b as usize] as usize] = b as u8;
        b += 1;
    }
    table
};

/// Decode a token ID to its GPT-2 encoded byte representation.
#[allow(dead_code)]
pub(crate) fn decode_token_bytes<'a>(data: &'a [u8], tok_offsets: *const u32, token_id: usize) -> &'a [u8] {
    // SAFETY: file-level invariants apply. Caller passes `token_id < vocab_size`
    // (every call site iterates over `[0, vocab_size)` or over a token id that
    // was produced by the BPE encode/decode path bounded by vocab_size).
    let off = unsafe { *tok_offsets.add(token_id) } as usize;
    let len = u16::from_le_bytes([data[off + 4], data[off + 5]]) as usize;
    &data[off + 6..off + 6 + len]
}

/// Get the BPE merge score for a token.
#[allow(dead_code)]
pub(crate) fn get_token_score(data: &[u8], tok_offsets: *const u32, token_id: usize) -> i32 {
    // SAFETY: file-level invariants apply. `token_id < vocab_size` is enforced
    // by the encode_bpe merge loop which only passes ids returned by
    // find_token_by_bytes (themselves bounded by vocab_size).
    let off = unsafe { *tok_offsets.add(token_id) } as usize;
    i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

/// Decode GPT-2 token bytes to raw bytes.
#[allow(dead_code)]
pub(crate) fn gpt2_decode_token(token_bytes: &[u8], out: &mut [u8]) -> usize {
    let mut i = 0usize;
    let mut o = 0usize;
    while i < token_bytes.len() && o < out.len() {
        let b = token_bytes[i];
        if b < 0x80 {
            if (b as usize) < GPT2_CODEPOINT_TO_BYTE.len() {
                out[o] = GPT2_CODEPOINT_TO_BYTE[b as usize];
            } else {
                out[o] = b'?';
            }
            o += 1;
            i += 1;
        } else if b >= 0xC0 && b < 0xE0 && i + 1 < token_bytes.len() {
            let cp = ((b as u16 & 0x1F) << 6) | (token_bytes[i + 1] as u16 & 0x3F);
            if (cp as usize) < GPT2_CODEPOINT_TO_BYTE.len() {
                out[o] = GPT2_CODEPOINT_TO_BYTE[cp as usize];
            } else {
                out[o] = b'?';
            }
            o += 1;
            i += 2;
        } else {
            out[o] = b'?';
            o += 1;
            i += 1;
        }
    }
    o
}

fn tok_bytes_cmp(data: &[u8], tok_offsets: *const u32, a: u32, b: u32) -> core::cmp::Ordering {
    let bytes_a = decode_token_bytes(data, tok_offsets, a as usize);
    let bytes_b = decode_token_bytes(data, tok_offsets, b as usize);
    bytes_a.cmp(bytes_b)
}

fn heapsort_sift_down(
    arr: &mut [u32], data: &[u8], tok_offsets: *const u32,
    start: usize, end: usize,
) {
    let mut root = start;
    loop {
        let left = 2 * root + 1;
        if left >= end {
            break;
        }
        let right = left + 1;
        let mut largest = root;

        if tok_bytes_cmp(data, tok_offsets, arr[left], arr[largest])
            == core::cmp::Ordering::Greater
        {
            largest = left;
        }
        if right < end
            && tok_bytes_cmp(data, tok_offsets, arr[right], arr[largest])
                == core::cmp::Ordering::Greater
        {
            largest = right;
        }
        if largest == root {
            break;
        }
        arr.swap(root, largest);
        root = largest;
    }
}

fn heapsort_vocab(arr: &mut [u32], data: &[u8], tok_offsets: *const u32) {
    let n = arr.len();
    if n <= 1 {
        return;
    }

    let mut i = n / 2;
    while i > 0 {
        i -= 1;
        heapsort_sift_down(arr, data, tok_offsets, i, n);
    }

    let mut end = n;
    while end > 1 {
        end -= 1;
        arr.swap(0, end);
        heapsort_sift_down(arr, data, tok_offsets, 0, end);
    }
}

/// Binary search the sorted vocabulary for a token matching the given bytes.
#[allow(dead_code)]
pub(crate) fn find_token_by_bytes(
    data: &[u8], tok_offsets: *const u32, sorted: *const u32,
    vocab_size: usize, query: &[u8],
) -> Option<u32> {
    let mut lo = 0usize;
    let mut hi = vocab_size;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // SAFETY: file-level invariants apply. `mid < hi <= vocab_size`,
        // and `sorted` covers exactly vocab_size u32 entries.
        let mid_id = unsafe { *sorted.add(mid) };
        let mid_bytes = decode_token_bytes(data, tok_offsets, mid_id as usize);
        match query.cmp(mid_bytes) {
            core::cmp::Ordering::Equal => return Some(mid_id),
            core::cmp::Ordering::Less => hi = mid,
            core::cmp::Ordering::Greater => lo = mid + 1,
        }
    }
    None
}

/// Build the sorted vocabulary index and byte-to-token lookup table.
pub(crate) fn build_sorted_vocab(slot: &mut ModelSlot) -> Result {
    let vocab_size = slot.config.vocab_size as usize;
    // SAFETY: file-level invariants apply. `data_addr`/`data_len` describe the
    // model blob held alive by the loaded slot; caller holds MODEL.lock().
    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let tok_offsets = slot.tok_offsets_addr as *const u32;

    let alloc_size = vocab_size * core::mem::size_of::<u32>();
    // SAFETY: FFI to kvrealloc with NULL `old` (fresh allocation). `alloc_size`
    // is bounded by MODEL_MAX_VOCAB * sizeof(u32) and cannot overflow usize.
    // Alignment is the natural alignment of u32. GFP_KERNEL is legal here:
    // build_sorted_vocab is called from load_model_if_needed in process
    // context (firmware load handler).
    let ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            alloc_size,
            core::mem::align_of::<u32>() as _,
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    } as *mut u32;

    if ptr.is_null() {
        pr_err!("hackbot: failed to allocate sorted vocab ({} bytes)\n", alloc_size);
        return Err(ENOMEM);
    }

    // SAFETY: `ptr` is non-null (checked above), points to `vocab_size` freshly
    // allocated u32s we own exclusively, and is u32-aligned. The borrow is
    // confined to the heapsort_vocab call below; no other view aliases this
    // region during the sort.
    let sorted = unsafe { core::slice::from_raw_parts_mut(ptr, vocab_size) };
    for i in 0..vocab_size {
        sorted[i] = i as u32;
    }

    heapsort_vocab(sorted, data, tok_offsets);

    slot.sorted_vocab_addr = ptr as usize;

    slot.byte_to_token = [TOKEN_ENDOFTEXT; 256];
    let mut n_found = 0u32;

    for b in 0u16..256 {
        let cp = GPT2_BYTE_TO_CODEPOINT[b as usize];
        let mut utf8 = [0u8; 2];
        let utf8_len;
        if cp < 128 {
            utf8[0] = cp as u8;
            utf8_len = 1;
        } else {
            utf8[0] = 0xC0 | ((cp >> 6) as u8);
            utf8[1] = 0x80 | ((cp & 0x3F) as u8);
            utf8_len = 2;
        }

        if let Some(tid) = find_token_by_bytes(data, tok_offsets, ptr, vocab_size, &utf8[..utf8_len]) {
            slot.byte_to_token[b as usize] = tid;
            n_found += 1;
        }
    }

    pr_info!("hackbot: tokenizer ready: sorted vocab + {}/256 byte tokens\n", n_found);
    pr_info!("hackbot: byte_to_token[0x0A](nl)={}, [0x20](sp)={}, [0x68](h)={}, [0x59](Y)={}\n",
             slot.byte_to_token[0x0A], slot.byte_to_token[0x20],
             slot.byte_to_token[0x68], slot.byte_to_token[0x59]);
    Ok(())
}

/// Preprocess raw input bytes for GPT-2 BPE encoding.
#[allow(dead_code)]
pub(crate) fn preprocess_gpt2(input: &[u8], out: &mut [u8]) -> usize {
    let mut pos = 0usize;
    for &b in input {
        let cp = GPT2_BYTE_TO_CODEPOINT[b as usize];
        if cp < 128 {
            if pos >= out.len() {
                break;
            }
            out[pos] = cp as u8;
            pos += 1;
        } else {
            if pos + 1 >= out.len() {
                break;
            }
            out[pos] = 0xC0 | ((cp >> 6) as u8);
            out[pos + 1] = 0x80 | ((cp & 0x3F) as u8);
            pos += 2;
        }
    }
    pos
}

/// Encode a byte string into BPE token IDs.
#[allow(dead_code)]
pub(crate) fn encode_bpe(slot: &ModelSlot, input: &[u8], out_tokens: &mut [u32]) -> usize {
    if input.is_empty() || out_tokens.is_empty() {
        return 0;
    }

    // SAFETY: file-level invariants apply. `data_addr`/`data_len` describe the
    // model blob held alive by the loaded slot; caller holds MODEL.lock().
    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let tok_offsets = slot.tok_offsets_addr as *const u32;
    let sorted = slot.sorted_vocab_addr as *const u32;
    let vocab_size = slot.config.vocab_size as usize;

    let mut preproc_buf = [0u8; MAX_PREPROC_INPUT];
    let preproc = &mut preproc_buf[..];
    let preproc_len = preprocess_gpt2(input, preproc);
    let preproc = &preproc[..preproc_len];

    let mut len = 0usize;
    let mut pi = 0usize;
    while pi < preproc_len && len < out_tokens.len() {
        let b = preproc[pi];
        if b < 0x80 {
            out_tokens[len] = slot.byte_to_token[b as usize];
            len += 1;
            pi += 1;
        } else if b >= 0xC0 && b < 0xE0 && pi + 1 < preproc_len {
            if let Some(tid) = find_token_by_bytes(data, tok_offsets, sorted, vocab_size, &preproc[pi..pi + 2]) {
                out_tokens[len] = tid;
            } else {
                out_tokens[len] = TOKEN_ENDOFTEXT;
            }
            len += 1;
            pi += 2;
        } else {
            pi += 1;
        }
    }

    // BPE merge loop
    let mut concat_buf = [0u8; 128];
    loop {
        if len < 2 {
            break;
        }

        let mut best_score = i32::MIN;
        let mut best_idx = 0usize;
        let mut best_token = 0u32;
        let mut found = false;

        for i in 0..len - 1 {
            let bytes_a = decode_token_bytes(data, tok_offsets, out_tokens[i] as usize);
            let bytes_b = decode_token_bytes(data, tok_offsets, out_tokens[i + 1] as usize);

            let total = bytes_a.len() + bytes_b.len();
            if total > 128 {
                continue;
            }

            concat_buf[..bytes_a.len()].copy_from_slice(bytes_a);
            concat_buf[bytes_a.len()..total].copy_from_slice(bytes_b);

            if let Some(merged_id) =
                find_token_by_bytes(data, tok_offsets, sorted, vocab_size, &concat_buf[..total])
            {
                let score = get_token_score(data, tok_offsets, merged_id as usize);
                if score > best_score {
                    best_score = score;
                    best_idx = i;
                    best_token = merged_id;
                    found = true;
                }
            }
        }

        if !found {
            break;
        }

        out_tokens[best_idx] = best_token;
        let mut i = best_idx + 1;
        while i < len - 1 {
            out_tokens[i] = out_tokens[i + 1];
            i += 1;
        }
        len -= 1;
    }

    len
}

/// Get the next token prediction from logits buffer (argmax).
#[allow(dead_code)]
pub(crate) fn get_next_token(slot: &ModelSlot) -> usize {
    if slot.format_version == MODEL_FORMAT_V2 {
        // SAFETY: FFI to hackbot_fpu.c. `fpu_state` was returned by
        // hackbot_fpu_alloc for this slot and is freed only on model unload
        // under MODEL.lock(). Caller holds MODEL.lock() across this call.
        let tok = unsafe {
            hackbot_fpu_get_next_token(slot.fpu_state as *const core::ffi::c_void)
        };
        return tok as usize;
    }
    let logits_ptr = (slot.inf_buf_addr as *const i32).wrapping_add(slot.inf_logits);
    let vocab_size = slot.config.vocab_size as usize;
    let mut best = 0usize;
    for i in 1..vocab_size {
        // SAFETY: logits region is `vocab_size` i32 elements long (allocated
        // in alloc_inference_state); both `i` and `best` are < vocab_size.
        // Caller holds MODEL.lock(); no &mut over the inf allocation is live.
        if unsafe { *logits_ptr.add(i) > *logits_ptr.add(best) } {
            best = i;
        }
    }
    best
}

/// Generate text from a pre-built token array using the in-kernel model.
#[allow(dead_code)]
pub(crate) fn generate_from_tokens(
    slot: &ModelSlot,
    prompt_tokens: &[u32],
    n_prompt: usize,
    output: &mut [u8],
    max_new_tokens: usize,
) -> usize {
    if n_prompt == 0 || n_prompt > INFERENCE_MAX_SEQ {
        return 0;
    }

    // SAFETY: file-level invariants apply. `data_addr`/`data_len` describe the
    // model blob held alive by the loaded slot; caller holds MODEL.lock().
    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let tok_offsets = slot.tok_offsets_addr as *const u32;

    reset_kv_cache(slot);

    // Prefill. forward_token returns Err only when the model state is
    // malformed (see R-025); abort generation rather than continuing with
    // a half-filled KV cache.
    for i in 0..n_prompt {
        if forward_token(slot, prompt_tokens[i] as usize, i).is_err() {
            pr_err!("hackbot: forward_token failed during prefill at token {}\n", i);
            return 0;
        }
    }

    // Autoregressive generation
    let mut pos = n_prompt;
    let mut out_len = 0usize;
    let mut next_token = get_next_token(slot);
    let gen_limit = max_new_tokens.min(INFERENCE_MAX_SEQ.saturating_sub(pos));

    pr_info!("hackbot: gen start: first_token={}, gen_limit={}, pos={}\n",
             next_token, gen_limit, pos);

    let mut decode_buf = [0u8; 64];

    for _ in 0..gen_limit {
        let tok = next_token as u32;
        if tok == TOKEN_ENDOFTEXT || tok == TOKEN_IM_END {
            pr_info!("hackbot: gen stop: token {} (EOS/IM_END) at pos {}\n", tok, pos);
            break;
        }
        if pos >= INFERENCE_MAX_SEQ {
            pr_info!("hackbot: gen stop: max seq at pos {}\n", pos);
            break;
        }

        let tok_bytes = decode_token_bytes(data, tok_offsets, next_token);
        let raw_len = gpt2_decode_token(tok_bytes, &mut decode_buf);
        let copy_len = raw_len.min(output.len().saturating_sub(out_len));
        if copy_len == 0 {
            break;
        }
        output[out_len..out_len + copy_len].copy_from_slice(&decode_buf[..copy_len]);
        out_len += copy_len;

        if forward_token(slot, next_token, pos).is_err() {
            pr_err!("hackbot: forward_token failed during gen at pos {}\n", pos);
            break;
        }
        pos += 1;
        next_token = get_next_token(slot);
    }

    out_len
}

/// Generate text from a raw text prompt.
#[allow(dead_code)]
pub(crate) fn generate(
    slot: &ModelSlot, prompt: &[u8], output: &mut [u8], max_new_tokens: usize,
) -> usize {
    let mut prompt_tokens = [0u32; 512];
    let n_encoded = encode_bpe(slot, prompt, &mut prompt_tokens);
    let n_prompt = n_encoded.min(INFERENCE_MAX_SEQ.saturating_sub(max_new_tokens));

    if n_prompt == 0 {
        return 0;
    }

    generate_from_tokens(slot, &prompt_tokens, n_prompt, output, max_new_tokens)
}
