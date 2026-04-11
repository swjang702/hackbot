// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_tokenizer.c — Semantic event tokenizer for kernel-as-language.
 *
 * Converts raw tracepoint events into structured 8-field tokens:
 *   [category, action, object_type, target_class,
 *    size_class, result_class, duration_class, gap_class]
 *
 * Designed for hot-path execution (~200-400ns) with zero allocations:
 * - Pure integer arithmetic, no FPU
 * - Per-CPU state via DEFINE_PER_CPU (no cache bouncing)
 * - Static const lookup tables (zero runtime init cost)
 * - Stack-allocated tokenized_event (8 bytes)
 *
 * Called from tracepoint callbacks in hackbot_trace.c with preemption
 * disabled. Must not sleep, allocate, or take sleeping locks.
 */

#include <linux/kernel.h>
#include <linux/percpu.h>
#include <linux/sched.h>
#include <linux/atomic.h>
#include <linux/string.h>
#include <linux/blkdev.h>
#include <linux/blk-mq.h>
#include <asm/ptrace.h>
#include <asm/unistd.h>
#include "hackbot_tokenizer.h"

/* ===================================================================
 * Per-CPU state
 * =================================================================== */

static DEFINE_PER_CPU(struct hackbot_tok_cpu, hackbot_tok_percpu);

/* ===================================================================
 * Global token ring buffer (for debugging/verification)
 * =================================================================== */

static struct token_ring_entry token_ring[TOKEN_RING_SIZE];
static atomic_t token_ring_head = ATOMIC_INIT(-1);
static atomic64_t token_total_count = ATOMIC64_INIT(0);

/* ===================================================================
 * Syscall → Action lookup table
 *
 * Maps x86_64 syscall numbers to action categories.
 * Unmapped syscalls default to ACT_OTHER.
 * =================================================================== */

#define MAX_SYSCALL_NR 512

