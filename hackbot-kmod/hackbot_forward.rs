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
        let ptr = unsafe {
            hackbot_fpu_alloc(
                dim as i32, hidden_dim as i32, n_layers as i32,
                n_heads as i32, n_kv_heads as i32, head_dim as i32,
                vocab_size as i32, INFERENCE_MAX_SEQ as i32,
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
            unsafe { hackbot_fpu_reset(slot.fpu_state as *mut core::ffi::c_void); }
        }
        return;
    }
    if slot.inf_buf_addr == 0 || slot.inf_kv_len == 0 {
        return;
    }
    let kv_bytes = slot.inf_kv_len * core::mem::size_of::<i32>();
    unsafe {
        core::ptr::write_bytes(slot.inf_buf_addr as *mut u8, 0, kv_bytes);
    }
}

/// Run one token through the transformer, writing logits to inf_logits buffer.
#[allow(dead_code)]
pub(crate) fn forward_token(slot: &ModelSlot, token_id: usize, pos: usize) {
    // v2: delegate to C float32 forward pass
    if slot.format_version == MODEL_FORMAT_V2 {
        let weights = (slot.data_addr + slot.weights_off) as *const core::ffi::c_void;
        let weights_len = slot.data_len - slot.weights_off;
        let ret = unsafe {
            hackbot_fpu_forward(
                slot.fpu_state as *mut core::ffi::c_void,
                weights, weights_len,
                token_id as i32, pos as i32,
            )
        };
        if ret != 0 {
            pr_err!("hackbot: FPU forward failed: ret={} (token={}, pos={})\n",
                    ret, token_id, pos);
        }
        return;
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

    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };

    let inf = slot.inf_buf_addr as *mut i32;

    let x = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_x), dim) };
    let xb = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_xb), dim) };
    let xb2 = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_xb2), dim) };
    let q_buf = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_q), dim) };
    let k_buf = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_k), kv_dim) };
    let v_buf = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_v), kv_dim) };
    let att = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_att), INFERENCE_MAX_SEQ) };
    let hb = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_hb), hidden_dim) };
    let hb2 = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_hb2), hidden_dim) };
    let logits = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_logits), vocab_size) };

    let kv_head_stride = INFERENCE_MAX_SEQ * head_dim;
    let kv_type_stride = n_kv_heads * kv_head_stride;
    let kv_layer_stride = 2 * kv_type_stride;

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
            x[c] = w * scale;
        }
    }

    if pos == 0 {
        pr_info!("hackbot: DEBUG embed[{}]: x[0..4] = [{}, {}, {}, {}]\n",
                 token_id, x[0], x[1], x[2], x[3]);
    }

    // Transformer layers
    for l in 0..n_layers {
        let layer = &slot.layers[l];

        rmsnorm_q16(xb, x, &data[layer.rms_att_off..], dim);

        matmul_q8(q_buf, xb,
                  &data[layer.wq.data_off..], &data[layer.wq.scale_off..],
                  dim, dim, gs);
        matmul_q8(k_buf, xb,
                  &data[layer.wk.data_off..], &data[layer.wk.scale_off..],
                  kv_dim, dim, gs);
        matmul_q8(v_buf, xb,
                  &data[layer.wv.data_off..], &data[layer.wv.scale_off..],
                  kv_dim, dim, gs);

        for h in 0..n_heads {
            rope_apply_q16(&mut q_buf[h * head_dim..(h + 1) * head_dim], pos, head_dim);
        }
        for h in 0..n_kv_heads {
            rope_apply_q16(&mut k_buf[h * head_dim..(h + 1) * head_dim], pos, head_dim);
        }

        let kv_base = l * kv_layer_stride;
        for h in 0..n_kv_heads {
            let k_dst = kv_base + h * kv_head_stride + pos * head_dim;
            let v_dst = kv_base + kv_type_stride + h * kv_head_stride + pos * head_dim;
            for d in 0..head_dim {
                unsafe {
                    *inf.add(k_dst + d) = k_buf[h * head_dim + d];
                    *inf.add(v_dst + d) = v_buf[h * head_dim + d];
                }
            }
        }

        for h in 0..n_heads {
            let kv_group = h / heads_per_group;
            let q_head = &q_buf[h * head_dim..(h + 1) * head_dim];

            for p in 0..=pos {
                let k_src = kv_base + kv_group * kv_head_stride + p * head_dim;
                let mut dot: i64 = 0;
                for d in 0..head_dim {
                    let k_val = unsafe { *inf.add(k_src + d) };
                    dot += q_head[d] as i64 * k_val as i64;
                }
                att[p] = (dot >> 19) as i32;
            }

            softmax_q16(att, pos + 1);

            let v_type_base = kv_base + kv_type_stride + kv_group * kv_head_stride;
            for d in 0..head_dim {
                let mut acc: i64 = 0;
                for p in 0..=pos {
                    let v_val = unsafe { *inf.add(v_type_base + p * head_dim + d) };
                    acc += att[p] as i64 * v_val as i64;
                }
                xb[h * head_dim + d] = (acc >> 16) as i32;
            }
        }

        matmul_q8(xb2, xb,
                  &data[layer.wo.data_off..], &data[layer.wo.scale_off..],
                  dim, dim, gs);

        for i in 0..dim {
            x[i] = x[i].wrapping_add(xb2[i]);
        }

        rmsnorm_q16(xb, x, &data[layer.rms_ffn_off..], dim);

        matmul_q8(hb, xb,
                  &data[layer.gate.data_off..], &data[layer.gate.scale_off..],
                  hidden_dim, dim, gs);
        matmul_q8(hb2, xb,
                  &data[layer.up.data_off..], &data[layer.up.scale_off..],
                  hidden_dim, dim, gs);

        silu_vec_q16(hb, hidden_dim);
        elementwise_mul_inplace_q16(hb, hb2, hidden_dim);

        matmul_q8(xb2, hb,
                  &data[layer.down.data_off..], &data[layer.down.scale_off..],
                  dim, hidden_dim, gs);

        for i in 0..dim {
            x[i] = x[i].wrapping_add(xb2[i]);
        }
    }

    if pos == 0 {
        pr_info!("hackbot: DEBUG after layers: x[0..4] = [{}, {}, {}, {}]\n",
                 x[0], x[1], x[2], x[3]);
    }

    // Final RMSNorm
    rmsnorm_q16(xb, x, &data[slot.rms_final_off..], dim);

    // Logits
    matmul_q8(logits, xb,
              &data[slot.embed.data_off..], &data[slot.embed.scale_off..],
              vocab_size, dim, gs);

    if pos == 0 {
        let mut best_i = 0usize;
        for i in 1..vocab_size {
            if logits[i] > logits[best_i] {
                best_i = i;
            }
        }
        pr_info!("hackbot: DEBUG logits[pos=0]: top1 = token {} (logit {})\n",
                 best_i, logits[best_i]);
    }
}
