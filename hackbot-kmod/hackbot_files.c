// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_files.c — List open file descriptors for a given process.
 *
 * Finds the task by PID, pins the task via get_task_struct, then
 * walks its fdtable under task_lock + file_lock (the canonical
 * pattern used by fget_task in fs/file.c), collects file
 * references, drops all locks, then uses d_path() to resolve each
 * open file descriptor to a human-readable path.
 *
 * d_path() can take sleeping locks (e.g., on some filesystems),
 * so it must NOT be called under spinlock or RCU read lock.
 * We use get_file()/fput() to hold references while resolving paths.
 *
 * Output format:
 *   FD       PATH
 *   0        /dev/null
 *   1        pipe:[12345]
 *   3        socket:[67890]
 *   ...
 */

#include <linux/sched.h>
#include <linux/pid.h>
#include <linux/fdtable.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/dcache.h>
#include <linux/rcupdate.h>
#include <linux/string.h>
#include <linux/slab.h>
#include "hackbot_files.h"

#define MAX_FD_ENTRIES    256   /* max FDs to list */
#define PATH_BUF_SIZE     256   /* max path length per entry */

/* Collected fd+file pair for deferred d_path() resolution. */
struct fd_ref {
	unsigned int fd;
	struct file *file;   /* held via get_file() */
};

/*
 * Append a decimal number to the output buffer.
 * Returns new write position, or -1 if buffer full.
 */
static int append_num(char *out, int pos, int maxlen, unsigned int val)
{
	char tmp[12];
	int len = 0;
	int i;

	if (val == 0) {
		tmp[len++] = '0';
	} else {
		while (val > 0 && len < (int)sizeof(tmp)) {
			tmp[len++] = '0' + (val % 10);
			val /= 10;
		}
	}
	/* Reverse */
	for (i = len - 1; i >= 0; i--) {
		if (pos >= maxlen)
			return -1;
		out[pos++] = tmp[i];
	}
	return pos;
}

/*
 * Append a string to the output buffer.
 * Returns new write position, or -1 if buffer full.
 */
static int append_str(char *out, int pos, int maxlen, const char *s, int slen)
{
	int i;

	for (i = 0; i < slen; i++) {
		if (pos >= maxlen)
			return -1;
		out[pos++] = s[i];
	}
	return pos;
}

/*
 * hackbot_list_fds — list open file descriptors for process @pid.
 *
 * Writes a formatted table into @out (up to @maxlen bytes).
 * Returns bytes written on success, or -errno on error:
 *   -ESRCH  — no process with that PID
 *   -EINVAL — invalid arguments
 *   -ENOMEM — allocation failure
 */