static const u8 syscall_to_action[MAX_SYSCALL_NR] = {
	/* Initialize all to ACT_OTHER, then override known syscalls.
	 * C99 designated initializers: unmentioned entries are zero-initialized,
	 * so we set ACT_OTHER = 31 explicitly for all, then override.
	 * Actually, zero-init means ACT_SWITCH_IN (0) which is wrong.
	 * Use a runtime init instead? No — we want static const.
	 *
	 * Solution: we check bounds and use a wrapper function that returns
	 * ACT_OTHER for unmapped entries. The table stores 0 for unmapped,
	 * and we treat 0 as "not mapped" (remap in the accessor).
	 *
	 * Better solution: use a non-zero sentinel. ACT_OTHER = 31.
	 * But C doesn't allow default initialization to non-zero.
	 *
	 * Final solution: store action+1, where 0 means unmapped→ACT_OTHER.
	 */

	[__NR_read]           = ACT_READ + 1,
	[__NR_pread64]        = ACT_READ + 1,
	[__NR_readv]          = ACT_READ + 1,
#ifdef __NR_preadv
	[__NR_preadv]         = ACT_READ + 1,
#endif
	[__NR_recvfrom]       = ACT_READ + 1,
	[__NR_recvmsg]        = ACT_READ + 1,
#ifdef __NR_recvmmsg
	[__NR_recvmmsg]       = ACT_READ + 1,
#endif
	[__NR_readlink]       = ACT_READ + 1,
#ifdef __NR_readahead
	[__NR_readahead]      = ACT_READ + 1,
#endif

	[__NR_write]          = ACT_WRITE + 1,
	[__NR_pwrite64]       = ACT_WRITE + 1,
	[__NR_writev]         = ACT_WRITE + 1,
#ifdef __NR_pwritev
	[__NR_pwritev]        = ACT_WRITE + 1,
#endif
	[__NR_sendto]         = ACT_WRITE + 1,
	[__NR_sendmsg]        = ACT_WRITE + 1,
#ifdef __NR_sendmmsg
	[__NR_sendmmsg]       = ACT_WRITE + 1,
#endif
	[__NR_sendfile]       = ACT_WRITE + 1,

	[__NR_open]           = ACT_OPEN + 1,
#ifdef __NR_openat
	[__NR_openat]         = ACT_OPEN + 1,
#endif
#ifdef __NR_openat2
	[__NR_openat2]        = ACT_OPEN + 1,
#endif
	[__NR_creat]          = ACT_OPEN + 1,

	[__NR_close]          = ACT_CLOSE + 1,
	[__NR_shutdown]       = ACT_CLOSE + 1,
#ifdef __NR_close_range
	[__NR_close_range]    = ACT_CLOSE + 1,
#endif

	[__NR_stat]           = ACT_STAT + 1,
	[__NR_fstat]          = ACT_STAT + 1,
	[__NR_lstat]          = ACT_STAT + 1,
#ifdef __NR_newfstatat
	[__NR_newfstatat]     = ACT_STAT + 1,
#endif
#ifdef __NR_statx
	[__NR_statx]          = ACT_STAT + 1,
#endif
	[__NR_access]         = ACT_STAT + 1,
#ifdef __NR_faccessat
	[__NR_faccessat]      = ACT_STAT + 1,
#endif

	[__NR_poll]           = ACT_POLL + 1,
	[__NR_select]         = ACT_POLL + 1,
#ifdef __NR_pselect6
	[__NR_pselect6]       = ACT_POLL + 1,
#endif
#ifdef __NR_ppoll
	[__NR_ppoll]          = ACT_POLL + 1,
#endif
#ifdef __NR_epoll_wait
	[__NR_epoll_wait]     = ACT_POLL + 1,
#endif
#ifdef __NR_epoll_pwait
	[__NR_epoll_pwait]    = ACT_POLL + 1,
#endif

	[__NR_lseek]          = ACT_SEEK + 1,

	[__NR_mmap]           = ACT_MMAP + 1,
	[__NR_mprotect]       = ACT_MMAP + 1,
	[__NR_munmap]         = ACT_MMAP + 1,
	[__NR_brk]            = ACT_MMAP + 1,
	[__NR_mremap]         = ACT_MMAP + 1,
	[__NR_madvise]        = ACT_MMAP + 1,
	[__NR_msync]          = ACT_MMAP + 1,
#ifdef __NR_mlock2
	[__NR_mlock2]         = ACT_MMAP + 1,
#endif

	[__NR_ioctl]          = ACT_IOCTL + 1,

	[__NR_dup]            = ACT_DUP + 1,
	[__NR_dup2]           = ACT_DUP + 1,
#ifdef __NR_dup3
	[__NR_dup3]           = ACT_DUP + 1,
#endif

	[__NR_pipe]           = ACT_PIPE + 1,
#ifdef __NR_pipe2
	[__NR_pipe2]          = ACT_PIPE + 1,
#endif

	[__NR_socket]         = ACT_SOCKET + 1,
	[__NR_socketpair]     = ACT_SOCKET + 1,
	[__NR_bind]           = ACT_SOCKET + 1,
	[__NR_listen]         = ACT_SOCKET + 1,
	[__NR_getsockname]    = ACT_SOCKET + 1,
	[__NR_getpeername]    = ACT_SOCKET + 1,
	[__NR_setsockopt]     = ACT_SOCKET + 1,
	[__NR_getsockopt]     = ACT_SOCKET + 1,

	[__NR_connect]        = ACT_CONNECT + 1,

	[__NR_accept]         = ACT_ACCEPT + 1,
#ifdef __NR_accept4
	[__NR_accept4]        = ACT_ACCEPT + 1,
#endif

	[__NR_fcntl]          = ACT_FCNTL + 1,
	[__NR_flock]          = ACT_FCNTL + 1,
	[__NR_fsync]          = ACT_FCNTL + 1,
	[__NR_fdatasync]      = ACT_FCNTL + 1,
#ifdef __NR_fallocate
	[__NR_fallocate]      = ACT_FCNTL + 1,
#endif

	[__NR_clone]          = ACT_FORK + 1,
	[__NR_fork]           = ACT_FORK + 1,
	[__NR_vfork]          = ACT_FORK + 1,
#ifdef __NR_clone3
	[__NR_clone3]         = ACT_FORK + 1,
#endif

	[__NR_execve]         = ACT_EXEC + 1,
#ifdef __NR_execveat
	[__NR_execveat]       = ACT_EXEC + 1,
#endif

	[__NR_exit]           = ACT_EXIT + 1,
	[__NR_exit_group]     = ACT_EXIT + 1,

	[__NR_wait4]          = ACT_WAIT + 1,
#ifdef __NR_waitid
	[__NR_waitid]         = ACT_WAIT + 1,
#endif

	[__NR_kill]           = ACT_KILL + 1,
	[__NR_tkill]          = ACT_KILL + 1,
	[__NR_tgkill]         = ACT_KILL + 1,

	[__NR_rt_sigaction]   = ACT_SIGNAL + 1,
	[__NR_rt_sigprocmask] = ACT_SIGNAL + 1,
	[__NR_rt_sigreturn]   = ACT_SIGNAL + 1,

#ifdef __NR_futex
	[__NR_futex]          = ACT_FUTEX + 1,
#endif
#ifdef __NR_futex_waitv
	[__NR_futex_waitv]    = ACT_FUTEX + 1,
#endif

#ifdef __NR_epoll_create
	[__NR_epoll_create]   = ACT_EPOLL_CTL + 1,
#endif
#ifdef __NR_epoll_ctl
	[__NR_epoll_ctl]      = ACT_EPOLL_CTL + 1,
#endif
#ifdef __NR_eventfd2
	[__NR_eventfd2]       = ACT_EPOLL_CTL + 1,
#endif
#ifdef __NR_timerfd_create
	[__NR_timerfd_create] = ACT_EPOLL_CTL + 1,
#endif
#ifdef __NR_timerfd_settime
	[__NR_timerfd_settime] = ACT_EPOLL_CTL + 1,
#endif
#ifdef __NR_signalfd4
	[__NR_signalfd4]      = ACT_EPOLL_CTL + 1,
#endif
#ifdef __NR_inotify_init
	[__NR_inotify_init]   = ACT_EPOLL_CTL + 1,
#endif

	[__NR_getdents]       = ACT_GETDENTS + 1,
	[__NR_getdents64]     = ACT_GETDENTS + 1,

	[__NR_rename]         = ACT_FSOP + 1,
#ifdef __NR_renameat
	[__NR_renameat]       = ACT_FSOP + 1,
#endif
#ifdef __NR_renameat2
	[__NR_renameat2]      = ACT_FSOP + 1,
#endif
	[__NR_unlink]         = ACT_FSOP + 1,
#ifdef __NR_unlinkat
	[__NR_unlinkat]       = ACT_FSOP + 1,
#endif
	[__NR_rmdir]          = ACT_FSOP + 1,
	[__NR_mkdir]          = ACT_FSOP + 1,
#ifdef __NR_mkdirat
	[__NR_mkdirat]        = ACT_FSOP + 1,
#endif
	[__NR_link]           = ACT_FSOP + 1,
	[__NR_symlink]        = ACT_FSOP + 1,
	[__NR_chmod]          = ACT_FSOP + 1,
	[__NR_fchmod]         = ACT_FSOP + 1,
	[__NR_chown]          = ACT_FSOP + 1,
	[__NR_fchown]         = ACT_FSOP + 1,
	[__NR_lchown]         = ACT_FSOP + 1,
	[__NR_truncate]       = ACT_FSOP + 1,
	[__NR_ftruncate]      = ACT_FSOP + 1,
	[__NR_chdir]          = ACT_FSOP + 1,
	[__NR_fchdir]         = ACT_FSOP + 1,
#ifdef __NR_copy_file_range
	[__NR_copy_file_range] = ACT_FSOP + 1,
#endif

	[__NR_sched_yield]    = ACT_SCHED_OP + 1,
	[__NR_nanosleep]      = ACT_SCHED_OP + 1,
	[__NR_pause]          = ACT_SCHED_OP + 1,
#ifdef __NR_clock_nanosleep
	[__NR_clock_nanosleep] = ACT_SCHED_OP + 1,
#endif

#ifdef __NR_io_uring_setup
	[__NR_io_uring_setup] = ACT_IORING + 1,
#endif
#ifdef __NR_io_uring_enter
	[__NR_io_uring_enter] = ACT_IORING + 1,
#endif
};

