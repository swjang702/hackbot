// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_ngram.c — In-kernel factorial bigram learning + anomaly detection.
 *
 * Learns P(field_t | field_{t-1}) for each of the 8 semantic token fields.
 * Computes surprise = -log2(P) ≈ ilog2(row_total) - ilog2(count).
 *
 * Milestone 1: Dual-model learning (baseline + adaptive with halving).
 * Milestone 2: Gated learning, classification, alert generation, grace period.
 *
 * Concurrency: Hogwild-style — no locks. READ_ONCE/WRITE_ONCE on shared
 * counts. wake_up() is safe from atomic context.
 *
 * Cost: ~300-600ns per event (pure integer arithmetic, no FPU).
 * Memory: ~70KB (two 33KB models + 4KB alert ring).
 */

#include <linux/kernel.h>
#include <linux/percpu.h>
#include <linux/slab.h>
#include <linux/atomic.h>
#include <linux/log2.h>
#include <linux/string.h>
#include <linux/timekeeping.h>
#include <linux/sched.h>
#include <linux/wait.h>
#include "hackbot_ngram.h"

/* ===================================================================
 * Global state
 * =================================================================== */

static struct ngram_state *ngram;

static DEFINE_PER_CPU(struct ngram_cpu_state, ngram_percpu);

static atomic_t ngram_debug_count = ATOMIC_INIT(0);

/* Classification names for output */
static const char * const class_names[] = {
	"NORMAL", "ANOMALY", "DRIFT", "REGRESSION", "UNCERTAIN"
};

/* Field labels for output */
static const char * const field_labels[TOK_NR_FIELDS] = {
	"cat", "act", "obj", "tgt", "size", "ret", "dur", "gap"
};

/* ===================================================================
 * Surprise computation
 * =================================================================== */

static inline u32 compute_field_surprise(const struct ngram_field_table *ft,
					 u8 prev_val, u8 curr_val)
{
	u32 rt, ct;

	rt = READ_ONCE(ft->row_total[prev_val]);
	ct = READ_ONCE(ft->count[prev_val][curr_val]);

	if (ct == 0 || rt == 0)
		return NGRAM_MAX_SURPRISE;
	if (ct >= rt)
		return 0;
	return ilog2(rt) - ilog2(ct);
}

static u32 compute_total_surprise(const struct ngram_model *m,
				  const struct tokenized_event *prev,
				  const struct tokenized_event *curr,
				  u8 *per_field_out)
{
	u32 total = 0;
	int f;

	for (f = 0; f < TOK_NR_FIELDS; f++) {
		u32 s = compute_field_surprise(&m->fields[f],
					       prev->fields[f],
					       curr->fields[f]);
		if (per_field_out)
			per_field_out[f] = (s > 255) ? 255 : (u8)s;
		total += s;
	}
	return total;
}

/* ===================================================================
 * Classification
 * =================================================================== */

static inline u8 classify_surprise(u32 base_surp, u32 adapt_surp)
{
	bool base_high = base_surp > NGRAM_SURPRISE_HIGH;
	bool base_low = base_surp <= NGRAM_SURPRISE_LOW;
	bool adapt_high = adapt_surp > NGRAM_SURPRISE_HIGH;
	bool adapt_low = adapt_surp <= NGRAM_SURPRISE_LOW;

	if (base_low && adapt_low)
		return NGRAM_CLASS_NORMAL;
	if (base_high && adapt_high)
		return NGRAM_CLASS_ANOMALY;
	if (base_high && adapt_low)
		return NGRAM_CLASS_DRIFT;
	if (base_low && adapt_high)
		return NGRAM_CLASS_REGRESSION;
	return NGRAM_CLASS_UNCERTAIN;
}

/* ===================================================================
 * Model update — Hogwild increment
 * =================================================================== */

