// SPDX-License-Identifier: GPL-2.0

//! Q16.16 fixed-point math primitives for in-kernel inference.
//!
//! All operations use Q16.16 fixed-point arithmetic (i32).
//! No FPU/SIMD — pure scalar integer math only.

/// 1.0 in Q16.16.
#[allow(dead_code)]
pub(crate) const Q16_ONE: i32 = 1 << 16;

/// 2π in Q16.16.
#[allow(dead_code)]
pub(crate) const TWO_PI_Q16: i64 = 411775;

/// exp(-k) in Q16.16 for k = 0..16.
#[allow(dead_code)]
pub(crate) const EXP_TABLE: [i32; 17] = [
    65536, 24109, 8869, 3263, 1200, 442, 162, 60, 22, 8, 3, 1, 0, 0, 0, 0, 0,
];

/// sin(2π·k/256) in Q16.16 for k = 0..255.
#[allow(dead_code)]
pub(crate) const SIN_TABLE: [i32; 256] = [
        0,  1608,  3216,  4821,  6424,  8022,  9616, 11204, 12785, 14359,
    15924, 17479, 19024, 20557, 22078, 23586, 25080, 26558, 28020, 29466,
    30893, 32303, 33692, 35062, 36410, 37736, 39040, 40320, 41576, 42806,
    44011, 45190, 46341, 47464, 48559, 49624, 50660, 51665, 52639, 53581,
    54491, 55368, 56212, 57022, 57798, 58538, 59244, 59914, 60547, 61145,
    61705, 62228, 62714, 63162, 63572, 63944, 64277, 64571, 64827, 65043,
    65220, 65358, 65457, 65516, 65536, 65516, 65457, 65358, 65220, 65043,
    64827, 64571, 64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
    60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368, 54491, 53581,
    52639, 51665, 50660, 49624, 48559, 47464, 46341, 45190, 44011, 42806,
    41576, 40320, 39040, 37736, 36410, 35062, 33692, 32303, 30893, 29466,
    28020, 26558, 25080, 23586, 22078, 20557, 19024, 17479, 15924, 14359,
    12785, 11204,  9616,  8022,  6424,  4821,  3216,  1608,     0, -1608,
    -3216, -4821, -6424, -8022, -9616,-11204,-12785,-14359,-15924,-17479,
   -19024,-20557,-22078,-23586,-25080,-26558,-28020,-29466,-30893,-32303,
   -33692,-35062,-36410,-37736,-39040,-40320,-41576,-42806,-44011,-45190,
   -46341,-47464,-48559,-49624,-50660,-51665,-52639,-53581,-54491,-55368,
   -56212,-57022,-57798,-58538,-59244,-59914,-60547,-61145,-61705,-62228,
   -62714,-63162,-63572,-63944,-64277,-64571,-64827,-65043,-65220,-65358,
   -65457,-65516,-65536,-65516,-65457,-65358,-65220,-65043,-64827,-64571,
   -64277,-63944,-63572,-63162,-62714,-62228,-61705,-61145,-60547,-59914,
   -59244,-58538,-57798,-57022,-56212,-55368,-54491,-53581,-52639,-51665,
   -50660,-49624,-48559,-47464,-46341,-45190,-44011,-42806,-41576,-40320,
   -39040,-37736,-36410,-35062,-33692,-32303,-30893,-29466,-28020,-26558,
   -25080,-23586,-22078,-20557,-19024,-17479,-15924,-14359,-12785,-11204,
    -9616, -8022, -6424, -4821, -3216, -1608,
];

/// RoPE frequencies for head_dim=64, theta=10000.
#[allow(dead_code)]
pub(crate) const ROPE_FREQS_64: [i32; 32] = [
    65536, 49145, 36854, 27636, 20724, 15541, 11654,  8739,
     6554,  4915,  3685,  2764,  2072,  1554,  1165,   874,
      655,   491,   369,   276,   207,   155,   117,    87,
       66,    49,    37,    28,    21,    16,    12,     9,
];

