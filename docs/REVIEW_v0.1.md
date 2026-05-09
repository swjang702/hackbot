# `hackbot-kmod` v0.1 Code Review

**Date:** 2026-05-09
**Scope:** entire `hackbot-kmod/` tree at tag `kmod-v0.1.0`
**Method:** three independent in-depth reviews driven by the Linus_Torvalds
agent persona, partitioned as (1) Rust + FFI surface, (2) C kernel hot paths,
(3) build/test/doc posture. Headline P1 findings spot-verified against the
actual source after the agents returned. Lower-severity findings are
agent-reported and not yet independently re-verified.

This document captures every finding so the patches can be picked up later.
**No code in `hackbot-kmod/` was modified as part of producing this review.**

---

## How to read this document

- Findings are stable-IDed (`R-001` … `R-040`). Future commits and PRs should
  reference these IDs so we can correlate fixes against the review.
- Severity:
  - **P1** — panic / undefined behavior / race / kernel-API contract violation. Will OOPS or silently corrupt under realistic conditions.
  - **P2** — correctness gap. Wrong output, leak on error path, or
    silent-partial-failure. Won't crash, will mislead.
  - **P3** — idiom, hardening, doc accuracy. Worth doing; not urgent.
- Verification flag:
  - **[V]** — I read the actual code after the agent flagged it; agent claim confirmed.
  - **[A]** — Agent-reported, not independently re-verified. Trust the line citation but check the surrounding context before patching.
- Every finding cites `file:line`. Lines are at `kmod-v0.1.0` (commit `8276882`).

---

## Composite verdict

`hackbot-kmod` is not yet ready to leave running on a busy machine. The
recent panic-fix commit `046b4c9` cleaned up the obvious stuff; the
next layer of bugs is what this review surfaces. Three production
failure modes dominate everything else:

1. **`hackbot_trace.c` is the worst offender.** Three system-wide
   `raw_spinlock`s are acquired by every CPU's tracepoint callback,
   serializing the hottest scheduler path on hot global cachelines.
   A signed `%` on a monotonically incremented counter goes negative
   after roughly 6 hours of uptime at 100 k events/s and writes
   out-of-bounds into adjacent slab memory. `pr_info` calls fire from
   inside `sched_switch` for the first ~10 events.
2. **`hackbot_fpu.c` holds `local_bh_disable` for the entire forward
   pass.** Tens-to-hundreds of milliseconds of soft-irqs disabled per
   token in scalar code drops timer interrupts and network completions
   visibly. The kernel's FPU API explicitly requires bounded windows.
3. **`hackbot_files.c` has a use-after-free on `task->files`.** Only
   RCU read-side is held, but `files_cachep` is not allocated with
   `SLAB_TYPESAFE_BY_RCU`. The struct can be freed (and the memory
   reused) between the load and the `spin_lock(&files->file_lock)`.

On top of this, the test harness can't catch any of the above —
`test.sh` never greps `dmesg` for `BUG:` / `KASAN:` / `KCSAN:` /
`Call Trace` markers, so sanitizers can fire silently while the suite
reports green. The "we ran sanitizers" claim in the README is currently
not underwritten by anything.

The Rust side is in better shape but has its own P1s: a sleeping
allocation under `rcu::read_lock()` in `tool_ps`, an unbounded write
size in `write_iter` (free DoS for any process holding the device fd),
and a forward-pass file (`hackbot_forward.rs`) where every `unsafe`
block lacks a `// SAFETY:` justification and the
multiple-mutable-borrow-from-raw-pointer pattern is borderline UB
under Rust's stacked-borrows model.

Build/test posture is honest about what it intends but underspecified
about what it actually verifies. `KASAN_SANITIZE := y` only does
something useful if the host kernel is built with `KASAN_GENERIC` or
similar; `make check-kconfig` only checks the parent option.

The good news: nothing here requires re-architecting. The trace hot
path needs a per-CPU rework and a `&` mask in place of `%`. The FPU
path needs tiling. The files tool needs `iterate_fd()` or
`task_lock()`-style bracketing. The Rust safety hygiene is a
bounded one-pass cleanup. The test harness needs a `dmesg` grep
helper. All bounded. Just real.

---

## Top three (the headline)

| ID | File | Severity | Why it leads |
|----|------|----------|--------------|
| **R-001** | `hackbot_trace.c:196,276,331` | P1 | Buried OOB write with a multi-hour uptime fuse |
| **R-007** | `hackbot_fpu.c:670-687` | P1 | Multi-millisecond `local_bh_disable` causes visible system stutters |
| **R-010** | `hackbot_files.c:139-159` | P1 | Use-after-free against an exiting target task |

---

## Findings index

Sorted by file. Severity then ID within each file. `[V]` = directly verified.

