// SPDX-License-Identifier: GPL-2.0

//! Model firmware loading, header parsing, and resource cleanup.
//!
//! # Unsafe invariants (apply to every `unsafe` block in this file)
//!
//! - **Provenance**: `tok_ptr` is a fresh allocation from
//!   `kvrealloc_node_align_noprof` with NULL `old`, sized to
//!   `vocab_size * sizeof(u32)`. `data_ptr` is a fresh allocation sized to
//!   `fw_len`. `slot.*_addr` fields hold the same pointers cast to usize and
//!   are valid until the matching `kvfree` in the cleanup paths or in
//!   `free_model_resources`.
//! - **Lifetime / lock**: every entry point in this file (`load_model_if_needed`,
//!   `free_model_resources`, `parse_and_store_model`) holds `MODEL.lock()` for
//!   its entire duration. The model blob is owned by the slot until either a
//!   parse failure path runs `kvfree(data_ptr)` or `free_model_resources` runs.
//! - **Bounds**: `parse_and_store_model` validates the firmware blob byte by
//!   byte: the magic / version / header dims are checked; `pos + 6 <= data.len()`
//!   gates each `*tok_ptr.add(i) = pos as u32` write; weight cursor advancement
//!   uses `*cursor > data_len` checks via `q8_ref_advance` /
//!   `norm_ref_advance`. `vocab_size <= MODEL_MAX_VOCAB`,
//!   `n_layers <= MODEL_MAX_LAYERS`.
//! - **Aliasing**: `tok_ptr` is written exactly once per index `i` from a
//!   single thread (under MODEL.lock()); no `&mut` view aliases it during the
//!   write loop. `data_ptr` is filled by a single `copy_nonoverlapping` and
//!   then exposed only as `&[u8]`.
//!
//! Per-block SAFETY comments below highlight only deviations or details
//! specific to that call site (alignment, size argument, cleanup pairing).

use kernel::{bindings, c_str, device::Device, firmware::Firmware, prelude::*};

use crate::config::*;
use crate::forward::alloc_inference_state;
use crate::state::MODEL;
use crate::tokenizer::build_sorted_vocab;
use crate::types::*;

/// Read a little-endian u32 from a byte slice at the given offset.
pub(crate) fn read_u32_le(data: &[u8], off: usize) -> Result<u32> {
    if off + 4 > data.len() {
        return Err(EINVAL);
    }
    Ok(u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]))
}

/// Read a little-endian u16 from a byte slice at the given offset.
pub(crate) fn read_u16_le(data: &[u8], off: usize) -> Result<u16> {
    if off + 2 > data.len() {
        return Err(EINVAL);
    }
    Ok(u16::from_le_bytes([data[off], data[off + 1]]))
}

/// Advance cursor past a Q8 weight matrix and return a Q8Ref.
pub(crate) fn q8_ref_advance(cursor: &mut usize, rows: usize, cols: usize, gs: usize, data_len: usize) -> Result<Q8Ref> {
    let data_off = *cursor;
    let data_size = rows * cols;
    *cursor += data_size;

    let scale_off = *cursor;
    let n_groups = cols / gs;
    let scale_size = rows * n_groups * 4;
    *cursor += scale_size;

    if *cursor > data_len {
        return Err(EINVAL);
    }

    Ok(Q8Ref { data_off, scale_off, rows, cols })
}

/// Advance cursor past a RMSNorm weight and return its offset.
pub(crate) fn norm_ref_advance(cursor: &mut usize, dim: usize, data_len: usize) -> Result<usize> {
    let off = *cursor;
    *cursor += dim * 4;
    if *cursor > data_len {
        return Err(EINVAL);
    }
    Ok(off)
}

