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
#include <linux/string.h>
#include <linux/atomic.h>
#include <linux/timekeeping.h>
#include <linux/slab.h>
#include <linux/log2.h>
#include <linux/percpu.h>
#include <linux/seqlock.h>
#include "hackbot_trace.h"
#include "hackbot_tokenizer.h"
#include "hackbot_ngram.h"

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

/* --- Tier 2: Feature vectors (LinnOS-style)
 *
 * R-002: Per-CPU + seqcount_t. Single writer per CPU (probe context with
 * IRQs disabled), many cross-CPU readers. seqcount_t (vanilla, not
 * seqcount_spinlock_t) is correct because mutual exclusion against nested
 * writers on the same CPU is provided by local_irq_save() — block_rq_complete
 * fires from softirq and could otherwise re-enter on top of a process-context
 * sys_enter on the same CPU.
 *
 * `seq` MUST be the first field so cross-CPU readers can access it
 * without relying on layout details. Remaining fields zero-init from BSS.
 */

struct sched_features {
	seqcount_t seq;
	u32 last_switch_intervals_us[FEATURE_WINDOW];
	u32 last_runqueue_lengths[FEATURE_WINDOW];
	u64 last_switch_ns;           /* timestamp of previous switch */
	u32 switches_in_window;       /* 1-second rolling count (per-CPU) */
	u64 window_start_ns;
};

struct syscall_features {
	seqcount_t seq;
	u32 last_syscall_ids[FEATURE_WINDOW];
	u32 last_intervals_us[FEATURE_WINDOW];
	u64 last_syscall_ns;
	u32 syscalls_in_window;       /* 1-second rolling count (per-CPU) */
	u64 window_start_ns;
};

struct io_features {
	seqcount_t seq;
	u32 last_latencies_us[FEATURE_WINDOW];   /* THE key LinnOS feature */
	u32 last_pending_ios[FEATURE_WINDOW];
	u32 current_queue_depth;
	u32 ios_in_window;            /* 1-second rolling count (per-CPU) */
	u64 window_start_ns;
};

/* Per-CPU feature vectors. Updated lock-free by the local CPU's probe
 * (IRQ-disabled, single-writer-per-CPU); cross-CPU readers use seqcount
 * retry loops. BSS-zero is a valid initial state. */