| ID | File:line | Sev | Title | Verif |
|----|-----------|-----|-------|-------|
| R-009 | hackbot_console.c:34-66 | P1 | `raw_spinlock` not NMI-safe; self-deadlock on NMI printk | [V] |
| R-027 | hackbot_console.c:62 | P2 | `console_total: u32` wraps after ~4 GB log volume | [A] |
| R-014 | hackbot_device.rs:122-140 | P1 | Unbounded user write — kvmalloc DoS | [V] |
| R-018 | hackbot_device.rs:122-140 | P2 | Last-writer-wins race on global `RESPONSE` | [A] |
| R-010 | hackbot_files.c:139-159 | P1 | UAF on `task->files` (no `SLAB_TYPESAFE_BY_RCU`) | [V] |
| R-011 | hackbot_files.c:158-171 | P1 | `spin_lock(&files->file_lock)` over `fdt->max_fds` walk | [A] |
| R-026 | hackbot_files.c:215-224 | P2 | `snprintf` return-value misuse → stack leak | [A] |
| R-015 | hackbot_forward.rs:146-161,217-243 | P1 | Aliased `&mut [i32]` plus raw-pointer access — stacked-borrows UB | [A] |
| R-016 | hackbot_forward.rs (file-wide) | P1 | Every `unsafe` block lacks `// SAFETY:` argument | [A] |
| R-017 | hackbot_forward.rs:179,234,246 | P1 | Silent integer overflow in Q16.16 inner loops | [A] |
| R-025 | hackbot_forward.rs:118-119 | P2 | `weights_len = data_len - weights_off` with no `checked_sub` | [A] |
| R-007 | hackbot_fpu.c:670-687 | P1 | Multi-millisecond `kernel_fpu_begin/end` window | [A] |
| R-008 | hackbot_fpu.c:389,488,507 | P1 | `pr_info` inside `kernel_fpu_begin/end` region | [A] |
| R-006 | hackbot_kprobe.c:71-105 | P1 | `register_kprobe` slot reuse + missing blacklist guard | [A] |
| R-024 | hackbot_kprobe.c:74-85 | P2 | Embedded NUL in symbol bypasses dedup | [A] |
| R-039 | hackbot_kprobe.c:110,237,265 | P3 | Unrate-limited `pr_info` on agent-driven attach/detach | [A] |
| R-029 | hackbot_model.rs:265 | P3 | 8.7 KiB on-stack `[LayerRef::ZERO; 32]` under `MODEL.lock()` | [A] |
| R-022 | hackbot_net.rs:177 | P2 | JSON escape silently drops control chars 0x01-0x1F | [A] |
| R-005 | hackbot_ngram.c:120-149 | P1 | Halving loop in tracepoint callback + half-Hogwild discipline | [A] |
| R-012 | hackbot_ngram.c:660-697 | P1 | `kfree(st)` after `wake_up_all` — UAF on wait-queue head | [A] |
| R-021 | hackbot_ngram.c (debug `pr_info`) | P2 | Debug `pr_info` from probe context | [A] |
| R-028 | hackbot_ngram.c:235 / :394 | P2 | `last_field_surprise` write/read not coordinated | [A] |
| R-001 | hackbot_trace.c:196,276,331,499,600,691 | P1 | Signed `%` on `atomic_inc_return` ring head — OOB after wrap | [V] |
| R-002 | hackbot_trace.c:206,219,284,298,340,347 | P1 | Three global `raw_spinlock`s in tracepoint callbacks | [V] |
| R-019 | hackbot_trace.c:316-328 | P2 | Bogus block-I/O latency when `rq->start_time_ns == 0` | [A] |
| R-020 | hackbot_trace.c:807-845 | P2 | `tracepoint_probe_register` partial-init returns 0 | [A] |
| R-028b | hackbot_trace.c:198-203 | P2 | Torn 16-byte memcpy of `task->comm` | [A] |
| R-013 | hackbot_tools.rs:117,167-209 | P1 | Sleeping `KVVec::push(GFP_KERNEL)` under `rcu::read_lock()` | [V] |
| R-003 | hackbot_tokenizer.c:458-479 | P1 | `pr_info` from inside `sched_switch` / `sys_enter` / `block_rq_complete` | [A] |
| R-021b | hackbot_tokenizer.rs (debug `pr_info!`) | P2 | DEBUG logs in inference hot path | [A] |
| R-004 | hackbot_trace.c (no `tracepoint_synchronize_unregister`) | ~~P1~~ | False alarm — already implemented at hackbot_trace.c:1006 | [V] |
| R-023 | hackbot_vllm.rs:81,86 | P2 | `-(e.to_errno())` overflow on `i32::MIN` | [A] |
| R-030 | test.sh (entire file) | P1 | No `dmesg` grep for `BUG:` / `KASAN:` / `KCSAN:` markers | [A] |
| R-031 | Makefile:67-69 | P1 | `check-kconfig` checks parent options only | [A] |
| R-032 | Makefile:95-101 | P1 | `make test` not idempotent if module already loaded | [A] |
| R-033 | Kbuild:38-46 | P1 | `kernel_version.rs` regenerates every build | [A] |
| R-034 | Makefile:51 | P2 | `make check-warn` ignores warnings | [A] |
| R-035 | test.sh:283-313 | P2 | `test_tools` skips `loadavg` and `trace` | [A] |
| R-036 | test.sh (file-wide) | P2 | No real test for n-gram / kprobe roundtrip / patrol shutdown / FPU | [A] |
| R-037 | TESTING.md:12,56; test.sh:17 | P3 | Stale vLLM IP (100.66.136.70 vs 100.103.180.11 in code) | [A] |
| R-038 | README.md:199 | P3 | Status line "Step 2k ... next is 2j" — non-monotonic | [A] |
| R-040 | Kbuild:33 | P3 | No defensive guard against float math leaking to other TUs | [A] |

41 rows, IDs preserved. Items in the "downgraded / speculative" appendix
do **not** appear in the table because I judged them not actionable.

---

## Detailed findings

Each finding is grouped by file, ordered by ID. The format is fixed:
**What / Why / Fix**, with citation, severity, and verification flag.

### `hackbot_trace.c`

#### R-001 — Signed modulo on atomic counter — OOB after wrap [P1 V]

**file:** `hackbot_trace.c:196,276,331,499,600,691`

**What.** Three call sites use the pattern
`int idx = atomic_inc_return(&ring_head) % RAW_RING_SIZE;` where
`atomic_inc_return` returns a signed `int`. After roughly 2.1 billion
events the counter wraps past `INT_MAX` and returns negative values;
signed `%` propagates the sign, producing a negative `idx`. The
subsequent stores `s->sched_ring[idx] = ...` (and the symmetric
syscall and I/O rings) then write at a negative array offset.

The same arithmetic appears on the read side at `:499,600,691` for
`(head - count + RAW_RING_SIZE) % RAW_RING_SIZE`, which produces
nonsense once `head` is negative.

**Why.** Out-of-bounds write into whatever the slab placed adjacent to
the ring's containing allocation. The fuse is roughly 6 hours at
100 k events/s, days at 10 k events/s — long enough to slip past
smoke tests, short enough to bite a real workstation. KASAN catches
it the instant it triggers, but only if KASAN is on; otherwise it
silently corrupts a neighboring slab object.

**Fix.** `RAW_RING_SIZE` is a power of two. Cast the counter to
unsigned and mask:
`unsigned int idx = (unsigned int)atomic_inc_return(&...) & (RAW_RING_SIZE - 1);`.
The token ring and alert ring already use this pattern — copy it.

#### R-002 — Three global `raw_spinlock`s on hot tracepoint callbacks [P1 V]

