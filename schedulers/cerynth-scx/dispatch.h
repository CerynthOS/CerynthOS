/* SPDX-License-Identifier: GPL-2.0 */
#ifndef CERYNTH_DISPATCH_H
#define CERYNTH_DISPATCH_H

/* bpf_user_ringbuf_drain: 0 continues, 1 stops. */
static long handle_dispatched_task(struct bpf_dynptr *dynptr, void *context)
{
	const struct dispatched_task_ctx *task;

	(void)context;
	task = bpf_dynptr_data(dynptr, 0, sizeof(*task));
	if (!task)
		return 0;

	dispatch_task(task);

	/* Stop only when the current ops.dispatch() batch has no slots left. */
	return scx_bpf_dispatch_nr_slots() == 0;
}

#endif
