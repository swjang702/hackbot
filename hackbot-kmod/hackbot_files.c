// SPDX-License-Identifier: GPL-2.0
/*
 * hackbot_files.c — List open file descriptors for a given process.
 *
 * Finds the task by PID, walks its fdtable under RCU, and uses d_path()
 * to resolve each open file descriptor to a human-readable path string.
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
#include <linux/fs.h>
#include <linux/dcache.h>
#include <linux/rcupdate.h>
#include <linux/string.h>
#include <linux/slab.h>
#include "hackbot_files.h"

#define MAX_FD_ENTRIES    256   /* max FDs to list */
#define PATH_BUF_SIZE     256   /* max path length per entry */

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
 */
int hackbot_list_fds(int pid, char *out, int maxlen)
{
	struct task_struct *task;
	struct files_struct *files;
	struct fdtable *fdt;
	struct file *file;
	char *path_buf;
	char *path;
	unsigned int fd;
	int pos = 0;
	int count = 0;
	int total_fds = 0;

	if (!out || maxlen <= 0)
		return -EINVAL;

	/* Allocate a temporary buffer for d_path(). */
	path_buf = kmalloc(PATH_BUF_SIZE, GFP_KERNEL);
	if (!path_buf)
		return -ENOMEM;

	/* Header */
	pos = append_str(out, pos, maxlen, "FD       PATH\n", 14);
	if (pos < 0)
		goto out_free;

	rcu_read_lock();

	/* Find the task by PID. */
	task = pid_task(find_vpid(pid), PIDTYPE_PID);
	if (!task) {
		rcu_read_unlock();
		kfree(path_buf);
		return -ESRCH;
	}

	/* Access the file descriptor table. */
	files = task->files;
	if (!files) {
		rcu_read_unlock();
		pos = append_str(out, pos, maxlen,
				 "[kernel thread — no open files]\n", 32);
		kfree(path_buf);
		return (pos > 0) ? pos : 0;
	}

	spin_lock(&files->file_lock);
	fdt = files_fdtable(files);

	for (fd = 0; fd < fdt->max_fds && count < MAX_FD_ENTRIES; fd++) {
		file = rcu_dereference_check_fdtable(files, fdt->fd[fd]);
		if (!file)
			continue;

		total_fds++;

		/* FD number */
		pos = append_num(out, pos, maxlen, fd);
		if (pos < 0)
			break;

		/* Padding to align paths */
		pos = append_str(out, pos, maxlen, "        ", 8);
		if (pos < 0)
			break;

		/* Resolve the file path. */
		path = d_path(&file->f_path, path_buf, PATH_BUF_SIZE);
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

		count++;
	}

	spin_unlock(&files->file_lock);
	rcu_read_unlock();

	/* Truncation notice */
	if (count >= MAX_FD_ENTRIES && total_fds > count) {
		char trunc_msg[64];
		int tlen;

		tlen = snprintf(trunc_msg, sizeof(trunc_msg),
				"[... truncated, showing %d of %d fds]\n",
				count, total_fds);
		if (pos >= 0 && tlen > 0)
			pos = append_str(out, pos, maxlen, trunc_msg, tlen);
	}

out_free:
	kfree(path_buf);
	return (pos > 0) ? pos : 0;
}
