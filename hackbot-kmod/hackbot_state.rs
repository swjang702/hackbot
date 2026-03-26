// SPDX-License-Identifier: GPL-2.0

//! Global state: MODEL and RESPONSE mutexes.

use kernel::prelude::*;
#[allow(unused_imports)]
use kernel::sync::Mutex;

use crate::config::*;
use crate::types::*;

kernel::sync::global_lock! {
    // SAFETY: Initialized in HackbotModule::init() before any device access.
    pub(crate) unsafe(uninit) static RESPONSE: Mutex<SharedResponse> = SharedResponse {
        data: [0u8; MAX_RESPONSE_SIZE],
        len: 0,
        ready: false,
    };
}

kernel::sync::global_lock! {
    // SAFETY: Initialized in HackbotModule::init() before any device access.
    // Model data is loaded on first device open and freed on module unload.
    pub(crate) unsafe(uninit) static MODEL: Mutex<ModelSlot> = ModelSlot {
        loaded: false,
        data_addr: 0,
        data_len: 0,
        tok_offsets_addr: 0,
        config: ModelConfig::ZERO,
        tok_section_off: 0,
        embed: Q8Ref::ZERO,
        layers: [LayerRef::ZERO; MODEL_MAX_LAYERS],
        rms_final_off: 0,
        inf_buf_addr: 0,
        inf_buf_len: 0,
        inf_kv_len: 0,
        inf_x: 0, inf_xb: 0, inf_xb2: 0,
        inf_q: 0, inf_k: 0, inf_v: 0,
        inf_att: 0, inf_hb: 0, inf_hb2: 0,
        inf_logits: 0,
        sorted_vocab_addr: 0,
        byte_to_token: [0u32; 256],
        format_version: 0,
        fpu_state: 0,
        weights_off: 0,
    };
}
