// SPDX-License-Identifier: GPL-2.0

//! Transformer forward pass: alloc_inference_state, reset_kv, forward_token.

use kernel::{bindings, prelude::*};

use crate::config::*;
use crate::math::*;
use crate::types::*;

/// Allocate inference state: KV cache and activation buffers.
pub(crate) fn alloc_inference_state(slot: &mut ModelSlot) -> Result {
    let dim = slot.config.dim as usize;
    let hidden_dim = slot.config.hidden_dim as usize;
    let n_layers = slot.config.n_layers as usize;
    let n_kv_heads = slot.config.n_kv_heads as usize;
    let head_dim = slot.config.head_dim as usize;
    let vocab_size = slot.config.vocab_size as usize;

    // Format v2: allocate float32 inference state via C helper
    if slot.format_version == MODEL_FORMAT_V2 {
        let n_heads = slot.config.n_heads as usize;
        // F-005: range-check every usize→i32 cast at the FFI boundary instead
        // of relying on upstream validation. SmolLM2-class config bounds make
        // these always succeed today, but a malformed model header that slipped
        // past parse_model_header (e.g. a future schema change) would otherwise
        // truncate silently into bogus i32 values.
        let dim_i      = i32::try_from(dim).map_err(|_| EINVAL)?;
        let hidden_i   = i32::try_from(hidden_dim).map_err(|_| EINVAL)?;
        let n_layers_i = i32::try_from(n_layers).map_err(|_| EINVAL)?;
        let n_heads_i  = i32::try_from(n_heads).map_err(|_| EINVAL)?;
        let n_kv_i     = i32::try_from(n_kv_heads).map_err(|_| EINVAL)?;
        let head_i     = i32::try_from(head_dim).map_err(|_| EINVAL)?;
        let vocab_i    = i32::try_from(vocab_size).map_err(|_| EINVAL)?;
        let max_seq_i  = i32::try_from(INFERENCE_MAX_SEQ).map_err(|_| EINVAL)?;

        // SAFETY: FFI call into hackbot_fpu.c. The C signature matches the
        // `extern "C"` declaration in hackbot_types.rs. All eight arguments
        // are config dimensions range-checked to fit in i32 above. Returns
        // NULL on allocation failure, checked immediately below.
        let ptr = unsafe {
            hackbot_fpu_alloc(
                dim_i, hidden_i, n_layers_i,
                n_heads_i, n_kv_i, head_i,
                vocab_i, max_seq_i,
            )
        };
        if ptr.is_null() {
            pr_err!("hackbot: failed to allocate FPU inference state\n");
            return Err(ENOMEM);
        }
        slot.fpu_state = ptr as usize;
        pr_info!("hackbot: FPU inference state allocated (v2/float32)\n");
        return Ok(());
    }

    // Format v1: INT8/Q16.16 inference state
    let kv_len = n_layers * 2 * n_kv_heads * INFERENCE_MAX_SEQ * head_dim;

    let mut c = kv_len;
    let x = c;       c += dim;
    let xb = c;      c += dim;
    let xb2 = c;     c += dim;
    let q = c;       c += dim;
    let k = c;       c += n_kv_heads * head_dim;
    let v = c;       c += n_kv_heads * head_dim;
    let att = c;     c += INFERENCE_MAX_SEQ;
    let hb = c;      c += hidden_dim;
    let hb2 = c;     c += hidden_dim;
    let logits = c;  c += vocab_size;

    let total_elems = c;
    let total_bytes = total_elems * core::mem::size_of::<i32>();

    // SAFETY: FFI to kvrealloc with NULL `old` (fresh allocation). total_bytes
    // is computed from validated config dims and cannot overflow usize for
    // supported model sizes. Alignment is the natural alignment of i32.
    // GFP_KERNEL is legal in process context; forward_token / alloc_inference_state
    // run from the write() handler, not from atomic / interrupt context.
    let ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            total_bytes,
            core::mem::align_of::<i32>() as _,
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    };
    if ptr.is_null() {
        pr_err!("hackbot: failed to allocate inference state ({} bytes)\n", total_bytes);
        return Err(ENOMEM);
    }

    // SAFETY: `ptr` is non-null (checked above), points to `total_bytes` of
    // freshly allocated memory we own exclusively, and is aligned to i32 which
    // is stricter than u8. write_bytes zero-initialises the activation/KV buffer.
    unsafe { core::ptr::write_bytes(ptr as *mut u8, 0, total_bytes) };

    slot.inf_buf_addr = ptr as usize;
    slot.inf_buf_len = total_bytes;
    slot.inf_kv_len = kv_len;
    slot.inf_x = x;
    slot.inf_xb = xb;
    slot.inf_xb2 = xb2;
    slot.inf_q = q;
    slot.inf_k = k;
    slot.inf_v = v;
    slot.inf_att = att;
    slot.inf_hb = hb;
    slot.inf_hb2 = hb2;
    slot.inf_logits = logits;

    pr_info!("hackbot: inference state: {} KB (KV cache {} KB, activations {} KB)\n",
             total_bytes / 1024,
             kv_len * 4 / 1024,
             (total_bytes - kv_len * 4) / 1024);

    Ok(())
}