static DEFINE_PER_CPU(struct sched_features, sched_pcpu);
static DEFINE_PER_CPU(struct syscall_features, syscall_pcpu);
static DEFINE_PER_CPU(struct io_features, io_pcpu);

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

	/* Sched channel (feature vector lives in sched_pcpu) */
	struct raw_sched_event *sched_ring;
	atomic_t sched_ring_head;
	struct sched_aggregates sched_agg;

	/* Syscall channel (feature vector lives in syscall_pcpu) */
	struct raw_syscall_event *syscall_ring;
	atomic_t syscall_ring_head;
	struct syscall_aggregates syscall_agg;

	/* I/O channel (feature vector lives in io_pcpu) */
	struct raw_io_event *io_ring;
	atomic_t io_ring_head;
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
	unsigned int idx;
	unsigned long flags;
	u32 interval_us;
	struct sched_features *p;

	/* Tier 1: Raw ring buffer */
	idx = (unsigned int)atomic_inc_return(&s->sched_ring_head)
	      & (RAW_RING_SIZE - 1);
	s->sched_ring[idx].timestamp_ns = now;
	s->sched_ring[idx].cpu = raw_smp_processor_id();
	s->sched_ring[idx].prev_pid = prev->pid;
	s->sched_ring[idx].next_pid = next->pid;
	s->sched_ring[idx].prev_state = prev_state;
	/*
	 * R-028b: comm is updated by set_task_comm() under task_lock + a
	 * seqcount; reading it lock-free here can produce a torn 16-byte
	 * value during a concurrent exec/prctl on the target task. comm is
	 * for human display only, so torn reads are tolerated. Wrap in
	 * data_race() to silence KCSAN. __get_task_comm() is not exported
	 * to modules in linux-6.19; if/when it is, prefer it over this.
	 */
	data_race(memcpy(s->sched_ring[idx].prev_comm, prev->comm, 16));
	data_race(memcpy(s->sched_ring[idx].next_comm, next->comm, 16));

	/* Tier 2: Feature vector (per-CPU, seqcount-protected).
	 * IRQ-disable blocks softirq re-entry from block_rq_complete on the
	 * same CPU; preempt is already disabled by tracepoint context, so
	 * this CPU is the only writer to its own slot. */
	local_irq_save(flags);
	p = this_cpu_ptr(&sched_pcpu);
	write_seqcount_begin(&p->seq);
	if (p->last_switch_ns) {
		interval_us = (u32)((now - p->last_switch_ns) / 1000);
		shift_u32_window(p->last_switch_intervals_us,
				 FEATURE_WINDOW, interval_us);
	}
	p->last_switch_ns = now;
	p->switches_in_window++;
	if (now - p->window_start_ns > 1000000000ULL) {
		p->switches_in_window = 1;
		p->window_start_ns = now;
	}
	write_seqcount_end(&p->seq);
	local_irq_restore(flags);

	/* Tier 3: Aggregates */
	atomic64_inc(&s->sched_agg.total);
	atomic64_inc(&s->sched_agg.total_since_reset);

	/* Tier 4: Semantic tokenization */
	hackbot_tokenize_sched(prev, next, prev_state, now);
	/* Tier 5: N-gram learning */
	hackbot_ngram_process(hackbot_tokenizer_last_token());

	/* Update per-task counter for 'next' */
	{
		int i, n_raw, n;

		/*
		 * Clamp to MAX_TRACKED_TASKS to prevent out-of-bounds reads.
		 * Multiple CPUs racing atomic_inc_return() below can push
		 * n_tasks above MAX_TRACKED_TASKS; without this clamp, the
		 * loop would read past the tasks[] array boundary.
		 */
		n_raw = atomic_read(&s->sched_agg.n_tasks);
		n = (n_raw < MAX_TRACKED_TASKS) ? n_raw : MAX_TRACKED_TASKS;

		for (i = 0; i < n; i++) {
			/*
			 * F-006: read pid with acquire ordering so that if we
			 * see a non-zero pid, the matching comm[]/count writes
			 * from the publisher below are also visible. A pid of
			 * 0 means another CPU has reserved this slot via
			 * atomic_inc_return(&n_tasks) but has not yet
			 * published. Skip — we'll match it on the next
			 * sched_switch for this task.
			 */
			s32 slot_pid = smp_load_acquire(&s->sched_agg.tasks[i].pid);
			if (slot_pid == 0)
				continue;
			if (slot_pid == next->pid) {
				atomic64_inc(&s->sched_agg.tasks[i].count);
				return;
			}
		}
		/* New task — try to add */
		if (n < MAX_TRACKED_TASKS) {
			int slot = atomic_inc_return(&s->sched_agg.n_tasks) - 1;
			if (slot < MAX_TRACKED_TASKS) {
				/*
				 * F-006: publish the slot's pid LAST with
				 * release ordering. The comm and count writes
				 * happen first; readers gating on
				 * pid != 0 (with acquire above) will only see
				 * the slot once those writes are visible.
				 * comm is 16 bytes and inherently torn under a
				 * concurrent reader without this barrier.
				 */
				data_race(memcpy(s->sched_agg.tasks[slot].comm, next->comm, 16));
				atomic64_set(&s->sched_agg.tasks[slot].count, 1);
				smp_store_release(&s->sched_agg.tasks[slot].pid, next->pid);
			} else {
				/* Lost race — another CPU took the last slot.
				 * Roll back to prevent n_tasks from growing
				 * unboundedly, which would waste cycles in the
				 * read loop above (clamped but still iterated). */
				atomic_dec(&s->sched_agg.n_tasks);
			}
		}
	}
}

