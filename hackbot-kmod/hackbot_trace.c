// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_trace.c — Continuous kernel tracepoint sensing (Layer 0).
 *
 * Always-on tracepoint callbacks that fire on every sched_switch,
 * sys_enter, and block_rq_complete event. Each callback updates three
 * data tiers simultaneously:
 *
 *   Tier 1: Raw event ring buffer (last 1024 events, ~64KB each)
 *   Tier 2: Feature vectors (LinnOS-style sliding window, for classifiers)
 *   Tier 3: Aggregate stats (atomic counters, per-task/per-syscall)
 *
 * Standard tracepoints are NOT exported to out-of-tree modules, so we
 * use for_each_kernel_tracepoint() to find them at runtime and register
 * via tracepoint_probe_register().
 *
 * Callbacks run with preemption disabled — no sleeping, no allocations.
 * All updates are atomic or protected by raw_spinlock (~100ns total).
 */

#include <linux/kernel.h>
#include <linux/tracepoint.h>
#include <linux/sched.h>
#include <linux/blkdev.h>
#include <linux/blk-mq.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/atomic.h>
#include <linux/timekeeping.h>
#include <linux/slab.h>
#include "hackbot_trace.h"

/* ===================================================================
 * Configuration
 * =================================================================== */

#define RAW_RING_SIZE     1024     /* events per tracepoint */
#define MAX_TRACKED_TASKS 64       /* top-N tasks in aggregates */
#define MAX_SYSCALL_ID    512      /* syscall ID range */
#define FEATURE_WINDOW    4        /* sliding window size (LinnOS uses 4) */
#define IO_LAT_BUCKETS    8        /* latency histogram buckets */

/* ===================================================================
 * Data structures — Three-tier per tracepoint
 * =================================================================== */

/* --- Tier 1: Raw event ring buffer --- */

struct raw_sched_event {
	u64 timestamp_ns;
	u32 cpu;
	s32 prev_pid;
	s32 next_pid;
	u32 prev_state;
	char prev_comm[16];
	char next_comm[16];
};

struct raw_syscall_event {
	u64 timestamp_ns;
	u32 cpu;
	s32 pid;
	long syscall_id;
	char comm[16];
};

struct raw_io_event {
	u64 timestamp_ns;
	u32 cpu;
	u64 sector;
	u32 nr_bytes;
	s32 error;
	u8  is_write;
};

/* --- Tier 2: Feature vectors (LinnOS-style) --- */

struct sched_features {
	u32 last_switch_intervals_us[FEATURE_WINDOW];
	u32 last_runqueue_lengths[FEATURE_WINDOW];
	u64 last_switch_ns;           /* timestamp of previous switch */
	u32 switches_in_window;       /* 1-second rolling count */
	u64 window_start_ns;
};

struct syscall_features {
	u32 last_syscall_ids[FEATURE_WINDOW];
	u32 last_intervals_us[FEATURE_WINDOW];
	u64 last_syscall_ns;
	u32 syscalls_in_window;
	u64 window_start_ns;
};

struct io_features {
	u32 last_latencies_us[FEATURE_WINDOW];   /* THE key LinnOS feature */
	u32 last_pending_ios[FEATURE_WINDOW];
	u32 current_queue_depth;
	u32 ios_in_window;
	u64 window_start_ns;
};

/* --- Tier 3: Aggregate stats --- */

struct task_entry {
	s32 pid;
	char comm[16];
	atomic64_t count;
};

struct sched_aggregates {
	atomic64_t total;
	atomic64_t total_since_reset;
	struct task_entry tasks[MAX_TRACKED_TASKS];
	atomic_t n_tasks;
};

struct syscall_aggregates {
	atomic64_t total;
	atomic64_t total_since_reset;
	atomic64_t per_syscall[MAX_SYSCALL_ID];
};

