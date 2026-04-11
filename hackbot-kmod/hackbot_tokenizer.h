/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_TOKENIZER_H
#define HACKBOT_TOKENIZER_H

/*
 * hackbot_tokenizer.h — Semantic event tokenizer for kernel-as-language.
 *
 * Converts raw kernel tracepoint events into structured 8-field tokens
 * suitable for n-gram and transformer-based anomaly detection.
 *
 * Each field is a u8 index into a small vocabulary. The 8 fields are:
 *   [category, action, object_type, target_class,
 *    size_class, result_class, duration_class, gap_class]
 *
 * Design: ~104 total sub-tokens across all fields.
 * Cost: ~200-400ns per tokenization (pure integer arithmetic).
 */

#include <linux/types.h>

struct pt_regs;
struct task_struct;
struct request;

/* ===================================================================
 * Field 0: Category — which subsystem generated this event
 * =================================================================== */

enum tok_category {
	CAT_SCHED    = 0,
	CAT_SYSCALL  = 1,
	CAT_BLOCK    = 2,
	CAT_NET      = 3,  /* future */
	CAT_MEM      = 4,  /* future */
	CAT_FS       = 5,  /* future */
	CAT_IRQ      = 6,  /* future */
	CAT_SIGNAL   = 7,  /* future */
	NR_CATEGORIES = 8,
};

/* ===================================================================
 * Field 1: Action — what operation was performed
 * =================================================================== */

enum tok_action {
	/* Sched actions */
	ACT_SWITCH_IN   = 0,
	ACT_SWITCH_OUT  = 1,

	/* Block actions */
	ACT_BLK_READ    = 2,
	ACT_BLK_WRITE   = 3,

	/* Syscall action groups (4-30) */
	ACT_READ        = 4,   /* read, pread64, readv, preadv, recvfrom, recvmsg */
	ACT_WRITE       = 5,   /* write, pwrite64, writev, pwritev, sendto, sendmsg */
	ACT_OPEN        = 6,   /* open, openat, openat2, creat */
	ACT_CLOSE       = 7,   /* close, close_range, shutdown */
	ACT_STAT        = 8,   /* stat, fstat, lstat, statx, newfstatat */
	ACT_POLL        = 9,   /* poll, ppoll, select, pselect6, epoll_wait, epoll_pwait */
	ACT_SEEK        = 10,  /* lseek */
	ACT_MMAP        = 11,  /* mmap, munmap, mprotect, mremap, madvise, brk */
	ACT_IOCTL       = 12,  /* ioctl */
	ACT_DUP         = 13,  /* dup, dup2, dup3 */
	ACT_PIPE        = 14,  /* pipe, pipe2 */
	ACT_SOCKET      = 15,  /* socket, socketpair, bind, listen */
	ACT_CONNECT     = 16,  /* connect */
	ACT_ACCEPT      = 17,  /* accept, accept4 */
	ACT_FCNTL       = 18,  /* fcntl, flock */
	ACT_FORK        = 19,  /* clone, clone3, fork, vfork */
	ACT_EXEC        = 20,  /* execve, execveat */
	ACT_EXIT        = 21,  /* exit, exit_group */
	ACT_WAIT        = 22,  /* wait4, waitid */
	ACT_KILL        = 23,  /* kill, tkill, tgkill */
	ACT_SIGNAL      = 24,  /* rt_sigaction, rt_sigprocmask, sigaltstack */
	ACT_FUTEX       = 25,  /* futex, futex_waitv */
	ACT_EPOLL_CTL   = 26,  /* epoll_create, epoll_ctl */
	ACT_GETDENTS    = 27,  /* getdents, getdents64 */
	ACT_FSOP        = 28,  /* rename, unlink, mkdir, rmdir, chmod, chown, link, symlink */
	ACT_SCHED_OP    = 29,  /* sched_yield, sched_setaffinity, nanosleep, clock_nanosleep */
	ACT_IORING      = 30,  /* io_uring_setup, io_uring_enter */

