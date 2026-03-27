// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_console.c — Console ring buffer for kernel log capture.
 *
 * Registers a minimal struct console driver that captures every printk
 * message into a 64 KB circular buffer.  hackbot_console_read() copies
 * the most recent bytes out for the dmesg tool.
 *
 * The write callback runs in ANY context (including hardirq and NMI),
 * so we use a raw spinlock with irqsave and never sleep.
 */

#include <linux/console.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/printk.h>
#include "hackbot_console.h"

#define HACKBOT_CONSOLE_BUF_SIZE  (64 * 1024)   /* 64 KB ring buffer */

static char console_buf[HACKBOT_CONSOLE_BUF_SIZE];
static unsigned int console_head;     /* next write position (circular) */
static unsigned int console_total;    /* total bytes ever written */
static DEFINE_RAW_SPINLOCK(hackbot_con_lock);

/*
 * Console write callback.  Called by printk for every message.
 * Must be safe in any context (hardirq, NMI, early boot).
 */
static void hackbot_console_write(struct console *con, const char *s,
				  unsigned int count)
{
	unsigned long flags;
	unsigned int i;

	raw_spin_lock_irqsave(&hackbot_con_lock, flags);
	for (i = 0; i < count; i++) {
		console_buf[console_head] = s[i];
		console_head = (console_head + 1) % HACKBOT_CONSOLE_BUF_SIZE;
	}
	console_total += count;
	raw_spin_unlock_irqrestore(&hackbot_con_lock, flags);
}

static struct console hackbot_con = {
	.name	= "hackbot",
	.write	= hackbot_console_write,
	.flags	= CON_ENABLED | CON_PRINTBUFFER,
	.index	= -1,
};

/*
 * hackbot_console_init — register the console driver.
 * Called during hackbot module init.
 */
int hackbot_console_init(void)
{
	console_head = 0;
	console_total = 0;
	register_console(&hackbot_con);
	return 0;
}

/*
 * hackbot_console_exit — unregister the console driver.
 * Called during hackbot module exit.
 */
void hackbot_console_exit(void)
{
	unregister_console(&hackbot_con);
}

/*
 * hackbot_console_read — copy the last @maxlen bytes from the ring buffer.
 *
 * Returns the number of bytes actually copied (0 if buffer is empty).
 * The output is a contiguous block of the most recent log messages.
 */
int hackbot_console_read(char *out, int maxlen)
{
	unsigned long flags;
	unsigned int avail, copy_len, start;
	unsigned int i;

	if (!out || maxlen <= 0)
		return 0;

	raw_spin_lock_irqsave(&hackbot_con_lock, flags);

	avail = (console_total < HACKBOT_CONSOLE_BUF_SIZE)
		? console_total : HACKBOT_CONSOLE_BUF_SIZE;

	copy_len = (avail < (unsigned int)maxlen) ? avail : (unsigned int)maxlen;

	/* Start position: go back copy_len bytes from head in the ring. */
	start = (console_head - copy_len + HACKBOT_CONSOLE_BUF_SIZE)
		% HACKBOT_CONSOLE_BUF_SIZE;

	for (i = 0; i < copy_len; i++)
		out[i] = console_buf[(start + i) % HACKBOT_CONSOLE_BUF_SIZE];

	raw_spin_unlock_irqrestore(&hackbot_con_lock, flags);

	return (int)copy_len;
}
