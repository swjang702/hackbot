// SPDX-License-Identifier: GPL-2.0

//! vLLM inference backend — the System 2 "brain".

use kernel::prelude::*;

use crate::agent::agent_loop_local;
use crate::config::*;
use crate::context::gather_kernel_context;
use crate::net::*;
use crate::state::MODEL;
use crate::tools::{execute_tool, parse_tool_call, ToolCallResult};

/// Agent loop dispatcher: selects inference backend based on INFERENCE_MODE.
pub(crate) fn agent_loop(prompt: &[u8]) -> Result<KVVec<u8>> {
    let use_local = match INFERENCE_MODE {
        INFERENCE_MODE_LOCAL => true,
        INFERENCE_MODE_VLLM => false,
        _ => {
            let slot = MODEL.lock();
            slot.loaded
        }
    };

    if use_local {
        match agent_loop_local(prompt) {
            Ok(response) if !response.is_empty() => {
                pr_info!("hackbot: using local inference backend\n");
                return Ok(response);
            }
            Ok(_) => {
                if INFERENCE_MODE == INFERENCE_MODE_LOCAL {
                    let mut r = KVVec::new();
                    let _ = r.extend_from_slice(
                        b"[hackbot] Local inference produced no output.\n", GFP_KERNEL,
                    );
                    return Ok(r);
                }
                pr_info!("hackbot: local inference empty, falling back to vLLM\n");
            }
            Err(e) => {
                if INFERENCE_MODE == INFERENCE_MODE_LOCAL {
                    return Err(e);
                }
                pr_info!("hackbot: local inference failed ({}), falling back to vLLM\n",
                         e.to_errno());
            }
        }
    }

    agent_loop_vllm(prompt)
}

/// Process a prompt through the agent loop and format the result.
pub(crate) fn process_prompt(prompt: &[u8]) -> KVVec<u8> {
    let prompt_trimmed = if prompt.last() == Some(&b'\n') {
        &prompt[..prompt.len() - 1]
    } else {
        prompt
    };

    if prompt_trimmed.is_empty() {
        let mut r = KVVec::new();
        let _ = r.extend_from_slice(b"[hackbot] Empty prompt.\n", GFP_KERNEL);
        return r;
    }

    pr_info!("hackbot: [user] query received ({} bytes)\n", prompt_trimmed.len());

    match agent_loop(prompt_trimmed) {
        Ok(mut text) => {
            if text.last() != Some(&b'\n') {
                let _ = text.push(b'\n', GFP_KERNEL);
            }
            text
        }
        Err(e) => {
            let mut r = KVVec::new();
            let _ = r.extend_from_slice(b"[hackbot] Inference error: ", GFP_KERNEL);
            let mut code_buf = [0u8; 20];
            let code = -(e.to_errno() as isize) as usize;
            let code_str = format_usize(code, &mut code_buf);
            let _ = r.extend_from_slice(code_str, GFP_KERNEL);
            let _ = r.extend_from_slice(b" (errno)\n", GFP_KERNEL);

            let hint = match -(e.to_errno()) {
                19 => b"No model loaded and no vLLM available.\n" as &[u8],
                111 => b"Connection refused - is vLLM running on port 8000?\n",
                110 => b"Connection timed out - check network/firewall.\n",
                _ => b"",
            };
            if !hint.is_empty() {
                let _ = r.extend_from_slice(hint, GFP_KERNEL);
            }
            r
        }
    }
}

/// Send a single chat completion request to vLLM.
fn vllm_call(model_name: &[u8], messages_json: &[u8]) -> Result<KVVec<u8>> {
    let request = build_vllm_request(model_name, messages_json)?;

    let sock = KernelSocket::connect_tcp(VLLM_ADDR, VLLM_PORT)?;
    sock.send_all(&request)?;

    let mut raw_response = KVVec::new();
    sock.recv_all(&mut raw_response, MAX_RESPONSE_SIZE)?;
    drop(sock);

    let status = parse_http_status(&raw_response);
    if status != 200 {
        pr_warn!("hackbot: vLLM returned HTTP {}\n", status);
        if status == 0 {
            return Err(EIO);
        }
        let body = find_http_body(&raw_response);
        let mut result = KVVec::new();
        let _ = result.extend_from_slice(b"[HTTP ", GFP_KERNEL);
        let mut status_buf = [0u8; 20];
        let status_str = format_usize(status as usize, &mut status_buf);
        let _ = result.extend_from_slice(status_str, GFP_KERNEL);
        let _ = result.extend_from_slice(b"] ", GFP_KERNEL);
        let _ = result.extend_from_slice(body, GFP_KERNEL);
        return Ok(result);
    }

    let body = find_http_body(&raw_response);

    match extract_text_from_json(body) {
        Some(escaped_text) => {
            let mut result = KVVec::new();
            json_unescape(escaped_text, &mut result);
            Ok(result)
        }
        None => {
            pr_warn!("hackbot: could not parse vLLM JSON response\n");
            let mut result = KVVec::new();
            let _ = result.extend_from_slice(b"[hackbot] Raw response: ", GFP_KERNEL);
            let _ = result.extend_from_slice(body, GFP_KERNEL);
            Ok(result)
        }
    }
}

