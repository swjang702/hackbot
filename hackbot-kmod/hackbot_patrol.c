// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_patrol.c — Autonomous patrol kernel thread.
 *
 * Creates a dedicated kthread [hackbot_patrol] that:
 *   1. Sleeps for HACKBOT_PATROL_INTERVAL seconds
 *   2. Calls hackbot_patrol_tick() (Rust callback → vLLM agent loop)
 *   3. Repeats until kthread_stop() is called
 *
 * The thread is visible in ps/top as [hackbot_patrol] and can be
 * observed by the agent's own <tool>ps</tool> tool.
 *
 * Clean shutdown: kthread_stop() wakes the sleeping thread and sets
 * the should_stop flag. If the thread is mid-vLLM-call, stop blocks
 * until the call completes (bounded by socket timeout).
 */

#include <linux/kthread.h>
#include <linux/sched.h>
#include <linux/delay.h>
#include <linux/printk.h>
#include "hackbot_patrol.h"

/* Patrol interval in seconds — must match PATROL_INTERVAL_SECS in hackbot_config.rs */
#define HACKBOT_PATROL_INTERVAL  120

/* Rust callback: runs one patrol cycle (vLLM agent loop + memory recording) */
extern void hackbot_patrol_tick(void);

static struct task_struct *patrol_task;

/*
 * Patrol thread function.
 *
 * Loops until kthread_should_stop(), sleeping between cycles.
 * The schedule_timeout_interruptible() call wakes immediately
 * when kthread_stop() sends a wake-up signal.
 */
static int hackbot_patrol_fn(void *data)
{
	pr_info("hackbot: patrol thread started (interval=%ds)\n",
		HACKBOT_PATROL_INTERVAL);

	/*
	 * Initial delay: let the module finish loading and vLLM become
	 * reachable before the first patrol cycle.
	 */
	schedule_timeout_interruptible(30 * HZ);

	while (!kthread_should_stop()) {
		hackbot_patrol_tick();

		/*
		 * Sleep for the patrol interval. This is interruptible:
		 * kthread_stop() will wake us immediately for clean shutdown.
		 */
		schedule_timeout_interruptible(
			(unsigned long)HACKBOT_PATROL_INTERVAL * HZ);
	}

	pr_info("hackbot: patrol thread stopped\n");
	return 0;
}

/*
 * hackbot_patrol_start — create and start the patrol kthread.
 *
 * Returns 0 on success, negative errno on failure.
 * Failure is non-fatal: the module works without patrol.
 */
int hackbot_patrol_start(void)
{
	patrol_task = kthread_run(hackbot_patrol_fn, NULL, "hackbot_patrol");
	if (IS_ERR(patrol_task)) {
		int err = PTR_ERR(patrol_task);
		patrol_task = NULL;
		return err;
	}

	pr_info("hackbot: patrol thread created (pid %d)\n",
		patrol_task->pid);
	return 0;
}

/*
 * hackbot_patrol_stop — stop the patrol kthread and wait for exit.
 *
 * Blocks until the thread exits. If the thread is mid-vLLM-call,
 * this waits for the current call to complete.
 */
void hackbot_patrol_stop(void)
{
	if (!patrol_task)
		return;

	pr_info("hackbot: stopping patrol thread...\n");
	kthread_stop(patrol_task);
	patrol_task = NULL;
}