	ACT_OTHER       = 31,  /* catch-all for unmapped syscalls */
	NR_ACTIONS      = 32,
};

/* ===================================================================
 * Field 2: Object Type — what kind of object is being operated on
 * =================================================================== */

enum tok_object_type {
	OBJ_NA          = 0,   /* not applicable / unknown */
	OBJ_TASK        = 1,   /* sched events — object is a task */
	OBJ_BLOCK_DEV   = 2,   /* block I/O events */
	OBJ_FD_FILE     = 3,   /* regular file (future: fd classification) */
	OBJ_FD_SOCK_TCP = 4,
	OBJ_FD_SOCK_UDP = 5,
	OBJ_FD_SOCK_UNX = 6,   /* unix domain socket */
	OBJ_FD_PIPE     = 7,
	OBJ_FD_EPOLL    = 8,
	OBJ_FD_TIMERFD  = 9,
	OBJ_FD_SIGNALFD = 10,
	OBJ_FD_EVENTFD  = 11,
	OBJ_FD_DEVICE   = 12,
	OBJ_FD_PROC     = 13,  /* /proc fd */
	OBJ_MEMORY      = 14,  /* mmap/brk */
	OBJ_FD_OTHER    = 15,
	NR_OBJ_TYPES    = 16,
};

/* ===================================================================
 * Field 3: Target Class — where is it directed
 * =================================================================== */

enum tok_target_class {
	TGT_NA          = 0,   /* not applicable / unknown */
	TGT_PATH_ETC    = 1,
	TGT_PATH_TMP    = 2,
	TGT_PATH_PROC   = 3,
	TGT_PATH_SYS    = 4,
	TGT_PATH_DEV    = 5,
	TGT_PATH_HOME   = 6,
	TGT_PATH_VARLOG = 7,
	TGT_ADDR_LOOP   = 8,   /* 127.0.0.0/8 */
	TGT_ADDR_LOCAL  = 9,   /* RFC1918 */
	TGT_ADDR_EXT    = 10,  /* everything else */
	TGT_PATH_LIB    = 11,  /* /lib, /usr/lib */
	TGT_PATH_BIN    = 12,  /* /bin, /usr/bin, /sbin */
	TGT_PATH_RUN    = 13,  /* /run, /var/run */
	TGT_SELF        = 14,  /* self-referential (e.g., own pid) */
	TGT_OTHER       = 15,
	NR_TGT_CLASSES  = 16,
};

/* ===================================================================
 * Field 4: Size Class — how much data
 * =================================================================== */

enum tok_size_class {
	SIZE_0          = 0,   /* 0 bytes */
	SIZE_TINY       = 1,   /* 1-64 */
	SIZE_SMALL      = 2,   /* 65-512 */
	SIZE_PAGE       = 3,   /* 513-4096 */
	SIZE_LARGE      = 4,   /* 4097-64K */
	SIZE_HUGE       = 5,   /* 64K-1M */
	SIZE_ENORMOUS   = 6,   /* > 1M */
	SIZE_NA         = 7,   /* not applicable */
	NR_SIZE_CLASSES = 8,
};

/* ===================================================================
 * Field 5: Result Class — what happened
 * =================================================================== */

enum tok_result_class {
	RET_SUCCESS     = 0,
	RET_PARTIAL     = 1,   /* future: partial read/write */
	RET_EAGAIN      = 2,   /* future: from sys_exit */
	RET_EPERM       = 3,   /* future */
	RET_ENOENT      = 4,   /* future */
	RET_EINTR       = 5,   /* future */
	RET_OTHER_ERR   = 6,
	RET_NA          = 7,   /* not applicable (sys_enter has no result) */
	NR_RET_CLASSES  = 8,
};

/* ===================================================================
 * Field 6: Duration Class — how long did it take
 * =================================================================== */