struct io_aggregates {
	atomic64_t total;
	atomic64_t total_since_reset;
	atomic64_t lat_buckets[IO_LAT_BUCKETS]; /* <100us,<500us,<1ms,<5ms,<10ms,<50ms,<100ms,>100ms */
	atomic64_t total_latency_us;            /* for computing average */
};

/* --- Combined state --- */

struct hackbot_trace_state {
	/* Tracepoint pointers (found at runtime) */
	struct tracepoint *tp_sched_switch;
	struct tracepoint *tp_sys_enter;
	struct tracepoint *tp_block_rq_complete;

	/* Sched channel */
	struct raw_sched_event *sched_ring;
	atomic_t sched_ring_head;
	struct sched_features sched_feat;
	raw_spinlock_t sched_feat_lock;
	struct sched_aggregates sched_agg;

	/* Syscall channel */
	struct raw_syscall_event *syscall_ring;
	atomic_t syscall_ring_head;
	struct syscall_features syscall_feat;
	raw_spinlock_t syscall_feat_lock;
	struct syscall_aggregates syscall_agg;

	/* I/O channel */
	struct raw_io_event *io_ring;
	atomic_t io_ring_head;
	struct io_features io_feat;
	raw_spinlock_t io_feat_lock;
	struct io_aggregates io_agg;

	/* Metadata */
	u64 start_ns;
	u64 reset_ns;
	bool active;
};

static struct hackbot_trace_state *trace_state;

/* ===================================================================
 * Helper: shift feature window
 * =================================================================== */

static inline void shift_u32_window(u32 *arr, int size, u32 new_val)
{
	int i;
	for (i = 0; i < size - 1; i++)
		arr[i] = arr[i + 1];
	arr[size - 1] = new_val;
}

/* ===================================================================
 * Tracepoint callbacks (preemption disabled, ~100ns each)
 * =================================================================== */

static void hackbot_probe_sched_switch(void *data, bool preempt,
	struct task_struct *prev, struct task_struct *next,
	unsigned int prev_state)
{
	struct hackbot_trace_state *s = data;
	u64 now = ktime_get_raw_fast_ns();
	int idx;
	unsigned long flags;
	u32 interval_us;

	/* Tier 1: Raw ring buffer */
	idx = atomic_inc_return(&s->sched_ring_head) % RAW_RING_SIZE;
	s->sched_ring[idx].timestamp_ns = now;
	s->sched_ring[idx].cpu = raw_smp_processor_id();
	s->sched_ring[idx].prev_pid = prev->pid;
	s->sched_ring[idx].next_pid = next->pid;
	s->sched_ring[idx].prev_state = prev_state;
	memcpy(s->sched_ring[idx].prev_comm, prev->comm, 16);
	memcpy(s->sched_ring[idx].next_comm, next->comm, 16);

	/* Tier 2: Feature vector */
	raw_spin_lock_irqsave(&s->sched_feat_lock, flags);
	if (s->sched_feat.last_switch_ns) {
		interval_us = (u32)((now - s->sched_feat.last_switch_ns) / 1000);
		shift_u32_window(s->sched_feat.last_switch_intervals_us,
				 FEATURE_WINDOW, interval_us);
	}
	s->sched_feat.last_switch_ns = now;
	s->sched_feat.switches_in_window++;
	/* Reset 1-second window */
	if (now - s->sched_feat.window_start_ns > 1000000000ULL) {
		s->sched_feat.switches_in_window = 1;
		s->sched_feat.window_start_ns = now;
	}
	raw_spin_unlock_irqrestore(&s->sched_feat_lock, flags);

	/* Tier 3: Aggregates */
	atomic64_inc(&s->sched_agg.total);
	atomic64_inc(&s->sched_agg.total_since_reset);

	/* Update per-task counter for 'next' */
	{
		int i, n = atomic_read(&s->sched_agg.n_tasks);
		for (i = 0; i < n; i++) {
			if (s->sched_agg.tasks[i].pid == next->pid) {
				atomic64_inc(&s->sched_agg.tasks[i].count);
				return;
			}
		}
		/* New task — try to add */
		if (n < MAX_TRACKED_TASKS) {
			int slot = atomic_inc_return(&s->sched_agg.n_tasks) - 1;
			if (slot < MAX_TRACKED_TASKS) {
				s->sched_agg.tasks[slot].pid = next->pid;
				memcpy(s->sched_agg.tasks[slot].comm, next->comm, 16);
				atomic64_set(&s->sched_agg.tasks[slot].count, 1);
			}
		}
	}
}

