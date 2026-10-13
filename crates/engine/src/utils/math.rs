//! `utils/math.h`.

#[inline]
pub fn square<T: std::ops::Mul<Output = T> + Copy>(a: T) -> T {
    a * a
}

#[inline]
pub fn round_divide_signed(dividend: i64, divisor: i64) -> i64 {
    if (dividend < 0) ^ (divisor < 0) {
        return (dividend - divisor / 2) / divisor;
    }
    (dividend + divisor / 2) / divisor
}

#[inline]
pub fn ceil_divide_signed(dividend: i64, divisor: i64) -> i64 {
    let mut quotient = dividend / divisor;
    let remainder = dividend % divisor;
    if remainder != 0 && ((dividend > 0 && divisor > 0) || (dividend < 0 && divisor < 0)) {
        quotient += 1;
    }
    quotient
}

#[inline]
pub fn floor_divide_signed(dividend: i64, divisor: i64) -> i64 {
    let quotient = dividend / divisor;
    let remainder = dividend % divisor;
    if remainder != 0 && ((dividend > 0 && divisor < 0) || (dividend < 0 && divisor > 0)) {
        return quotient - 1;
    }
    quotient
}

#[inline]
pub fn round_divide(dividend: u64, divisor: u64) -> u64 {
    (dividend + divisor / 2) / divisor
}

#[inline]
pub fn round_up_divide(dividend: u64, divisor: u64) -> u64 {
    (dividend + divisor - 1) / divisor
}

/// `inverse_lerp` for floating point values.
#[inline]
pub fn inverse_lerp(range_min: f64, range_max: f64, value: f64) -> f64 {
    if range_min == range_max {
        return 0.0;
    }
    (value - range_min) / (range_max - range_min)
}

/// `inverse_lerp` for integer values (`static_cast<double>(value - range_min) / (range_max - range_min)`).
#[inline]
pub fn inverse_lerp_i(range_min: i64, range_max: i64, value: i64) -> f64 {
    if range_min == range_max {
        return 0.0;
    }
    (value - range_min) as f64 / (range_max - range_min) as f64
}

/// `is_zero` for doubles.
#[inline]
pub fn is_zero(value: f64) -> bool {
    value.abs() < f64::EPSILON * 100.0
}

/// `sign`: 1 for positive (including +0), -1 for negative.
#[inline]
pub fn sign(value: f64) -> i8 {
    if value.is_sign_negative() {
        -1
    } else {
        1
    }
}