/// Zero the KV cache between conversations.
#[allow(dead_code)]
pub(crate) fn reset_kv_cache(slot: &ModelSlot) {
    if slot.format_version == MODEL_FORMAT_V2 {
        if slot.fpu_state != 0 {
            // SAFETY: FFI into hackbot_fpu.c. `fpu_state` is non-null
            // (checked above) and was returned by a prior hackbot_fpu_alloc
            // for this slot. Caller holds MODEL.lock() so no concurrent
            // forward pass touches this state.
            unsafe { hackbot_fpu_reset(slot.fpu_state as *mut core::ffi::c_void); }
        }
        return;
    }
    if slot.inf_buf_addr == 0 || slot.inf_kv_len == 0 {
        return;
    }
    let kv_bytes = slot.inf_kv_len * core::mem::size_of::<i32>();
    // SAFETY: `inf_buf_addr` is non-null (checked above) and was allocated
    // in alloc_inference_state with at least `inf_kv_len * sizeof(i32)` bytes
    // at the start of the buffer for the KV cache. Caller holds MODEL.lock().
    unsafe {
        core::ptr::write_bytes(slot.inf_buf_addr as *mut u8, 0, kv_bytes);
    }
}

/// Run one token through the transformer, writing logits to inf_logits buffer.
///
/// Returns `Err(EINVAL)` if `slot.weights_off > slot.data_len`. That should
/// never happen — model load enforces the invariant — but a bare subtraction
/// here would underflow silently and the FPU side would walk past the blob.
/// See R-025 in docs/REVIEW_v0.1.md.
#[allow(dead_code)]
pub(crate) fn forward_token(slot: &ModelSlot, token_id: usize, pos: usize) -> Result<()> {
    // v2: delegate to C float32 forward pass
    if slot.format_version == MODEL_FORMAT_V2 {
        let weights = (slot.data_addr + slot.weights_off) as *const core::ffi::c_void;
        let weights_len = slot.data_len.checked_sub(slot.weights_off).ok_or(EINVAL)?;
        // F-005: range-check the usize→i32 casts even though both are bounded
        // upstream (token_id < vocab_size < MODEL_MAX_VOCAB; pos < INFERENCE_MAX_SEQ).
        // try_from makes the contract explicit and catches a future widening
        // of either bound that would silently truncate.
        let token_i = i32::try_from(token_id).map_err(|_| EINVAL)?;
        let pos_i   = i32::try_from(pos).map_err(|_| EINVAL)?;
        // SAFETY: FFI to hackbot_fpu.c. `fpu_state` was returned by
        // hackbot_fpu_alloc for this slot and is freed only on model unload
        // under MODEL.lock(). `weights` points into the model blob mapped at
        // slot.data_addr with at least `weights_len` valid bytes (weights_off
        // and data_len are validated at model load). token_id and pos fit in
        // i32 (range-checked above).
        // Caller holds MODEL.lock() for the entirety of the call.
        let ret = unsafe {
            hackbot_fpu_forward(
                slot.fpu_state as *mut core::ffi::c_void,
                weights, weights_len,
                token_i, pos_i,
            )
        };
        if ret != 0 {
            pr_err!("hackbot: FPU forward failed: ret={} (token={}, pos={})\n",
                    ret, token_id, pos);
            return Err(EINVAL);
        }
        return Ok(());
    }

    // v1: Q16.16 fixed-point forward pass
    let dim = slot.config.dim as usize;
    let hidden_dim = slot.config.hidden_dim as usize;
    let n_layers = slot.config.n_layers as usize;
    let n_heads = slot.config.n_heads as usize;
    let n_kv_heads = slot.config.n_kv_heads as usize;
    let head_dim = slot.config.head_dim as usize;
    let kv_dim = n_kv_heads * head_dim;
    let gs = slot.config.group_size as usize;
    let vocab_size = slot.config.vocab_size as usize;
    let heads_per_group = n_heads / n_kv_heads;

    // SAFETY: `data_addr`/`data_len` describe the model blob mapped at module
    // load (vmalloc'd, kept alive for the lifetime of the loaded model).
    // Caller holds MODEL.lock() for the duration of this function, so the
    // mapping is not torn down underneath us. `data_len` is the exact byte
    // length of that allocation. The slice is read-only (`&[u8]`).
    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };

    let inf = slot.inf_buf_addr as *mut i32;

    // R-015 fix: instead of materialising ten persistent `&mut [i32]` views
    // into the same `inf` allocation (which would alias under stacked-borrows
    // the moment we touch the KV cache via raw pointer arithmetic), we keep
    // raw `*mut i32` aliases for each activation buffer and only build slices
    // momentarily inside helper-call statements. No two `&mut [i32]` over
    // `inf` are ever live simultaneously, and no slice borrow coexists with
    // raw-pointer access.
    //
    // SAFETY (this and the nine following `wrapping_add` calls): all offsets
    // were computed in alloc_inference_state and stored on the slot; each
    // is < total_elems, so the resulting pointer is in-bounds of the
    // allocation. `wrapping_add` cannot itself trigger UB even on overflow;
    // dereference safety is argued at each use site.
    let x_ptr      = inf.wrapping_add(slot.inf_x);
    let xb_ptr     = inf.wrapping_add(slot.inf_xb);
    let xb2_ptr    = inf.wrapping_add(slot.inf_xb2);
    let q_ptr      = inf.wrapping_add(slot.inf_q);
    let k_ptr      = inf.wrapping_add(slot.inf_k);
    let v_ptr      = inf.wrapping_add(slot.inf_v);
    let att_ptr    = inf.wrapping_add(slot.inf_att);
    let hb_ptr     = inf.wrapping_add(slot.inf_hb);
    let hb2_ptr    = inf.wrapping_add(slot.inf_hb2);
    let logits_ptr = inf.wrapping_add(slot.inf_logits);

    let kv_head_stride = INFERENCE_MAX_SEQ * head_dim;
    let kv_type_stride = n_kv_heads * kv_head_stride;
    let kv_layer_stride = 2 * kv_type_stride;

    // Q16.16 saturation policy: integer multiplications and shifted casts in
    // the inner loops below are saturated (clamped to i32::MIN..=i32::MAX)
    // rather than wrapped. Wrapping a Q16.16 value mid-pipeline produces
    // arbitrary nonsense logits; saturating preserves sign and worst-case
    // magnitude so a runaway weight clamps an activation rather than
    // sign-flipping it. Residual-stream additions at the end of each layer
    // intentionally use wrapping_add - the dynamic range of the residual is
    // engineered to fit in i32, matching reference llama2.c-style fixed-point
    // implementations.

    // Embedding lookup
    let e = &slot.embed;
    let n_groups_e = dim / gs;
    let row_off = token_id * dim;
    let scale_row_off = token_id * n_groups_e * 4;

    for g in 0..n_groups_e {
        let sb = e.scale_off + scale_row_off + g * 4;
        let scale = i32::from_le_bytes([data[sb], data[sb+1], data[sb+2], data[sb+3]]);
        for j in 0..gs {
            let c = g * gs + j;
            let w = data[e.data_off + row_off + c] as i8 as i32;
            // R-017: w in [-128,127], scale is an arbitrary i32 from the
            // weight blob; saturate the product instead of letting it wrap.
            // SAFETY: c < n_groups_e * gs == dim <= slot.inf_x's length;
            // x_ptr.add(c) stays within the activation buffer. No &mut
            // [i32] is live over `inf` here.
            unsafe { *x_ptr.add(c) = w.saturating_mul(scale); }
        }
    }

    if pos == 0 {
        // SAFETY: dim >= 4 for any supported config; reads x[0..4] which is
        // within the activation buffer. No concurrent writer (caller holds
        // MODEL.lock()).
        let (x0, x1, x2, x3) = unsafe {
            (*x_ptr, *x_ptr.add(1), *x_ptr.add(2), *x_ptr.add(3))
        };
        pr_info!("hackbot: DEBUG embed[{}]: x[0..4] = [{}, {}, {}, {}]\n",
                 token_id, x0, x1, x2, x3);
    }

    // Transformer layers
    for l in 0..n_layers {
        let layer = &slot.layers[l];

        // SAFETY (helper-call invariants - apply to every `unsafe { ... }`
        // block in this layer that constructs slices via `from_raw_parts(_mut)`):
        // - Provenance: `inf` and all `*_ptr` aliases derive from
        //   slot.inf_buf_addr (kvrealloc), non-null by gate at function entry.
        // - Length: each slice constructor uses the exact allocated length of
        //   that buffer (dim, kv_dim, hidden_dim, INFERENCE_MAX_SEQ, vocab_size,
        //   or head_dim for sub-head slices), all <= the offsets recorded in
        //   alloc_inference_state.
        // - Lifetime: caller holds MODEL.lock() so the inf allocation cannot
        //   be freed mid-call.
        // - Aliasing: each block constructs at most one `&mut [i32]` and zero
        //   or more `&[i32]` over disjoint sub-ranges of the inf allocation;
        //   the borrow is confined to the call expression and does not coexist
        //   with raw-pointer access through `inf`/`_ptr`.
        // Per-block SAFETY comments below highlight only deviations from this
        // invariant (e.g. tighter sub-range bounds for sub-head slices).
        unsafe {
            let xb_s  = core::slice::from_raw_parts_mut(xb_ptr, dim);
            let x_s   = core::slice::from_raw_parts(x_ptr, dim);
            rmsnorm_q16(xb_s, x_s, &data[layer.rms_att_off..], dim);
        }

        // SAFETY: see helper-call invariants above. q is `dim` elements.
        unsafe {
            let q_s  = core::slice::from_raw_parts_mut(q_ptr, dim);
            let xb_s = core::slice::from_raw_parts(xb_ptr, dim);
            matmul_q8(q_s, xb_s,
                      &data[layer.wq.data_off..], &data[layer.wq.scale_off..],
                      dim, dim, gs);
        }
        // SAFETY: see helper-call invariants above. k is `kv_dim` elements.
        unsafe {
            let k_s  = core::slice::from_raw_parts_mut(k_ptr, kv_dim);
            let xb_s = core::slice::from_raw_parts(xb_ptr, dim);
            matmul_q8(k_s, xb_s,
                      &data[layer.wk.data_off..], &data[layer.wk.scale_off..],
                      kv_dim, dim, gs);
        }
        // SAFETY: see helper-call invariants above. v is `kv_dim` elements.
        unsafe {
            let v_s  = core::slice::from_raw_parts_mut(v_ptr, kv_dim);
            let xb_s = core::slice::from_raw_parts(xb_ptr, dim);
            matmul_q8(v_s, xb_s,
                      &data[layer.wv.data_off..], &data[layer.wv.scale_off..],
                      kv_dim, dim, gs);
        }

        for h in 0..n_heads {
            // SAFETY: head h*head_dim..(h+1)*head_dim is within q_buf's `dim`
            // elements (h < n_heads, n_heads*head_dim == dim by construction).
            // Borrow is local to this expression.
            unsafe {
                let q_head = core::slice::from_raw_parts_mut(
                    q_ptr.add(h * head_dim), head_dim);
                rope_apply_q16(q_head, pos, head_dim);
            }
        }
        for h in 0..n_kv_heads {
            // SAFETY: head h*head_dim..(h+1)*head_dim is within k_buf's
            // `kv_dim` (= n_kv_heads * head_dim) elements.
            unsafe {
                let k_head = core::slice::from_raw_parts_mut(
                    k_ptr.add(h * head_dim), head_dim);
                rope_apply_q16(k_head, pos, head_dim);
            }
        }

        let kv_base = l * kv_layer_stride;
        for h in 0..n_kv_heads {
            let k_dst = kv_base + h * kv_head_stride + pos * head_dim;
            let v_dst = kv_base + kv_type_stride + h * kv_head_stride + pos * head_dim;
            // SAFETY:
            // - Provenance: `inf` is `slot.inf_buf_addr as *mut i32`, allocated
            //   in alloc_inference_state and non-null because we only run when
            //   that succeeded (caller-side gate).
            // - Lifetime: caller holds MODEL.lock(); the buffer is freed only
            //   on model unload under the same lock.
            // - Bounds: kv_base + kv_layer_stride <= inf_kv_len for all
            //   l < n_layers; pos < INFERENCE_MAX_SEQ enforced upstream;
            //   d < head_dim; the destination indices fall inside the KV
            //   cache region [0, inf_kv_len). k_ptr/v_ptr reads stay within
            //   their respective `kv_dim`-sized buffers.
            // - Aliasing: no &mut [i32] over the inf allocation is live; the
            //   helper-call slices above have all returned.
            for d in 0..head_dim {
                unsafe {
                    let kv = *k_ptr.add(h * head_dim + d);
                    let vv = *v_ptr.add(h * head_dim + d);
                    *inf.add(k_dst + d) = kv;
                    *inf.add(v_dst + d) = vv;
                }
            }
        }

        for h in 0..n_heads {
            let kv_group = h / heads_per_group;
            let q_head_off = h * head_dim;

            for p in 0..=pos {
                let k_src = kv_base + kv_group * kv_head_stride + p * head_dim;
                let mut dot: i64 = 0;
                for d in 0..head_dim {
                    // SAFETY (covers both `unsafe { *inf.add(...) }` and
                    // `unsafe { *q_ptr.add(...) }` reads in this iteration):
                    // - Provenance / lifetime / lock: as for the KV write block above.
                    // - Bounds: k_src + d < kv_base + kv_layer_stride
                    //   <= inf_kv_len; q_head_off + d < dim (q_ptr covers dim
                    //   elements; q_head_off = h*head_dim, h < n_heads,
                    //   n_heads*head_dim == dim).
                    // - Aliasing: no &mut [i32] over inf is live here.
                    let k_val = unsafe { *inf.add(k_src + d) };
                    let q_val = unsafe { *q_ptr.add(q_head_off + d) };
                    dot += q_val as i64 * k_val as i64;
                }
                // R-017: clamp the shifted i64 to i32 range before casting.
                let r = (dot >> 19).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                // SAFETY: p <= pos < INFERENCE_MAX_SEQ == att buffer length.
                unsafe { *att_ptr.add(p) = r; }
            }

            // SAFETY: att buffer is INFERENCE_MAX_SEQ long; pos+1 <=
            // INFERENCE_MAX_SEQ. softmax mutates only its slice argument,
            // which is the only borrow over `inf` live during the call.
            unsafe {
                let att_s = core::slice::from_raw_parts_mut(att_ptr, INFERENCE_MAX_SEQ);
                softmax_q16(att_s, pos + 1);
            }

            let v_type_base = kv_base + kv_type_stride + kv_group * kv_head_stride;
            for d in 0..head_dim {
                let mut acc: i64 = 0;
                for p in 0..=pos {
                    // SAFETY (covers both `unsafe` reads in this iteration):
                    // v_type_base + pos*head_dim + d < kv_base + kv_layer_stride
                    // <= inf_kv_len; att index p <= pos < INFERENCE_MAX_SEQ.
                    // No &mut [i32] over inf is live here.
                    let v_val = unsafe { *inf.add(v_type_base + p * head_dim + d) };
                    let a_val = unsafe { *att_ptr.add(p) };
                    acc += a_val as i64 * v_val as i64;
                }
                // R-017: clamp acc>>16 to i32 before cast.
                let r = (acc >> 16).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                // SAFETY: h*head_dim + d < n_heads*head_dim == dim
                // == xb buffer length.
                unsafe { *xb_ptr.add(h * head_dim + d) = r; }
            }
        }

        // SAFETY: same shape as the matmul calls above; borrow scoped to call.
        unsafe {
            let xb2_s = core::slice::from_raw_parts_mut(xb2_ptr, dim);
            let xb_s  = core::slice::from_raw_parts(xb_ptr, dim);
            matmul_q8(xb2_s, xb_s,
                      &data[layer.wo.data_off..], &data[layer.wo.scale_off..],
                      dim, dim, gs);
        }

        // Residual: x += xb2. Wrapping is intentional (see policy comment).
        for i in 0..dim {
            // SAFETY: i < dim; both pointers cover dim elements; no slice
            // borrow over `inf` is live.
            unsafe {
                let xi = *x_ptr.add(i);
                let yi = *xb2_ptr.add(i);
                *x_ptr.add(i) = xi.wrapping_add(yi);
            }
        }

        // SAFETY: same shape as the rmsnorm above.
        unsafe {
            let xb_s = core::slice::from_raw_parts_mut(xb_ptr, dim);
            let x_s  = core::slice::from_raw_parts(x_ptr, dim);
            rmsnorm_q16(xb_s, x_s, &data[layer.rms_ffn_off..], dim);
        }

        // SAFETY: see helper-call invariants above. hb/hb2 are hidden_dim
        // elements; gate/up/down weights cover hidden_dim*dim weights.
        unsafe {
            let hb_s  = core::slice::from_raw_parts_mut(hb_ptr, hidden_dim);
            let xb_s  = core::slice::from_raw_parts(xb_ptr, dim);
            matmul_q8(hb_s, xb_s,
                      &data[layer.gate.data_off..], &data[layer.gate.scale_off..],
                      hidden_dim, dim, gs);
        }
        // SAFETY: see helper-call invariants above.
        unsafe {
            let hb2_s = core::slice::from_raw_parts_mut(hb2_ptr, hidden_dim);
            let xb_s  = core::slice::from_raw_parts(xb_ptr, dim);
            matmul_q8(hb2_s, xb_s,
                      &data[layer.up.data_off..], &data[layer.up.scale_off..],
                      hidden_dim, dim, gs);
        }

        // SAFETY: see helper-call invariants above. silu_vec_q16 mutates
        // only its slice argument.
        unsafe {
            let hb_s = core::slice::from_raw_parts_mut(hb_ptr, hidden_dim);
            silu_vec_q16(hb_s, hidden_dim);
        }
        // SAFETY: see helper-call invariants above. The two slices borrow
        // disjoint sub-ranges of the inf allocation (hb vs hb2).
        unsafe {
            let hb_s  = core::slice::from_raw_parts_mut(hb_ptr, hidden_dim);
            let hb2_s = core::slice::from_raw_parts(hb2_ptr, hidden_dim);
            elementwise_mul_inplace_q16(hb_s, hb2_s, hidden_dim);
        }

        // SAFETY: same shape as matmul calls above.
        unsafe {
            let xb2_s = core::slice::from_raw_parts_mut(xb2_ptr, dim);
            let hb_s  = core::slice::from_raw_parts(hb_ptr, hidden_dim);
            matmul_q8(xb2_s, hb_s,
                      &data[layer.down.data_off..], &data[layer.down.scale_off..],
                      dim, hidden_dim, gs);
        }

        // Residual: x += xb2. Wrapping is intentional.
        for i in 0..dim {
            // SAFETY: i < dim; same as the first residual loop.
            unsafe {
                let xi = *x_ptr.add(i);
                let yi = *xb2_ptr.add(i);
                *x_ptr.add(i) = xi.wrapping_add(yi);
            }
        }
    }

    if pos == 0 {
        // SAFETY: dim >= 4 for any supported config.
        let (x0, x1, x2, x3) = unsafe {
            (*x_ptr, *x_ptr.add(1), *x_ptr.add(2), *x_ptr.add(3))
        };
        pr_info!("hackbot: DEBUG after layers: x[0..4] = [{}, {}, {}, {}]\n",
                 x0, x1, x2, x3);
    }

    // Final RMSNorm. SAFETY: as for rmsnorm calls above.
    unsafe {
        let xb_s = core::slice::from_raw_parts_mut(xb_ptr, dim);
        let x_s  = core::slice::from_raw_parts(x_ptr, dim);
        rmsnorm_q16(xb_s, x_s, &data[slot.rms_final_off..], dim);
    }

    // Logits. SAFETY: vocab_size matches logits buffer length; xb has dim
    // elements. Disjoint sub-ranges of the inf allocation.
    unsafe {
        let logits_s = core::slice::from_raw_parts_mut(logits_ptr, vocab_size);
        let xb_s     = core::slice::from_raw_parts(xb_ptr, dim);
        matmul_q8(logits_s, xb_s,
                  &data[slot.embed.data_off..], &data[slot.embed.scale_off..],
                  vocab_size, dim, gs);
    }

    if pos == 0 {
        let mut best_i = 0usize;
        // SAFETY (this and the loop's `unsafe { *logits_ptr.add(i) }` below):
        // 0 <= i < vocab_size and the logits buffer is vocab_size i32 elements
        // long; no &mut [i32] over inf is live here (matmul above has returned).
        let mut best_v = unsafe { *logits_ptr };
        for i in 1..vocab_size {
            let v = unsafe { *logits_ptr.add(i) };
            if v > best_v {
                best_i = i;
                best_v = v;
            }
        }
        pr_info!("hackbot: DEBUG logits[pos=0]: top1 = token {} (logit {})\n",
                 best_i, best_v);
    }

    Ok(())
}