static inline u8 get_syscall_action(long id)
{
	u8 val;

	if (id < 0 || id >= MAX_SYSCALL_NR)
		return ACT_OTHER;
	val = syscall_to_action[id];
	return val ? (val - 1) : ACT_OTHER;
}

/* ===================================================================
 * Quantizer functions — pure integer, no FPU
 * =================================================================== */

/*
 * Quantize inter-event gap (nanoseconds) into 8 buckets.
 * Thresholds: 1μs, 10μs, 100μs, 1ms, 10ms, 100ms, 1s
 */
static inline u8 quantize_gap_ns(u64 delta_ns)
{
	if (delta_ns < 1000ULL)           return GAP_BURST;    /* < 1μs */
	if (delta_ns < 10000ULL)          return GAP_RAPID;    /* 1-10μs */
	if (delta_ns < 100000ULL)         return GAP_FAST;     /* 10-100μs */
	if (delta_ns < 1000000ULL)        return GAP_NORMAL;   /* 100μs-1ms */
	if (delta_ns < 10000000ULL)       return GAP_PAUSE;    /* 1-10ms */
	if (delta_ns < 100000000ULL)      return GAP_SLOW;     /* 10-100ms */
	if (delta_ns < 1000000000ULL)     return GAP_IDLE;     /* 100ms-1s */
	return GAP_DORMANT;                                     /* > 1s */
}