static void hackbot_probe_sys_enter(void *data, struct pt_regs *regs, long id)
{
	struct hackbot_trace_state *s = data;
	u64 now = ktime_get_raw_fast_ns();
	int idx;
	unsigned long flags;
	u32 interval_us;

	/* Tier 1: Raw ring */
	idx = atomic_inc_return(&s->syscall_ring_head) % RAW_RING_SIZE;
	s->syscall_ring[idx].timestamp_ns = now;
	s->syscall_ring[idx].cpu = raw_smp_processor_id();
	s->syscall_ring[idx].pid = current->pid;
	s->syscall_ring[idx].syscall_id = id;
	memcpy(s->syscall_ring[idx].comm, current->comm, 16);

	/* Tier 2: Features */
	raw_spin_lock_irqsave(&s->syscall_feat_lock, flags);
	if (s->syscall_feat.last_syscall_ns) {
		interval_us = (u32)((now - s->syscall_feat.last_syscall_ns) / 1000);
		shift_u32_window(s->syscall_feat.last_intervals_us,
				 FEATURE_WINDOW, interval_us);
	}
	shift_u32_window(s->syscall_feat.last_syscall_ids,
			 FEATURE_WINDOW, (u32)id);
	s->syscall_feat.last_syscall_ns = now;
	s->syscall_feat.syscalls_in_window++;
	if (now - s->syscall_feat.window_start_ns > 1000000000ULL) {
		s->syscall_feat.syscalls_in_window = 1;
		s->syscall_feat.window_start_ns = now;
	}
	raw_spin_unlock_irqrestore(&s->syscall_feat_lock, flags);

	/* Tier 3: Aggregates */
	atomic64_inc(&s->syscall_agg.total);
	atomic64_inc(&s->syscall_agg.total_since_reset);
	if (id >= 0 && id < MAX_SYSCALL_ID)
		atomic64_inc(&s->syscall_agg.per_syscall[id]);
}

static void hackbot_probe_block_rq_complete(void *data, struct request *rq,
	blk_status_t error, unsigned int nr_bytes)
{
	struct hackbot_trace_state *s = data;
	u64 now = ktime_get_raw_fast_ns();
	int idx;
	unsigned long flags;
	u32 latency_us;
	int bucket;

	/* Compute I/O latency from request start time */
	latency_us = (u32)((now - rq->start_time_ns) / 1000);

	/* Tier 1: Raw ring */
	idx = atomic_inc_return(&s->io_ring_head) % RAW_RING_SIZE;
	s->io_ring[idx].timestamp_ns = now;
	s->io_ring[idx].cpu = raw_smp_processor_id();
	s->io_ring[idx].sector = blk_rq_pos(rq);
	s->io_ring[idx].nr_bytes = nr_bytes;
	s->io_ring[idx].error = blk_status_to_errno(error);
	s->io_ring[idx].is_write = op_is_write(req_op(rq)) ? 1 : 0;

	/* Tier 2: Features */
	raw_spin_lock_irqsave(&s->io_feat_lock, flags);
	shift_u32_window(s->io_feat.last_latencies_us, FEATURE_WINDOW, latency_us);
	s->io_feat.ios_in_window++;
	if (now - s->io_feat.window_start_ns > 1000000000ULL) {
		s->io_feat.ios_in_window = 1;
		s->io_feat.window_start_ns = now;
	}
	raw_spin_unlock_irqrestore(&s->io_feat_lock, flags);

	/* Tier 3: Aggregates */
	atomic64_inc(&s->io_agg.total);
	atomic64_inc(&s->io_agg.total_since_reset);
	atomic64_add(latency_us, &s->io_agg.total_latency_us);

	/* Latency histogram */
	if (latency_us < 100)        bucket = 0;
	else if (latency_us < 500)   bucket = 1;
	else if (latency_us < 1000)  bucket = 2;
	else if (latency_us < 5000)  bucket = 3;
	else if (latency_us < 10000) bucket = 4;
	else if (latency_us < 50000) bucket = 5;
	else if (latency_us < 100000) bucket = 6;
	else                          bucket = 7;
	atomic64_inc(&s->io_agg.lat_buckets[bucket]);
}

