# hackbot Kernel Module

Rust kernel module that creates `/dev/hackbot` — the in-kernel LLM agent interface.

## Step 1: Prompt/Response Skeleton

Currently a dummy that echoes prompts back. The inference engine comes in Step 2.

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

## Build

```bash
cd ~/projects/hackbot/hackbot-kmod
make
```

## Usage

```bash
# Load the module
sudo insmod hackbot.ko

# Check it loaded
dmesg | tail
ls -la /dev/hackbot

# Send a prompt and read the response
echo "what processes are running?" > /dev/hackbot
cat /dev/hackbot
# Output: [hackbot] I received: what processes are running?

# Unload
sudo rmmod hackbot
```

## Troubleshooting

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
