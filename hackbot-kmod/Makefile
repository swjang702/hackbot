# SPDX-License-Identifier: GPL-2.0
#
# hackbot kernel module — build against a specific kernel source tree.
#
# Usage:
#   make                    # build using default KDIR
#   make KDIR=/path/to/src  # build against a specific kernel
#   make clean              # clean build artifacts
#   make load               # insmod the module (requires sudo)
#   make unload             # rmmod the module (requires sudo)
#   make test               # quick smoke test (load → write → read → unload)
#   make test-full           # full integration test suite
#   make test-tools          # test all 6 tools via vLLM
#   make test-patrol         # test patrol thread (waits ~35s)

KDIR ?= $(HOME)/sources/linux-6.19.8
PWD := $(shell pwd)

default:
	$(MAKE) -C $(KDIR) M=$(PWD) modules

clean:
	$(MAKE) -C $(KDIR) M=$(PWD) clean

# ---------------------------------------------------------------------------
# Safety checks
# ---------------------------------------------------------------------------

# Static analysis with sparse (Linux's semantic checker, C=2 = all files).
# Catches: lock discipline violations, __rcu/__user pointer misuse,
# sleeping-in-atomic context, type confusion.
#
# Requires the `sparse` binary in PATH. We verify before invoking the
# kernel build to fail early with an actionable message instead of a
# cryptic mid-build failure.
check:
	@command -v sparse >/dev/null 2>&1 || { \
		echo "ERROR: 'sparse' not found in PATH."; \
		echo "Install it:"; \
		echo "  Fedora/RHEL: sudo dnf install sparse"; \
		echo "  Debian/Ubuntu: sudo apt install sparse"; \
		echo "  Arch:        sudo pacman -S sparse"; \
		echo "  From source: git clone https://git.kernel.org/pub/scm/devel/sparse/sparse.git"; \
		exit 1; \
	}
	$(MAKE) -C $(KDIR) M=$(PWD) C=2 modules

# Build with extra kernel warnings (W=1).
# Catches: unused variables, missing prototypes, suspicious casts.
check-warn:
	$(MAKE) -C $(KDIR) M=$(PWD) W=1 modules

# Run both sparse and extra warnings.
check-all: check check-warn
	@echo "=== All checks passed ==="

# Verify the build kernel has safety configs enabled (KASAN, UBSAN, etc.).
check-kconfig:
	@echo "=== Kernel Safety Config Check ($(KDIR)) ==="
	@KCONFIG="$(KDIR)/.config"; \
	if [ ! -f "$$KCONFIG" ]; then \
		echo "ERROR: No .config found at $$KCONFIG"; \
		echo "The kernel source must be configured to check safety options."; \
		exit 1; \
	fi; \
	PASS=0; FAIL=0; \
	for cfg in CONFIG_KASAN CONFIG_UBSAN CONFIG_KCSAN CONFIG_LOCKDEP \
	           CONFIG_PROVE_LOCKING CONFIG_DEBUG_ATOMIC_SLEEP \
	           CONFIG_FORTIFY_SOURCE CONFIG_SCHED_STACK_END_CHECK; do \
		if grep -q "^$$cfg=y" "$$KCONFIG" 2>/dev/null; then \
			printf "  [OK]   %s\n" "$$cfg"; \
			PASS=$$((PASS + 1)); \
		else \
			printf "  [MISS] %s\n" "$$cfg"; \
			FAIL=$$((FAIL + 1)); \
		fi; \
	done; \
	echo "---"; \
	echo "$$PASS enabled, $$FAIL missing"; \
	if [ $$FAIL -gt 0 ]; then \
		echo "Tip: enable missing configs in kernel .config for full safety coverage."; \
		echo "  KASAN  = array OOB, use-after-free    UBSAN  = integer overflow, UB"; \
		echo "  KCSAN  = data races (lockless code)    LOCKDEP = sleep-under-spinlock"; \
	fi

load:
	sudo insmod hackbot.ko
	@echo "hackbot: module loaded"
	@echo "hackbot: check dmesg for output"

unload:
	sudo rmmod hackbot
	@echo "hackbot: module unloaded"

test: load
	@echo "--- Writing prompt ---"
	echo "hello from userspace" > /dev/hackbot
	@echo "--- Reading response ---"
	cat /dev/hackbot
	@echo "--- Done ---"
	$(MAKE) unload

test-full:
	sudo bash test.sh full

test-tools:
	sudo bash test.sh tools

test-patrol:
	sudo bash test.sh patrol

.PHONY: default clean load unload test test-full test-tools test-patrol \
       check check-warn check-all check-kconfig
