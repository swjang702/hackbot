#!/bin/bash
# SPDX-License-Identifier: GPL-2.0
#
# test_tokenizer.sh — Milestone 0 smoke test for the semantic tokenizer.
#
# Tests that kernel events are being tokenized correctly by checking
# dmesg output from the first 10 debug prints.
#
# Usage: sudo bash test_tokenizer.sh
#
# Prerequisites: hackbot.ko built (run `make` first)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODULE="$SCRIPT_DIR/hackbot.ko"
DEVICE="/dev/hackbot"

RED='\033[31m'
GREEN='\033[32m'
YELLOW='\033[33m'
RESET='\033[0m'

pass() { echo -e "  [${GREEN}PASS${RESET}] $1"; }
fail() { echo -e "  [${RED}FAIL${RESET}] $1"; FAILURES=$((FAILURES+1)); }
info() { echo -e "  [....] $1"; }

FAILURES=0

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------

cleanup() {
    if lsmod | grep -q hackbot; then
        info "Unloading module..."
        rmmod hackbot 2>/dev/null
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------

if [ "$(id -u)" -ne 0 ]; then
    echo "Error: must run as root (sudo bash $0)"
    exit 1
fi

if [ ! -f "$MODULE" ]; then
    echo "Error: hackbot.ko not found. Run 'make' first."
    exit 1
fi

# ---------------------------------------------------------------------------
# Test
# ---------------------------------------------------------------------------

echo "=== Milestone 0: Semantic Tokenizer Test ==="
echo

# 1. Unload if already loaded
if lsmod | grep -q hackbot; then
    info "Unloading existing hackbot module..."
    rmmod hackbot 2>/dev/null
    sleep 1
fi

# 2. Clear kernel ring buffer so we only see messages from this test.
dmesg -C

# 3. Load module
info "Loading hackbot.ko..."
if insmod "$MODULE"; then
    pass "Module loaded"
else
    fail "Module failed to load"
    exit 1
fi

# 4. Trigger device open (initializes trace subsystem + tokenizer)
info "Opening /dev/hackbot to trigger trace init..."
sleep 1
if [ -c "$DEVICE" ]; then
    # Read triggers the open() → trace_init → tokenizer_init
    timeout 2 cat "$DEVICE" > /dev/null 2>&1 &
    sleep 3
    pass "Device opened, waiting for events..."
else
    fail "/dev/hackbot not found"
    exit 1
fi

# 5. Grab all hackbot dmesg messages (buffer was cleared before loading).
HACKBOT_DMESG=$(dmesg | grep "hackbot:")

# 6. Check tokenizer init message
echo
echo "--- dmesg: tokenizer init ---"
if echo "$HACKBOT_DMESG" | grep -q "tokenizer: semantic event tokenizer initialized"; then
    pass "Tokenizer initialized"
    echo "$HACKBOT_DMESG" | grep "tokenizer:" | head -5
else
    fail "Tokenizer init message not found"
    info "Recent hackbot dmesg:"
    echo "$HACKBOT_DMESG" | tail -10
fi

# 7. Check debug token prints
echo
echo "--- dmesg: first tokenized events ---"
TOKEN_LINES=$(echo "$HACKBOT_DMESG" | grep "token\[")
TOKEN_COUNT=$(echo "$TOKEN_LINES" | grep -c "token\[" || true)

if [ "$TOKEN_COUNT" -gt 0 ]; then
    pass "Found $TOKEN_COUNT debug token prints"
    echo "$TOKEN_LINES" | head -10
else
    fail "No token debug prints found"
    info "Recent hackbot dmesg:"
    echo "$HACKBOT_DMESG" | tail -20
fi

# 8. Verify token format (should contain field names)
echo
if echo "$TOKEN_LINES" | grep -qE "\[(SCHED|SYSCALL|BLOCK)"; then
    pass "Tokens contain valid category names"
else
    fail "Token format doesn't match expected pattern"
fi

if echo "$TOKEN_LINES" | grep -qE "(SW_OUT|READ|WRITE|OPEN|CLOSE|STAT|POLL|MMAP|FUTEX|SIG|OTHER)"; then
    pass "Tokens contain valid action names"
else
    fail "No valid action names in tokens"
fi

if echo "$TOKEN_LINES" | grep -qE "(BURST|RAPID|FAST|NORM|PAUSE|SLOW|IDLE|DORMT)"; then
    pass "Tokens contain valid gap class names"
else
    fail "No valid gap class names in tokens"
fi

# 9. Unload and check total count
echo
info "Unloading module..."
rmmod hackbot 2>/dev/null
sleep 1

echo
echo "--- dmesg: tokenizer shutdown ---"
SHUTDOWN_MSG=$(dmesg | grep "hackbot: tokenizer: shutdown" | tail -1)
if [ -n "$SHUTDOWN_MSG" ]; then
    pass "Tokenizer shutdown message found"
    echo "  $SHUTDOWN_MSG"
    # Extract count
    EVENT_COUNT=$(echo "$SHUTDOWN_MSG" | grep -oP '\d+ events' | grep -oP '\d+')
    if [ -n "$EVENT_COUNT" ] && [ "$EVENT_COUNT" -gt 0 ]; then
        pass "Tokenized $EVENT_COUNT events total"
    else
        fail "Zero events tokenized"
    fi
else
    fail "Tokenizer shutdown message not found"
    dmesg | tail -10 | grep "hackbot:"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo
echo "=== Results ==="
if [ "$FAILURES" -eq 0 ]; then
    echo -e "${GREEN}All tests passed!${RESET}"
else
    echo -e "${RED}$FAILURES test(s) failed${RESET}"
fi
exit $FAILURES