/* ===================================================================
 * Tracepoint discovery (runtime lookup)
 * =================================================================== */

struct tp_lookup {
	const char *name;
	struct tracepoint **result;
};

static void find_tracepoint_cb(struct tracepoint *tp, void *priv)
{
	struct tp_lookup *lookup = priv;
	if (!strcmp(tp->name, lookup->name))
		*lookup->result = tp;
}

static struct tracepoint *find_tracepoint(const char *name)
{
	struct tracepoint *tp = NULL;
	struct tp_lookup lookup = { .name = name, .result = &tp };
	for_each_kernel_tracepoint(find_tracepoint_cb, &lookup);
	return tp;
}

/* ===================================================================
 * Output formatting helpers
 * =================================================================== */

static int append_num(char *out, int pos, int maxlen, long long val)
{
	char tmp[24];
	int len = 0, i;
	int neg = 0;

	if (val < 0) { neg = 1; val = -val; }
	if (val == 0) { tmp[len++] = '0'; }
	else { while (val > 0 && len < 20) { tmp[len++] = '0' + (char)(val % 10); val /= 10; } }
	if (neg && pos < maxlen) out[pos++] = '-';
	for (i = len - 1; i >= 0; i--) { if (pos >= maxlen) return -1; out[pos++] = tmp[i]; }
	return pos;
}

static int append_str(char *out, int pos, int maxlen, const char *s)
{
	while (*s && pos < maxlen) out[pos++] = *s++;
	return pos;
}

/* ===================================================================
 * Read functions — format data for the agent
 * =================================================================== */

