# hackbot Kernel Module — Testing Guide

## Prerequisites

```bash
cd hackbot-kmod
make clean && make          # Build the module
```

Requires:
- Linux 6.19.8 kernel with Rust support
- vLLM server reachable at 100.66.136.70:8000 (via Tailscale) for System 2
- Root access (sudo) for module load/unload

---

## Quick Smoke Test

```bash
make test
```

This loads the module, writes "hello from userspace" to `/dev/hackbot`, reads the response, and unloads. Verifies basic I/O works.

---

## Full Automated Test Suite

```bash
sudo bash test.sh full       # All tests (~10 min, waits for patrol)
sudo bash test.sh quick      # Load/unload + basic I/O only (~2 min)
sudo bash test.sh tools      # All 6 tools via vLLM (~5 min)
sudo bash test.sh patrol     # Patrol thread test (~40s)
sudo bash test.sh memory     # Agent memory persistence (~3 min)
```

The test script shows real-time output: prompts sent, responses received, and dmesg logs from the kernel module.

---

## Manual Testing

### 1. Module Lifecycle

```bash
# Load
sudo insmod hackbot.ko

# Verify device created
ls -la /dev/hackbot

# Check startup messages
dmesg | grep hackbot
# Expected:
#   hackbot: loading module, creating /dev/hackbot
#   hackbot: vLLM endpoint = 100.66.136.70:8000
#   hackbot: patrol thread created (pid XXXX)
#   hackbot: patrol thread started (interval=120s)

# Verify patrol kthread is visible
ps -eo pid,comm | grep hackbot
# Expected:
#   XXXX hackbot_patrol

# Unload
sudo rmmod hackbot

# Check clean shutdown
dmesg | tail -5
# Expected:
#   hackbot: stopping patrol thread...
#   hackbot: patrol thread stopped
#   hackbot: unloading module
```

### 2. Basic Conversation

```bash
sudo insmod hackbot.ko

# Simple greeting
echo "hello" > /dev/hackbot && cat /dev/hackbot

# Ask about the system
echo "what processes are running on this system?" > /dev/hackbot && cat /dev/hackbot
```

### 3. Testing All 6 Tools

Each tool is triggered by the LLM when it decides it needs data. The prompts below are designed to encourage the agent to use specific tools.

```bash
# Tool: ps — process listing
echo "use the ps tool to list all running processes" > /dev/hackbot && cat /dev/hackbot

# Tool: mem — memory statistics
echo "show me detailed memory statistics using the mem tool" > /dev/hackbot && cat /dev/hackbot

# Tool: loadavg — load averages
echo "what is the current system load? use the loadavg tool" > /dev/hackbot && cat /dev/hackbot

# Tool: dmesg — kernel log messages
echo "show me recent kernel log messages using the dmesg tool" > /dev/hackbot && cat /dev/hackbot

# Tool: files — open file descriptors for a process
echo "what files does PID 1 (systemd) have open? use the files tool" > /dev/hackbot && cat /dev/hackbot

# Tool: kprobe — kernel function instrumentation
echo "attach a kprobe to do_sys_openat2, wait a moment, then check the hit count" > /dev/hackbot && cat /dev/hackbot
```

After each query, you can check dmesg for tool call details:

```bash
dmesg | tail -30 | grep hackbot
# Look for:
#   hackbot: agent iteration 1/10
#   hackbot: tool call 'ps' at iteration 1
#   hackbot: tool call 'kprobe attach do_sys_openat2' at iteration 2
#   hackbot: final answer at iteration 3
```

### 4. Testing Agent Memory

The memory system stores findings from both user queries and patrol cycles. It's injected into the system prompt of subsequent queries.

```bash
sudo insmod hackbot.ko
sleep 3

# --- Query 1: Populate memory ---
echo "what is the current system load and memory usage?" > /dev/hackbot && cat /dev/hackbot

# Check that the finding was recorded:
dmesg | grep "hackbot: memory:"
# Expected:
#   hackbot: memory: no findings to inject into prompt   ← memory was empty
#   hackbot: memory: recorded finding #1 from 'user' (XXX bytes)

# --- Query 2: Memory should be injected ---
echo "what did you observe earlier? any changes?" > /dev/hackbot && cat /dev/hackbot

# Check that memory was injected:
dmesg | grep "hackbot: memory:"
# Expected:
#   hackbot: memory: injecting 1 findings into system prompt  ← injected!
#   hackbot: memory: recorded finding #2 from 'user' (XXX bytes)

# --- Query 3: Agent should reference past findings ---
echo "summarize all your observations so far" > /dev/hackbot && cat /dev/hackbot

# The agent should mention findings from queries 1 and 2 in its response.

# Check memory accumulation:
dmesg | grep "hackbot: memory: recorded"
# Expected:
#   hackbot: memory: recorded finding #1 from 'user' (...)
#   hackbot: memory: recorded finding #2 from 'user' (...)
#   hackbot: memory: recorded finding #3 from 'user' (...)
```

### 5. Testing Patrol Thread

The patrol kthread wakes every 120 seconds (with a 30-second initial delay) and autonomously investigates the system using vLLM.