/// Parse the binary header and populate a ModelConfig.
fn parse_model_header(data: &[u8]) -> Result<ModelConfig> {
    if data.len() < MODEL_HEADER_SIZE {
        pr_err!("hackbot: model file too small ({} bytes, need >= {})\n",
                data.len(), MODEL_HEADER_SIZE);
        return Err(EINVAL);
    }

    let magic = read_u32_le(data, 0)?;
    if magic != MODEL_MAGIC {
        pr_err!("hackbot: bad model magic: 0x{:08X} (expected 0x{:08X})\n",
                magic, MODEL_MAGIC);
        return Err(EINVAL);
    }

    let version = read_u32_le(data, 4)?;
    if version != MODEL_FORMAT_V1 && version != MODEL_FORMAT_V2 {
        pr_err!("hackbot: unsupported model version: {} (expected {} or {})\n",
                version, MODEL_FORMAT_V1, MODEL_FORMAT_V2);
        return Err(EINVAL);
    }

    let config = ModelConfig {
        dim:        read_u32_le(data, 8)?,
        hidden_dim: read_u32_le(data, 12)?,
        n_layers:   read_u32_le(data, 16)?,
        n_heads:    read_u32_le(data, 20)?,
        n_kv_heads: read_u32_le(data, 24)?,
        vocab_size: read_u32_le(data, 28)?,
        seq_len:    read_u32_le(data, 32)?,
        group_size: read_u32_le(data, 36)?,
        head_dim:   read_u32_le(data, 40)?,
        kv_dim:     read_u32_le(data, 44)?,
        rope_theta: read_u32_le(data, 48)?,
    };

    if config.n_layers as usize > MODEL_MAX_LAYERS {
        pr_err!("hackbot: too many layers: {} (max {})\n",
                config.n_layers, MODEL_MAX_LAYERS);
        return Err(EINVAL);
    }
    if config.vocab_size as usize > MODEL_MAX_VOCAB {
        pr_err!("hackbot: vocab too large: {} (max {})\n",
                config.vocab_size, MODEL_MAX_VOCAB);
        return Err(EINVAL);
    }
    if config.dim == 0 || config.hidden_dim == 0 || config.head_dim == 0 {
        pr_err!("hackbot: invalid model dimensions\n");
        return Err(EINVAL);
    }
    if version == MODEL_FORMAT_V1 {
        if config.group_size == 0 {
            pr_err!("hackbot: group_size cannot be zero\n");
            return Err(EINVAL);
        }
        let gs = config.group_size as usize;
        if config.dim as usize % gs != 0 {
            pr_err!("hackbot: dim {} not divisible by group_size {}\n", config.dim, gs);
            return Err(EINVAL);
        }
        if config.hidden_dim as usize % gs != 0 {
            pr_err!("hackbot: hidden_dim {} not divisible by group_size {}\n",
                    config.hidden_dim, gs);
            return Err(EINVAL);
        }
    }

    Ok(config)
}

