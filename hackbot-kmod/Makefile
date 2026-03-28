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

.PHONY: default clean load unload test test-full test-tools test-patrol
