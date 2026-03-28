/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_PATROL_H
#define HACKBOT_PATROL_H

/*
 * hackbot_patrol.h — Autonomous patrol kthread lifecycle.
 *
 * Creates a kernel thread [hackbot_patrol] that wakes periodically,
 * calls the Rust patrol tick (which runs the vLLM agent loop), and
 * records findings into the agent memory ring buffer.
 *
 * Called from Rust (hackbot_device.rs) via extern "C" FFI.
 */

int  hackbot_patrol_start(void);
void hackbot_patrol_stop(void);

#endif /* HACKBOT_PATROL_H */