static inline void update_model(struct ngram_model *m,
				const struct tokenized_event *prev,
				const struct tokenized_event *curr)
{
	int f;
	u32 ct, rt;

	for (f = 0; f < TOK_NR_FIELDS; f++) {
		u8 p = prev->fields[f];
		u8 c = curr->fields[f];
		struct ngram_field_table *ft = &m->fields[f];

		ct = READ_ONCE(ft->count[p][c]);
		if (likely(ct < 0x7FFFFFFFU))
			WRITE_ONCE(ft->count[p][c], ct + 1);

		rt = READ_ONCE(ft->row_total[p]);
		if (likely(rt < 0x7FFFFFFFU))
			WRITE_ONCE(ft->row_total[p], rt + 1);
	}

	m->total_events++;

	if (m->halve_interval > 0 &&
	    m->total_events % m->halve_interval == 0) {
		int i, j;

		for (f = 0; f < TOK_NR_FIELDS; f++) {
			struct ngram_field_table *ft = &m->fields[f];
			for (i = 0; i < NGRAM_DIM; i++) {
				for (j = 0; j < NGRAM_DIM; j++)
					ft->count[i][j] >>= 1;
				ft->row_total[i] >>= 1;
			}
		}
		m->halve_count++;
	}
}

/* ===================================================================
 * Alert generation
 * =================================================================== */

static void generate_alert(struct ngram_state *st,
			   const struct tokenized_event *tok,
			   u32 base_surp, u32 adapt_surp,
			   u8 classification, const u8 *field_surprise,
			   u64 now_ns)
{
	int idx;
	struct ngram_alert *a;

	/* Debounce: skip if too soon after last alert */
	if (now_ns - READ_ONCE(st->last_alert_ns) < NGRAM_ALERT_COOLDOWN_NS) {
		atomic64_inc(&st->suppressed_count);
		return;
	}
	WRITE_ONCE(st->last_alert_ns, now_ns);

	/* Write alert to ring buffer */
	idx = atomic_inc_return(&st->alert_ring_head) & NGRAM_ALERT_RING_MASK;
	a = &st->alert_ring[idx];

	a->timestamp_ns = now_ns;
	a->cpu = raw_smp_processor_id();
	a->pid = current->pid;
	memcpy(a->comm, current->comm, 16);
	a->baseline_surprise = base_surp;
	a->adaptive_surprise = adapt_surp;
	a->classification = classification;
	memcpy(a->field_surprise, field_surprise, TOK_NR_FIELDS);
	a->token = *tok;

	atomic64_inc(&st->alert_count);
	atomic_inc(&st->alert_pending);

	/* Wake patrol thread */
	wake_up(&st->alert_wq);
}

/* ===================================================================
 * Main entry point — called from tracepoint callbacks
 * =================================================================== */

void hackbot_ngram_process(const struct tokenized_event *tok)
{
	struct ngram_cpu_state *cpu = this_cpu_ptr(&ngram_percpu);
	struct ngram_state *st = ngram;
	u32 base_surp, adapt_surp, combined;
	u8 field_surp[TOK_NR_FIELDS];
	u8 classification;
	long long count;
	u64 now_ns, elapsed_ns;
	int dbg;

	if (unlikely(!st || !st->active))
		return;

	/* First event on this CPU — just save and return */
	if (unlikely(!cpu->has_prev)) {
		cpu->prev_token = *tok;
		cpu->has_prev = true;
		return;
	}

	now_ns = ktime_get_raw_fast_ns();

	/* 1. Compute surprise from BOTH models BEFORE updating */
	base_surp = compute_total_surprise(st->baseline, &cpu->prev_token,
					   tok, NULL);
	adapt_surp = compute_total_surprise(st->adaptive, &cpu->prev_token,
					    tok, field_surp);

	combined = (base_surp + adapt_surp) / 2;

	/* 2. Classify */
	classification = classify_surprise(base_surp, adapt_surp);

	/* 3. Store latest scores (Hogwild) */
	WRITE_ONCE(st->last_baseline_surprise, base_surp);
	WRITE_ONCE(st->last_adaptive_surprise, adapt_surp);
	WRITE_ONCE(st->last_combined_surprise, combined);
	memcpy(st->last_field_surprise, field_surp, TOK_NR_FIELDS);

	/* 4. Update models — GATED LEARNING for adaptive */
	update_model(st->baseline, &cpu->prev_token, tok);

	if (adapt_surp < NGRAM_LEARN_THRESHOLD) {
		/* Normal: learn from this event */
		update_model(st->adaptive, &cpu->prev_token, tok);
	} else {
		/* Anomalous: skip adaptive learning to prevent normalization */
		atomic64_inc(&st->gated_count);
	}

	/* 5. Alert generation (skip during grace period) */
	elapsed_ns = now_ns - st->init_ns;
	if (adapt_surp >= NGRAM_ALERT_THRESHOLD &&
	    elapsed_ns > (u64)NGRAM_GRACE_PERIOD_S * 1000000000ULL) {
		generate_alert(st, tok, base_surp, adapt_surp,
			       classification, field_surp, now_ns);
	}

	/* 6. Update counters and save prev */
	count = atomic64_inc_return(&st->event_count);
	cpu->prev_token = *tok;

	/* 7. Debug prints */
	dbg = atomic_inc_return(&ngram_debug_count);
	if (dbg <= NGRAM_DEBUG_PRINTS) {
		pr_info("hackbot: ngram[%lld]: %s surprise base=%u adapt=%u "
			"fields=[%u,%u,%u,%u,%u,%u,%u,%u]\n",
			count - 1,
			class_names[classification],
			base_surp, adapt_surp,
			field_surp[0], field_surp[1],
			field_surp[2], field_surp[3],
			field_surp[4], field_surp[5],
			field_surp[6], field_surp[7]);
	}
}