static void hackbot_probe_sys_enter(void *data, struct pt_regs *regs, long id)
{
	struct hackbot_trace_state *s = data;
	u64 now = ktime_get_raw_fast_ns();
	unsigned int idx;
	unsigned long flags;
	u32 interval_us;
	struct syscall_features *p;

	/* Tier 1: Raw ring */
	idx = (unsigned int)atomic_inc_return(&s->syscall_ring_head)
	      & (RAW_RING_SIZE - 1);
	s->syscall_ring[idx].timestamp_ns = now;
	s->syscall_ring[idx].cpu = raw_smp_processor_id();
	s->syscall_ring[idx].pid = current->pid;
	s->syscall_ring[idx].syscall_id = id;
	data_race(memcpy(s->syscall_ring[idx].comm, current->comm, 16));

	/* Tier 2: Features (per-CPU, seqcount-protected). */
	local_irq_save(flags);
	p = this_cpu_ptr(&syscall_pcpu);
	write_seqcount_begin(&p->seq);
	if (p->last_syscall_ns) {
		interval_us = (u32)((now - p->last_syscall_ns) / 1000);
		shift_u32_window(p->last_intervals_us,
				 FEATURE_WINDOW, interval_us);
	}
	shift_u32_window(p->last_syscall_ids, FEATURE_WINDOW, (u32)id);
	p->last_syscall_ns = now;
	p->syscalls_in_window++;
	if (now - p->window_start_ns > 1000000000ULL) {
		p->syscalls_in_window = 1;
		p->window_start_ns = now;
	}
	write_seqcount_end(&p->seq);
	local_irq_restore(flags);

	/* Tier 3: Aggregates */
	atomic64_inc(&s->syscall_agg.total);
	atomic64_inc(&s->syscall_agg.total_since_reset);
	if (id >= 0 && id < MAX_SYSCALL_ID)
		atomic64_inc(&s->syscall_agg.per_syscall[id]);

	/* Tier 4: Semantic tokenization */
	hackbot_tokenize_syscall(regs, id, now);
	/* Tier 5: N-gram learning */
	hackbot_ngram_process(hackbot_tokenizer_last_token());
}

