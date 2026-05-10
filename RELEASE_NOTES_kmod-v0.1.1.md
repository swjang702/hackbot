# hackbot kmod — v0.1.1

**Tag**: `kmod-v0.1.1`
**Date**: 2026-05-09
**Scope**: in-kernel LLM agent — `hackbot-kmod/` and supporting `tools/`
**Predecessor**: `kmod-v0.1.0`

A consolidation release. No new features; 47 commits closing every finding
from the v0.1 code review (`docs/REVIEW_v0.1.md`) plus seven follow-up
hardening fixes that surfaced during the review work. The trace hot path,
n-gram learner, FPU forward, files tool, kprobe manager, console capture,
and Rust safety hygiene are all materially better than at v0.1.0.

## Summary

41 review findings closed (40 fixed in code, 1 closed after audit confirmed
the fix was already in place). 7 Linus-flagged follow-ups closed. Run
`git log kmod-v0.1.0..kmod-v0.1.1` for the full series.

The review doc itself (`docs/REVIEW_v0.1.md`) is part of this release and
includes a Status column mapping every finding ID to its closing commit
SHA — that's the canonical fix-trail.

## What changed by area

### Trace hot path (`hackbot_trace.c` / `hackbot_ngram.c` / `hackbot_tokenizer.c`)

- **OOB write fixed.** Signed `%` on `atomic_inc_return` ring head no longer
  goes negative after counter wrap (R-001).
- **SMP scaling unbroken.** Three global `raw_spinlock`s in tracepoint
  callbacks replaced with per-CPU `seqcount`-protected feature vectors
  (R-002).
- **Probe-context `pr_info` removed.** Tokenizer and n-gram debug prints
  no longer fire from `sched_switch` / `sys_enter` / `block_rq_complete`
  callbacks (R-003 / R-021).
- **Tracepoint init is atomic.** Partial registration now rolls back via a
  `goto err_*` chain instead of silently leaving the agent half-blind
  (R-020).
- **Accounted vs un-accounted I/O latency** distinguished. `rq->start_time_ns
  == 0` no longer poisons the latency histogram, window, or duration class
  (R-019).
- **`tasks[]` slot publish ordered.** Writers `smp_store_release(pid)` after
  the comm/count fields; readers `smp_load_acquire` and skip pid==0 slots
  (F-006).
- **`task->comm` torn reads acknowledged.** `data_race(memcpy(...))` makes
  KCSAN happy; the policy comment cites the upgrade path
  (`__get_task_comm()` is not module-exported in 6.19) (R-028b).
- **N-gram halving moved out of probe context.** Per-event `atomic_inc`
  with a workqueue-driven decay loop replaces the in-probe halving and
  half-Hogwild discipline (R-005).
- **N-gram alert wait-queue lifetime decoupled.** `alert_wq` is a
  module-static `DECLARE_WAIT_QUEUE_HEAD`; `kfree(state)` no longer
  races a sleeper's `finish_wait` (R-012).
- **`find_tracepoint_cb` short-circuit.** Skips strcmp once a match is
  found (F-007).

### FPU forward & sampler (`hackbot_fpu.c`)

- **Forward pass tiled.** ~100 ms `kernel_fpu_begin/end` window broken
  into ~200 ≤1 ms tiles per token; `cond_resched()` between layers and
  per logits chunk. No more per-token system stutters (R-007).
- **No `pr_info` inside FPU windows** (R-008).
- **Sampler tiled.** `hackbot_fpu_get_next_token` decomposed into
  ~50 µs windows of 4096 logits each plus a small final softmax+sample
  tile (F-003).

### Console (`hackbot_console.c`)

- **NMI-safe capture.** `in_nmi()` branch uses `raw_spin_trylock_irqsave`
  and drops the message on contention; non-NMI keeps the blocking lock.
  Closes the self-deadlock if NMI watchdog fires while the same CPU
  holds the lock (R-009).
- **`console_total` widened to u64** to survive >4 GB cumulative log
  volume (R-027).

### Files tool (`hackbot_files.c`)

- **`task->files` UAF fixed.** Pin `task` via `get_task_struct`, take
  `task_lock` for the duration of the bounded fdtable walk, drop locks,
  then `put_task_struct`. Matches the kernel's own `fget_task` pattern
  (`fs/file.c:1117-1147`) (R-010).
- **`fdt->max_fds` walk bounded** to `MAX_FD_ENTRIES * 16` slots,
  independent of target `ulimit -n` (R-011).
- **`snprintf` truncation length clamped** before `append_str` to avoid
  reading past the formatted region (R-026).

### Rust safety (`hackbot_*.rs`)

- **`tool_ps` no longer sleeps under RCU.** Snapshot pid/ppid/state/comm
  into a heap-allocated KVec under RCU, drop the lock, then format
  outside (R-013).
- **`/dev/hackbot` write capped at 64 KiB.** Returns `EFBIG` on oversized
  writes before any kvmalloc (R-014).