/* ===================================================================
 * Patrol thread integration
 * =================================================================== */

long hackbot_ngram_wait_or_timeout(long timeout_jiffies)
{
	struct ngram_state *st = ngram;

	if (!st || !st->active)
		return schedule_timeout_interruptible(timeout_jiffies);

	return wait_event_interruptible_timeout(st->alert_wq,
		atomic_read(&st->alert_pending) > 0,
		timeout_jiffies);
}

int hackbot_ngram_has_pending_alerts(void)
{
	struct ngram_state *st = ngram;
	if (!st)
		return 0;
	return atomic_read(&st->alert_pending) > 0;
}

void hackbot_ngram_clear_pending(void)
{
	struct ngram_state *st = ngram;
	if (st)
		atomic_set(&st->alert_pending, 0);
}

/* ===================================================================
 * Output formatting helpers
 * =================================================================== */

static int ng_append_str(char *out, int pos, int maxlen, const char *s)
{
	while (*s && pos < maxlen)
		out[pos++] = *s++;
	return pos;
}

static int ng_append_num(char *out, int pos, int maxlen, long long val)
{
	char tmp[24];
	int len = 0, i;

	if (pos >= maxlen)
		return pos;
	if (val < 0) {
		if (pos < maxlen)
			out[pos++] = '-';
		val = -val;
	}
	if (val == 0) {
		tmp[len++] = '0';
	} else {
		while (val > 0 && len < 20) {
			tmp[len++] = '0' + (char)(val % 10);
			val /= 10;
		}
	}
	for (i = len - 1; i >= 0; i--) {
		if (pos >= maxlen)
			return pos;
		out[pos++] = tmp[i];
	}
	return pos;
}

/* ===================================================================
 * Read functions — format state for agent tools
 * =================================================================== */

