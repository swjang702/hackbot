# hackbot Kernel Module

Rust kernel module that creates `/dev/hackbot` — the in-kernel LLM agent interface.

## Current Step: Step 2a — vLLM Inference via Kernel Socket

The module connects to a vLLM server via TCP kernel socket (`sock_create_kern` / `kernel_connect` / `kernel_sendmsg` / `kernel_recvmsg`). Write a prompt to `/dev/hackbot`, read the LLM response back.

### How It Works

```
User writes to /dev/hackbot
    → hackbot.ko creates TCP socket to 127.0.0.1:8000
    → Sends HTTP POST /v1/completions with prompt
    → Receives JSON response from vLLM
    → Extracts "text" field
    → User reads response from /dev/hackbot
```

## Prerequisites

### 1. Install bindgen-cli (Fedora)

```bash
sudo dnf install bindgen-cli
```

### 2. Configure the kernel with Rust support

```bash
cd ~/sources/linux-6.19.8

# Option A: Start from your running kernel's config
cp /boot/config-$(uname -r) .config
# Or from the 6.16 config:
# cp ~/sources/linux-6.16/.config .config

# Enable Rust support
scripts/config --enable CONFIG_RUST
scripts/config --enable CONFIG_SAMPLES
scripts/config --enable CONFIG_SAMPLES_RUST
scripts/config --enable CONFIG_SAMPLE_RUST_MISC_DEVICE

# Resolve any new config questions
make olddefconfig

# Verify Rust toolchain is compatible
make rustavailable
```

### 3. Prepare the kernel for module building

```bash
cd ~/sources/linux-6.19.8
make modules_prepare -j$(nproc)
```

This is MUCH faster than a full kernel build — it only compiles what's needed
for out-of-tree module compilation (including the Rust `kernel` crate).

### 4. Start vLLM server

```bash
# Install vLLM (if not already installed)
pip install vllm

# Start serving a model on port 8000
vllm serve <model-name> --port 8000
```

The kernel module connects to `127.0.0.1:8000` by default.

**No local GPU?** Connect to a remote vLLM server:

```bash
# Option A: SSH tunnel (recommended — encrypted, zero code change)
ssh -L 8000:localhost:8000 gpu-server
# The kernel module connects to localhost:8000, SSH forwards to remote.

# Option B: Direct connection (change constant, rebuild)
# Edit hackbot.rs: VLLM_ADDR = u32::from_be_bytes([192, 168, 1, 100]);
# WARNING: plaintext HTTP over the network — no TLS.
```

## Build

```bash
cd ~/projects/hackbot/hackbot-kmod
make
```

## Usage

```bash
# Start vLLM first (in another terminal)
vllm serve <model-name> --port 8000

# Load the module
sudo insmod hackbot.ko

# Check it loaded
dmesg | tail
ls -la /dev/hackbot

# Make device writable (root-only by default)
sudo chmod 666 /dev/hackbot

# Send a prompt and read the response
echo "what processes are running?" > /dev/hackbot
cat /dev/hackbot
# Output: LLM-generated response from vLLM

# If vLLM is not running, you'll get:
# [hackbot] vLLM error: 111 (errno)
# Connection refused - is vLLM running on port 8000?

# Unload
sudo rmmod hackbot
```

## Troubleshooting

### "Connection refused" (errno 111)
vLLM server is not running or not listening on port 8000.
```bash
# Check if vLLM is running
curl http://localhost:8000/v1/models
```

### "module verification failed"
If your running kernel has module signing enabled but your module isn't signed:
```bash
# Disable module signature verification (for development only!)
# Add to kernel command line: module.sig_enforce=0
# Or in .config: CONFIG_MODULE_SIG_FORCE=n
```

### "Invalid module format"
The module must be built against the SAME kernel version you're running.
If you're running 6.14.x but building against 6.19.8, you'll need to either:
- Boot the 6.19.8 kernel, or
- Build against your running kernel's source tree instead

## Architecture

```
┌─────────────────────────────────────┐
│            KERNEL SPACE             │
│                                     │
│  hackbot.ko (Rust kernel module)    │
│  ├── /dev/hackbot (miscdevice)      │
│  ├── Per-fd state (Mutex + CondVar) │
│  ├── KernelSocket (TCP client)      │
│  ├── HTTP/1.1 request builder       │
│  └── JSON response parser           │
│         │                           │
│         │ TCP socket (127.0.0.1)    │
│         ▼                           │
└─────────┼───────────────────────────┘
          │
┌─────────┼───────────────────────────┐
│         │     USER SPACE            │
│         ▼                           │
│  vLLM server (port 8000)            │
│  ├── /v1/completions endpoint       │
│  └── GPU-accelerated inference      │
└─────────────────────────────────────┘
```