```bash
sudo insmod hackbot.ko

# Verify patrol thread is running
ps -eo pid,comm | grep hackbot_patrol

# Wait for first patrol cycle (30s initial delay)
echo "Waiting 35 seconds for first patrol..."
sleep 35

# Check patrol activity
dmesg | grep "hackbot: patrol"
# Expected:
#   hackbot: patrol thread started (interval=120s)
#   hackbot: patrol cycle starting
#   hackbot: patrol finding: <agent's analysis of system state>
#   hackbot: memory: recorded finding #1 from 'patrol' (XXX bytes)

# Now make a user query — it should see the patrol finding in memory
echo "what has your patrol observed?" > /dev/hackbot && cat /dev/hackbot

dmesg | grep "hackbot: memory: injecting"
# Expected:
#   hackbot: memory: injecting 1 findings into system prompt
```

### 6. Testing Kprobe Lifecycle

```bash
sudo insmod hackbot.ko

# Attach a kprobe
echo "attach a kprobe to the do_sys_openat2 function" > /dev/hackbot && cat /dev/hackbot

# Generate some file opens (in another terminal)
ls /tmp; cat /etc/hostname; ls /dev

# Check hit count
echo "check the kprobe hit counts" > /dev/hackbot && cat /dev/hackbot

# Detach
echo "detach the kprobe from do_sys_openat2" > /dev/hackbot && cat /dev/hackbot

# Unload — should clean up any remaining kprobes
sudo rmmod hackbot
dmesg | grep "hackbot: kprobe\|hackbot: cleanup"
```

### 7. Error Handling (vLLM Unreachable)

```bash
# If vLLM server is down, hackbot should handle gracefully:
echo "hello" > /dev/hackbot && cat /dev/hackbot
# Expected response:
#   [hackbot] Connection refused - is vLLM running on port 8000?
# or:
#   [hackbot] Connection timed out - check network/firewall.

# The patrol thread also handles this:
dmesg | grep "hackbot: patrol"
# Expected:
#   hackbot: patrol cycle failed: error -111   (ECONNREFUSED)
```

---

## Understanding dmesg Output

All hackbot kernel messages are prefixed with `hackbot:`. Key message categories:

| Message Pattern | What It Means |
|----------------|---------------|
| `hackbot: loading module` | Module initialized successfully |
| `hackbot: vLLM endpoint = ...` | Shows configured vLLM server address |
| `hackbot: patrol thread created (pid N)` | Patrol kthread spawned |
| `hackbot: patrol thread started (interval=120s)` | Patrol loop running |
| `hackbot: agent iteration N/10` | OODA loop iteration (vLLM call) |
| `hackbot: tool call 'NAME' at iteration N` | Agent invoked a tool |
| `hackbot: final answer at iteration N` | Agent produced final response |
| `hackbot: memory: no findings to inject` | Memory empty (first query) |
| `hackbot: memory: injecting N findings` | Past findings included in prompt |
| `hackbot: memory: recorded finding #N from 'SOURCE'` | New finding saved |
| `hackbot: patrol cycle starting` | Patrol woke up and is investigating |
| `hackbot: patrol finding: ...` | Patrol produced a finding |
| `hackbot: patrol cycle failed: error N` | vLLM unreachable during patrol |
| `hackbot: stopping patrol thread...` | Module unload — stopping patrol |
| `hackbot: patrol thread stopped` | Patrol exited cleanly |
| `hackbot: kprobe attached to 'FUNC'` | Kprobe registered |
| `hackbot: kprobe detached from 'FUNC'` | Kprobe unregistered |
| `hackbot: cleanup kprobe 'FUNC'` | Kprobe cleaned up during unload |
| `hackbot: unloading module` | Module removed cleanly |

---

## System 1 vs System 2

hackbot has two inference backends:

| | System 1 (Local) | System 2 (vLLM) |
|---|---|---|
| **Model** | SmolLM2-135M (FP16) | Qwen 7B (on GPU) |
| **Speed** | ~10ms/token | ~50-500ms total |
| **Quality** | Limited (FP16 precision) | Good (full precision) |
| **When used** | If `/lib/firmware/hackbot-model.bin` exists | Default, or fallback |
| **Config** | `INFERENCE_MODE=1` forces local | `INFERENCE_MODE=2` forces vLLM |

Default (`INFERENCE_MODE=0`): tries local first if model loaded, falls back to vLLM.

To test local inference specifically:

```bash
# Install FP16 model as firmware
sudo cp hackbot-model-fp16.bin /lib/firmware/hackbot-model.bin
sudo insmod hackbot.ko

# First device open triggers model load
echo "hello" > /dev/hackbot && cat /dev/hackbot

# Check which backend was used:
dmesg | grep "hackbot: using\|hackbot: FPU\|hackbot: local\|hackbot: vLLM"
```

---

## Makefile Targets

```bash
make                # Build hackbot.ko
make clean          # Clean build artifacts
make load           # sudo insmod hackbot.ko
make unload         # sudo rmmod hackbot
make test           # Quick smoke test (load → write → read → unload)
make test-full      # Full test suite (all categories)
make test-tools     # Test all 6 tools
make test-patrol    # Test patrol thread
```