- **`/dev/hackbot` writers serialized via `EBUSY`.** Concurrent writers
  no longer race on the global RESPONSE; one wins, others get -EBUSY
  (R-018).
- **Aliased `&mut [i32]` removed from `hackbot_forward.rs`.** Raw-pointer
  pattern with one-time `wrapping_add` aliases; no persistent slice
  borrows over the inference buffer; no stacked-borrows UB (R-015).
- **Q16.16 saturation policy.** All i64→i32 truncations clamp to
  `i32::MIN..=MAX` (R-017, F-004).
- **FFI `usize as i32` casts checked.** `i32::try_from(...)?` at every
  FFI boundary; `EINVAL` propagates instead of silent truncation (F-005).
- **SAFETY arguments for every unsafe block.** `hackbot_forward.rs`
  (R-016), `hackbot_tokenizer.rs` (F-001), `hackbot_model.rs` (F-002).
- **`weights_len` uses `checked_sub`** with `EINVAL` on the impossible-
  but-fragile invariant (R-025).
- **JSON escape emits `\u00XX`** for control bytes 0x01-0x1F (was: silent
  drop) (R-022).
- **`-(e.to_errno())` overflow** avoided via `unsigned_abs()` and
  match-on-signed (R-023).
- **`LayerRef` scratch heap-allocated** to keep `parse_and_store_model`'s
  ~8.7 KiB temporary off the kernel stack (R-029).
- **Debug `pr_info!` removed from inference hot path** (R-021b).

### Kprobe (`hackbot_kprobe.c`)

- **Symbol storage decoupled from slot lifetime.** `kstrndup` + `kfree`
  with strict order: `unregister_kprobe → kfree(symbol) → NULL`. Better
  errno-mapped diagnostics for register failures (R-006).
- **Embedded NUL in symbol input rejected** with `-EINVAL` (R-024).
- **`pr_info` rate-limited** on agent-driven attach/detach loops (R-039).

### Test harness (`test.sh`, `Makefile`, `Kbuild`)

- **dmesg fault-marker scan after every test.** Curated regex catches
  `BUG: / Oops: / Kernel panic / KASAN: / UBSAN: / KCSAN: / KFENCE: /
  scheduling while atomic / sleeping in invalid context /` etc. The
  suite can no longer report green while sanitizers fire (R-030).
- **`check-kconfig` validates sanitizer backends.** `CONFIG_KASAN=y` is
  inert without `KASAN_GENERIC|SW_TAGS|HW_TAGS`; the check now requires
  one (R-031).
- **`make test` idempotent and self-cleaning.** `lsmod` precheck +
  `trap rmmod EXIT` (R-032).
- **`make` is incremental again.** `kernel_version.rs` writes via
  `cmp -s + mv` so unchanged content preserves mtime (R-033).
- **`make check-warn` fails on warnings** via `KCFLAGS=-Werror` (R-034).
- **`test_tools` covers all seven tools** including `loadavg` and trace
  subcommands (R-035).
- **New tests:** `test_ngram`, `test_kprobe_lifecycle`,
  `test_shutdown_timing`, `test_fpu_inference` (R-036).
- **Build refuses to compile if non-FPU C files use float math.** A
  defensive grep guard runs before the kernel build (R-040).

### Doc accuracy

- **vLLM IP references** point at `hackbot_config.rs::VLLM_ADDR` instead
  of the stale hardcoded address (R-037).
- **Step-numbering wording** clarified to make the parallel sub-step
  structure explicit (R-038).

## Backwards-compatibility implications

- `write(/dev/hackbot, ...)` now returns:
  - **`-EFBIG`** on writes > 64 KiB (was: silent kvmalloc DoS).
  - **`-EBUSY`** if another writer is mid-prompt (was: silent
    last-writer-wins corruption).
  Userspace callers that retried on `-1` already handle these cleanly;
  callers that didn't should add a small backoff loop on `EBUSY`.
- `make test` is rewritten as a self-contained shell stanza with `set
  -e` and `trap EXIT`. Targets `load` and `unload` are unchanged for
  explicit invocation.

## Verification

```
cd hackbot-kmod
make                           # incremental; will be silent on no-change
make check-all                 # sparse + W=1 -Werror
make check-kconfig             # validates sanitizer backends
make test-full                 # full integration suite with dmesg fault scan
```

## Known limitations / not in v0.1.1

- **Step 2j** (cross-subsystem anomaly demo) is still the next planned
  feature, deferred to `kmod-v0.2.0`.
- **Viz subsystem** is unchanged at `viz-v0.1.0`.
- **Model binaries** still produced locally via `tools/export_hackbot.py`
  / `tools/export_hackbot_fp16.py`; no firmware shipped in the tag.

## References

- `docs/REVIEW_v0.1.md` — the v0.1 code review with status column for
  every finding.
- `RELEASE_NOTES_kmod-v0.1.md` — predecessor release notes.
- `git log kmod-v0.1.0..kmod-v0.1.1` — full commit series.
- `git log --grep='R-NNN'` / `git log --grep='F-NNN'` — find the fix
  commit for any specific finding.