int hackbot_ngram_read_surprise(char *out, int maxlen)
{
	struct ngram_state *st = ngram;
	int pos = 0;
	long long events;
	u64 uptime_s, elapsed_ns;
	int f;
	u32 bs, as;

	if (!st || !st->active) {
		pos = ng_append_str(out, pos, maxlen,
				    "[N-gram not active]\n");
		return pos;
	}

	events = atomic64_read(&st->event_count);
	elapsed_ns = ktime_get_raw_fast_ns() - st->init_ns;
	uptime_s = elapsed_ns / 1000000000ULL;

	pos = ng_append_str(out, pos, maxlen,
			    "=== N-gram Surprise (");
	pos = ng_append_num(out, pos, maxlen, events);
	pos = ng_append_str(out, pos, maxlen, " events, ");
	pos = ng_append_num(out, pos, maxlen, (long long)uptime_s);
	pos = ng_append_str(out, pos, maxlen, "s) ===\n");

	bs = READ_ONCE(st->last_baseline_surprise);
	as = READ_ONCE(st->last_adaptive_surprise);

	pos = ng_append_str(out, pos, maxlen, "Surprise: baseline=");
	pos = ng_append_num(out, pos, maxlen, bs);
	pos = ng_append_str(out, pos, maxlen, " adaptive=");
	pos = ng_append_num(out, pos, maxlen, as);
	pos = ng_append_str(out, pos, maxlen, " combined=");
	pos = ng_append_num(out, pos, maxlen,
			    READ_ONCE(st->last_combined_surprise));
	pos = ng_append_str(out, pos, maxlen, "\n");

	/* Per-field surprise */
	pos = ng_append_str(out, pos, maxlen, "Per-field (adaptive):");
	for (f = 0; f < TOK_NR_FIELDS; f++) {
		pos = ng_append_str(out, pos, maxlen, " ");
		pos = ng_append_str(out, pos, maxlen, field_labels[f]);
		pos = ng_append_str(out, pos, maxlen, "=");
		pos = ng_append_num(out, pos, maxlen,
				    READ_ONCE(st->last_field_surprise[f]));
	}
	pos = ng_append_str(out, pos, maxlen, "\n");

	/* Classification */
	pos = ng_append_str(out, pos, maxlen, "Status: ");
	pos = ng_append_str(out, pos, maxlen,
			    class_names[classify_surprise(bs, as)]);
	pos = ng_append_str(out, pos, maxlen, "\n");

	/* Alert stats */
	pos = ng_append_str(out, pos, maxlen, "Alerts: ");
	pos = ng_append_num(out, pos, maxlen,
			    atomic64_read(&st->alert_count));
	pos = ng_append_str(out, pos, maxlen, " generated, ");
	pos = ng_append_num(out, pos, maxlen,
			    atomic64_read(&st->suppressed_count));
	pos = ng_append_str(out, pos, maxlen, " suppressed, ");
	pos = ng_append_num(out, pos, maxlen,
			    atomic64_read(&st->gated_count));
	pos = ng_append_str(out, pos, maxlen, " gated\n");

	/* Grace period status */
	if (elapsed_ns < (u64)NGRAM_GRACE_PERIOD_S * 1000000000ULL) {
		u64 remaining_s = NGRAM_GRACE_PERIOD_S -
				  (elapsed_ns / 1000000000ULL);
		pos = ng_append_str(out, pos, maxlen,
				    "Grace period: ");
		pos = ng_append_num(out, pos, maxlen, (long long)remaining_s);
		pos = ng_append_str(out, pos, maxlen,
				    "s remaining (no alerts)\n");
	}

	pos = ng_append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_ngram_read_stats(char *out, int maxlen)
{
	struct ngram_state *st = ngram;
	int pos = 0;
	long long events;
	u64 uptime_s;
	int f, i, j;
	u32 max_count, max_total;

	if (!st || !st->active) {
		pos = ng_append_str(out, pos, maxlen,
				    "[N-gram not active]\n");
		return pos;
	}

	events = atomic64_read(&st->event_count);
	uptime_s = (ktime_get_raw_fast_ns() - st->init_ns) / 1000000000ULL;

	pos = ng_append_str(out, pos, maxlen, "=== N-gram Stats (");
	pos = ng_append_num(out, pos, maxlen, events);
	pos = ng_append_str(out, pos, maxlen, " events, ");
	pos = ng_append_num(out, pos, maxlen, (long long)uptime_s);
	pos = ng_append_str(out, pos, maxlen, "s) ===\n");

	pos = ng_append_str(out, pos, maxlen, "Adaptive: events=");
	pos = ng_append_num(out, pos, maxlen,
			    (long long)st->adaptive->total_events);
	pos = ng_append_str(out, pos, maxlen, " halves=");
	pos = ng_append_num(out, pos, maxlen, st->adaptive->halve_count);
	pos = ng_append_str(out, pos, maxlen, "\n");

	pos = ng_append_str(out, pos, maxlen, "Baseline: events=");
	pos = ng_append_num(out, pos, maxlen,
			    (long long)st->baseline->total_events);
	pos = ng_append_str(out, pos, maxlen, " halves=");
	pos = ng_append_num(out, pos, maxlen, st->baseline->halve_count);
	pos = ng_append_str(out, pos, maxlen, "\n");

	if (uptime_s > 0) {
		pos = ng_append_str(out, pos, maxlen, "Rate: ");
		pos = ng_append_num(out, pos, maxlen,
				    events / (long long)uptime_s);
		pos = ng_append_str(out, pos, maxlen, " events/s\n");
	}

	pos = ng_append_str(out, pos, maxlen, "Gated: ");
	pos = ng_append_num(out, pos, maxlen,
			    atomic64_read(&st->gated_count));
	pos = ng_append_str(out, pos, maxlen, " events skipped\n");

	/* Per-field coverage for adaptive model */
	pos = ng_append_str(out, pos, maxlen, "Adaptive field coverage:\n");
	for (f = 0; f < TOK_NR_FIELDS; f++) {
		const struct ngram_field_table *ft = &st->adaptive->fields[f];
		u32 nonzero = 0;

		max_count = 0;
		max_total = 0;
		for (i = 0; i < NGRAM_DIM; i++) {
			if (ft->row_total[i] > max_total)
				max_total = ft->row_total[i];
			for (j = 0; j < NGRAM_DIM; j++) {
				if (ft->count[i][j] > 0)
					nonzero++;
				if (ft->count[i][j] > max_count)
					max_count = ft->count[i][j];
			}
		}
		pos = ng_append_str(out, pos, maxlen, "  ");
		pos = ng_append_str(out, pos, maxlen, field_labels[f]);
		pos = ng_append_str(out, pos, maxlen, ": nonzero=");
		pos = ng_append_num(out, pos, maxlen, nonzero);
		pos = ng_append_str(out, pos, maxlen, " max_count=");
		pos = ng_append_num(out, pos, maxlen, max_count);
		pos = ng_append_str(out, pos, maxlen, " max_row=");
		pos = ng_append_num(out, pos, maxlen, max_total);
		pos = ng_append_str(out, pos, maxlen, "\n");
		if (pos >= maxlen - 128)
			break;
	}

	pos = ng_append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

int hackbot_ngram_read_alerts(char *out, int maxlen, int count)
{
	struct ngram_state *st = ngram;
	int pos = 0, i;
	int head, start;
	long long total;

	if (!st || !st->active) {
		pos = ng_append_str(out, pos, maxlen,
				    "[N-gram not active]\n");
		return pos;
	}

	if (count <= 0 || count > NGRAM_ALERT_RING_SIZE)
		count = 10;

	head = atomic_read(&st->alert_ring_head);
	total = atomic64_read(&st->alert_count);

	pos = ng_append_str(out, pos, maxlen, "=== Alerts (last ");
	pos = ng_append_num(out, pos, maxlen, count);
	pos = ng_append_str(out, pos, maxlen, " of ");
	pos = ng_append_num(out, pos, maxlen, total);
	pos = ng_append_str(out, pos, maxlen, " total) ===\n");

	if (total == 0) {
		pos = ng_append_str(out, pos, maxlen, "[No alerts]\n");
		pos = ng_append_str(out, pos, maxlen, "===\n");
		/* Clear pending since there's nothing */
		hackbot_ngram_clear_pending();
		return (pos > 0) ? pos : 0;
	}

	start = (head - count + 1 + NGRAM_ALERT_RING_SIZE)
		& NGRAM_ALERT_RING_MASK;

	for (i = 0; i < count && pos < maxlen - 200; i++) {
		int idx = (start + i) & NGRAM_ALERT_RING_MASK;
		struct ngram_alert *a = &st->alert_ring[idx];

		if (a->timestamp_ns == 0)
			continue;

		/* Format: [ANOMALY] CPU3 bash(1234) base=42 adapt=38 fields=[...] */
		pos = ng_append_str(out, pos, maxlen, "[");
		if (a->classification < 5)
			pos = ng_append_str(out, pos, maxlen,
					    class_names[a->classification]);
		else
			pos = ng_append_str(out, pos, maxlen, "?");
		pos = ng_append_str(out, pos, maxlen, "] CPU");
		pos = ng_append_num(out, pos, maxlen, a->cpu);
		pos = ng_append_str(out, pos, maxlen, " ");
		pos = ng_append_str(out, pos, maxlen, a->comm);
		pos = ng_append_str(out, pos, maxlen, "(");
		pos = ng_append_num(out, pos, maxlen, a->pid);
		pos = ng_append_str(out, pos, maxlen, ") base=");
		pos = ng_append_num(out, pos, maxlen, a->baseline_surprise);
		pos = ng_append_str(out, pos, maxlen, " adapt=");
		pos = ng_append_num(out, pos, maxlen, a->adaptive_surprise);
		pos = ng_append_str(out, pos, maxlen, " fields=[");
		{
			int f;
			for (f = 0; f < TOK_NR_FIELDS; f++) {
				if (f > 0)
					pos = ng_append_str(out, pos,
							    maxlen, ",");
				pos = ng_append_num(out, pos, maxlen,
						    a->field_surprise[f]);
			}
		}
		pos = ng_append_str(out, pos, maxlen, "]\n");
	}

	/* Mark alerts as read */
	hackbot_ngram_clear_pending();

	pos = ng_append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

/* ===================================================================
 * Init / Exit
 * =================================================================== */

int hackbot_ngram_init(void)
{
	struct ngram_state *st;
	int cpu;

	st = kzalloc(sizeof(*st), GFP_KERNEL);
	if (!st)
		return -ENOMEM;

	st->baseline = kvzalloc(sizeof(struct ngram_model), GFP_KERNEL);
	st->adaptive = kvzalloc(sizeof(struct ngram_model), GFP_KERNEL);
	st->alert_ring = kvzalloc(sizeof(struct ngram_alert) *
				  NGRAM_ALERT_RING_SIZE, GFP_KERNEL);

	if (!st->baseline || !st->adaptive || !st->alert_ring) {
		pr_err("hackbot: ngram: failed to allocate models\n");
		kvfree(st->baseline);
		kvfree(st->adaptive);
		kvfree(st->alert_ring);
		kfree(st);
		return -ENOMEM;
	}

	st->baseline->halve_interval = NGRAM_BASELINE_HALVE_INTERVAL;
	st->adaptive->halve_interval = NGRAM_ADAPTIVE_HALVE_INTERVAL;
	st->init_ns = ktime_get_raw_fast_ns();

	/* Initialize alert system */
	init_waitqueue_head(&st->alert_wq);
	atomic_set(&st->alert_ring_head, -1);
	atomic64_set(&st->alert_count, 0);
	atomic64_set(&st->suppressed_count, 0);
	atomic64_set(&st->gated_count, 0);
	atomic_set(&st->alert_pending, 0);
	st->last_alert_ns = st->init_ns;

	for_each_possible_cpu(cpu) {
		struct ngram_cpu_state *s =
			per_cpu_ptr(&ngram_percpu, cpu);
		memset(s, 0, sizeof(*s));
	}

	atomic64_set(&st->event_count, 0);
	atomic_set(&ngram_debug_count, 0);

	st->active = true;
	ngram = st;

	pr_info("hackbot: ngram: initialized "
		"(2 models, %d fields, %dx%d, ~%zuKB, "
		"alert_threshold=%d, grace=%ds)\n",
		TOK_NR_FIELDS, NGRAM_DIM, NGRAM_DIM,
		(sizeof(struct ngram_model) * 2 +
		 sizeof(struct ngram_alert) * NGRAM_ALERT_RING_SIZE) / 1024,
		NGRAM_ALERT_THRESHOLD, NGRAM_GRACE_PERIOD_S);

	return 0;
}

void hackbot_ngram_exit(void)
{
	struct ngram_state *st = ngram;

	if (!st)
		return;

	st->active = false;

	/*
	 * Publish NULL before freeing memory. Any concurrent reader
	 * (e.g., a stale function pointer or late patrol-thread call)
	 * will see ngram == NULL and bail out before touching freed
	 * memory. The smp_wmb() ensures the NULL is globally visible
	 * before we free the backing allocations.
	 */
	WRITE_ONCE(ngram, NULL);
	smp_wmb();

	/* Wake anyone waiting on alerts so they can exit */
	wake_up_all(&st->alert_wq);

	pr_info("hackbot: ngram: shutdown (%lld events, "
		"%lld alerts, %lld suppressed, %lld gated, "
		"adaptive halves=%u, "
		"last surprise: base=%u adapt=%u)\n",
		atomic64_read(&st->event_count),
		atomic64_read(&st->alert_count),
		atomic64_read(&st->suppressed_count),
		atomic64_read(&st->gated_count),
		st->adaptive->halve_count,
		READ_ONCE(st->last_baseline_surprise),
		READ_ONCE(st->last_adaptive_surprise));

	kvfree(st->alert_ring);
	kvfree(st->baseline);
	kvfree(st->adaptive);
	kfree(st);
}