/// Integer square root via Newton's method.
#[allow(dead_code)]
pub(crate) fn isqrt_u64(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let bits = 64 - n.leading_zeros();
    let mut x = 1u64 << ((bits + 1) / 2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// Exponential function for non-positive arguments in Q16.16.
#[allow(dead_code)]
pub(crate) fn exp_q16_neg(x: i32) -> i32 {
    if x >= 0 {
        return Q16_ONE;
    }
    let x_int = x >> 16;
    let idx = (-x_int) as usize;
    if idx >= EXP_TABLE.len() {
        return 0;
    }
    let exp_int = EXP_TABLE[idx] as i64;

    let x_frac = (x - (x_int << 16)) as i64;
    let f = x_frac;
    let f2 = (f * f) >> 16;
    let f3 = (f2 * f) >> 16;
    let exp_frac = Q16_ONE as i64 + f + (f2 >> 1) + f3 / 6;

    ((exp_int * exp_frac) >> 16) as i32
}

/// Sigmoid in Q16.16: σ(x) = 1/(1+exp(-x)).
#[allow(dead_code)]
pub(crate) fn sigmoid_q16(x: i32) -> i32 {
    if x >= 0 {
        let e = exp_q16_neg(-x) as i64;
        let num = (Q16_ONE as i64) * (Q16_ONE as i64);
        (num / (Q16_ONE as i64 + e)) as i32
    } else {
        let e = exp_q16_neg(x) as i64;
        ((e * Q16_ONE as i64) / (Q16_ONE as i64 + e)) as i32
    }
}

/// SiLU (Swish) activation in Q16.16.
#[allow(dead_code)]
pub(crate) fn silu_q16(x: i32) -> i32 {
    let sig = sigmoid_q16(x) as i64;
    ((x as i64 * sig) >> 16) as i32
}

/// Sine lookup with linear interpolation.
#[allow(dead_code)]
pub(crate) fn sin_q16(angle_q16: i32) -> i32 {
    let two_pi = TWO_PI_Q16;
    let mut a = angle_q16 as i64 % two_pi;
    if a < 0 {
        a += two_pi;
    }
    let idx_fixed = (a << 8) / two_pi;
    let idx = idx_fixed as usize;
    let frac = ((a << 8) - idx_fixed * two_pi) as i32;

    let s0 = SIN_TABLE[idx % 256] as i64;
    let s1 = SIN_TABLE[(idx + 1) % 256] as i64;
    let interp = s0 + ((s1 - s0) * frac as i64) / two_pi;
    interp as i32
}

/// Cosine via sin(angle + π/2).
#[allow(dead_code)]
pub(crate) fn cos_q16(angle_q16: i32) -> i32 {
    sin_q16(angle_q16.wrapping_add(102944))
}

/// Matrix-vector multiply with INT8 quantized weights.
#[allow(dead_code)]
pub(crate) fn matmul_q8(
    out: &mut [i32],
    input: &[i32],
    w_data: &[u8],
    w_scales: &[u8],
    rows: usize,
    cols: usize,
    gs: usize,
) {
    let n_groups = cols / gs;

    for r in 0..rows {
        let mut row_acc: i64 = 0;
        let row_base = r * cols;
        let scale_row_base = r * n_groups * 4;

        for g in 0..n_groups {
            let sb = scale_row_base + g * 4;
            let scale = i32::from_le_bytes([
                w_scales[sb], w_scales[sb + 1], w_scales[sb + 2], w_scales[sb + 3],
            ]) as i64;

            let data_base = row_base + g * gs;
            let x_base = g * gs;
            let mut group_acc: i64 = 0;

            for j in 0..gs {
                let w = w_data[data_base + j] as i8 as i64;
                let x = input[x_base + j] as i64;
                group_acc += w * x;
            }

            row_acc += (group_acc * scale) >> 16;
        }

        out[r] = row_acc as i32;
    }
}

/// RMS normalization in Q16.16 fixed-point.
#[allow(dead_code)]
pub(crate) fn rmsnorm_q16(out: &mut [i32], input: &[i32], weight: &[u8], dim: usize) {
    let mut ss: u64 = 0;
    for i in 0..dim {
        let xi = input[i] as i64;
        ss += (xi * xi) as u64;
    }

    let mean_sq = ss / dim as u64;
    let mean_sq_eps = mean_sq + 42950;
    let rms_q16 = isqrt_u64(mean_sq_eps);

    if rms_q16 == 0 {
        for i in 0..dim {
            out[i] = 0;
        }
        return;
    }
    let rsqrt_q16 = ((1u64 << 32) / rms_q16) as i64;

    for i in 0..dim {
        let wb = i * 4;
        let w = i32::from_le_bytes([
            weight[wb], weight[wb + 1], weight[wb + 2], weight[wb + 3],
        ]) as i64;

        let x = input[i] as i64;
        let x_norm = (x * rsqrt_q16) >> 16;
        out[i] = ((x_norm * w) >> 16) as i32;
    }
}

/// Softmax in Q16.16, operating in-place.
#[allow(dead_code)]
pub(crate) fn softmax_q16(x: &mut [i32], len: usize) {
    if len == 0 {
        return;
    }
    if len == 1 {
        x[0] = Q16_ONE;
        return;
    }

    let mut max_val = x[0];
    for i in 1..len {
        if x[i] > max_val {
            max_val = x[i];
        }
    }

    let mut sum: i64 = 0;
    for i in 0..len {
        let e = exp_q16_neg(x[i] - max_val);
        x[i] = e;
        sum += e as i64;
    }

    if sum == 0 {
        x[0] = Q16_ONE;
        return;
    }
    for i in 0..len {
        x[i] = ((x[i] as i64 * Q16_ONE as i64) / sum) as i32;
    }
}

/// Apply RoPE to a single attention head vector.
#[allow(dead_code)]
pub(crate) fn rope_apply_q16(vec: &mut [i32], pos: usize, head_dim: usize) {
    let n_pairs = head_dim / 2;
    for i in 0..n_pairs {
        let freq = if i < ROPE_FREQS_64.len() {
            ROPE_FREQS_64[i] as i64
        } else {
            0i64
        };

        let theta_q16 = ((pos as i64 * freq) % TWO_PI_Q16) as i32;

        let cos_val = cos_q16(theta_q16) as i64;
        let sin_val = sin_q16(theta_q16) as i64;

        let v0 = vec[2 * i] as i64;
        let v1 = vec[2 * i + 1] as i64;

        vec[2 * i]     = ((v0 * cos_val - v1 * sin_val) >> 16) as i32;
        vec[2 * i + 1] = ((v0 * sin_val + v1 * cos_val) >> 16) as i32;
    }
}

/// Element-wise multiply in Q16.16.
#[allow(dead_code)]
pub(crate) fn elementwise_mul_q16(out: &mut [i32], a: &[i32], b: &[i32], len: usize) {
    for i in 0..len {
        out[i] = ((a[i] as i64 * b[i] as i64) >> 16) as i32;
    }
}

/// Vector addition.
#[allow(dead_code)]
pub(crate) fn vec_add_q16(out: &mut [i32], a: &[i32], b: &[i32], len: usize) {
    for i in 0..len {
        out[i] = a[i].wrapping_add(b[i]);
    }
}

/// Apply SiLU activation in-place.
#[allow(dead_code)]
pub(crate) fn silu_vec_q16(vec: &mut [i32], len: usize) {
    for i in 0..len {
        vec[i] = silu_q16(vec[i]);
    }
}

/// In-place element-wise multiply in Q16.16.
#[allow(dead_code)]
pub(crate) fn elementwise_mul_inplace_q16(a: &mut [i32], b: &[i32], len: usize) {
    for i in 0..len {
        a[i] = ((a[i] as i64 * b[i] as i64) >> 16) as i32;
    }
}

/// Find the index of the maximum value (argmax).
#[allow(dead_code)]
pub(crate) fn argmax_q16(data: &[i32], len: usize) -> usize {
    let mut best = 0;
    for i in 1..len {
        if data[i] > data[best] {
            best = i;
        }
    }
    best
}