int hackbot_trace_read_sched(char *out, int maxlen)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0;
	long long total, since_reset, uptime_s;
	unsigned long flags;
	int i, n;

	if (!s || !s->active) {
		pos = append_str(out, pos, maxlen, "[Trace not active]\n");
		return pos;
	}

	total = atomic64_read(&s->sched_agg.total);
	since_reset = atomic64_read(&s->sched_agg.total_since_reset);
	uptime_s = (long long)(ktime_get_raw_fast_ns() - s->start_ns) / 1000000000LL;

	pos = append_str(out, pos, maxlen, "=== Scheduler Trace (active ");
	pos = append_num(out, pos, maxlen, uptime_s / 60);
	pos = append_str(out, pos, maxlen, "m, ");
	pos = append_num(out, pos, maxlen, total);
	pos = append_str(out, pos, maxlen, " events) ===\n");

	if (uptime_s > 0) {
		pos = append_str(out, pos, maxlen, "Rate: ");
		pos = append_num(out, pos, maxlen, total / uptime_s);
		pos = append_str(out, pos, maxlen, "/s\n");
	}

	/* Top tasks */
	n = atomic_read(&s->sched_agg.n_tasks);
	if (n > 0) {
		pos = append_str(out, pos, maxlen, "Top tasks:");
		/* Simple: show first 10 tasks (not sorted — good enough for now) */
		for (i = 0; i < n && i < 10; i++) {
			long long cnt = atomic64_read(&s->sched_agg.tasks[i].count);
			if (cnt == 0) continue;
			pos = append_str(out, pos, maxlen, " ");
			pos = append_str(out, pos, maxlen, s->sched_agg.tasks[i].comm);
			pos = append_str(out, pos, maxlen, "(");
			pos = append_num(out, pos, maxlen, s->sched_agg.tasks[i].pid);
			pos = append_str(out, pos, maxlen, ")=");
			pos = append_num(out, pos, maxlen, cnt);
			if (pos < 0) break;
		}
		pos = append_str(out, pos, maxlen, "\n");
	}

	/* Features */
	raw_spin_lock_irqsave(&s->sched_feat_lock, flags);
	pos = append_str(out, pos, maxlen, "Features: intervals=[");
	for (i = 0; i < FEATURE_WINDOW; i++) {
		if (i > 0) pos = append_str(out, pos, maxlen, ",");
		pos = append_num(out, pos, maxlen, s->sched_feat.last_switch_intervals_us[i]);
	}
	pos = append_str(out, pos, maxlen, "]us rate=");
	pos = append_num(out, pos, maxlen, s->sched_feat.switches_in_window);
	pos = append_str(out, pos, maxlen, "/s\n");
	raw_spin_unlock_irqrestore(&s->sched_feat_lock, flags);

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_sched_raw(char *out, int maxlen, int count)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i;
	int head, start;

	if (!s || !s->active) return 0;
	if (count <= 0 || count > RAW_RING_SIZE) count = 20;

	head = atomic_read(&s->sched_ring_head);
	start = (head - count + RAW_RING_SIZE) % RAW_RING_SIZE;

	pos = append_str(out, pos, maxlen, "=== Raw: sched_switch (last ");
	pos = append_num(out, pos, maxlen, count);
	pos = append_str(out, pos, maxlen, ") ===\n");

	for (i = 0; i < count && pos > 0 && pos < maxlen - 128; i++) {
		int idx = (start + i) % RAW_RING_SIZE;
		struct raw_sched_event *ev = &s->sched_ring[idx];
		u64 rel_s, rel_ms;

		if (ev->timestamp_ns == 0) continue; /* empty slot */

		rel_s = (ev->timestamp_ns - s->start_ns) / 1000000000ULL;
		rel_ms = ((ev->timestamp_ns - s->start_ns) / 1000000ULL) % 1000;

		pos = append_str(out, pos, maxlen, "[+");
		pos = append_num(out, pos, maxlen, (long long)(rel_s / 60));
		pos = append_str(out, pos, maxlen, "m:");
		pos = append_num(out, pos, maxlen, (long long)(rel_s % 60));
		pos = append_str(out, pos, maxlen, ".");
		/* zero-pad ms to 3 digits */
		if (rel_ms < 100) pos = append_str(out, pos, maxlen, "0");
		if (rel_ms < 10) pos = append_str(out, pos, maxlen, "0");
		pos = append_num(out, pos, maxlen, (long long)rel_ms);
		pos = append_str(out, pos, maxlen, "] CPU");
		pos = append_num(out, pos, maxlen, ev->cpu);
		pos = append_str(out, pos, maxlen, ": ");
		pos = append_str(out, pos, maxlen, ev->prev_comm);
		pos = append_str(out, pos, maxlen, "(");
		pos = append_num(out, pos, maxlen, ev->prev_pid);
		pos = append_str(out, pos, maxlen, ") -> ");
		pos = append_str(out, pos, maxlen, ev->next_comm);
		pos = append_str(out, pos, maxlen, "(");
		pos = append_num(out, pos, maxlen, ev->next_pid);
		pos = append_str(out, pos, maxlen, ")\n");
	}

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_syscall(char *out, int maxlen)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i;
	long long total;
	unsigned long flags;

	if (!s || !s->active) {
		pos = append_str(out, pos, maxlen, "[Trace not active]\n");
		return pos;
	}

	total = atomic64_read(&s->syscall_agg.total);

	pos = append_str(out, pos, maxlen, "=== Syscall Trace (");
	pos = append_num(out, pos, maxlen, total);
	pos = append_str(out, pos, maxlen, " events) ===\n");

	/* Top syscalls */
	pos = append_str(out, pos, maxlen, "Top syscalls:");
	for (i = 0; i < MAX_SYSCALL_ID && pos > 0; i++) {
		long long cnt = atomic64_read(&s->syscall_agg.per_syscall[i]);
		if (cnt > 0) {
			pos = append_str(out, pos, maxlen, " ");
			pos = append_num(out, pos, maxlen, i);
			pos = append_str(out, pos, maxlen, "=");
			pos = append_num(out, pos, maxlen, cnt);
		}
	}
	pos = append_str(out, pos, maxlen, "\n");

	/* Features */
	raw_spin_lock_irqsave(&s->syscall_feat_lock, flags);
	pos = append_str(out, pos, maxlen, "Features: ids=[");
	for (i = 0; i < FEATURE_WINDOW; i++) {
		if (i > 0) pos = append_str(out, pos, maxlen, ",");
		pos = append_num(out, pos, maxlen, s->syscall_feat.last_syscall_ids[i]);
	}
	pos = append_str(out, pos, maxlen, "] intervals=[");
	for (i = 0; i < FEATURE_WINDOW; i++) {
		if (i > 0) pos = append_str(out, pos, maxlen, ",");
		pos = append_num(out, pos, maxlen, s->syscall_feat.last_intervals_us[i]);
	}
	pos = append_str(out, pos, maxlen, "]us\n");
	raw_spin_unlock_irqrestore(&s->syscall_feat_lock, flags);

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_syscall_raw(char *out, int maxlen, int count)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i, head, start;

	if (!s || !s->active) return 0;
	if (count <= 0 || count > RAW_RING_SIZE) count = 20;

	head = atomic_read(&s->syscall_ring_head);
	start = (head - count + RAW_RING_SIZE) % RAW_RING_SIZE;

	pos = append_str(out, pos, maxlen, "=== Raw: sys_enter (last ");
	pos = append_num(out, pos, maxlen, count);
	pos = append_str(out, pos, maxlen, ") ===\n");

	for (i = 0; i < count && pos > 0 && pos < maxlen - 100; i++) {
		int idx = (start + i) % RAW_RING_SIZE;
		struct raw_syscall_event *ev = &s->syscall_ring[idx];
		if (ev->timestamp_ns == 0) continue;

		pos = append_str(out, pos, maxlen, "CPU");
		pos = append_num(out, pos, maxlen, ev->cpu);
		pos = append_str(out, pos, maxlen, " ");
		pos = append_str(out, pos, maxlen, ev->comm);
		pos = append_str(out, pos, maxlen, "(");
		pos = append_num(out, pos, maxlen, ev->pid);
		pos = append_str(out, pos, maxlen, "): syscall ");
		pos = append_num(out, pos, maxlen, ev->syscall_id);
		pos = append_str(out, pos, maxlen, "\n");
	}

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_io(char *out, int maxlen)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i;
	long long total, total_lat;
	unsigned long flags;
	static const char *bucket_names[] = {
		"<100us", "<500us", "<1ms", "<5ms",
		"<10ms", "<50ms", "<100ms", ">100ms"
	};

	if (!s || !s->active) {
		pos = append_str(out, pos, maxlen, "[Trace not active]\n");
		return pos;
	}

	total = atomic64_read(&s->io_agg.total);
	total_lat = atomic64_read(&s->io_agg.total_latency_us);

	pos = append_str(out, pos, maxlen, "=== I/O Trace (");
	pos = append_num(out, pos, maxlen, total);
	pos = append_str(out, pos, maxlen, " events) ===\n");

	if (total > 0) {
		pos = append_str(out, pos, maxlen, "Avg latency: ");
		pos = append_num(out, pos, maxlen, total_lat / total);
		pos = append_str(out, pos, maxlen, "us\n");
	}

	/* Histogram */
	pos = append_str(out, pos, maxlen, "Histogram:");
	for (i = 0; i < IO_LAT_BUCKETS; i++) {
		long long cnt = atomic64_read(&s->io_agg.lat_buckets[i]);
		if (cnt > 0) {
			pos = append_str(out, pos, maxlen, " ");
			pos = append_str(out, pos, maxlen, bucket_names[i]);
			pos = append_str(out, pos, maxlen, "=");
			pos = append_num(out, pos, maxlen, cnt);
		}
	}
	pos = append_str(out, pos, maxlen, "\n");

	/* Features */
	raw_spin_lock_irqsave(&s->io_feat_lock, flags);
	pos = append_str(out, pos, maxlen, "Features: lats=[");
	for (i = 0; i < FEATURE_WINDOW; i++) {
		if (i > 0) pos = append_str(out, pos, maxlen, ",");
		pos = append_num(out, pos, maxlen, s->io_feat.last_latencies_us[i]);
	}
	pos = append_str(out, pos, maxlen, "]us\n");
	raw_spin_unlock_irqrestore(&s->io_feat_lock, flags);

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_io_raw(char *out, int maxlen, int count)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i, head, start;

	if (!s || !s->active) return 0;
	if (count <= 0 || count > RAW_RING_SIZE) count = 20;

	head = atomic_read(&s->io_ring_head);
	start = (head - count + RAW_RING_SIZE) % RAW_RING_SIZE;

	pos = append_str(out, pos, maxlen, "=== Raw: block_rq_complete (last ");
	pos = append_num(out, pos, maxlen, count);
	pos = append_str(out, pos, maxlen, ") ===\n");

	for (i = 0; i < count && pos > 0 && pos < maxlen - 100; i++) {
		int idx = (start + i) % RAW_RING_SIZE;
		struct raw_io_event *ev = &s->io_ring[idx];
		if (ev->timestamp_ns == 0) continue;

		pos = append_str(out, pos, maxlen, ev->is_write ? "WRITE" : "READ ");
		pos = append_str(out, pos, maxlen, " sector=");
		pos = append_num(out, pos, maxlen, (long long)ev->sector);
		pos = append_str(out, pos, maxlen, " bytes=");
		pos = append_num(out, pos, maxlen, ev->nr_bytes);
		pos = append_str(out, pos, maxlen, "\n");
	}

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

