/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_TRACE_H
#define HACKBOT_TRACE_H

/*
 * hackbot_trace.h — Continuous kernel tracepoint sensing.
 *
 * Registers callbacks on kernel tracepoints (sched_switch, sys_enter,
 * block_rq_complete) that fire continuously. Data accumulates in three
 * tiers: raw event ring buffer, LinnOS-style feature vectors, and
 * aggregate statistics.
 *
 * Since standard tracepoints (sched_switch etc.) are NOT exported to
 * out-of-tree modules, we use for_each_kernel_tracepoint() to find them
 * at runtime, then register via tracepoint_probe_register().
 *
 * Called from Rust (hackbot_trace.rs) via extern "C" FFI.
 */

int  hackbot_trace_init(void);
void hackbot_trace_exit(void);

/* Read aggregate stats formatted as text. Returns bytes written. */
int  hackbot_trace_read_sched(char *out, int maxlen);
int  hackbot_trace_read_syscall(char *out, int maxlen);
int  hackbot_trace_read_io(char *out, int maxlen);

/* Read raw events. Returns bytes written. */
int  hackbot_trace_read_sched_raw(char *out, int maxlen, int count);
int  hackbot_trace_read_syscall_raw(char *out, int maxlen, int count);
int  hackbot_trace_read_io_raw(char *out, int maxlen, int count);

/* Read semantic tokens (Tier 4). Returns bytes written. */
int  hackbot_trace_read_tokens(char *out, int maxlen, int count);

/* Read n-gram surprise scores (Tier 5). Returns bytes written. */
int  hackbot_trace_read_ngram_surprise(char *out, int maxlen);
int  hackbot_trace_read_ngram_stats(char *out, int maxlen);
int  hackbot_trace_read_ngram_alerts(char *out, int maxlen, int count);

/* Reset "since last reset" counters. */
void hackbot_trace_reset(void);

#endif /* HACKBOT_TRACE_H */
