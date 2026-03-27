// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_kprobe.c — Kprobe instrumentation manager for the hackbot agent.
 *
 * Maintains up to MAX_KPROBES active kprobes.  Each probe has:
 *   - A kernel function symbol name
 *   - An atomic64 hit counter (incremented lock-free in the pre_handler)
 *   - The struct kprobe itself
 *
 * The pre_handler is extremely lightweight: just an atomic_inc.
 * Slot management (attach/detach) is serialized by a mutex.
 *
 * All probes are unregistered on module unload via hackbot_kprobe_cleanup().
 */

#include <linux/kprobes.h>
#include <linux/mutex.h>
#include <linux/atomic.h>
#include <linux/string.h>
#include <linux/slab.h>
#include <linux/printk.h>
#include "hackbot_kprobe.h"

#define MAX_KPROBES      8
#define MAX_SYMBOL_LEN   64

struct hackbot_kprobe_slot {
	bool active;
	char symbol[MAX_SYMBOL_LEN];
	atomic64_t count;
	struct kprobe kp;
};

static struct hackbot_kprobe_slot slots[MAX_KPROBES];
static DEFINE_MUTEX(kprobe_mutex);

/*
 * Kprobe pre_handler — called on every hit of the probed function.
 * Must be minimal: just increment the counter.  Runs with preemption
 * disabled; no sleeping, no heavy work.
 */
static int hackbot_kprobe_pre_handler(struct kprobe *p, struct pt_regs *regs)
{
	struct hackbot_kprobe_slot *slot =
		container_of(p, struct hackbot_kprobe_slot, kp);
	atomic64_inc(&slot->count);
	return 0;
}

/*
 * hackbot_kprobe_attach — attach a kprobe to a kernel function.
 *
 * @symbol: function name (not necessarily null-terminated)
 * @len:    length of the symbol name
 *
 * Returns 0 on success, or:
 *   -EINVAL   invalid symbol or too long
 *   -EEXIST   already probing that function
 *   -ENOSPC   all slots in use
 *   -ENOENT   function not found (from register_kprobe)
 *   -other    register_kprobe failure
 */
int hackbot_kprobe_attach(const char *symbol, int len)
{
	int i, free_slot = -1;
	int ret;

	if (!symbol || len <= 0 || len >= MAX_SYMBOL_LEN)
		return -EINVAL;

	mutex_lock(&kprobe_mutex);

	/* Check for duplicate and find a free slot. */
	for (i = 0; i < MAX_KPROBES; i++) {
		if (slots[i].active) {
			if ((int)strlen(slots[i].symbol) == len &&
			    memcmp(slots[i].symbol, symbol, len) == 0) {
				mutex_unlock(&kprobe_mutex);
				return -EEXIST;
			}
		} else if (free_slot < 0) {
			free_slot = i;
		}
	}

	if (free_slot < 0) {
		mutex_unlock(&kprobe_mutex);
		return -ENOSPC;
	}

	/* Initialize the slot. */
	memset(&slots[free_slot], 0, sizeof(slots[free_slot]));
	memcpy(slots[free_slot].symbol, symbol, len);
	slots[free_slot].symbol[len] = '\0';
	atomic64_set(&slots[free_slot].count, 0);

	slots[free_slot].kp.pre_handler = hackbot_kprobe_pre_handler;
	slots[free_slot].kp.symbol_name = slots[free_slot].symbol;

	ret = register_kprobe(&slots[free_slot].kp);
	if (ret < 0) {
		memset(&slots[free_slot], 0, sizeof(slots[free_slot]));
		mutex_unlock(&kprobe_mutex);
		return ret;
	}

	slots[free_slot].active = true;
	mutex_unlock(&kprobe_mutex);

	pr_info("hackbot: kprobe attached to '%s'\n", slots[free_slot].symbol);
	return 0;
}

/*
 * Append a decimal number to the output buffer.
 * Returns new write position, or -1 if buffer full.
 */