/*
 * Quantize byte count into 8 buckets.
 * Thresholds: 0, 64, 512, 4096, 64K, 1M
 */
static inline u8 quantize_size(u64 bytes)
{
	if (bytes == 0)                   return SIZE_0;
	if (bytes <= 64)                  return SIZE_TINY;
	if (bytes <= 512)                 return SIZE_SMALL;
	if (bytes <= 4096)                return SIZE_PAGE;
	if (bytes <= 65536)               return SIZE_LARGE;
	if (bytes <= 1048576)             return SIZE_HUGE;
	return SIZE_ENORMOUS;
}

/*
 * Quantize latency (microseconds) into 8 buckets.
 * Thresholds: 1μs, 100μs, 1ms, 10ms, 100ms, 1s
 */
static inline u8 quantize_duration_us(u32 lat_us)
{
	if (lat_us < 1)                   return DUR_INSTANT;  /* < 1μs */
	if (lat_us < 100)                 return DUR_FAST;     /* 1-100μs */
	if (lat_us < 1000)                return DUR_NORMAL;   /* 100μs-1ms */
	if (lat_us < 10000)               return DUR_SLOW;     /* 1-10ms */
	if (lat_us < 100000)              return DUR_VSLOW;    /* 10-100ms */
	if (lat_us < 1000000)             return DUR_BLOCKED;  /* 100ms-1s */
	return DUR_HUNG;                                        /* > 1s */
}

/* ===================================================================
 * Gap class helper — uses per-CPU prev_ns
 *
 * Must be called with preemption disabled.
 * =================================================================== */

static inline u8 compute_gap_class(u64 now_ns)
{
	struct hackbot_tok_cpu *cpu_state = this_cpu_ptr(&hackbot_tok_percpu);
	u64 prev = cpu_state->prev_ns;
	u64 delta;

	if (unlikely(prev == 0))
		return GAP_DORMANT;  /* first event after init */

	delta = now_ns - prev;
	return quantize_gap_ns(delta);
}

/* ===================================================================
 * Field name tables — used by store_token debug prints and read output
 * =================================================================== */

static const char * const cat_names[NR_CATEGORIES] = {
	"SCHED", "SYSCALL", "BLOCK", "NET", "MEM", "FS", "IRQ", "SIGNAL"
};

static const char * const act_names[NR_ACTIONS] = {
	"SW_IN", "SW_OUT", "BLK_R", "BLK_W",
	"READ", "WRITE", "OPEN", "CLOSE", "STAT", "POLL", "SEEK", "MMAP",
	"IOCTL", "DUP", "PIPE", "SOCK", "CONN", "ACPT", "FCNTL", "FORK",
	"EXEC", "EXIT", "WAIT", "KILL", "SIG", "FUTEX", "EPOLL", "GDENT",
	"FSOP", "SCHED", "IORING", "OTHER"
};

static const char * const obj_names[NR_OBJ_TYPES] = {
	"NA", "TASK", "BDEV", "FILE", "TCP", "UDP", "UNIX", "PIPE",
	"EPOLL", "TIMFD", "SIGFD", "EVTFD", "DEV", "PROC", "MEM", "OTH"
};

