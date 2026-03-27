/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_CONSOLE_H
#define HACKBOT_CONSOLE_H

/*
 * hackbot_console.h — Console ring buffer for kernel log capture.
 *
 * Registers a lightweight struct console driver that copies every printk
 * message into a 64 KB circular buffer.  The dmesg tool reads from this
 * buffer, giving the LLM agent visibility into kernel log messages.
 *
 * Called from Rust (hackbot_device.rs) via extern "C" FFI.
 */

int  hackbot_console_init(void);
void hackbot_console_exit(void);
int  hackbot_console_read(char *out, int maxlen);

#endif /* HACKBOT_CONSOLE_H */
