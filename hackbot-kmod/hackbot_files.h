/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_FILES_H
#define HACKBOT_FILES_H

/*
 * hackbot_files.h — List open file descriptors for a process.
 *
 * Walks task_struct->files->fdtable and calls d_path() to resolve
 * each open file to a human-readable path.
 *
 * Called from Rust (hackbot_tools.rs) via extern "C" FFI.
 */

int hackbot_list_fds(int pid, char *out, int maxlen);

#endif /* HACKBOT_FILES_H */