static void hackbot_probe_block_rq_complete(void *data, struct request *rq,
	blk_status_t error, unsigned int nr_bytes)
{
	struct hackbot_trace_state *s = data;
	u64 now = ktime_get_ns();  /* Must match block layer's clock (not raw) */
	unsigned int idx;
	unsigned long flags;
	u32 latency_us = 0;
	bool latency_valid;
	int bucket;
	struct io_features *p;

	/*
	 * Compute I/O latency from request start time.
	 * rq->start_time_ns uses ktime_get_ns() (adjusted monotonic), so we
	 * must use the same clock here — NOT ktime_get_raw_fast_ns().
	 *
	 * R-019: rq->start_time_ns is only populated for accounted requests
	 * (RQF_IO_STAT|RQF_STATS|RQF_USE_SCHED). For un-accounted requests
	 * the field can be 0 or stale-from-recycled-struct. The previous
	 * `now > rq->start_time_ns` test was true for start=0 and yielded
	 * latency_us = now/1000 ≈ billions of µs, poisoning the histogram,
	 * the per-CPU latency window, and the tokenizer's duration_class.
	 * Gate latency-derived stats on a known-good start_time_ns.
	 * Aggregate I/O counters still increment — we observed an I/O
	 * completion, just don't have a reliable latency for it.
	 */
	if (rq->start_time_ns && now > rq->start_time_ns) {
		latency_us = (u32)((now - rq->start_time_ns) / 1000);
		latency_valid = true;
	} else {
		latency_valid = false;
	}

	/* Tier 1: Raw ring */
	idx = (unsigned int)atomic_inc_return(&s->io_ring_head)
	      & (RAW_RING_SIZE - 1);
	s->io_ring[idx].timestamp_ns = now;
	s->io_ring[idx].cpu = raw_smp_processor_id();
	s->io_ring[idx].sector = blk_rq_pos(rq);
	s->io_ring[idx].nr_bytes = nr_bytes;
	s->io_ring[idx].error = blk_status_to_errno(error);
	s->io_ring[idx].is_write = op_is_write(req_op(rq)) ? 1 : 0;

	/* Tier 2: Features (per-CPU, seqcount-protected).
	 * block_rq_complete fires from softirq; local_irq_save ensures we
	 * are not interrupted on this CPU mid-update by, e.g., another
	 * softirq raising the same CPU's queue. */
	local_irq_save(flags);
	p = this_cpu_ptr(&io_pcpu);
	write_seqcount_begin(&p->seq);
	if (latency_valid)
		shift_u32_window(p->last_latencies_us, FEATURE_WINDOW, latency_us);
	p->ios_in_window++;
	if (now - p->window_start_ns > 1000000000ULL) {
		p->ios_in_window = 1;
		p->window_start_ns = now;
	}
	write_seqcount_end(&p->seq);
	local_irq_restore(flags);

	/* Tier 3: Aggregates */
	atomic64_inc(&s->io_agg.total);
	atomic64_inc(&s->io_agg.total_since_reset);
	if (latency_valid) {
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

	/* Tier 4: Semantic tokenization */
	hackbot_tokenize_io(rq, blk_status_to_errno(error), nr_bytes,
			    latency_us, now);
	/* Tier 5: N-gram learning */
	hackbot_ngram_process(hackbot_tokenizer_last_token());
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
	/*
	 * F-007: for_each_kernel_tracepoint in linux-6.19.8 takes a
	 * `void (*)(struct tracepoint *, void *)` callback with no early-
	 * out (see kernel/tracepoint.c:740). We can't stop the walk, but we
	 * can skip the strcmp once we've already found our match — a tiny
	 * win that compounds across init when looking up multiple
	 * tracepoints.
	 */
	if (*lookup->result)
		return;
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
	long long total, uptime_s;
	int i, n, cpu;
	u32 total_rate = 0;

	if (!s || !s->active) {
		pos = append_str(out, pos, maxlen, "[Trace not active]\n");
		return pos;
	}

	total = atomic64_read(&s->sched_agg.total);
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

	/* Top tasks (clamp to array bounds — n_tasks can race past MAX) */
	n = atomic_read(&s->sched_agg.n_tasks);
	if (n > MAX_TRACKED_TASKS)
		n = MAX_TRACKED_TASKS;
	if (n > 0) {
		pos = append_str(out, pos, maxlen, "Top tasks:");
		/* Simple: show first 10 tasks (not sorted — good enough for now) */
		for (i = 0; i < n && i < 10; i++) {
			long long cnt;
			/*
			 * F-006: gate on smp_load_acquire(pid). A zero pid
			 * means the slot has been reserved by an in-flight
			 * publish on another CPU but the comm/count writes
			 * are not yet visible — skip rather than print a
			 * torn entry. Pairs with smp_store_release in
			 * hackbot_probe_sched_switch.
			 */
			s32 slot_pid = smp_load_acquire(&s->sched_agg.tasks[i].pid);
			if (slot_pid == 0)
				continue;
			cnt = atomic64_read(&s->sched_agg.tasks[i].count);
			if (cnt == 0) continue;
			pos = append_str(out, pos, maxlen, " ");
			pos = append_str(out, pos, maxlen, s->sched_agg.tasks[i].comm);
			pos = append_str(out, pos, maxlen, "(");
			pos = append_num(out, pos, maxlen, slot_pid);
			pos = append_str(out, pos, maxlen, ")=");
			pos = append_num(out, pos, maxlen, cnt);
			if (pos < 0) break;
		}
		pos = append_str(out, pos, maxlen, "\n");
	}

	/* Features: per-CPU rows. The 4-element interval window is per-CPU
	 * by design (cross-CPU temporal ordering is undefined, so we don't
	 * concatenate). Rate is summed across CPUs. */
	pos = append_str(out, pos, maxlen, "Features (per-CPU):\n");
	for_each_possible_cpu(cpu) {
		struct sched_features *p = per_cpu_ptr(&sched_pcpu, cpu);
		struct sched_features local;
		unsigned int seq;

		do {
			seq = read_seqcount_begin(&p->seq);
			memcpy(&local, p, sizeof(local));
		} while (read_seqcount_retry(&p->seq, seq));

		total_rate += local.switches_in_window;

		/* Suppress empty CPUs to keep output bounded. */
		if (local.last_switch_ns == 0 && local.switches_in_window == 0)
			continue;

		pos = append_str(out, pos, maxlen, "  CPU");
		pos = append_num(out, pos, maxlen, cpu);
		pos = append_str(out, pos, maxlen, ": intervals=[");
		for (i = 0; i < FEATURE_WINDOW; i++) {
			if (i > 0) pos = append_str(out, pos, maxlen, ",");
			pos = append_num(out, pos, maxlen,
					 local.last_switch_intervals_us[i]);
		}
		pos = append_str(out, pos, maxlen, "]us rate=");
		pos = append_num(out, pos, maxlen, local.switches_in_window);
		pos = append_str(out, pos, maxlen, "/s\n");
		if (pos >= maxlen - 128)
			break;
	}
	pos = append_str(out, pos, maxlen, "Total rate=");
	pos = append_num(out, pos, maxlen, total_rate);
	pos = append_str(out, pos, maxlen, "/s\n");

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_sched_raw(char *out, int maxlen, int count)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i;
	unsigned int head, start;

	if (!s || !s->active) return 0;
	if (count <= 0 || count > RAW_RING_SIZE) count = 20;

	head = (unsigned int)atomic_read(&s->sched_ring_head);
	start = (head - (unsigned int)count) & (RAW_RING_SIZE - 1);

	pos = append_str(out, pos, maxlen, "=== Raw: sched_switch (last ");
	pos = append_num(out, pos, maxlen, count);
	pos = append_str(out, pos, maxlen, ") ===\n");

	for (i = 0; i < count && pos > 0 && pos < maxlen - 128; i++) {
		unsigned int idx = (start + (unsigned int)i) & (RAW_RING_SIZE - 1);
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
	int pos = 0, i, cpu;
	long long total;
	u32 total_rate = 0;

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

	/* Features: per-CPU rows. */
	pos = append_str(out, pos, maxlen, "Features (per-CPU):\n");
	for_each_possible_cpu(cpu) {
		struct syscall_features *p = per_cpu_ptr(&syscall_pcpu, cpu);
		struct syscall_features local;
		unsigned int seq;

		do {
			seq = read_seqcount_begin(&p->seq);
			memcpy(&local, p, sizeof(local));
		} while (read_seqcount_retry(&p->seq, seq));

		total_rate += local.syscalls_in_window;

		if (local.last_syscall_ns == 0 && local.syscalls_in_window == 0)
			continue;

		pos = append_str(out, pos, maxlen, "  CPU");
		pos = append_num(out, pos, maxlen, cpu);
		pos = append_str(out, pos, maxlen, ": ids=[");
		for (i = 0; i < FEATURE_WINDOW; i++) {
			if (i > 0) pos = append_str(out, pos, maxlen, ",");
			pos = append_num(out, pos, maxlen,
					 local.last_syscall_ids[i]);
		}
		pos = append_str(out, pos, maxlen, "] intervals=[");
		for (i = 0; i < FEATURE_WINDOW; i++) {
			if (i > 0) pos = append_str(out, pos, maxlen, ",");
			pos = append_num(out, pos, maxlen,
					 local.last_intervals_us[i]);
		}
		pos = append_str(out, pos, maxlen, "]us rate=");
		pos = append_num(out, pos, maxlen, local.syscalls_in_window);
		pos = append_str(out, pos, maxlen, "/s\n");
		if (pos >= maxlen - 128)
			break;
	}
	pos = append_str(out, pos, maxlen, "Total rate=");
	pos = append_num(out, pos, maxlen, total_rate);
	pos = append_str(out, pos, maxlen, "/s\n");

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_syscall_raw(char *out, int maxlen, int count)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i;
	unsigned int head, start;

	if (!s || !s->active) return 0;
	if (count <= 0 || count > RAW_RING_SIZE) count = 20;

	head = (unsigned int)atomic_read(&s->syscall_ring_head);
	start = (head - (unsigned int)count) & (RAW_RING_SIZE - 1);

	pos = append_str(out, pos, maxlen, "=== Raw: sys_enter (last ");
	pos = append_num(out, pos, maxlen, count);
	pos = append_str(out, pos, maxlen, ") ===\n");

	for (i = 0; i < count && pos > 0 && pos < maxlen - 100; i++) {
		unsigned int idx = (start + (unsigned int)i) & (RAW_RING_SIZE - 1);
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
	int pos = 0, i, cpu;
	long long total, total_lat;
	u32 total_rate = 0;
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

	/* Features: per-CPU rows. */
	pos = append_str(out, pos, maxlen, "Features (per-CPU):\n");
	for_each_possible_cpu(cpu) {
		struct io_features *p = per_cpu_ptr(&io_pcpu, cpu);
		struct io_features local;
		unsigned int seq;

		do {
			seq = read_seqcount_begin(&p->seq);
			memcpy(&local, p, sizeof(local));
		} while (read_seqcount_retry(&p->seq, seq));

		total_rate += local.ios_in_window;

		if (local.ios_in_window == 0 && local.window_start_ns == 0)
			continue;

		pos = append_str(out, pos, maxlen, "  CPU");
		pos = append_num(out, pos, maxlen, cpu);
		pos = append_str(out, pos, maxlen, ": lats=[");
		for (i = 0; i < FEATURE_WINDOW; i++) {
			if (i > 0) pos = append_str(out, pos, maxlen, ",");
			pos = append_num(out, pos, maxlen,
					 local.last_latencies_us[i]);
		}
		pos = append_str(out, pos, maxlen, "]us rate=");
		pos = append_num(out, pos, maxlen, local.ios_in_window);
		pos = append_str(out, pos, maxlen, "/s\n");
		if (pos >= maxlen - 128)
			break;
	}
	pos = append_str(out, pos, maxlen, "Total rate=");
	pos = append_num(out, pos, maxlen, total_rate);
	pos = append_str(out, pos, maxlen, "/s\n");

	pos = append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_trace_read_io_raw(char *out, int maxlen, int count)
{
	struct hackbot_trace_state *s = trace_state;
	int pos = 0, i;
	unsigned int head, start;

	if (!s || !s->active) return 0;
	if (count <= 0 || count > RAW_RING_SIZE) count = 20;

	head = (unsigned int)atomic_read(&s->io_ring_head);
	start = (head - (unsigned int)count) & (RAW_RING_SIZE - 1);

	pos = append_str(out, pos, maxlen, "=== Raw: block_rq_complete (last ");
	pos = append_num(out, pos, maxlen, count);
	pos = append_str(out, pos, maxlen, ") ===\n");

	for (i = 0; i < count && pos > 0 && pos < maxlen - 100; i++) {
		unsigned int idx = (start + (unsigned int)i) & (RAW_RING_SIZE - 1);
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
 * Token output (delegates to hackbot_tokenizer.c)
 * =================================================================== */

int hackbot_trace_read_tokens(char *out, int maxlen, int count)
{
	return hackbot_tokenizer_read(out, maxlen, count);
}

/* ===================================================================
 * N-gram output (delegates to hackbot_ngram.c)
 * =================================================================== */

int hackbot_trace_read_ngram_surprise(char *out, int maxlen)
{
	return hackbot_ngram_read_surprise(out, maxlen);
}

int hackbot_trace_read_ngram_stats(char *out, int maxlen)
{
	return hackbot_ngram_read_stats(out, maxlen);
}

int hackbot_trace_read_ngram_alerts(char *out, int maxlen, int count)
{
	return hackbot_ngram_read_alerts(out, maxlen, count);
}

/* ===================================================================
 * Init / Exit
 * =================================================================== */

int hackbot_trace_init(void)
{
	struct hackbot_trace_state *s;
	int ret, cpu;

	/* Ring index masking below assumes RAW_RING_SIZE is a power of two. */
	BUILD_BUG_ON(!is_power_of_2(RAW_RING_SIZE));

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

	/* Initialize per-CPU feature seqcounts BEFORE any tracepoint
	 * callback can run. Other fields are BSS-zero. */
	for_each_possible_cpu(cpu) {
		seqcount_init(&per_cpu_ptr(&sched_pcpu, cpu)->seq);
		seqcount_init(&per_cpu_ptr(&syscall_pcpu, cpu)->seq);
		seqcount_init(&per_cpu_ptr(&io_pcpu, cpu)->seq);
	}

	s->start_ns = ktime_get_raw_fast_ns();
	s->reset_ns = s->start_ns;

	/* Initialize semantic tokenizer (Tier 4) */
	ret = hackbot_tokenizer_init();
	if (ret) {
		pr_warn("hackbot: trace: tokenizer init failed (%d)\n", ret);
		/* Non-fatal: tracing works without tokenization */
	}

	/* Initialize n-gram learning (Tier 5) */
	ret = hackbot_ngram_init();
	if (ret) {
		pr_warn("hackbot: trace: ngram init failed (%d)\n", ret);
		/* Non-fatal: tracing works without n-gram learning */
	}

	/* Discover tracepoints by name. A tracepoint that isn't present in
	 * this kernel's build (NULL pointer here) is non-fatal — we just
	 * register what's available. A tracepoint that IS present but whose
	 * registration call fails is fatal: we roll back so the contract
	 * is "all probes we asked for are on, or none". */
	s->tp_sched_switch = find_tracepoint("sched_switch");
	s->tp_sys_enter = find_tracepoint("sys_enter");
	s->tp_block_rq_complete = find_tracepoint("block_rq_complete");

	/* Register callbacks. R-020: failure of any registration unwinds
	 * the previously-registered probes. */
	if (s->tp_sched_switch) {
		ret = tracepoint_probe_register(s->tp_sched_switch,
			hackbot_probe_sched_switch, s);
		if (ret) {
			pr_warn("hackbot: trace: sched_switch register failed (%d)\n", ret);
			goto err_sched;
		}
		pr_info("hackbot: trace: sched_switch registered\n");
	} else {
		pr_warn("hackbot: trace: sched_switch tracepoint not found\n");
	}

	if (s->tp_sys_enter) {
		ret = tracepoint_probe_register(s->tp_sys_enter,
			hackbot_probe_sys_enter, s);
		if (ret) {
			pr_warn("hackbot: trace: sys_enter register failed (%d)\n", ret);
			goto err_syscall;
		}
		pr_info("hackbot: trace: sys_enter registered\n");
	} else {
		pr_warn("hackbot: trace: sys_enter tracepoint not found\n");
	}

	if (s->tp_block_rq_complete) {
		ret = tracepoint_probe_register(s->tp_block_rq_complete,
			hackbot_probe_block_rq_complete, s);
		if (ret) {
			pr_warn("hackbot: trace: block_rq_complete register failed (%d)\n", ret);
			goto err_io;
		}
		pr_info("hackbot: trace: block_rq_complete registered\n");
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

	/* R-020 rollback chain: unregister whatever we registered, drain
	 * in-flight callbacks via tracepoint_synchronize_unregister(),
	 * tear down the tokenizer/ngram subsystems we already brought up
	 * above, then fall through to fail: for buffer cleanup. */
err_io:
	if (s->tp_sys_enter)
		tracepoint_probe_unregister(s->tp_sys_enter,
			hackbot_probe_sys_enter, s);
err_syscall:
	if (s->tp_sched_switch)
		tracepoint_probe_unregister(s->tp_sched_switch,
			hackbot_probe_sched_switch, s);
err_sched:
	tracepoint_synchronize_unregister();
	hackbot_ngram_exit();
	hackbot_tokenizer_exit();

fail:
	if (s->sched_ring)   kvfree(s->sched_ring);
	if (s->syscall_ring) kvfree(s->syscall_ring);
	if (s->io_ring)      kvfree(s->io_ring);
	kfree(s);
	return ret;
}

/*
 * Reader/writer lifecycle invariants for trace_state (`s`):
 *
 *   1. Probe-context readers (hackbot_probe_*): drained by
 *      tracepoint_synchronize_unregister() below, which waits for all
 *      in-flight callbacks on every CPU to return before we proceed
 *      to free `s`.
 *
 *   2. Misc-device-context readers (hackbot_trace_read_* called from
 *      <tool>trace ...</tool> via the agent's write_iter path): gated
 *      by the misc device's file refcount. An in-flight write_iter
 *      syscall holds an fdget ref on the file, which holds a module
 *      ref via fops->owner = THIS_MODULE; rmmod cannot run while that
 *      ref is held, so this exit function cannot run concurrently
 *      with a reader. No internal synchronize is required for path 2.
 *
 * Both invariants must hold before the kvfree/kfree calls at the end.
 */
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

	/* Shutdown n-gram learning (after tracepoints are unregistered) */
	hackbot_ngram_exit();

	/* Shutdown tokenizer */
	hackbot_tokenizer_exit();

	/* Free resources */
	kvfree(s->sched_ring);
	kvfree(s->syscall_ring);
	kvfree(s->io_ring);
	kfree(s);
	trace_state = NULL;

	pr_info("hackbot: trace: sensory layer shutdown\n");
}