**file:** `hackbot_trace.c:206,219,284,298,340,347`

**What.** Each callback acquires a single global
`raw_spin_lock_irqsave(&s->{sched,syscall,io}_feat_lock)` to update a
single global feature struct. The lock spans a `shift_u32_window`
memmove and a window-arithmetic block.

**Why.** Tracepoint callbacks fire concurrently on every CPU. A single
global raw spinlock serializes all of them. On a 16-CPU machine doing
a few hundred thousand context switches per second, this is hundreds
of microseconds of cross-CPU cacheline ping-pong on three hot global
locks. The header comment claims "~100 ns total" — that is true with
one CPU, not under load. This isn't a panic; it's a kernel-wide
scheduler stall machine.

**Fix.** Make `sched_feat`, `syscall_feat`, `io_feat` per-CPU
(`DEFINE_PER_CPU`). Tracepoint callbacks already run with preemption
disabled, so `this_cpu_ptr` plus `local_irq_save` is sufficient and
needs no cross-CPU synchronization on the write path. The reader (slow
path, in `hackbot_trace_read_*`) iterates per-CPU and merges.

#### R-019 — Bogus block-I/O latency when `rq->start_time_ns == 0` [P2 A]

**file:** `hackbot_trace.c:316-328`

**What.** The block layer only populates `rq->start_time_ns` for
requests with `RQF_IO_STAT | RQF_STATS | RQF_USE_SCHED`. For others
the field can be 0 or stale. The current code does
`if (now > rq->start_time_ns) latency_us = (u32)((now - rq->start_time_ns) / 1000);`
which is true when `start_time_ns == 0` and produces `now/1000`
microseconds — billions, after the cast.

**Why.** Poisons the latency histogram and the tokenizer's
`duration_class`. `quantize_duration_us` returns `DUR_HUNG` for
every un-accounted request.

**Fix.** Guard with `if (rq->start_time_ns && now > rq->start_time_ns)`;
otherwise skip the histogram update or assign a "no-stat" bucket.

#### R-020 — Silent partial init of tracepoint registration [P2 A]

**file:** `hackbot_trace.c:807-845`

**What.** If `tracepoint_probe_register` succeeds for `sched_switch`
but fails for `sys_enter`, the code logs a warn, sets the failed
pointer to NULL, and proceeds. `hackbot_trace_init` returns 0 and the
agent thinks tracing is fully online when only one channel is.

**Fix.** Either roll back on any registration failure, or expose a
`hackbot_trace_active_channels()` accessor that the agent and the
status path can query.

#### R-004 — `tracepoint_synchronize_unregister()` not called before `kfree` [~~P1~~ — false alarm, closed without code change]

**file:** `hackbot_trace.c` (exit path, file-wide)

**Status (2026-05-09):** **False alarm.** Audit confirmed
`tracepoint_synchronize_unregister();` is already called at
`hackbot_trace.c:1006`, between the probe-unregister loop and the
`hackbot_ngram_exit()` / `hackbot_tokenizer_exit()` / `kvfree(s->...)`
/ `kfree(s)` calls. The original C-paths review agent missed it.

The misc-device-context reader UAF this finding could plausibly have
covered (`hackbot_trace_read_*` racing `kvfree` from another CPU) is
also closed today: misc-device fd refcounting holds the file ref
through any in-flight `write_iter`, which holds a module ref, which
prevents `rmmod` from running. The `s->active` check in reader paths
is defensive; the real safety property is the fd refcount.

A hardening comment was added above `hackbot_trace_exit` documenting
this dual reader-drain story (probe context drained by
`tracepoint_synchronize_unregister`; misc-device context drained by
fd refcounting). No structural code change in this round.

Original (now-superseded) text retained for reference:

> **Original What.** After unregistering tracepoint probes, the kernel
> does not guarantee that in-flight probes have completed. The required
> pairing is `tracepoint_probe_unregister(...)` then
> `tracepoint_synchronize_unregister()` then free.
>
> **Original Why.** Without the synchronize, a probe currently running
> on another CPU can still touch the structures we just freed.
>
> **Original Fix.** Add `tracepoint_synchronize_unregister()` between
> the unregister loop and any `kfree` / `kvfree` of state.

#### R-028b — Torn 16-byte memcpy of `task->comm` [P2 A]

**file:** `hackbot_trace.c:198-203`

**What.** `memcpy(prev_comm, prev->comm, 16)` runs lock-free against a
field updated by `set_task_comm()` under task_lock + seqcount. Result
is occasional torn names (halves of two values).

**Fix.** Use `__get_task_comm(buf, sizeof(buf), prev)` or document the
torn read and accept it. Today these strings are only displayed; if
they ever feed hashing or anomaly detection, fix it first.

### `hackbot_tokenizer.c`

#### R-003 — `pr_info` from inside `sched_switch` / `sys_enter` / `block_rq_complete` [P1 A]

**file:** `hackbot_tokenizer.c:458-479`; same pattern at
`hackbot_ngram.c:263-271`

**What.** `store_token` does
`if (atomic_inc_return(&token_debug_count) <= TOKEN_DEBUG_PRINTS) pr_info(...)`.
Runs from preempt-disabled tracepoint context for the first ~10
events of each event type. The `printk` itself feeds back through the
registered `hackbot_console` (which takes a `raw_spinlock`).

**Why.** `printk` from a `sched_switch` tracepoint is a known
landmine. The printk path can wake klogd, schedule console work, and
recurse through other tracepoints fired from inside printk's own
paths. Even when no recursion happens, it blows the tracepoint
latency budget by ~100×. Recursive lockdep reports and printk
self-recursion are common failure modes.

**Fix.** Delete the debug `pr_info`. If early diagnostics matter, gate
behind a one-shot per-CPU flag and use `printk_deferred_once` —
*never* plain `pr_info` from a `sched_switch` callback.

### `hackbot_ngram.c`

#### R-005 — Halving loop in callback + half-Hogwild discipline [P1 A]

**file:** `hackbot_ngram.c:120-149`

**What.** Two intertwined defects:

1. `update_model` does `m->total_events++` (plain non-atomic
   non-`WRITE_ONCE` increment on a shared `u64`). When
   `total_events % halve_interval == 0`, it executes a triple-nested
   loop walking 8 fields × 32 × 32 = 8192 read-modify-writes, plus
   256 row totals. With `halve_interval = 10000` this fires from a
   tracepoint callback every ~3 seconds. Multiple CPUs hitting the
   boundary can both run the entire halving.
2. The bigram updates use `READ_ONCE` + plain add + `WRITE_ONCE`,
   while the halving uses unmarked `>>=`. The two paths race on the
   same cells without coordination. KCSAN-fail by construction.

**Why.** (a) 8 k+ memory ops per halve from soft-irq context blows the
budget. (b) `total_events++` from multiple CPUs has torn-write
potential on 32-bit kernels and KCSAN-instrumented builds. (c) The
mixed marked/unmarked memory access pattern makes the file
unanalyzable by the very tool the project opts into.

**Fix.** `atomic64_inc_return(&m->total_events)` and elect a single
halve-winner via the returned value. Move the actual halving out of
the probe path entirely — schedule it on a workqueue or drive it
from the patrol kthread. While in the probe, do not touch the entire
table. For the count cells: either commit fully to
`atomic_t count[NGRAM_DIM][NGRAM_DIM]` with `atomic_inc` (one
`LOCK INC` per event is trivial) or restructure to RCU-style table
swap. Don't ship the half-and-half pattern.

#### R-012 — `kfree(st)` after `wake_up_all` — UAF on wait-queue head [P1 A]

**file:** `hackbot_ngram.c:660-697`

**What.** `hackbot_ngram_exit` does
`WRITE_ONCE(ngram, NULL); smp_wmb(); wake_up_all(&st->alert_wq); ...; kfree(st);`.
The `alert_wq` is embedded in `*st`. If the patrol thread is currently
in `wait_event_interruptible_timeout(st->alert_wq, ...)` and has not
yet returned from the macro by the time `kfree(st)` runs, the macro's
internal `finish_wait()` touches the freed head.

**Why.** `wake_up_all` does not wait for waiters to exit the wait
macro; it only flips them to runnable. `finish_wait` runs later on
the waiter's CPU, dereferences the wq head, and lists list_del_init.
UAF window. Whether this actually fires today depends on
`hackbot_main.rs` calling `hackbot_patrol_stop` *before*
`hackbot_ngram_exit` — there is no internal enforcement.

**Fix.** Either (a) explicitly assert in `hackbot_ngram_exit` that no
external thread is in a wait on this wq, (b) move the wait-queue head
out of `*st` (static, or kept in a global), or (c) add
`synchronize_rcu()` after the wakeup, paired with making `ngram` an
RCU-protected pointer that waiters re-read.

#### R-021 — Debug `pr_info` from probe context [P2 A]

**file:** `hackbot_ngram.c:263-271`

**What.** Same class as R-003 but lower volume: a few `pr_info`
calls in the n-gram update path that fire only under specific
conditions.

**Fix.** Delete or gate behind `printk_deferred_ratelimited`.

#### R-028 — `last_field_surprise` write/read not coordinated [P2 A]

**file:** `hackbot_ngram.c:235` (writer) / `:394` (reader)

**What.** Writer does
`memcpy(st->last_field_surprise, field_surp, TOK_NR_FIELDS)` — non-
atomic 8-byte memcpy. Reader does `READ_ONCE(...[f])` per field. KCSAN
will flag the memcpy/READ_ONCE pairing as non-marked.

**Fix.** Pack the 8 bytes into a `u64` and use
`WRITE_ONCE`/`READ_ONCE` on the u64, or annotate the memcpy with
`data_race()` if staleness is acceptable.

### `hackbot_fpu.c`

#### R-007 — Multi-ms `kernel_fpu_begin/end` window [P1 A]

**file:** `hackbot_fpu.c:670-687`

**What.** `hackbot_fpu_forward` calls `kernel_fpu_begin()`, runs
`forward_token_impl` (30 layers × dim=576 × FP16-decoded scalar
multiply-adds; `matmul_fp16` walks every weight one by one), then
`kernel_fpu_end()`. For SmolLM2-135M this is on the order of tens to
hundreds of milliseconds per token in scalar C with no SIMD.

**Why.** `kernel_fpu_begin` does `local_bh_disable` (and on
`PREEMPT_RT`, `preempt_disable`). Soft-irqs are disabled for the
duration. Holding soft-irqs disabled for 100 ms drops timer
interrupts, network completions, RCU softirq processing — the system
stutters or hangs visibly. The kernel docs and history (zswap,
raid6, AES-NI) explicitly chunk FPU work into bounded windows for
this reason.

**Fix.** Tile the forward pass: do one matmul row (or one layer),
`kernel_fpu_end()`, `cond_resched()`, `kernel_fpu_begin()`, repeat.
Activation buffers stay valid across the boundary (they're plain
memory, not FPU registers). Document a max-microseconds-per-window
budget.

#### R-008 — `pr_info` inside `kernel_fpu_begin/end` [P1 A]

**file:** `hackbot_fpu.c:389,488,507`

**What.** Three debug prints inside the FPU-protected region, plus
`get_random_u32()` in `hackbot_fpu_get_next_token` (which uses
chacha state with its own per-CPU spinlocks).

**Why.** Extends the soft-irq-disabled window arbitrarily; printk
recursion via the registered `hackbot_console`.

**Fix.** Stash diagnostic values into local ints, `kernel_fpu_end()`,
then `printk`. Same for the sampler.

### `hackbot_console.c`

#### R-009 — `raw_spinlock` is not NMI-safe [P1 V]

**file:** `hackbot_console.c:34-66`

**What.** The file's own comment at `:28` reads "Must be safe in any
context (hardirq, NMI, early boot)". The implementation uses
`raw_spin_lock_irqsave(&hackbot_con_lock, ...)`. An NMI on a CPU
that already holds `hackbot_con_lock` (e.g., NMI watchdog firing
while another path holds the lock) will spin forever — `raw_spinlock`
does not handle NMI re-entry on the same CPU.

**Why.** Self-deadlock. NMI watchdogs fire under load. The author's
own contract is unsatisfied.

**Fix.** Drop the lock entirely and use a per-CPU ring; or use
`arch_spin_trylock` and silently drop on contention; or check
`in_nmi()` and return without writing. The kernel printk
infrastructure itself routes NMI prints through a per-CPU
printk-safe buffer for exactly this reason — mirror that pattern.

