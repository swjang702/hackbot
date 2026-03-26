// SPDX-License-Identifier: GPL-2.0

//! Local OODA agent loop with ChatML format.

use kernel::prelude::*;

use crate::config::*;
use crate::forward::{forward_token, reset_kv_cache};
use crate::state::MODEL;
use crate::tokenizer::{encode_bpe, generate_from_tokens, get_next_token};
use crate::tools::{execute_tool, parse_tool_call, ToolCallResult};

/// Append a ChatML message to the token array.
#[allow(dead_code)]
fn append_chat_tokens(
    slot: &crate::types::ModelSlot,
    tokens: &mut [u32],
    pos: usize,
    role: &[u8],
    content: &[u8],
) -> usize {
    let mut p = pos;
    let max = tokens.len();
    let nl_token = slot.byte_to_token[0x0A];

    if p < max { tokens[p] = TOKEN_IM_START; p += 1; }

    if p < max {
        let n = encode_bpe(slot, role, &mut tokens[p..]);
        p += n;
    }

    if p < max { tokens[p] = nl_token; p += 1; }

    if p < max {
        let n = encode_bpe(slot, content, &mut tokens[p..]);
        p += n;
    }

    if p < max { tokens[p] = TOKEN_IM_END; p += 1; }

    if p < max { tokens[p] = nl_token; p += 1; }

    p
}

/// Begin an assistant turn in ChatML format.
#[allow(dead_code)]
fn begin_assistant_turn(
    slot: &crate::types::ModelSlot,
    tokens: &mut [u32],
    pos: usize,
) -> usize {
    let mut p = pos;
    let max = tokens.len();
    let nl_token = slot.byte_to_token[0x0A];

    if p < max { tokens[p] = TOKEN_IM_START; p += 1; }
    if p < max {
        let n = encode_bpe(slot, b"assistant", &mut tokens[p..]);
        p += n;
    }
    if p < max { tokens[p] = nl_token; p += 1; }

    p
}

/// Local inference OODA agent loop.
#[allow(dead_code)]
pub(crate) fn agent_loop_local(prompt: &[u8]) -> Result<KVVec<u8>> {
    let slot = MODEL.lock();
    if !slot.loaded {
        return Err(ENODEV);
    }

    let _data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let _tok_offsets = slot.tok_offsets_addr as *const u32;

    // DEBUG: Quick single-token sanity test
    {
        reset_kv_cache(&slot);
        forward_token(&slot, 1, 0);
        let top1 = get_next_token(&slot);
        pr_info!("hackbot: DEBUG single-token test: after token 1, top1 = {}\n", top1);
        if slot.format_version != MODEL_FORMAT_V2 && slot.inf_buf_addr != 0 {
            let logits_ptr = (slot.inf_buf_addr as *const i32).wrapping_add(slot.inf_logits);
            let top1_logit = unsafe { *logits_ptr.add(top1) };
            pr_info!("hackbot: DEBUG top1 logit = {}\n", top1_logit);
            let logit_28 = unsafe { *logits_ptr.add(28) };
            let logit_198 = unsafe { *logits_ptr.add(198) };
            pr_info!("hackbot: DEBUG logits: token28={}, token198={}\n", logit_28, logit_198);
        }
    }

    let mut tokens = [0u32; INFERENCE_MAX_SEQ];
    let mut n_tokens = 0usize;

    n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens, b"system", LOCAL_SYSTEM_PROMPT);
    n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens, b"user", prompt);
    n_tokens = begin_assistant_turn(&slot, &mut tokens, n_tokens);

    pr_info!("hackbot: local inference: {} prompt tokens\n", n_tokens);

    let mut final_answer = KVVec::new();
    let mut got_final_answer = false;

    for iteration in 0..LOCAL_MAX_ITERATIONS {
        let gen_budget = INFERENCE_MAX_SEQ.saturating_sub(n_tokens);
        if gen_budget < 4 {
            pr_warn!("hackbot: local: context full at iteration {}\n", iteration + 1);
            break;
        }
        let gen_tokens = gen_budget.min(MAX_GEN_TOKENS);

        pr_info!("hackbot: local agent iteration {}/{} ({} tokens, {} gen budget)\n",
                 iteration + 1, LOCAL_MAX_ITERATIONS, n_tokens, gen_tokens);

        let mut response_buf = [0u8; 2048];
        let resp_len = generate_from_tokens(
            &slot, &tokens, n_tokens, &mut response_buf, gen_tokens,
        );
        let response = &response_buf[..resp_len];

        if resp_len == 0 {
            pr_warn!("hackbot: local: empty generation at iteration {}\n", iteration + 1);
            break;
        }

        match parse_tool_call(response) {
            ToolCallResult::FinalAnswer(text) => {
                pr_info!("hackbot: local: final answer at iteration {}\n", iteration + 1);
                let _ = final_answer.extend_from_slice(text, GFP_KERNEL);
                got_final_answer = true;
                break;
            }
            ToolCallResult::ToolCall { name, prefix } => {
                pr_info!("hackbot: local: tool call '{}' at iteration {}\n",
                         core::str::from_utf8(name).unwrap_or("?"), iteration + 1);

                let _ = final_answer.extend_from_slice(prefix, GFP_KERNEL);

                if iteration == LOCAL_MAX_ITERATIONS - 1 {
                    let tool_output = execute_tool(name);
                    let _ = final_answer.extend_from_slice(b"\n\n", GFP_KERNEL);
                    let _ = final_answer.extend_from_slice(&tool_output, GFP_KERNEL);
                    got_final_answer = true;
                    break;
                }

                let tool_output = execute_tool(name);

                let mut assistant_content = KVVec::new();
                let _ = assistant_content.extend_from_slice(prefix, GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(b"<tool>", GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(name, GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(b"</tool>", GFP_KERNEL);

                let mut tool_content = KVVec::new();
                let _ = tool_content.extend_from_slice(b"[Tool: ", GFP_KERNEL);
                let _ = tool_content.extend_from_slice(name, GFP_KERNEL);
                let _ = tool_content.extend_from_slice(b"]\n", GFP_KERNEL);
                let tool_slice = if tool_output.len() > LOCAL_MAX_TOOL_OUTPUT {
                    &tool_output[..LOCAL_MAX_TOOL_OUTPUT]
                } else {
                    &tool_output
                };
                let _ = tool_content.extend_from_slice(tool_slice, GFP_KERNEL);
                let _ = tool_content.extend_from_slice(b"\n[End Tool]", GFP_KERNEL);

                n_tokens = 0;
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"system", LOCAL_SYSTEM_PROMPT);
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"user", prompt);
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"assistant", &assistant_content);
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"user", &tool_content);
                n_tokens = begin_assistant_turn(&slot, &mut tokens, n_tokens);

                pr_info!("hackbot: local: rebuilt conversation: {} tokens\n", n_tokens);
            }
        }
    }

    if !got_final_answer && final_answer.is_empty() {
        let _ = final_answer.extend_from_slice(
            b"[hackbot] Local inference produced no output.\n", GFP_KERNEL,
        );
    }

    Ok(final_answer)
}