/// Parse and store the model from a firmware data blob into a ModelSlot.
fn parse_and_store_model(data: &[u8], slot: &mut ModelSlot) -> Result {
    let config = parse_model_header(data)?;
    let _gs = config.group_size as usize;
    let dim = config.dim as usize;
    let hidden_dim = config.hidden_dim as usize;
    let n_layers = config.n_layers as usize;
    let n_heads = config.n_heads as usize;
    let n_kv_heads = config.n_kv_heads as usize;
    let head_dim = config.head_dim as usize;
    let vocab_size = config.vocab_size as usize;

    pr_info!("hackbot: model config: dim={}, hidden_dim={}, layers={}, heads={}/{}, vocab={}\n",
             dim, hidden_dim, n_layers, n_heads, n_kv_heads, vocab_size);

    // --- Parse tokenizer section ---
    let tok_section_off = MODEL_HEADER_SIZE;
    let mut pos = tok_section_off;

    let tok_n_vocab = read_u32_le(data, pos)? as usize;
    pos += 4;
    let _tok_max_len = read_u32_le(data, pos)?;
    pos += 4;

    if tok_n_vocab != vocab_size {
        pr_err!("hackbot: tokenizer vocab mismatch: {} vs header {}\n",
                tok_n_vocab, vocab_size);
        return Err(EINVAL);
    }

    let tok_alloc_size = vocab_size * core::mem::size_of::<u32>();
    // SAFETY: file-level invariants apply. Fresh allocation: `old` is NULL,
    // size is bounded by MODEL_MAX_VOCAB * 4, alignment is u32's natural
    // alignment, GFP_KERNEL is legal in firmware-load (process) context.
    let tok_ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            tok_alloc_size,
            core::mem::align_of::<u32>() as _,
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    } as *mut u32;

    if tok_ptr.is_null() {
        pr_err!("hackbot: failed to allocate tokenizer index ({} bytes)\n", tok_alloc_size);
        return Err(ENOMEM);
    }

    for i in 0..vocab_size {
        if pos + 6 > data.len() {
            // SAFETY: file-level invariants apply. `tok_ptr` was returned by
            // kvrealloc above and has not been freed elsewhere; release on the
            // error path before returning.
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: tokenizer truncated at token {}\n", i);
            return Err(EINVAL);
        }
        // SAFETY: file-level invariants apply. `i < vocab_size` and `tok_ptr`
        // points to `vocab_size` u32 entries; this single-threaded write does
        // not race with any reader (sorted vocab is built only after parse
        // returns Ok).
        unsafe { *tok_ptr.add(i) = pos as u32 };
        pos += 4;
        let token_len = read_u16_le(data, pos)? as usize;
        pos += 2;
        pos += token_len;
    }

    if pos > data.len() {
        // SAFETY: cleanup of `tok_ptr` from the kvrealloc above; same rationale
        // as the in-loop free site.
        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
        pr_err!("hackbot: tokenizer extends past end of file\n");
        return Err(EINVAL);
    }

    pr_info!("hackbot: tokenizer parsed: {} tokens, {} bytes\n",
             vocab_size, pos - tok_section_off);

    // --- Parse weight offsets ---
    let mut cursor = pos;
    let weights_start = pos;

    if config.group_size == 0 {
        // Format v2: FP16 weights
        let mut expected = 0usize;
        expected += vocab_size * dim * 2;
        for _l in 0..n_layers {
            expected += dim * 4;
            expected += n_heads * head_dim * dim * 2;
            expected += n_kv_heads * head_dim * dim * 2;
            expected += n_kv_heads * head_dim * dim * 2;
            expected += dim * n_heads * head_dim * 2;
            expected += dim * 4;
            expected += hidden_dim * dim * 2;
            expected += hidden_dim * dim * 2;
            expected += dim * hidden_dim * 2;
        }
        expected += dim * 4;

        let available = data.len().saturating_sub(weights_start);
        if available < expected {
            // SAFETY: cleanup of `tok_ptr` allocated above; same rationale as
            // the tokenizer-loop cleanup sites.
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: v2 weights truncated: need {} bytes, have {}\n",
                    expected, available);
            return Err(EINVAL);
        }

        cursor = weights_start + expected;
        if cursor < data.len() {
            pr_warn!("hackbot: model has {} trailing bytes\n", data.len() - cursor);
        }

        pr_info!("hackbot: v2 (FP16) weights: {} bytes ({} layers)\n",
                 expected, n_layers);

        slot.config = config;
        slot.config.group_size = 0;
        slot.tok_section_off = tok_section_off;
        slot.tok_offsets_addr = tok_ptr as usize;
        slot.embed = Q8Ref::ZERO;
        slot.layers = [LayerRef::ZERO; MODEL_MAX_LAYERS];
        slot.rms_final_off = 0;
        slot.format_version = MODEL_FORMAT_V2;
        slot.weights_off = weights_start;

        return Ok(());
    }

    // Format v1: INT8 weights
    let gs = config.group_size as usize;
    if dim % gs != 0 {
        // SAFETY: cleanup of `tok_ptr`; same rationale as above.
        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
        pr_err!("hackbot: dim {} not divisible by group_size {}\n", dim, gs);
        return Err(EINVAL);
    }
    if hidden_dim % gs != 0 {
        // SAFETY: cleanup of `tok_ptr`; same rationale as above.
        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
        pr_err!("hackbot: hidden_dim {} not divisible by group_size {}\n", hidden_dim, gs);
        return Err(EINVAL);
    }

    let embed = q8_ref_advance(&mut cursor, vocab_size, dim, gs, data.len())
        .map_err(|e| {
            // SAFETY: cleanup of `tok_ptr`; same rationale as above.
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: embedding weight offset overflow\n");
            e
        })?;

    // Heap-allocate the layer scratch array. `[LayerRef; MODEL_MAX_LAYERS]`
    // is ~8.7 KiB and would otherwise live on the kernel stack alongside
    // parse_and_store_model's other locals while MODEL.lock() is held,
    // pushing close to the 16 KiB stack limit. See R-029 in
    // docs/REVIEW_v0.1.md.
    let mut layers: KBox<[LayerRef; MODEL_MAX_LAYERS]> =
        KBox::new([LayerRef::ZERO; MODEL_MAX_LAYERS], GFP_KERNEL).map_err(|e| {
            // SAFETY: cleanup of `tok_ptr`; same rationale as above.
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: failed to allocate layer scratch array\n");
            e
        })?;
    for l in 0..n_layers {
        let rms_att_off = norm_ref_advance(&mut cursor, dim, data.len())
            .map_err(|e| {
                // SAFETY: cleanup of `tok_ptr`; same rationale as above.
                unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
                pr_err!("hackbot: layer {} rms_att overflow\n", l);
                e
            })?;

        macro_rules! q8_or_cleanup {
            ($rows:expr, $cols:expr, $name:expr) => {
                q8_ref_advance(&mut cursor, $rows, $cols, gs, data.len())
                    .map_err(|e| {
                        // SAFETY: cleanup of `tok_ptr`; same rationale as
                        // the kvfree(tok_ptr) sites above.
                        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
                        pr_err!("hackbot: layer {} {} overflow\n", l, $name);
                        e
                    })?
            };
        }
        macro_rules! norm_or_cleanup {
            ($dim:expr, $name:expr) => {
                norm_ref_advance(&mut cursor, $dim, data.len())
                    .map_err(|e| {
                        // SAFETY: cleanup of `tok_ptr`; same rationale as
                        // the kvfree(tok_ptr) sites above.
                        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
                        pr_err!("hackbot: layer {} {} overflow\n", l, $name);
                        e
                    })?
            };
        }

        let wq = q8_or_cleanup!(n_heads * head_dim, dim, "wq");
        let wk = q8_or_cleanup!(n_kv_heads * head_dim, dim, "wk");
        let wv = q8_or_cleanup!(n_kv_heads * head_dim, dim, "wv");
        let wo = q8_or_cleanup!(dim, n_heads * head_dim, "wo");
        let rms_ffn_off = norm_or_cleanup!(dim, "rms_ffn");
        let gate = q8_or_cleanup!(hidden_dim, dim, "gate");
        let up = q8_or_cleanup!(hidden_dim, dim, "up");
        let down = q8_or_cleanup!(dim, hidden_dim, "down");

        layers[l] = LayerRef {
            rms_att_off, wq, wk, wv, wo, rms_ffn_off, gate, up, down,
        };
    }

    let rms_final_off = norm_ref_advance(&mut cursor, dim, data.len())
        .map_err(|e| {
            // SAFETY: cleanup of `tok_ptr`; same rationale as above.
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: rms_final overflow\n");
            e
        })?;

    if cursor != data.len() {
        pr_warn!("hackbot: model has {} trailing bytes (expected end at {}, file is {})\n",
                 data.len() - cursor, cursor, data.len());
    }

    pr_info!("hackbot: v1 (INT8) weights parsed: {} bytes ({} layers)\n",
             cursor - pos, n_layers);

    slot.config = config;
    slot.tok_section_off = tok_section_off;
    slot.tok_offsets_addr = tok_ptr as usize;
    slot.embed = embed;
    slot.layers = *layers;
    slot.rms_final_off = rms_final_off;
    slot.format_version = MODEL_FORMAT_V1;
    slot.weights_off = weights_start;

    Ok(())
}