int hackbot_list_fds(int pid, char *out, int maxlen)
{
	struct task_struct *task;
	struct files_struct *files;
	struct fdtable *fdt;
	struct file *file;
	struct fd_ref *refs;
	char *path_buf;
	char *path;
	unsigned int fd;
	unsigned int max_scan;
	int pos = 0;
	int count = 0;
	int total_fds = 0;
	int i;

	if (!out || maxlen <= 0)
		return -EINVAL;

	/* Allocate a temporary buffer for d_path(). */
	path_buf = kmalloc(PATH_BUF_SIZE, GFP_KERNEL);
	if (!path_buf)
		return -ENOMEM;

	/* Allocate array to collect file references under lock. */
	refs = kmalloc_array(MAX_FD_ENTRIES, sizeof(*refs), GFP_KERNEL);
	if (!refs) {
		kfree(path_buf);
		return -ENOMEM;
	}

	/* Header */
	pos = append_str(out, pos, maxlen, "FD       PATH\n", 14);
	if (pos < 0)
		goto out_free;

	/*
	 * Phase 1: Collect file references under task_lock + file_lock.
	 *
	 * Lifecycle (R-010): files_cachep is NOT SLAB_TYPESAFE_BY_RCU and
	 * neither put_files_struct nor get_files_struct is exported, so we
	 * cannot take an extra ref on files_struct ourselves. Instead, pin
	 * the task_struct via get_task_struct, then take task_lock to
	 * serialize against exit_files() (fs/file.c: task_lock; tsk->files
	 * = NULL; task_unlock; put_files_struct). While task_lock is held,
	 * task->files is stable and the underlying files_struct cannot be
	 * freed. This mirrors the kernel's own fget_task (fs/file.c:1117).
	 *
	 * Lock ordering: task_lock -> file_lock. Verified against
	 * fs/file.c and kernel/exit.c — no path takes file_lock then
	 * nests task_lock, so there is no AB-BA.
	 *
	 * We use get_file() to take a reference on each open file so the
	 * file object survives after we drop the locks; d_path() can sleep
	 * and must NOT be called under spinlock or RCU read-side.
	 */
	rcu_read_lock();
	task = pid_task(find_vpid(pid), PIDTYPE_PID);
	if (!task) {
		rcu_read_unlock();
		kfree(refs);
		kfree(path_buf);
		return -ESRCH;
	}
	get_task_struct(task);
	rcu_read_unlock();

	task_lock(task);
	files = task->files;
	if (!files) {
		task_unlock(task);
		put_task_struct(task);
		pos = append_str(out, pos, maxlen,
				 "[kernel thread — no open files]\n", 32);
		kfree(refs);
		kfree(path_buf);
		return (pos > 0) ? pos : 0;
	}

	spin_lock(&files->file_lock);
	fdt = files_fdtable(files);

	/*
	 * R-011: bound the scan. fdt->max_fds can be 1M+ on hosts with a
	 * raised ulimit -n; walking that many slots under file_lock stalls
	 * every fd op on the target. MAX_FD_ENTRIES * 16 leaves headroom
	 * for sparse fdtables while keeping the lock window bounded.
	 *
	 * Note: total_fds reflects populated slots within [0, max_scan),
	 * not the entire fdtable. The truncation message below is therefore
	 * windowed-scan accurate, not whole-fdtable accurate. Acceptable —
	 * the agent treats the count as informational.
	 */
	max_scan = min(fdt->max_fds, (unsigned int)(MAX_FD_ENTRIES * 16));

	for (fd = 0; fd < max_scan && count < MAX_FD_ENTRIES; fd++) {
		file = rcu_dereference_check_fdtable(files, fdt->fd[fd]);
		if (!file)
			continue;

		total_fds++;
		get_file(file);
		refs[count].fd = fd;
		refs[count].file = file;
		count++;
	}

	spin_unlock(&files->file_lock);
	task_unlock(task);
	put_task_struct(task);

	/*
	 * Phase 2: Resolve paths — NO locks held.
	 *
	 * d_path() can take sleeping locks internally (rename_lock,
	 * mount locks), so it must not be called under spinlock or
	 * with preemption disabled.
	 */
	for (i = 0; i < count; i++) {
		/* FD number */
		pos = append_num(out, pos, maxlen, refs[i].fd);
		if (pos < 0)
			break;

		/* Padding to align paths */
		pos = append_str(out, pos, maxlen, "        ", 8);
		if (pos < 0)
			break;

		/* Resolve the file path. */
		path = d_path(&refs[i].file->f_path, path_buf, PATH_BUF_SIZE);
		if (IS_ERR(path)) {
			pos = append_str(out, pos, maxlen, "[error]", 7);
		} else {
			int plen = (int)strlen(path);
			pos = append_str(out, pos, maxlen, path, plen);
		}
		if (pos < 0)
			break;

		pos = append_str(out, pos, maxlen, "\n", 1);
		if (pos < 0)
			break;
	}

	/* Release all file references. */
	for (i = 0; i < count; i++)
		fput(refs[i].file);

	/* Truncation notice */
	if (count >= MAX_FD_ENTRIES && total_fds > count) {
		char trunc_msg[64];
		int tlen;

		tlen = snprintf(trunc_msg, sizeof(trunc_msg),
				"[... truncated, showing %d of %d fds]\n",
				count, total_fds);
		/*
		 * snprintf returns the would-have-been length when the output
		 * is truncated, NOT the bytes actually written. Passing that
		 * unclamped to append_str would read past the NUL into
		 * uninitialised stack bytes (R-026 in docs/REVIEW_v0.1.md).
		 */
		if (tlen >= (int)sizeof(trunc_msg))
			tlen = (int)sizeof(trunc_msg) - 1;
		if (pos >= 0 && tlen > 0)
			pos = append_str(out, pos, maxlen, trunc_msg, tlen);
	}

out_free:
	kfree(refs);
	kfree(path_buf);
	return (pos > 0) ? pos : 0;
}