static const char * const tgt_names[NR_TGT_CLASSES] = {
	"NA", "ETC", "TMP", "PROC", "SYS", "DEV", "HOME", "VLOG",
	"LOOP", "LAN", "EXT", "LIB", "BIN", "RUN", "SELF", "OTH"
};

static const char * const size_names[NR_SIZE_CLASSES] = {
	"0", "TINY", "SMALL", "PAGE", "LARGE", "HUGE", "ENORM", "NA"
};

static const char * const ret_names[NR_RET_CLASSES] = {
	"OK", "PART", "EAGN", "EPRM", "ENOT", "EINT", "ERR", "NA"
};

static const char * const dur_names[NR_DUR_CLASSES] = {
	"INST", "FAST", "NORM", "SLOW", "VSLO", "BLKD", "HUNG", "NA"
};

static const char * const gap_names[NR_GAP_CLASSES] = {
	"BURST", "RAPID", "FAST", "NORM", "PAUSE", "SLOW", "IDLE", "DORMT"
};

static inline const char *safe_field_name(const char * const *names,
					  int nr_names, u8 val)
{
	if (val < nr_names && names[val])
		return names[val];
	return "?";
}

/* ===================================================================
 * Store token — update per-CPU state + global ring
 *
 * Must be called with preemption disabled.
 * =================================================================== */

/* Debug: log first N tokens to dmesg for verification */
#define TOKEN_DEBUG_PRINTS 10
static atomic_t token_debug_count = ATOMIC_INIT(0);

static inline void store_token(const struct tokenized_event *tok, u64 now_ns)
{
	struct hackbot_tok_cpu *cpu_state = this_cpu_ptr(&hackbot_tok_percpu);
	int idx;
	long long count;

	/* Update per-CPU state */
	cpu_state->last_token = *tok;
	cpu_state->prev_ns = now_ns;

	/* Append to global ring buffer (Hogwild — no lock) */
	idx = atomic_inc_return(&token_ring_head) & TOKEN_RING_MASK;
	token_ring[idx].token = *tok;
	token_ring[idx].timestamp_ns = now_ns;
	token_ring[idx].cpu = raw_smp_processor_id();
	token_ring[idx].pid = current->pid;

	count = atomic64_inc_return(&token_total_count);

	/* Print first few tokens to dmesg for debugging/verification */
	if (atomic_inc_return(&token_debug_count) <= TOKEN_DEBUG_PRINTS) {
		pr_info("hackbot: token[%lld]: [%s,%s,%s,%s,%s,%s,%s,%s] "
			"cpu=%u pid=%d\n",
			count - 1,
			safe_field_name(cat_names, NR_CATEGORIES,
					tok->fields[TOK_FIELD_CATEGORY]),
			safe_field_name(act_names, NR_ACTIONS,
					tok->fields[TOK_FIELD_ACTION]),
			safe_field_name(obj_names, NR_OBJ_TYPES,
					tok->fields[TOK_FIELD_OBJ_TYPE]),
			safe_field_name(tgt_names, NR_TGT_CLASSES,
					tok->fields[TOK_FIELD_TARGET]),
			safe_field_name(size_names, NR_SIZE_CLASSES,
					tok->fields[TOK_FIELD_SIZE]),
			safe_field_name(ret_names, NR_RET_CLASSES,
					tok->fields[TOK_FIELD_RESULT]),
			safe_field_name(dur_names, NR_DUR_CLASSES,
					tok->fields[TOK_FIELD_DURATION]),
			safe_field_name(gap_names, NR_GAP_CLASSES,
					tok->fields[TOK_FIELD_GAP]),
			raw_smp_processor_id(), current->pid);
	}
}

/* ===================================================================
 * Syscall size extraction (x86_64)
 *
 * For read/write family, arg2 (rdx) is the count/length.
 * For mmap, arg1 (rsi) is the length.
 * Returns SIZE_NA for syscalls without a meaningful size.
 * =================================================================== */

static inline u8 extract_syscall_size(u8 action, struct pt_regs *regs)
{
	u64 size_val;

	switch (action) {
	case ACT_READ:
	case ACT_WRITE:
		/* arg2 = count for read/write/recv/send family */
		size_val = regs->dx;
		return quantize_size(size_val);
	case ACT_MMAP:
		/* arg1 = length for mmap */
		size_val = regs->si;
		return quantize_size(size_val);
	default:
		return SIZE_NA;
	}
}