/// Attempt to load the model firmware on first device open.
pub(crate) fn load_model_if_needed(dev: &Device) {
    let mut slot = MODEL.lock();
    if slot.loaded {
        return;
    }

    let fw = match Firmware::request_nowarn(c_str!("hackbot-model.bin"), dev) {
        Ok(fw) => fw,
        Err(_) => {
            pr_info!("hackbot: no model firmware found, local inference disabled\n");
            pr_info!("hackbot: to enable: sudo cp hackbot-model.bin /lib/firmware/\n");
            return;
        }
    };

    let fw_data = fw.data();
    let fw_len = fw_data.len();
    pr_info!("hackbot: loaded firmware: {} bytes ({} MB)\n",
             fw_len, fw_len / (1024 * 1024));

    // SAFETY: file-level invariants apply. Fresh allocation: `old` is NULL,
    // size is the firmware blob length (already loaded into kernel memory by
    // request_firmware), alignment 1 is fine for u8, GFP_KERNEL is legal in
    // process context.
    let data_ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            fw_len,
            1,
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    } as *mut u8;

    if data_ptr.is_null() {
        pr_err!("hackbot: failed to allocate {} bytes for model data\n", fw_len);
        return;
    }

    // SAFETY: `data_ptr` is non-null (checked) and we just allocated `fw_len`
    // bytes there. `fw_data.as_ptr()` is valid for `fw_len` bytes (Firmware
    // contract). The two regions are disjoint allocations, so
    // `copy_nonoverlapping` is correct.
    unsafe {
        core::ptr::copy_nonoverlapping(fw_data.as_ptr(), data_ptr, fw_len);
    }

    drop(fw);

    // SAFETY: `data_ptr` is non-null, points to `fw_len` initialized bytes
    // we own, and we hold MODEL.lock() so no other thread mutates the region.
    // The slice borrow ends before any cleanup kvfree below.
    let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, fw_len) };

    match parse_and_store_model(data_slice, &mut slot) {
        Ok(()) => {
            slot.data_addr = data_ptr as usize;
            slot.data_len = fw_len;

            match alloc_inference_state(&mut slot) {
                Ok(()) => {
                    match build_sorted_vocab(&mut slot) {
                        Ok(()) => {
                            slot.loaded = true;
                            pr_info!("hackbot: model ready for inference ({}×{}, {} layers)\n",
                                     slot.config.dim, slot.config.hidden_dim, slot.config.n_layers);
                        }
                        Err(_) => {
                            // SAFETY (this and the four kvfree/hackbot_fpu_free
                            // calls in this error path): cleanup of allocations
                            // owned by the slot. Each pointer was returned by a
                            // matching kvrealloc/hackbot_fpu_alloc earlier in
                            // this function and has not yet been freed; freeing
                            // and zeroing the slot field is the standard
                            // ownership transfer back to the allocator.
                            // MODEL.lock() is held throughout.
                            if slot.fpu_state != 0 {
                                unsafe { hackbot_fpu_free(slot.fpu_state as *mut core::ffi::c_void) };
                                slot.fpu_state = 0;
                            }
                            unsafe { bindings::kvfree(slot.inf_buf_addr as *const core::ffi::c_void) };
                            slot.inf_buf_addr = 0;
                            slot.inf_buf_len = 0;
                            if slot.tok_offsets_addr != 0 {
                                unsafe { bindings::kvfree(slot.tok_offsets_addr as *const core::ffi::c_void) };
                                slot.tok_offsets_addr = 0;
                            }
                            unsafe { bindings::kvfree(data_ptr as *const core::ffi::c_void) };
                            slot.data_addr = 0;
                            slot.data_len = 0;
                            pr_err!("hackbot: sorted vocab build failed\n");
                        }
                    }
                }
                Err(_) => {
                    // SAFETY (this and the kvfree(data_ptr) below): cleanup of
                    // allocations owned by the slot when inference-state
                    // allocation failed. Same rationale as the sorted-vocab
                    // failure cleanup block above; MODEL.lock() held.
                    if slot.tok_offsets_addr != 0 {
                        unsafe { bindings::kvfree(slot.tok_offsets_addr as *const core::ffi::c_void) };
                        slot.tok_offsets_addr = 0;
                    }
                    unsafe { bindings::kvfree(data_ptr as *const core::ffi::c_void) };
                    slot.data_addr = 0;
                    slot.data_len = 0;
                    pr_err!("hackbot: inference state allocation failed\n");
                }
            }
        }
        Err(_) => {
            // SAFETY: cleanup of `data_ptr` (allocated above, never moved into
            // slot because parse failed). MODEL.lock() held.
            unsafe { bindings::kvfree(data_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: model loading failed, local inference disabled\n");
        }
    }
}

/// Free model resources. Called during module unload.
pub(crate) fn free_model_resources() {
    let mut slot = MODEL.lock();
    if !slot.loaded {
        return;
    }

    // SAFETY (this whole function): final teardown under MODEL.lock(). Each
    // `if x != 0` gate ensures we only free pointers that were successfully
    // allocated and stored on the slot; each kvfree/hackbot_fpu_free pairs
    // with the matching kvrealloc/hackbot_fpu_alloc in load_model_if_needed
    // / build_sorted_vocab / alloc_inference_state. The slot fields are
    // zeroed after free so a subsequent reload starts from a clean state.
    if slot.data_addr != 0 {
        unsafe { bindings::kvfree(slot.data_addr as *const core::ffi::c_void) };
        slot.data_addr = 0;
        slot.data_len = 0;
    }

    if slot.tok_offsets_addr != 0 {
        unsafe { bindings::kvfree(slot.tok_offsets_addr as *const core::ffi::c_void) };
        slot.tok_offsets_addr = 0;
    }

    if slot.fpu_state != 0 {
        unsafe { hackbot_fpu_free(slot.fpu_state as *mut core::ffi::c_void) };
        slot.fpu_state = 0;
    }

    if slot.inf_buf_addr != 0 {
        unsafe { bindings::kvfree(slot.inf_buf_addr as *const core::ffi::c_void) };
        slot.inf_buf_addr = 0;
        slot.inf_buf_len = 0;
    }

    if slot.sorted_vocab_addr != 0 {
        unsafe { bindings::kvfree(slot.sorted_vocab_addr as *const core::ffi::c_void) };
        slot.sorted_vocab_addr = 0;
    }

    slot.loaded = false;
    pr_info!("hackbot: model resources freed\n");
}