/// Discover the served model name from vLLM.
fn discover_model_name() -> Result<KVVec<u8>> {
    let mut req = KVVec::new();
    let _ = req.extend_from_slice(b"GET /v1/models HTTP/1.1\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Host: ", GFP_KERNEL);
    append_ipv4(&mut req, VLLM_ADDR);
    let _ = req.extend_from_slice(b"\r\nConnection: close\r\n\r\n", GFP_KERNEL);

    let sock = KernelSocket::connect_tcp(VLLM_ADDR, VLLM_PORT)?;
    sock.send_all(&req)?;

    let mut raw_response = KVVec::new();
    sock.recv_all(&mut raw_response, MAX_RESPONSE_SIZE)?;
    drop(sock);

    let status = parse_http_status(&raw_response);
    if status != 200 {
        pr_warn!("hackbot: /v1/models returned HTTP {}\n", status);
        return Err(EIO);
    }

    let body = find_http_body(&raw_response);

    let pattern = b"\"id\":\"";
    let pos = find_subsequence(body, pattern).ok_or(EIO)?;
    let value_start = pos + pattern.len();
    let value_end = find_json_string_end(body, value_start).ok_or(EIO)?;

    let model_name_raw = &body[value_start..value_end];
    let mut model_name = KVVec::new();
    json_unescape(model_name_raw, &mut model_name);

    pr_info!(
        "hackbot: discovered model: {}\n",
        core::str::from_utf8(&model_name).unwrap_or("?"),
    );

    Ok(model_name)
}