/* ===================================================================
 * Public tokenization functions
 * =================================================================== */

void hackbot_tokenize_sched(struct task_struct *prev,
			    struct task_struct *next,
			    unsigned int prev_state, u64 now_ns)
{
	struct tokenized_event tok;

	tok.fields[TOK_FIELD_CATEGORY] = CAT_SCHED;
	/*
	 * From the perspective of the CPU: prev is switching OUT,
	 * next is switching IN. We emit one token per event.
	 * Use SWITCH_OUT since this captures the "prev left" semantics.
	 * The n-gram learns the prev→next transition naturally.
	 */
	tok.fields[TOK_FIELD_ACTION]   = ACT_SWITCH_OUT;
	tok.fields[TOK_FIELD_OBJ_TYPE] = OBJ_TASK;
	tok.fields[TOK_FIELD_TARGET]   = TGT_NA;
	tok.fields[TOK_FIELD_SIZE]     = SIZE_NA;
	tok.fields[TOK_FIELD_RESULT]   = RET_NA;
	tok.fields[TOK_FIELD_DURATION] = DUR_NA;
	tok.fields[TOK_FIELD_GAP]      = compute_gap_class(now_ns);

	store_token(&tok, now_ns);
}

void hackbot_tokenize_syscall(struct pt_regs *regs, long syscall_id,
			      u64 now_ns)
{
	struct tokenized_event tok;
	u8 action;

	action = get_syscall_action(syscall_id);

	tok.fields[TOK_FIELD_CATEGORY] = CAT_SYSCALL;
	tok.fields[TOK_FIELD_ACTION]   = action;
	tok.fields[TOK_FIELD_OBJ_TYPE] = OBJ_NA;   /* fd classification deferred */
	tok.fields[TOK_FIELD_TARGET]   = TGT_NA;   /* path classification deferred */
	tok.fields[TOK_FIELD_SIZE]     = extract_syscall_size(action, regs);
	tok.fields[TOK_FIELD_RESULT]   = RET_NA;   /* sys_enter has no result */
	tok.fields[TOK_FIELD_DURATION] = DUR_NA;   /* sys_enter has no duration */
	tok.fields[TOK_FIELD_GAP]      = compute_gap_class(now_ns);

	store_token(&tok, now_ns);
}

void hackbot_tokenize_io(struct request *rq, int error,
			 unsigned int nr_bytes, u32 latency_us,
			 u64 now_ns)
{
	struct tokenized_event tok;

	tok.fields[TOK_FIELD_CATEGORY] = CAT_BLOCK;
	tok.fields[TOK_FIELD_ACTION]   = op_is_write(req_op(rq))
					 ? ACT_BLK_WRITE : ACT_BLK_READ;
	tok.fields[TOK_FIELD_OBJ_TYPE] = OBJ_BLOCK_DEV;
	tok.fields[TOK_FIELD_TARGET]   = TGT_NA;
	tok.fields[TOK_FIELD_SIZE]     = quantize_size(nr_bytes);
	tok.fields[TOK_FIELD_RESULT]   = error ? RET_OTHER_ERR : RET_SUCCESS;
	tok.fields[TOK_FIELD_DURATION] = quantize_duration_us(latency_us);
	tok.fields[TOK_FIELD_GAP]      = compute_gap_class(now_ns);

	store_token(&tok, now_ns);
}

struct tokenized_event *hackbot_tokenizer_last_token(void)
{
	return &this_cpu_ptr(&hackbot_tok_percpu)->last_token;
}

/* ===================================================================
 * Debug output — format token ring as human-readable text
 * =================================================================== */

/* Simple append helpers (same pattern as hackbot_trace.c) */
static int tok_append_str(char *out, int pos, int maxlen, const char *s)
{
	while (*s && pos < maxlen)
		out[pos++] = *s++;
	return pos;
}

static int tok_append_num(char *out, int pos, int maxlen, long long val)
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