enum tok_duration_class {
	DUR_INSTANT     = 0,   /* < 1μs */
	DUR_FAST        = 1,   /* 1-100μs */
	DUR_NORMAL      = 2,   /* 100μs-1ms */
	DUR_SLOW        = 3,   /* 1-10ms */
	DUR_VSLOW       = 4,   /* 10-100ms */
	DUR_BLOCKED     = 5,   /* 100ms-1s */
	DUR_HUNG        = 6,   /* > 1s */
	DUR_NA          = 7,   /* not applicable */
	NR_DUR_CLASSES  = 8,
};

/* ===================================================================
 * Field 7: Gap Class — time since previous event on this CPU
 * =================================================================== */

enum tok_gap_class {
	GAP_BURST       = 0,   /* < 1μs */
	GAP_RAPID       = 1,   /* 1-10μs */
	GAP_FAST        = 2,   /* 10-100μs */
	GAP_NORMAL      = 3,   /* 100μs-1ms */
	GAP_PAUSE       = 4,   /* 1-10ms */
	GAP_SLOW        = 5,   /* 10-100ms */
	GAP_IDLE        = 6,   /* 100ms-1s */
	GAP_DORMANT     = 7,   /* > 1s (or first event) */
	NR_GAP_CLASSES  = 8,
};

/* ===================================================================
 * Core types
 * =================================================================== */

/* Field index constants */
#define TOK_FIELD_CATEGORY   0
#define TOK_FIELD_ACTION     1
#define TOK_FIELD_OBJ_TYPE   2
#define TOK_FIELD_TARGET     3
#define TOK_FIELD_SIZE       4
#define TOK_FIELD_RESULT     5
#define TOK_FIELD_DURATION   6
#define TOK_FIELD_GAP        7
#define TOK_NR_FIELDS        8

/* The semantic token: 8 bytes, one per field */
struct tokenized_event {
	u8 fields[TOK_NR_FIELDS];
};

/* Per-CPU tokenizer state */
struct hackbot_tok_cpu {
	u64                    prev_ns;    /* timestamp of last event on this CPU */
	struct tokenized_event last_token; /* most recent token (for n-gram bigrams) */
};

/* Global token ring buffer entry (for debugging/verification) */
#define TOKEN_RING_SIZE      64
#define TOKEN_RING_MASK      (TOKEN_RING_SIZE - 1)

struct token_ring_entry {
	struct tokenized_event token;
	u64                    timestamp_ns;
	u32                    cpu;
	s32                    pid;
};

/* ===================================================================
 * Public API
 * =================================================================== */

int  hackbot_tokenizer_init(void);
void hackbot_tokenizer_exit(void);

/*
 * Tokenize a sched_switch event.
 * Called from hackbot_probe_sched_switch() with preemption disabled.
 */
void hackbot_tokenize_sched(struct task_struct *prev,
			    struct task_struct *next,
			    unsigned int prev_state, u64 now_ns);

/*
 * Tokenize a sys_enter event.
 * Called from hackbot_probe_sys_enter() with preemption disabled.
 */
void hackbot_tokenize_syscall(struct pt_regs *regs, long syscall_id,
			      u64 now_ns);

/*
 * Tokenize a block_rq_complete event.
 * Called from hackbot_probe_block_rq_complete() with preemption disabled.
 */
void hackbot_tokenize_io(struct request *rq, int error,
			 unsigned int nr_bytes, u32 latency_us,
			 u64 now_ns);

/*
 * Read last N tokenized events formatted as text.
 * Follows the same pattern as hackbot_trace_read_*_raw().
 */
int  hackbot_tokenizer_read(char *out, int maxlen, int count);

/*
 * Get the per-CPU last token (for future n-gram consumption).
 * Must be called with preemption disabled.
 */
struct tokenized_event *hackbot_tokenizer_last_token(void);

#endif /* HACKBOT_TOKENIZER_H */