#### R-027 — `console_total: u32` wraps after ~4 GB [P2 A]

**file:** `hackbot_console.c:62`

**What.** `console_total` is `unsigned int` and increments on every
write. After ~4 GB of cumulative log volume it wraps; the read path's
`avail = (console_total < BUF_SIZE) ? console_total : BUF_SIZE` then
returns near zero until the next full ring's worth of writes.

**Fix.** Use `u64`, or store `bool wrapped` and compute `avail` from
the wrap flag.

### `hackbot_files.c`

#### R-010 — Use-after-free on `task->files` [P1 V]

**file:** `hackbot_files.c:139-159`

**What.** Code path: `rcu_read_lock()`,
`task = pid_task(find_vpid(pid), PIDTYPE_PID)`,
`files = task->files`, then `spin_lock(&files->file_lock)`. RCU
read-side keeps `task_struct` alive, but `files_cachep` is allocated
*without* `SLAB_TYPESAFE_BY_RCU` (kernel's `kernel/fork.c` confirms
the flags are only `SLAB_HWCACHE_ALIGN | SLAB_PANIC | SLAB_ACCOUNT`).
`exit_files()` takes `task_lock`, sets `tsk->files = NULL`, and calls
`put_files_struct(files)` which `kmem_cache_free`s immediately.