/// Build the HTTP POST request for vLLM's /v1/chat/completions endpoint.
fn build_vllm_request(model_name: &[u8], messages_json: &[u8]) -> Result<KVVec<u8>> {
    let mut body = KVVec::new();
    let _ = body.extend_from_slice(b"{\"model\":\"", GFP_KERNEL);
    json_escape(model_name, &mut body);
    let _ = body.extend_from_slice(b"\",\"messages\":", GFP_KERNEL);
    let _ = body.extend_from_slice(messages_json, GFP_KERNEL);
    let _ = body.extend_from_slice(b",\"max_tokens\":", GFP_KERNEL);
    let mut num_buf = [0u8; 20];
    let _ = body.extend_from_slice(format_usize(VLLM_MAX_TOKENS, &mut num_buf), GFP_KERNEL);
    let _ = body.extend_from_slice(
        b",\"temperature\":0.7,\"repetition_penalty\":1.1,\"stop\":[\"</tool>\"]}",
        GFP_KERNEL,
    );

    let mut req = KVVec::new();
    let _ = req.extend_from_slice(b"POST /v1/chat/completions HTTP/1.1\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Host: ", GFP_KERNEL);
    append_ipv4(&mut req, VLLM_ADDR);
    let _ = req.extend_from_slice(b"\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Content-Type: application/json\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Connection: close\r\n", GFP_KERNEL);

    let _ = req.extend_from_slice(b"Content-Length: ", GFP_KERNEL);
    let mut len_buf = [0u8; 20];
    let len_str = format_usize(body.len(), &mut len_buf);
    let _ = req.extend_from_slice(len_str, GFP_KERNEL);
    let _ = req.extend_from_slice(b"\r\n", GFP_KERNEL);

    let _ = req.extend_from_slice(b"\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(&body, GFP_KERNEL);

    Ok(req)
}

/// OODA agent loop with vLLM backend.
fn agent_loop_vllm(prompt: &[u8]) -> Result<KVVec<u8>> {
    let model_name = discover_model_name()?;

    let kernel_ctx = gather_kernel_context();

    let mut system_content = KVVec::new();
    let _ = system_content.extend_from_slice(SYSTEM_IDENTITY, GFP_KERNEL);
    let _ = system_content.extend_from_slice(&kernel_ctx, GFP_KERNEL);
    crate::memory::format_memory_for_prompt(&mut system_content);
    let _ = system_content.extend_from_slice(TOOL_DESCRIPTION, GFP_KERNEL);

    let mut messages = KVVec::new();
    append_message_to_json(&mut messages, b"system", &system_content);
    append_message_to_json(&mut messages, b"user", prompt);

    let mut final_answer = KVVec::new();
    let mut got_final_answer = false;

    // Track last tool pair for context-aware truncation rebuild.
    let mut last_asst = KVVec::new();
    let mut last_tool = KVVec::new();
    let mut has_tool_history = false;

    for iteration in 0..MAX_AGENT_ITERATIONS {
        pr_info!(
            "hackbot: agent iteration {}/{}\n",
            iteration + 1,
            MAX_AGENT_ITERATIONS,
        );

        // Context budget: rebuild conversation if over limit (sliding window).
        // Keeps system + user + last tool pair, discards older tool results.
        if messages.len() > VLLM_CONTEXT_BUDGET {
            pr_info!("hackbot: truncating conversation ({} bytes > {} budget)\n",
                     messages.len(), VLLM_CONTEXT_BUDGET);
            messages = KVVec::new();
            append_message_to_json(&mut messages, b"system", &system_content);
            append_message_to_json(&mut messages, b"user", prompt);
            if has_tool_history {
                let mut noted_asst = KVVec::new();
                let _ = noted_asst.extend_from_slice(
                    b"[Earlier tool calls were truncated to fit context.] ",
                    GFP_KERNEL,
                );
                let _ = noted_asst.extend_from_slice(&last_asst, GFP_KERNEL);
                append_message_to_json(&mut messages, b"assistant", &noted_asst);
                append_message_to_json(&mut messages, b"user", &last_tool);
            }
            pr_info!("hackbot: truncated to {} bytes\n", messages.len());
        }

        let response = match vllm_call(&model_name, &messages) {
            Ok(r) => r,
            Err(e) => {
                pr_err!("hackbot: vLLM call failed at iteration {}: {}\n", iteration + 1, e.to_errno());
                if !final_answer.is_empty() {
                    let _ = final_answer.extend_from_slice(
                        b"\n[hackbot] vLLM error during agent loop.\n",
                        GFP_KERNEL,
                    );
                    break;
                }
                return Err(e);
            }
        };

        match parse_tool_call(&response) {
            ToolCallResult::FinalAnswer(text) => {
                pr_info!("hackbot: final answer at iteration {}\n", iteration + 1);
                let _ = final_answer.extend_from_slice(text, GFP_KERNEL);
                got_final_answer = true;
                break;
            }
            ToolCallResult::ToolCall { name, prefix } => {
                pr_info!(
                    "hackbot: tool call '{}' at iteration {}\n",
                    core::str::from_utf8(name).unwrap_or("?"),
                    iteration + 1,
                );

                if iteration == MAX_AGENT_ITERATIONS - 1 {
                    pr_info!("hackbot: last iteration, forcing final answer with raw tool output\n");
                    let tool_output = execute_tool(name);
                    final_answer.truncate(0);
                    if !prefix.is_empty() {
                        let _ = final_answer.extend_from_slice(prefix, GFP_KERNEL);
                        let _ = final_answer.extend_from_slice(b"\n\n", GFP_KERNEL);
                    }
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

                let mut tool_result = KVVec::new();
                let _ = tool_result.extend_from_slice(b"[Tool: ", GFP_KERNEL);
                let _ = tool_result.extend_from_slice(name, GFP_KERNEL);
                let _ = tool_result.extend_from_slice(b"]\n", GFP_KERNEL);
                let _ = tool_result.extend_from_slice(&tool_output, GFP_KERNEL);
                let _ = tool_result.extend_from_slice(
                    b"[End Tool]\nAbove is live data from the kernel. \
Analyze it thoughtfully and respond to the user.",
                    GFP_KERNEL,
                );

                // Save last pair for potential truncation rebuild.
                last_asst.truncate(0);
                let _ = last_asst.extend_from_slice(&assistant_content, GFP_KERNEL);
                last_tool.truncate(0);
                let _ = last_tool.extend_from_slice(&tool_result, GFP_KERNEL);
                has_tool_history = true;

                append_message_to_json(&mut messages, b"assistant", &assistant_content);
                append_message_to_json(&mut messages, b"user", &tool_result);

                let _ = final_answer.extend_from_slice(prefix, GFP_KERNEL);
            }
        }
    }

    if !got_final_answer && final_answer.is_empty() {
        let _ = final_answer.extend_from_slice(
            b"[hackbot] Agent completed maximum iterations without a final answer.\n",
            GFP_KERNEL,
        );
    } else if !got_final_answer {
        let _ = final_answer.extend_from_slice(
            b"\n[hackbot] Agent stopped after maximum iterations.\n",
            GFP_KERNEL,
        );
    }

    // Record this interaction in agent memory for future context.
    if got_final_answer && !final_answer.is_empty() {
        crate::memory::record_finding(crate::config::SOURCE_USER, &final_answer);
    }

    Ok(final_answer)
}