void hackbot_trace_reset(void)
{
	struct hackbot_trace_state *s = trace_state;
	if (!s) return;

	atomic64_set(&s->sched_agg.total_since_reset, 0);
	atomic64_set(&s->syscall_agg.total_since_reset, 0);
	atomic64_set(&s->io_agg.total_since_reset, 0);
	s->reset_ns = ktime_get_raw_fast_ns();
}

/* ===================================================================
 * Init / Exit
 * =================================================================== */

int hackbot_trace_init(void)
{
	struct hackbot_trace_state *s;
	int ret;

	s = kzalloc(sizeof(*s), GFP_KERNEL);
	if (!s)
		return -ENOMEM;

	/* Allocate ring buffers */
	s->sched_ring = kvmalloc(sizeof(struct raw_sched_event) * RAW_RING_SIZE, GFP_KERNEL);
	s->syscall_ring = kvmalloc(sizeof(struct raw_syscall_event) * RAW_RING_SIZE, GFP_KERNEL);
	s->io_ring = kvmalloc(sizeof(struct raw_io_event) * RAW_RING_SIZE, GFP_KERNEL);

	if (!s->sched_ring || !s->syscall_ring || !s->io_ring) {
		pr_err("hackbot: trace: failed to allocate ring buffers\n");
		ret = -ENOMEM;
		goto fail;
	}

	memset(s->sched_ring, 0, sizeof(struct raw_sched_event) * RAW_RING_SIZE);
	memset(s->syscall_ring, 0, sizeof(struct raw_syscall_event) * RAW_RING_SIZE);
	memset(s->io_ring, 0, sizeof(struct raw_io_event) * RAW_RING_SIZE);

	raw_spin_lock_init(&s->sched_feat_lock);
	raw_spin_lock_init(&s->syscall_feat_lock);
	raw_spin_lock_init(&s->io_feat_lock);

	s->start_ns = ktime_get_raw_fast_ns();
	s->reset_ns = s->start_ns;

	/* Discover tracepoints by name */
	s->tp_sched_switch = find_tracepoint("sched_switch");
	s->tp_sys_enter = find_tracepoint("sys_enter");
	s->tp_block_rq_complete = find_tracepoint("block_rq_complete");

	/* Register callbacks */
	if (s->tp_sched_switch) {
		ret = tracepoint_probe_register(s->tp_sched_switch,
			hackbot_probe_sched_switch, s);
		if (ret) {
			pr_warn("hackbot: trace: sched_switch register failed (%d)\n", ret);
			s->tp_sched_switch = NULL;
		} else {
			pr_info("hackbot: trace: sched_switch registered\n");
		}
	} else {
		pr_warn("hackbot: trace: sched_switch tracepoint not found\n");
	}

	if (s->tp_sys_enter) {
		ret = tracepoint_probe_register(s->tp_sys_enter,
			hackbot_probe_sys_enter, s);
		if (ret) {
			pr_warn("hackbot: trace: sys_enter register failed (%d)\n", ret);
			s->tp_sys_enter = NULL;
		} else {
			pr_info("hackbot: trace: sys_enter registered\n");
		}
	} else {
		pr_warn("hackbot: trace: sys_enter tracepoint not found\n");
	}

	if (s->tp_block_rq_complete) {
		ret = tracepoint_probe_register(s->tp_block_rq_complete,
			hackbot_probe_block_rq_complete, s);
		if (ret) {
			pr_warn("hackbot: trace: block_rq_complete register failed (%d)\n", ret);
			s->tp_block_rq_complete = NULL;
		} else {
			pr_info("hackbot: trace: block_rq_complete registered\n");
		}
	} else {
		pr_info("hackbot: trace: block_rq_complete tracepoint not found (no block devices?)\n");
	}

	s->active = true;
	trace_state = s;

	pr_info("hackbot: trace: sensory layer initialized (%s%s%s)\n",
		s->tp_sched_switch ? "sched " : "",
		s->tp_sys_enter ? "syscall " : "",
		s->tp_block_rq_complete ? "io " : "");

	return 0;

fail:
	if (s->sched_ring)   kvfree(s->sched_ring);
	if (s->syscall_ring) kvfree(s->syscall_ring);
	if (s->io_ring)      kvfree(s->io_ring);
	kfree(s);
	return ret;
}

void hackbot_trace_exit(void)
{
	struct hackbot_trace_state *s = trace_state;
	if (!s) return;

	s->active = false;

	/* Unregister tracepoints (synchronizes — waits for all CPUs) */
	if (s->tp_sched_switch)
		tracepoint_probe_unregister(s->tp_sched_switch,
			hackbot_probe_sched_switch, s);
	if (s->tp_sys_enter)
		tracepoint_probe_unregister(s->tp_sys_enter,
			hackbot_probe_sys_enter, s);
	if (s->tp_block_rq_complete)
		tracepoint_probe_unregister(s->tp_block_rq_complete,
			hackbot_probe_block_rq_complete, s);

	/* Ensure all CPUs have exited callbacks */
	tracepoint_synchronize_unregister();

	/* Free resources */
	kvfree(s->sched_ring);
	kvfree(s->syscall_ring);
	kvfree(s->io_ring);
	kfree(s);
	trace_state = NULL;

	pr_info("hackbot: trace: sensory layer shutdown\n");
}
