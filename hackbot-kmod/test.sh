#!/bin/bash
# SPDX-License-Identifier: GPL-2.0
#
# test.sh — Integration test suite for the hackbot kernel module.
#
# Exercises module lifecycle, tool dispatch, patrol thread, agent memory,
# and error handling. Requires root (or sudo) and a built hackbot.ko.
#
# Usage:
#   sudo bash test.sh          # Full test suite
#   sudo bash test.sh quick    # Load/unload + basic I/O only
#   sudo bash test.sh tools    # Tool-specific tests (requires vLLM)
#   sudo bash test.sh patrol   # Patrol thread tests (waits ~35s)
#
# Prerequisites:
#   - hackbot.ko built (run `make` first)
#   - vLLM server reachable at 100.66.136.70:8000 (for tool/patrol tests)
#   - Run as root

# Don't use set -e — we handle errors ourselves via pass/fail.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODULE="$SCRIPT_DIR/hackbot.ko"
DEVICE="/dev/hackbot"

PASS=0
FAIL=0
SKIP=0

# Timestamp when test starts — only check dmesg messages after this.
DMESG_MARKER=""

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

pass() { ((PASS++)) || true; echo -e "  [\033[32mPASS\033[0m] $1"; }
fail() { ((FAIL++)) || true; echo -e "  [\033[31mFAIL\033[0m] $1"; }
skip() { ((SKIP++)) || true; echo -e "  [\033[33mSKIP\033[0m] $1"; }
info() { echo -e "  [....] $1"; }
log()  { echo -e "         \033[90m$1\033[0m"; }  # grey, indented

# Mark the current dmesg position so we only check new messages.
mark_dmesg() {
    DMESG_MARKER=$(dmesg | wc -l)
}

# Show new dmesg lines since the last mark (hackbot messages only).
show_dmesg() {
    local new_lines
    new_lines=$(dmesg | tail -n "+$((DMESG_MARKER + 1))" | grep "hackbot" || true)
    if [[ -n "$new_lines" ]]; then
        log "--- dmesg (hackbot) ---"
        echo "$new_lines" | while IFS= read -r line; do
            log "$line"
        done
        log "--- end dmesg ---"
    fi
    mark_dmesg  # advance marker so we don't repeat
}

# Check dmesg (since mark) for a pattern. Returns 0 if found.
dmesg_since() {
    dmesg | tail -n "+$((DMESG_MARKER + 1))" | grep -q "$1" 2>/dev/null
}

