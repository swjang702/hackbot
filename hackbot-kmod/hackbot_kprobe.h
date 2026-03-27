/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_KPROBE_H
#define HACKBOT_KPROBE_H

/*
 * hackbot_kprobe.h — Kprobe instrumentation for the hackbot agent.
 *
 * Manages up to 8 active kprobes that the LLM agent can attach to
 * arbitrary kernel functions.  Each kprobe tracks a hit counter
 * (atomic64_t) that the agent can query.
 *
 * This is a Tier 1 (instrumentation) capability — reversible, no
 * kernel state mutation, only performance overhead from probe hits.
 *
 * Called from Rust (hackbot_tools.rs) via extern "C" FFI.
 */

int  hackbot_kprobe_attach(const char *symbol, int len);
int  hackbot_kprobe_check(char *out, int maxlen);
int  hackbot_kprobe_detach(const char *symbol, int len);
void hackbot_kprobe_cleanup(void);

#endif /* HACKBOT_KPROBE_H */
