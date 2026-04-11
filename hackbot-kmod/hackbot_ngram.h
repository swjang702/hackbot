/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_NGRAM_H
#define HACKBOT_NGRAM_H

/*
 * hackbot_ngram.h — In-kernel n-gram learning + anomaly detection.
 *
 * Maintains factorial bigram tables: one per semantic field.
 * Each table tracks P(field_value_t | field_value_{t-1}) via raw counts.
 * Surprise = -log2(P) ≈ ilog2(row_total) - ilog2(count).
 *
 * Two models:
 *   - Baseline: accumulates long-term (frozen reference)
 *   - Adaptive: counts halved periodically (tracks current behavior)
 *
 * Anomaly detection (Milestone 2):
 *   - Gated learning: don't learn from anomalous events
 *   - Classification: NORMAL / ANOMALY / DRIFT / REGRESSION
 *   - Alert ring buffer with waitqueue (wakes patrol thread)
 *   - Grace period: suppress alerts for first 30s after init
 *
 * Designed for tracepoint callback context:
 *   - No allocations, no sleeping, no FPU
 *   - Hogwild concurrency (no locks, statistical tolerance for races)
 *   - ~200-500ns per event
 */

#include <linux/types.h>
#include <linux/wait.h>
#include <linux/atomic.h>
#include "hackbot_tokenizer.h"

/* ===================================================================
 * Configuration
 * =================================================================== */

/* Bigram table dimensions (uniform for all fields) */
#define NGRAM_DIM              32

/* Maximum surprise per field in bits (for unseen bigrams) */
#define NGRAM_MAX_SURPRISE     20

/* Adaptive model halves all counts every N events (~3.3s at 3000/s) */
#define NGRAM_ADAPTIVE_HALVE_INTERVAL  10000

/* Baseline model halves every N events to prevent u32 overflow (~55min) */
#define NGRAM_BASELINE_HALVE_INTERVAL  10000000

/* Number of debug surprise prints to dmesg on startup */
#define NGRAM_DEBUG_PRINTS     20

/* --- Anomaly detection thresholds --- */

/* Surprise below this = "expected" (used for classification) */
#define NGRAM_SURPRISE_LOW     12

/* Surprise above this = "surprising" (used for classification) */
#define NGRAM_SURPRISE_HIGH    24

/* Don't update adaptive model when surprise exceeds this.
 * Prevents the model from learning anomalous patterns as "normal". */
#define NGRAM_LEARN_THRESHOLD  16

/* Generate an alert when adaptive surprise exceeds this. */
#define NGRAM_ALERT_THRESHOLD  30

/* Suppress alerts for this many seconds after init (learning period). */
#define NGRAM_GRACE_PERIOD_S   30

/* Minimum nanoseconds between consecutive alerts (debounce).
 * 100ms = prevents alert floods during sustained anomalies. */
#define NGRAM_ALERT_COOLDOWN_NS  (100ULL * 1000000ULL)

/* ===================================================================
 * Classification
 * =================================================================== */

enum ngram_classification {
	NGRAM_CLASS_NORMAL     = 0,   /* LOW base + LOW adaptive */
	NGRAM_CLASS_ANOMALY    = 1,   /* HIGH base + HIGH adaptive */
	NGRAM_CLASS_DRIFT      = 2,   /* HIGH base + LOW adaptive */
	NGRAM_CLASS_REGRESSION = 3,   /* LOW base + HIGH adaptive */
	NGRAM_CLASS_UNCERTAIN  = 4,   /* borderline values */
};

/* ===================================================================
 * Alert system
 * =================================================================== */

#define NGRAM_ALERT_RING_SIZE  64
#define NGRAM_ALERT_RING_MASK  (NGRAM_ALERT_RING_SIZE - 1)

struct ngram_alert {
	u64 timestamp_ns;
	u32 cpu;
	s32 pid;
	char comm[16];
	u32 baseline_surprise;
	u32 adaptive_surprise;
	u8  classification;            /* enum ngram_classification */
	u8  field_surprise[TOK_NR_FIELDS];
	struct tokenized_event token;  /* the event that triggered the alert */
};

/* ===================================================================
 * Data structures
 * =================================================================== */

struct ngram_field_table {
	u32 count[NGRAM_DIM][NGRAM_DIM];
	u32 row_total[NGRAM_DIM];
};

struct ngram_model {
	struct ngram_field_table fields[TOK_NR_FIELDS];
	u32 halve_interval;
	u32 halve_count;
	u64 total_events;
};

struct ngram_state {
	struct ngram_model *baseline;
	struct ngram_model *adaptive;

	/* Latest surprise scores (written by callback, read by stats) */
	u32 last_baseline_surprise;
	u32 last_adaptive_surprise;
	u32 last_combined_surprise;
	u8 last_field_surprise[TOK_NR_FIELDS];

	/* Global counters */
	atomic64_t event_count;
	u64 init_ns;
	bool active;

	/* Alert system (Milestone 2) */
	struct ngram_alert *alert_ring;    /* kvmalloc'd */
	atomic_t alert_ring_head;
	atomic64_t alert_count;            /* total alerts generated */
	atomic64_t suppressed_count;       /* alerts suppressed (grace/cooldown) */
	atomic64_t gated_count;            /* events where learning was skipped */
	wait_queue_head_t alert_wq;        /* wakes patrol on alert */
	atomic_t alert_pending;            /* >0 means unread alerts */
	u64 last_alert_ns;                 /* debounce: last alert timestamp */
};

struct ngram_cpu_state {
	struct tokenized_event prev_token;
	bool has_prev;
};

/* ===================================================================
 * Public API
 * =================================================================== */

int  hackbot_ngram_init(void);
void hackbot_ngram_exit(void);

/* Process a tokenized event: compute surprise, classify, learn, alert.
 * Called from tracepoint callbacks with preemption disabled. */
void hackbot_ngram_process(const struct tokenized_event *tok);

/* Read surprise scores formatted as text. Returns bytes written. */
int hackbot_ngram_read_surprise(char *out, int maxlen);

/* Read model statistics formatted as text. Returns bytes written. */
int hackbot_ngram_read_stats(char *out, int maxlen);

/* Read recent alerts formatted as text. Returns bytes written. */
int hackbot_ngram_read_alerts(char *out, int maxlen, int count);

/* Sleep for up to timeout_jiffies, but wake early on alert.
 * Returns remaining jiffies (0 = timeout, >0 = woken by alert).
 * Used by patrol thread. */
long hackbot_ngram_wait_or_timeout(long timeout_jiffies);

/* Check if there are pending unread alerts. */
int hackbot_ngram_has_pending_alerts(void);

/* Clear the pending alert flag (called after patrol reads alerts). */
void hackbot_ngram_clear_pending(void);

#endif /* HACKBOT_NGRAM_H */