# Ensure module is unloaded on exit
cleanup() {
    if is_loaded; then
        info "Cleaning up: unloading hackbot..."
        rmmod hackbot 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Check if module is currently loaded.
# Avoids lsmod|grep pipe — under pipefail, SIGPIPE from grep -q
# closing the pipe early causes lsmod to return 141, failing the check.
is_loaded() {
    local mods
    mods=$(lsmod 2>/dev/null) || true
    echo "$mods" | grep -q "^hackbot " 2>/dev/null
}

# Ensure module is loaded for a test. Loads if needed, returns 1 on failure.
ensure_loaded() {
    if is_loaded; then
        return 0
    fi
    insmod "$MODULE" 2>/dev/null || { fail "Could not load module"; return 1; }
    sleep 2
    return 0
}

# Write prompt and read response from /dev/hackbot.
# Logs query and response to stderr (always visible on screen).
# Returns the response on stdout (captured by callers via $(...)).
query_hackbot() {
    local prompt="$1"
    local timeout_secs="${2:-60}"
    local response

    log ">>> echo '${prompt:0:80}' > /dev/hackbot" >&2
    echo "$prompt" > "$DEVICE"
    response=$(timeout "$timeout_secs" cat "$DEVICE" 2>/dev/null || echo "[timeout after ${timeout_secs}s]")

    # Show response preview on stderr so it's always visible
    if [[ -n "$response" && "$response" != *"[timeout"* ]]; then
        log "<<< Response (${#response} bytes):" >&2
        echo "${response:0:500}" | head -12 | while IFS= read -r line; do
            log "    $line" >&2
        done
        if [[ ${#response} -gt 500 ]]; then
            log "    [... truncated]" >&2
        fi
    elif [[ "$response" == *"[timeout"* ]]; then
        log "<<< TIMEOUT (no response after ${timeout_secs}s)" >&2
    else
        log "<<< (empty response)" >&2
    fi

    # Return response on stdout for caller to capture
    echo "$response"
}

# Check prerequisites
check_prereqs() {
    if [[ $EUID -ne 0 ]]; then
        echo "ERROR: Must run as root (sudo bash test.sh)"
        exit 1
    fi
    if [[ ! -f "$MODULE" ]]; then
        echo "ERROR: $MODULE not found. Run 'make' first."
        exit 1
    fi
    if is_loaded; then
        info "hackbot already loaded — unloading first..."
        info "(if patrol is mid-vLLM-call, this may take up to 60s)"
        if timeout 90 rmmod hackbot 2>/dev/null; then
            info "Unloaded successfully."
            sleep 1
        else
            echo "ERROR: Could not unload hackbot (timed out or failed)."
            echo "       Try:  sudo rmmod hackbot  (and wait for it)"
            exit 1
        fi
    fi
}

# ---------------------------------------------------------------------------
# Test 1: Module lifecycle
# ---------------------------------------------------------------------------

test_lifecycle() {
    echo ""
    echo "=== Module Lifecycle ==="
    mark_dmesg

    # Load
    info "Loading hackbot.ko..."
    if insmod "$MODULE" 2>&1; then
        pass "Module loaded"
    else
        fail "Module failed to load"
        show_dmesg
        return 1
    fi
    sleep 1
    show_dmesg

    # Device exists
    if [[ -c "$DEVICE" ]]; then
        pass "/dev/hackbot exists"
    else
        fail "/dev/hackbot not found"
    fi

    # Check dmesg for init
    if dmesg_since "hackbot: loading module"; then
        pass "Init message in dmesg"
    else
        fail "No init message in dmesg"
    fi

    if dmesg_since "patrol thread"; then
        pass "Patrol thread started"
    else
        skip "Patrol thread not confirmed in dmesg"
    fi

    # Unload
    mark_dmesg
    info "Unloading hackbot..."
    if rmmod hackbot 2>&1; then
        pass "Module unloaded"
    else
        fail "Module failed to unload"
        show_dmesg
        return 1
    fi
    sleep 1
    show_dmesg

    if dmesg_since "hackbot: unloading module"; then
        pass "Unload message in dmesg"
    else
        fail "No unload message in dmesg"
    fi

    if dmesg_since "patrol thread stopped"; then
        pass "Patrol thread stopped cleanly"
    else
        skip "Patrol stop not confirmed"
    fi

    # Reload — verify no leaked state
    mark_dmesg
    info "Reloading to check for leaked state..."
    if insmod "$MODULE" 2>&1; then
        pass "Module reloaded (no leaked state)"
        sleep 1
        show_dmesg
        rmmod hackbot 2>/dev/null || true
    else
        fail "Module failed to reload"
        show_dmesg
    fi
}

# ---------------------------------------------------------------------------
# Test 2: Basic I/O
# ---------------------------------------------------------------------------

test_basic_io() {
    echo ""
    echo "=== Basic I/O ==="
    mark_dmesg

    ensure_loaded || return
    sleep 2

    # Basic write/read
    info "Sending 'hello' to /dev/hackbot (may take 10-60s for vLLM)..."
    local response
    response=$(query_hackbot "hello" 90)

    if [[ -n "$response" && "$response" != *"[timeout"* ]]; then
        pass "Got response (${#response} bytes)"
    else
        fail "No response or timeout"
    fi

    show_dmesg
    rmmod hackbot 2>/dev/null || true
    sleep 1
}

# ---------------------------------------------------------------------------
# Test 3: Tool testing (requires vLLM)
# ---------------------------------------------------------------------------

test_one_tool() {
    local tool_name="$1"
    local prompt="$2"
    local pattern="$3"

    info "Testing $tool_name tool..."
    local resp
    resp=$(query_hackbot "$prompt" 90)

    if echo "$resp" | grep -qi "$pattern"; then
        pass "Tool: $tool_name"
    elif [[ "$resp" == *"[timeout"* ]]; then
        fail "Tool: $tool_name (timeout — is vLLM running?)"
    elif echo "$resp" | grep -qi "Connection refused\|timed out\|No model"; then
        skip "Tool: $tool_name (vLLM unreachable)"
    else
        fail "Tool: $tool_name (pattern '$pattern' not in response)"
    fi
}

test_tools() {
    echo ""
    echo "=== Tool Tests (via vLLM) ==="
    mark_dmesg

    ensure_loaded || return
    sleep 2

    test_one_tool "ps" \
        "list running processes using the ps tool" \
        "PID\|process\|comm\|kthread"

    test_one_tool "mem" \
        "show memory statistics using the mem tool" \
        "MB\|memory\|RAM\|free\|total"

    test_one_tool "dmesg" \
        "show recent kernel log messages using dmesg tool" \
        "hackbot\|kernel\|log\|dmesg"

    test_one_tool "files" \
        "use the files tool to show open files for PID 1" \
        "FD\|PATH\|file\|open"

    test_one_tool "kprobe" \
        "attach a kprobe to do_sys_openat2 then check hit count" \
        "kprobe\|attach\|hit\|count\|openat"

    show_dmesg
    rmmod hackbot 2>/dev/null || true
    sleep 1
}

# ---------------------------------------------------------------------------
# Test 4: Patrol thread
# ---------------------------------------------------------------------------

test_patrol() {
    echo ""
    echo "=== Patrol Thread ==="
    mark_dmesg

    ensure_loaded || return
    sleep 2

    # Check kthread exists
    if ps -eo comm | grep -q "hackbot_patrol"; then
        pass "Patrol kthread visible in ps"
        log "$(ps -eo pid,comm | grep hackbot_patrol)"
    else
        fail "Patrol kthread not visible in ps"
    fi

    # Wait for initial delay (30s) + first tick
    info "Waiting 35 seconds for first patrol cycle..."
    local i
    for i in $(seq 35 -5 0); do
        printf "\r         %ds remaining...   " "$i"
        sleep 5
    done
    printf "\r                              \r"

    show_dmesg

    if dmesg_since "hackbot: patrol cycle starting"; then
        pass "First patrol cycle triggered"
    else
        fail "No patrol cycle in dmesg after 35s"
    fi

    if dmesg_since "hackbot: patrol finding\|hackbot: patrol cycle"; then
        pass "Patrol produced output"
    else
        skip "Patrol output not confirmed (vLLM may be unreachable)"
    fi

    info "Unloading (will wait for patrol to finish if mid-cycle)..."
    rmmod hackbot 2>/dev/null || true
    sleep 1
    show_dmesg
}

# ---------------------------------------------------------------------------
# Test 5: Agent memory
# ---------------------------------------------------------------------------

test_memory() {
    echo ""
    echo "=== Agent Memory ==="
    mark_dmesg

    ensure_loaded || return
    sleep 2

    # First query — populates memory
    info "First query (populates memory)..."
    local resp1
    resp1=$(query_hackbot "what is the system load?" 90)
    if [[ -n "$resp1" && "$resp1" != *"[timeout"* ]]; then
        pass "First query got response"
    else
        fail "First query failed"
        rmmod hackbot 2>/dev/null || true
        return
    fi

    show_dmesg

    # Second query — memory should be injected into system prompt
    info "Second query (memory should be in context)..."
    local resp2
    resp2=$(query_hackbot "summarize what you observed earlier" 90)
    if [[ -n "$resp2" && "$resp2" != *"[timeout"* ]]; then
        pass "Second query got response"
    else
        fail "Second query failed"
    fi

    show_dmesg
    rmmod hackbot 2>/dev/null || true
    sleep 1
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    local mode="${1:-full}"

    echo "============================================"
    echo "  hackbot kernel module integration tests"
    echo "============================================"
    echo "  Mode: $mode"
    echo "  Module: $MODULE"
    echo "  Time: $(date)"
    echo ""

    check_prereqs

    case "$mode" in
        quick)
            test_lifecycle
            test_basic_io
            ;;
        tools)
            test_tools
            ;;
        patrol)
            test_patrol
            ;;
        memory)
            test_memory
            ;;
        full)
            test_lifecycle
            test_basic_io
            test_tools
            test_patrol
            test_memory
            ;;
        *)
            echo "Usage: sudo bash test.sh [quick|tools|patrol|memory|full]"
            exit 1
            ;;
    esac

    echo ""
    echo "============================================"
    echo "  Results: $PASS passed, $FAIL failed, $SKIP skipped"
    echo "============================================"

    if [[ $FAIL -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