static int kp_append_num(char *out, int pos, int maxlen, long long val)
{
	char tmp[24];
	int len = 0;
	int i;
	int negative = 0;

	if (val < 0) {
		negative = 1;
		val = -val;
	}

	if (val == 0) {
		tmp[len++] = '0';
	} else {
		while (val > 0 && len < (int)sizeof(tmp)) {
			tmp[len++] = '0' + (char)(val % 10);
			val /= 10;
		}
	}

	if (negative) {
		if (pos >= maxlen) return -1;
		out[pos++] = '-';
	}

	for (i = len - 1; i >= 0; i--) {
		if (pos >= maxlen) return -1;
		out[pos++] = tmp[i];
	}
	return pos;
}

/*
 * hackbot_kprobe_check — report all active kprobes and their hit counts.
 *
 * Writes a formatted table into @out (up to @maxlen bytes).
 * Returns bytes written (0 if no active kprobes).
 */
int hackbot_kprobe_check(char *out, int maxlen)
{
	static const char hdr[] = "SYMBOL                           HITS\n";
	int pos = 0;
	int i, slen, pad;
	long long count;
	int found = 0;

	if (!out || maxlen <= 0)
		return 0;

	/* Header */
	slen = (int)sizeof(hdr) - 1;
	if (pos + slen > maxlen)
		return 0;
	memcpy(out + pos, hdr, slen);
	pos += slen;

	mutex_lock(&kprobe_mutex);

	for (i = 0; i < MAX_KPROBES; i++) {
		if (!slots[i].active)
			continue;

		found++;
		slen = (int)strlen(slots[i].symbol);

		/* Symbol name */
		if (pos + slen >= maxlen)
			break;
		memcpy(out + pos, slots[i].symbol, slen);
		pos += slen;

		/* Padding (align to column 33) */
		pad = 33 - slen;
		if (pad < 1) pad = 1;
		while (pad-- > 0 && pos < maxlen)
			out[pos++] = ' ';

		/* Hit count */
		count = atomic64_read(&slots[i].count);
		pos = kp_append_num(out, pos, maxlen, count);
		if (pos < 0)
			break;

		if (pos < maxlen)
			out[pos++] = '\n';
	}

	mutex_unlock(&kprobe_mutex);

	if (!found)
		return 0;

	return (pos > 0) ? pos : 0;
}

/*
 * hackbot_kprobe_detach — remove a kprobe from a kernel function.
 *
 * @symbol: function name (not necessarily null-terminated)
 * @len:    length of the symbol name
 *
 * Returns 0 on success, -ENOENT if no matching kprobe found.
 */
int hackbot_kprobe_detach(const char *symbol, int len)
{
	int i;

	if (!symbol || len <= 0)
		return -EINVAL;

	mutex_lock(&kprobe_mutex);

	for (i = 0; i < MAX_KPROBES; i++) {
		if (!slots[i].active)
			continue;
		if ((int)strlen(slots[i].symbol) == len &&
		    memcmp(slots[i].symbol, symbol, len) == 0) {
			unregister_kprobe(&slots[i].kp);
			pr_info("hackbot: kprobe detached from '%s' (hits: %lld)\n",
				slots[i].symbol,
				atomic64_read(&slots[i].count));
			slots[i].active = false;
			memset(&slots[i], 0, sizeof(slots[i]));
			mutex_unlock(&kprobe_mutex);
			return 0;
		}
	}

	mutex_unlock(&kprobe_mutex);
	return -ENOENT;
}

/*
 * hackbot_kprobe_cleanup — unregister ALL active kprobes.
 * Called during module unload to ensure no dangling probes.
 */
void hackbot_kprobe_cleanup(void)
{
	int i;

	mutex_lock(&kprobe_mutex);

	for (i = 0; i < MAX_KPROBES; i++) {
		if (!slots[i].active)
			continue;
		unregister_kprobe(&slots[i].kp);
		pr_info("hackbot: cleanup kprobe '%s' (hits: %lld)\n",
			slots[i].symbol,
			atomic64_read(&slots[i].count));
		slots[i].active = false;
		memset(&slots[i], 0, sizeof(slots[i]));
	}

	mutex_unlock(&kprobe_mutex);
}
