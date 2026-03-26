/* SPDX-License-Identifier: GPL-2.0 */
#ifndef HACKBOT_FPU_H
#define HACKBOT_FPU_H

/*
 * hackbot_fpu.h — Public interface for the float32 FPU inference engine.
 * Called from Rust (hackbot_main.rs) via extern "C" FFI.
 */

void *hackbot_fpu_alloc(int dim, int hidden_dim, int n_layers,
			int n_heads, int n_kv_heads, int head_dim,
			int vocab_size, int max_seq);
void hackbot_fpu_free(void *state);
void hackbot_fpu_reset(void *state);
int hackbot_fpu_forward(void *state, const void *weights,
			size_t weights_len, int token_id, int pos);
int hackbot_fpu_get_next_token(const void *state);

#endif /* HACKBOT_FPU_H */