**Why.** Use-after-free: between the `task->files` load and the
`spin_lock`, the struct can be freed. The kernel's own canonical
pattern (`fs/file.c`'s `fget_task` and friends) takes `task_lock`
precisely for this reason. KASAN catches it, but only if the race
fires.

**Fix.** Use `iterate_fd()` from `<linux/fdtable.h>` (encapsulates
the locking). If that doesn't fit, mirror `get_files_struct`'s
discipline: `task_lock(task); files = task->files; if (files)
atomic_inc(&files->count); task_unlock(task);`, work outside, then
`put_files_struct(files)`.

#### R-011 — `spin_lock(&files->file_lock)` over `fdt->max_fds` walk [P1 A]

**file:** `hackbot_files.c:158-171`

**What.** After acquiring `file_lock`, the loop iterates
`for (fd = 0; fd < fdt->max_fds && count < MAX_FD_ENTRIES; fd++)`.
`fdt->max_fds` can be 1 M+ on servers with raised `ulimit -n`. Even
when `count` saturates at `MAX_FD_ENTRIES`, the early-exit only
matters if the early entries are populated; the walk over empty
slots still runs.

**Why.** Spinlock hold time should be tens of microseconds. A 1 M-
entry walk under a hot per-task spinlock stalls the target process
and any other thread sharing the fdtable.

**Fix.** Bound the scan: `unsigned int max_scan = min(fdt->max_fds,
(unsigned int)MAX_FD_ENTRIES * 16);` and document the truncation. Or
drop the file_lock and use the RCU + `__fget_files_rcu` pattern the
kernel itself uses for similar walks.

#### R-026 — `snprintf` return-value misuse → potential stack leak [P2 A]

**file:** `hackbot_files.c:215-224`

**What.** `tlen = snprintf(trunc_msg, sizeof(trunc_msg), "[... truncated, showing %d of %d fds]\n", count, total_fds);`
— `snprintf` returns the would-have-been length when truncated. If
the formatted output exceeds 64 bytes, `tlen` is too large and the
subsequent `append_str` reads past the valid portion.

**Fix.** `tlen = min((int)sizeof(trunc_msg) - 1, tlen);` before the
append.

### `hackbot_kprobe.c`

#### R-006 — Slot reuse + missing blacklist guard [P1 A]

**file:** `hackbot_kprobe.c:71-105`

**What.** On `register_kprobe` failure the code zeros the slot via
`memset(&slots[free_slot], 0, ...)`. If `register_kprobe` had
partially linked the kprobe, the memset corrupts kprobe-core state.
The current `register_kprobe` cleans up on failure, so this is mostly
safe today — but the symbol-name pointer's lifetime is fragile: it
remains `slots[i].symbol` after success, and a future bug that lets
attach race with reuse turns into kprobe-core dereferencing
freed-equivalent memory. There is also no explicit
`kprobe_blacklisted` check before `register_kprobe`.

**Fix.** Allocate `symbol` per-slot independently (`kstrndup` /
`kstrdup_const`) and free on detach. Add an explicit
`within_kprobe_blacklist((unsigned long)kallsyms_lookup_name(symbol))`
check so blacklisted symbols return `-EINVAL` with a clear message.

#### R-024 — Embedded NUL in symbol bypasses dedup [P2 A]

**file:** `hackbot_kprobe.c:74-85`

**What.** Caller-supplied `symbol` with embedded NUL (e.g.,
`"foo\0bar"` with `len=7`) gets stored as `"foo"` plus
`strlen == 3`; dedup compares the new candidate against `len` bytes,
mismatching even when the displayed name is the same. Confused
deputy.

**Fix.** Reject NULs at the entry: `if (memchr(symbol, 0, len)) return -EINVAL;`.

#### R-039 — Unrate-limited `pr_info` on agent-driven attach/detach [P3 A]

**file:** `hackbot_kprobe.c:110,237,265`

**What.** The agent can attach + detach in a tight LLM-driven loop;
each prints to dmesg.

**Fix.** `pr_info_ratelimited`.

### `hackbot_device.rs`

#### R-014 — Unbounded user write — kvmalloc DoS [P1 V]

**file:** `hackbot_device.rs:122-140`

**What.** `iov.copy_from_iter_vec(&mut prompt, GFP_KERNEL)?` — no
length cap on the user-supplied prompt. Anyone with the device fd
(and the README's `chmod 666` example puts it in everyone's reach)
can `write(fd, ., INT_MAX)` and force kvmalloc to attempt gigabytes.

**Why.** Even when `kvmalloc` returns `ENOMEM`, intermediate memory
pressure can OOM-kill unrelated tasks. There is no rate limit, no
ulimit-equivalent, no capability check.

**Fix.** Cap at the top of `write_iter`:
`if iov.iter_count() > MAX_PROMPT_BYTES { return Err(EFBIG); }`,
where `MAX_PROMPT_BYTES` matches the local-inference preprocessing
buffer (e.g., 64 KiB or 2 KiB depending on the intended use case).

#### R-018 — Last-writer-wins race on global `RESPONSE` [P2 A]

**file:** `hackbot_device.rs:122-140`

**What.** `process_prompt` runs outside the lock; only the final copy
into `RESPONSE` takes the lock. Two concurrent writers race; readers
see a payload from one writer with the `len`/`ready` flags from the
other.

**Fix.** Move the response onto per-fd state, or reject concurrent
writes with `EBUSY` while a prompt is being processed.

### `hackbot_tools.rs`

#### R-013 — Sleeping allocation under `rcu::read_lock()` [P1 V]

**file:** `hackbot_tools.rs:117,167-209`

**What.** `tool_ps` takes an RCU read-side lock at `:167`, then walks
the task list calling `format_task` (`:117`). `format_task` does
many `output.push(..., GFP_KERNEL)` and
`output.extend_from_slice(..., GFP_KERNEL)` calls. `KVVec`'s push
calls `reserve` which can `kvmalloc(GFP_KERNEL)` and sleep. The
entire two-pass walk runs inside one RCU critical section.

**Why.** Sleeping inside `rcu_read_lock()` is a hard kernel rule
violation. `CONFIG_PROVE_RCU` triggers a sleeping-in-invalid-context
BUG; without it, you can extend grace periods indefinitely or
observe freed `task_struct`s after migration. `kernel::sync::rcu::Guard`
is a thin wrapper around `rcu_read_lock()` with no compile-time
may-sleep enforcement.

**Fix.** Snapshot the (pid, ppid, state, comm, mm-null) tuples into a
fixed-size on-stack array under RCU, drop the lock, then format the
output buffer outside. Or take RCU only around `(*current).next`
traversal and copy fields per-task before unlocking; the
`task_struct` is gone after unlock so all reads must finish under
RCU.

### `hackbot_forward.rs`

#### R-015 — Aliased `&mut [i32]` plus raw-pointer access [P1 A]

**file:** `hackbot_forward.rs:146-161,217-243`

**What.** Eight
`unsafe { core::slice::from_raw_parts_mut(inf.add(off), len) }`
calls construct overlapping `&mut [i32]` views into the same `inf`
allocation. After these mutable slices exist, the function then
reads/writes the same allocation through the original raw pointer
at `:217-218,231,243`.

**Why.** Under stacked-borrows, while a `&mut` exists the parent
allocation cannot be accessed via any other pointer, even into a
disjoint subrange. The optimizer can reorder loads/stores assuming
`noalias`. None of the unsafe blocks carry a `// SAFETY:` argument.

**Fix.** Pick one and stick to it: (a) split via `split_at_mut`
chains so the borrow checker witnesses disjointness; or (b) keep
raw pointers throughout and never materialize `&mut [i32]` — index
via `ptr.add(off + i)` with one SAFETY comment at the top of
`forward_token` covering provenance, lifetime
(`MODEL.lock()` held), and disjointness.

#### R-016 — Every `unsafe` block lacks `// SAFETY:` argument [P1 A]

**file:** `hackbot_forward.rs` (file-wide); same in `hackbot_tokenizer.rs`
(lines 47, 54, 75, 160, 174-176, 263-266, 354-356, 383-385) and
`hackbot_model.rs` (lines 158-166, 175, 179, 187, 218, 248, 253,
357-365, 372-374, 378, 395, 398, 402, 405, 414, 417, 425, 439, 445,
450, 455, 461).

**Why.** Per kernel-Rust style, every `unsafe` block must have a
one-line SAFETY argument citing (1) provenance, (2) lifetime / which
lock is held, (3) bounds / checked offset, (4) aliasing / no live
`&mut`. Without it, the next refactor breaks the invariant
silently.

**Fix.** One-pass cleanup. For each block, write the four-part
SAFETY argument that actually holds. The previous cleanup
(`9e49969`) addressed dead code but did not address SAFETY hygiene.

#### R-017 — Silent integer overflow in Q16.16 inner loops [P1 A]

**file:** `hackbot_forward.rs:179,234,246`

**What.** Line 179: `x[c] = w * scale;` where `w: i32` (sign-extended
i8) and `scale: i32` is read directly from the firmware blob with no
clamp; product stored in `i32`. Line 246:
`xb[h*head_dim+d] = (acc >> 16) as i32;` where `acc: i64` accumulates
`att[p] as i64 * v_val as i64` over up to 256 positions × 64 head
dims; `acc >> 16` can exceed `i32::MAX` and is truncated by `as i32`.
Line 234 has the same shape with `(dot >> 19) as i32`.

**Why.** Release builds wrap two's-complement; the math is wrong on
prompt-controlled paths (large prompts → large attention sums).
Debug or UBSAN builds panic. Either way, wrong answer.

**Fix.** Use `wrapping_mul`/`saturating_mul` deliberately and
document the Q16.16 saturation policy. For i64→i32 truncation:
`let r = (acc >> 16).clamp(i32::MIN as i64, i32::MAX as i64) as i32;`.

#### R-025 — `weights_len = data_len - weights_off` with no `checked_sub` [P2 A]

**file:** `hackbot_forward.rs:118-119`

**What.** Bare subtraction relies on a distant invariant in
`parse_and_store_model` (`hackbot_model.rs:186-190`).

**Fix.** `let weights_len = slot.data_len.checked_sub(slot.weights_off).ok_or(EINVAL)?;`
and document the invariant on the field declaration in `ModelSlot`.

### `hackbot_model.rs`

#### R-029 — 8.7 KiB on-stack `[LayerRef::ZERO; 32]` under `MODEL.lock()` [P3 A]

**file:** `hackbot_model.rs:265`

**What.** `LayerRef` is ~272 bytes × 32 layers = ~8.7 KiB on stack
inside `parse_and_store_model`, which is called from
`load_model_if_needed` while holding `MODEL.lock()`. Combined with
caller frames this approaches the 16 KiB kernel stack limit.

**Fix.** `KBox<[LayerRef; MODEL_MAX_LAYERS]>` or `KVVec<LayerRef>`
for the temporary; copy out before storing into `slot.layers`. Apply
the same scrutiny to the agent inference frame's nested fixed-size
arrays (`tokens [u32; 256]` 1 KiB + `response_buf [u8; 2048]` 2 KiB +
preproc/concat/decode buffers).

### `hackbot_net.rs`

#### R-022 — JSON escape silently drops control chars 0x01–0x1F [P2 A]

**file:** `hackbot_net.rs:177`

**What.** Match arm `c if c < 0x20 => {}` emits nothing for control
bytes other than `\n` `\r` `\t`. The user prompt (raw bytes from
`write_iter`) can contain these; the LLM sees a different prompt
than the user sent.

**Fix.** Emit `\u00XX` for any control char per RFC 8259, or reject
the prompt at the device boundary.

### `hackbot_vllm.rs`

#### R-023 — `-(e.to_errno())` overflow on `i32::MIN` [P2 A]

**file:** `hackbot_vllm.rs:81,86`

**What.** `let code = -(e.to_errno() as isize) as usize;` and a
similar negation in a match. Real errno values stay in `-1..-4095`,
so this is not exploitable today, but the negation panics in debug
on `i32::MIN`.

**Fix.** `let code = e.to_errno().unsigned_abs() as usize;` and
match on the original signed value: `match e.to_errno() { -19 => ..., -110 => ..., -111 => ..., _ => ... }`.

### `hackbot_tokenizer.rs`

#### R-021b — DEBUG `pr_info!` in inference hot path [P2 A]

**file:** `hackbot_tokenizer.rs:391-396,419-422,431-432,448-450`;
also `hackbot_agent.rs:81-95,184,280,292-301`

**What.** `// DEBUG`-tagged `pr_info!` calls fire per-token /
per-prefill on every inference run, plus a single-token sanity test
runs unconditionally on every `agent_loop_local` call
(`hackbot_agent.rs:81-95`).

**Why.** dmesg ring is small; one inference flushes anything older.
Worse, `hackbot_console.c` mirrors all printk into the agent's own
`dmesg` tool output — these debug lines pollute every subsequent
`<tool>dmesg</tool>` result that the LLM reasons over. Self-feedback
noise loop.

**Fix.** Gate behind `cfg(debug_assertions)` or a dedicated `pr_dbg!`
macro that compiles out in release. Delete the unconditional sanity
test or move it to a kunit test.

### Build and test posture

#### R-030 — `test.sh` never greps `dmesg` for sanitizer reports [P1 A]

**file:** `test.sh` (entire file)

**What.** Tests grep for positive success markers. None grep for
`BUG:`, `WARNING:`, `Call Trace`, `KASAN:`, `UBSAN:`, `KCSAN:`,
`general protection fault`, `unable to handle`, `lockdep`, or
`bad: scheduling while atomic`. A KASAN report fires asynchronously
after the offending op; a test that ran the call, got a response, and
unloaded — while KASAN screamed in dmesg — passes.

**Why.** This is the single biggest gap between "we ran sanitizers"
and "the sanitizers caught nothing." The whole point of opting into
KASAN/UBSAN/KCSAN is that the test harness checks for their reports.

**Fix.** Add a `check_no_kernel_errors` helper that runs after every
test and greps `dmesg` (since the per-test marker) for
`BUG:|WARNING:|Call Trace|KASAN:|UBSAN:|KCSAN:|general protection|unable to handle|scheduling while atomic`.
Failure increments the test failure counter.

#### R-031 — `check-kconfig` checks parent options only [P1 A]

**file:** `Makefile:67-69`

**What.** Verifies `CONFIG_KASAN`, `CONFIG_UBSAN`, `CONFIG_KCSAN`,
`CONFIG_LOCKDEP`, `CONFIG_PROVE_LOCKING`, `CONFIG_DEBUG_ATOMIC_SLEEP`,
`CONFIG_FORTIFY_SOURCE`, `CONFIG_SCHED_STACK_END_CHECK`. Missing the
sub-options that actually do the work:
- `CONFIG_KASAN_GENERIC` (or `_SW_TAGS` / `_HW_TAGS` — one is
  required for `KASAN_SANITIZE := y` to instrument)
- `CONFIG_UBSAN_BOUNDS`, `CONFIG_UBSAN_SHIFT` (parent does nothing
  without these)
- `CONFIG_KCSAN_REPORT_VALUE_CHANGE_ONLY` (otherwise KCSAN is silent)
- `CONFIG_DEBUG_KERNEL` (gates many of the above)

**Fix.** Expand the config list. Split into "required parent" and
"required sub-option" buckets so the missing-but-needed delta is
visible at a glance.

#### R-032 — `make test` not idempotent [P1 A]

**file:** `Makefile:95-101`

**What.** `test` depends on `load`, which is `sudo insmod hackbot.ko`.
If the module is already resident (previous failed run, manual
insmod), `insmod` returns `EEXIST` and the target aborts. No `EXIT`
trap; if any step between `load` and the final `unload` fails, the
module stays loaded.

**Fix.** Make `load` idempotent
(`sudo rmmod hackbot 2>/dev/null || true; sudo insmod hackbot.ko`)
or rewrite `test` as a single shell stanza with
`set -e; ...; trap 'sudo rmmod hackbot 2>/dev/null || true' EXIT`.

#### R-033 — `kernel_version.rs` regenerates every build [P1 A]

**file:** `Kbuild:38-46`

**What.** `$(obj)/kernel_version.rs: FORCE` always re-fires; the
recipe overwrites with `>` unconditionally. mtime advances every
build → `hackbot_main.o` is rebuilt every time. `make -jN` can race
on `>` to the same file.

**Fix.** Standard kbuild idiom — write to `$@.tmp`, `cmp -s` against
the existing file, `mv` only when changed. Preserves incremental
compilation.

#### R-034 — `make check-warn` ignores warnings [P2 A]

**file:** `Makefile:51`

**What.** `W=1` enables additional warnings but doesn't fail the
build; exit code is 0 even when the wall is yellow.

**Fix.** Add `KCFLAGS=-Werror` (or selectively
`-Werror=<cat>`). Document any warning intentionally tolerated.

#### R-035 — `test_tools` skips `loadavg` and `trace` [P2 A]

**file:** `test.sh:283-313`

**What.** Hardcoded list covers `ps, mem, dmesg, files, kprobe` —
five tools. Missing: `loadavg`, `trace` (and trace's subcommands
`sched`, `syscall`, `io`, `tokens`, `ngram`, `reset`, `list`). The
README and Makefile claim "all 6 tools"; truth is 7 and only 5 are
tested.

**Fix.** Add the missing entries. Fix the docstring to match
reality.

#### R-036 — No real test for n-gram / kprobe roundtrip / patrol shutdown / FPU [P2 A]

**file:** `test.sh` (file-wide)

**What.** Recently-added surfaces have no real assertions:
- **n-gram**: no test reads `trace ngram stats` or
  `trace ngram alerts`; no test injects synthetic anomalous traffic
  and asserts that surprise rises.
- **kprobe roundtrip**: only greps the LLM's prose for keywords, not
  `/sys/kernel/debug/kprobes/list` or a hit count. No assertion that
  attach + detach leaves the module clean.
- **patrol shutdown timing**: just `rmmod` and `sleep 1`; no timed
  assertion despite the prereq comment admitting shutdown can take
  60 s if the patrol is mid-vLLM-call.
- **FPU forward correctness**: no test forces `INFERENCE_MODE=1` and
  asserts the output is plausible.

**Fix.** Add `test_ngram`, `test_kprobe_lifecycle`,
`test_shutdown_timing`, `test_fpu_inference` (skip if no firmware).

#### R-037 — Stale vLLM IP in docs [P3 A]

**file:** `TESTING.md:12,56`; `test.sh:17`

**What.** Docs say `100.66.136.70:8000`. Code at
`hackbot_config.rs:11-13` says `100.103.180.11:8000`.

**Fix.** Update the docs and stop hard-coding the IP in three
places — point readers at `VLLM_ADDR` in `hackbot_config.rs`.

#### R-038 — README status line is non-monotonic [P3 A]

**file:** `README.md:199`

**What.** "Through Step 2k of the in-kernel LLM agent track plus the
kernel-as-language anomaly detection layer. ... Step 2j
(cross-subsystem anomaly demo) is the next planned milestone."
Current = 2k, next = 2j — next milestone has a lower number than
current.

**Fix.** Reconcile against `docs/PLAN.md` and pick a consistent step
number. Likely intent: through 2k, next planned is 2l (or rename
2j).

#### R-040 — No defensive guard against float math leaking to other TUs [P3 A]

**file:** `Kbuild:33`

**What.** `CFLAGS_hackbot_fpu.o := -mhard-float -msse -msse2` is
correctly file-scoped. Today only `hackbot_fpu.c` uses float. If
anyone adds float math to another C file in `hackbot-y` tomorrow,
`-mhard-float` won't apply and the compiler will silently fall back
to softfloat lib calls, and any FPU instructions emitted will run
outside `kernel_fpu_begin/end`, corrupting user FPU state.

**Why.** This is the most dangerous build-system invariant in the
module. A regression is silent data corruption, not a compile error.

**Fix.** Add a build-time grep guard: a Kbuild rule that fails if
any C file other than `hackbot_fpu.c` contains `float`, `double`,
or `_mm_` outside comments. Or `CFLAGS_REMOVE_<file>.o := -mhard-float -msse -msse2`
boilerplate for every other C TU as documentation.

---

## Suggested triage order

A possible sequencing — not a project plan. Each phase shrinks the
next.

1. **Trace hot path.** R-001, R-002, R-003, R-004, R-005, R-021,
   R-028. All in `hackbot_trace.c` + `hackbot_tokenizer.c` +
   `hackbot_ngram.c`. Single coherent PR. Biggest production-risk
   reduction; turns the hot path into something that survives
   `KCSAN=y` and uptime > 6 h.
2. **FPU bracketing.** R-007, R-008. Tile the forward pass; remove
   `pr_info` from inside FPU regions.
3. **Files tool UAF and bounded walk.** R-010, R-011. Use
   `iterate_fd()` if it fits.
4. **Rust safety basics.** R-013 (RCU sleep), R-014 (unbounded
   write). Both small, both serious.
5. **Test harness sanitizer assertions.** R-030, R-031. Without
   these, you can't tell whether (1)–(4) are real fixes.
6. **Build idempotence and doc accuracy.** R-032, R-033, R-037,
   R-038. Trivial; closes the credibility gap.
7. **Rust safety hygiene pass.** R-015, R-016, R-017, and the
   remaining P2/P3 in `hackbot_forward.rs`, `hackbot_tokenizer.rs`,
   `hackbot_model.rs`, `hackbot_vllm.rs`.

After each phase, re-run `make check-all` and `make test-full` and
verify `dmesg` is clean.

---

## Limits of this review

- **Lower-severity items (P2/P3) are mostly agent-reported, not
  re-verified.** Spot-check before patching. Headline P1s
  (R-001/R-002/R-009/R-010/R-013/R-014) were verified directly.
- **No runtime testing was performed.** All findings are static. A
  follow-up that runs the module under
  `CONFIG_KASAN=y CONFIG_UBSAN=y CONFIG_KCSAN=y CONFIG_PROVE_RCU=y`
  on a real workload would surface more — but only after R-030 lands
  (otherwise sanitizer fires won't be detected by the suite).
- **The agents had no access to runtime logs or production
  workloads.** The "uptime > 6 h" estimate for R-001 assumes a
  particular event rate. The actual fuse depends on traffic.
- **The viz subsystem (`server-rs/`, `frontend/`) was not reviewed.**
  Out of scope for this pass.
- **Items I judged speculative or already-handled were dropped from
  the table.** Examples that did *not* make the cut: minor stylistic
  preferences, items that the recent `9e49969` cleanup already
  addressed, and a handful of P3-level "consider documenting"
  suggestions that don't translate into a concrete patch.

---

## Cross-references

- `docs/IMPLEMENTATION_REPORT.md` — historical context for which step
  added each surface. Useful for blame / archaeology.
- `docs/PLAN.md` — the canonical step list; R-038 needs reconciling
  against this.
- `RELEASE_NOTES_kmod-v0.1.md` — what was advertised at this tag.
  Several P3 doc-accuracy findings (R-037, R-038) have implications
  for v0.2's release notes.