int hackbot_tokenizer_read(char *out, int maxlen, int count)
{
	int pos = 0, i;
	int head, start;
	long long total;

	if (count <= 0 || count > TOKEN_RING_SIZE)
		count = 20;

	head = atomic_read(&token_ring_head);
	total = atomic64_read(&token_total_count);

	pos = tok_append_str(out, pos, maxlen, "=== Tokens (last ");
	pos = tok_append_num(out, pos, maxlen, count);
	pos = tok_append_str(out, pos, maxlen, " of ");
	pos = tok_append_num(out, pos, maxlen, total);
	pos = tok_append_str(out, pos, maxlen, " total) ===\n");

	if (total == 0) {
		pos = tok_append_str(out, pos, maxlen, "[No tokens yet]\n");
		pos = tok_append_str(out, pos, maxlen, "===\n");
		return (pos > 0) ? pos : 0;
	}

	/* Walk backwards from head */
	start = (head - count + 1 + TOKEN_RING_SIZE) & TOKEN_RING_MASK;

	for (i = 0; i < count && pos > 0 && pos < maxlen - 128; i++) {
		int idx = (start + i) & TOKEN_RING_MASK;
		struct token_ring_entry *e = &token_ring[idx];
		struct tokenized_event *t = &e->token;

		if (e->timestamp_ns == 0)
			continue;  /* empty slot */

		/* Format: CPU0 pid=1234: [SYSCALL,READ,NA,NA,PAGE,NA,NA,NORM] */
		pos = tok_append_str(out, pos, maxlen, "CPU");
		pos = tok_append_num(out, pos, maxlen, e->cpu);
		pos = tok_append_str(out, pos, maxlen, " pid=");
		pos = tok_append_num(out, pos, maxlen, e->pid);
		pos = tok_append_str(out, pos, maxlen, ": [");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(cat_names, NR_CATEGORIES,
					t->fields[TOK_FIELD_CATEGORY]));
		pos = tok_append_str(out, pos, maxlen, ",");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(act_names, NR_ACTIONS,
					t->fields[TOK_FIELD_ACTION]));
		pos = tok_append_str(out, pos, maxlen, ",");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(obj_names, NR_OBJ_TYPES,
					t->fields[TOK_FIELD_OBJ_TYPE]));
		pos = tok_append_str(out, pos, maxlen, ",");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(tgt_names, NR_TGT_CLASSES,
					t->fields[TOK_FIELD_TARGET]));
		pos = tok_append_str(out, pos, maxlen, ",");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(size_names, NR_SIZE_CLASSES,
					t->fields[TOK_FIELD_SIZE]));
		pos = tok_append_str(out, pos, maxlen, ",");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(ret_names, NR_RET_CLASSES,
					t->fields[TOK_FIELD_RESULT]));
		pos = tok_append_str(out, pos, maxlen, ",");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(dur_names, NR_DUR_CLASSES,
					t->fields[TOK_FIELD_DURATION]));
		pos = tok_append_str(out, pos, maxlen, ",");
		pos = tok_append_str(out, pos, maxlen,
			safe_field_name(gap_names, NR_GAP_CLASSES,
					t->fields[TOK_FIELD_GAP]));
		pos = tok_append_str(out, pos, maxlen, "]\n");
	}

	pos = tok_append_str(out, pos, maxlen, "===\n");
	return (pos > 0) ? pos : 0;
}

/* ===================================================================
 * Init / Exit
 * =================================================================== */

int hackbot_tokenizer_init(void)
{
	int cpu;

	/* Zero per-CPU state */
	for_each_possible_cpu(cpu) {
		struct hackbot_tok_cpu *s = per_cpu_ptr(&hackbot_tok_percpu, cpu);
		memset(s, 0, sizeof(*s));
	}

	/* Zero ring buffer */
	memset(token_ring, 0, sizeof(token_ring));
	atomic_set(&token_ring_head, -1);
	atomic64_set(&token_total_count, 0);

	pr_info("hackbot: tokenizer: semantic event tokenizer initialized "
		"(%d categories, %d actions, %d fields)\n",
		NR_CATEGORIES, NR_ACTIONS, TOK_NR_FIELDS);

	return 0;
}

void hackbot_tokenizer_exit(void)
{
	pr_info("hackbot: tokenizer: shutdown (%lld events tokenized)\n",
		atomic64_read(&token_total_count));
}
