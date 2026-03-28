// SPDX-License-Identifier: GPL-2.0

//! hackbot — In-kernel LLM agent character device.
//!
//! An autonomous OODA agent living in ring 0 with kernel observation tools.
//! Creates /dev/hackbot: write a prompt, read the LLM response.
//!
//! The module supports two inference backends:
//! - Local: in-kernel SmolLM2-135M (INT8/Q16.16 or FP16/float32)
//! - Remote: vLLM server via kernel TCP socket
//!
//! Available tools (Tier 0 — read-only observation):
//!   ps      — List running processes (PID, PPID, state, comm)
//!   mem     — Detailed memory statistics (total, free, buffers, swap)
//!   loadavg — System load averages (1/5/15 min) and task counts

use kernel::prelude::*;

use crate::device::HackbotModule;

// Module declaration — must be in the root file.
module! {
    type: HackbotModule,
    name: "hackbot",
    authors: ["Sunwoo Jang"],
    description: "In-kernel LLM agent — OODA agent loop with kernel tools",
    license: "GPL",
}

// Submodules — kernel Rust build compiles hackbot_main.rs as the root.
// Each module lives in its own file (hackbot_foo.rs).

#[path = "hackbot_config.rs"]
mod config;

#[path = "hackbot_types.rs"]
mod types;

#[path = "hackbot_state.rs"]
mod state;

#[path = "hackbot_context.rs"]
mod context;

#[path = "hackbot_net.rs"]
mod net;

#[path = "hackbot_tools.rs"]
mod tools;

#[path = "hackbot_math.rs"]
mod math;

#[path = "hackbot_model.rs"]
mod model;

#[path = "hackbot_forward.rs"]
mod forward;

#[path = "hackbot_tokenizer.rs"]
mod tokenizer;

#[path = "hackbot_agent.rs"]
mod agent;

#[path = "hackbot_vllm.rs"]
mod vllm;

#[path = "hackbot_memory.rs"]
mod memory;

#[path = "hackbot_device.rs"]
mod device;
