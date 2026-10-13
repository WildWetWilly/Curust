//! Port of `boost/polygon/detail/voronoi_robust_fpt.hpp`.

use crate::ctypes::{BigInt, Efpt};

const ROUNDING_ERROR: f64 = 1.0;

#[inline]
fn max_f(a: f64, b: f64) -> f64 {
    // std::max(a, b) == (a < b) ? b : a
    if a < b {
        b
    } else {
        a
    }
}

/// `robust_fpt<double>`: a value with its relative error (in EPS).
#[derive(Clone, Copy, Debug)]
pub struct RobustFpt {
    fpv: f64,
    re: f64,
}

impl Default for RobustFpt {
    fn default() -> Self {
        RobustFpt { fpv: 0.0, re: 0.0 }
    }
}

impl RobustFpt {
    #[inline]
    pub fn new(fpv: f64) -> Self {
        RobustFpt { fpv, re: 0.0 }
    }

    #[inline]
    pub fn with_error(fpv: f64, error: f64) -> Self {
        RobustFpt { fpv, re: error }
    }

    #[inline]
    pub fn fpv(&self) -> f64 {
        self.fpv
    }

    #[inline]
    pub fn re(&self) -> f64 {
        self.re
    }

    #[inline]
    pub fn ulp(&self) -> f64 {
        self.re
    }

    #[inline]
    pub fn is_pos(&self) -> bool {
        self.fpv > 0.0
    }

    #[inline]
    pub fn is_neg(&self) -> bool {
        self.fpv < 0.0
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.fpv == 0.0
    }

    #[inline]
    pub fn neg(&self) -> Self {
        RobustFpt { fpv: -self.fpv, re: self.re }
    }

    pub fn add_assign(&mut self, that: &RobustFpt) {
        let fpv = self.fpv + that.fpv;
        if (!(self.fpv < 0.0) && !(that.fpv < 0.0)) || (!(self.fpv > 0.0) && !(that.fpv > 0.0)) {
            self.re = max_f(self.re, that.re) + ROUNDING_ERROR;
        } else {
            let mut temp = (self.fpv * self.re - that.fpv * that.re) / fpv;
            if temp < 0.0 {
                temp = -temp;
            }
            self.re = temp + ROUNDING_ERROR;
        }
        self.fpv = fpv;
    }

    pub fn sub_assign(&mut self, that: &RobustFpt) {
        let fpv = self.fpv - that.fpv;
        if (!(self.fpv < 0.0) && !(that.fpv > 0.0)) || (!(self.fpv > 0.0) && !(that.fpv < 0.0)) {
            self.re = max_f(self.re, that.re) + ROUNDING_ERROR;
        } else {
            let mut temp = (self.fpv * self.re + that.fpv * that.re) / fpv;
            if temp < 0.0 {
                temp = -temp;
            }
            self.re = temp + ROUNDING_ERROR;
        }
        self.fpv = fpv;
    }

    pub fn mul_assign(&mut self, that: &RobustFpt) {
        self.re += that.re + ROUNDING_ERROR;
        self.fpv *= that.fpv;
    }

    pub fn div_assign(&mut self, that: &RobustFpt) {
        self.re += that.re + ROUNDING_ERROR;
        self.fpv /= that.fpv;
    }

    pub fn add(&self, that: &RobustFpt) -> RobustFpt {
        let fpv = self.fpv + that.fpv;
        let re;
        if (!(self.fpv < 0.0) && !(that.fpv < 0.0)) || (!(self.fpv > 0.0) && !(that.fpv > 0.0)) {
            re = max_f(self.re, that.re) + ROUNDING_ERROR;
        } else {
            let mut temp = (self.fpv * self.re - that.fpv * that.re) / fpv;
            if temp < 0.0 {
                temp = -temp;
            }
            re = temp + ROUNDING_ERROR;
        }
        RobustFpt { fpv, re }
    }

    pub fn sub(&self, that: &RobustFpt) -> RobustFpt {
        let fpv = self.fpv - that.fpv;
        let re;
        if (!(self.fpv < 0.0) && !(that.fpv > 0.0)) || (!(self.fpv > 0.0) && !(that.fpv < 0.0)) {
            re = max_f(self.re, that.re) + ROUNDING_ERROR;
        } else {
            let mut temp = (self.fpv * self.re + that.fpv * that.re) / fpv;
            if temp < 0.0 {
                temp = -temp;
            }
            re = temp + ROUNDING_ERROR;
        }
        RobustFpt { fpv, re }
    }

    pub fn mul(&self, that: &RobustFpt) -> RobustFpt {
        let fpv = self.fpv * that.fpv;
        let re = self.re + that.re + ROUNDING_ERROR;
        RobustFpt { fpv, re }
    }

    pub fn div(&self, that: &RobustFpt) -> RobustFpt {
        let fpv = self.fpv / that.fpv;
        let re = self.re + that.re + ROUNDING_ERROR;
        RobustFpt { fpv, re }
    }

    pub fn sqrt(&self) -> RobustFpt {
        RobustFpt { fpv: self.fpv.sqrt(), re: self.re * 0.5 + ROUNDING_ERROR }
    }
}

impl std::ops::Add for RobustFpt {
    type Output = RobustFpt;
    fn add(self, rhs: RobustFpt) -> RobustFpt {
        RobustFpt::add(&self, &rhs)
    }
}

impl std::ops::Sub for RobustFpt {
    type Output = RobustFpt;
    fn sub(self, rhs: RobustFpt) -> RobustFpt {
        RobustFpt::sub(&self, &rhs)
    }
}

impl std::ops::Mul for RobustFpt {
    type Output = RobustFpt;
    fn mul(self, rhs: RobustFpt) -> RobustFpt {
        RobustFpt::mul(&self, &rhs)
    }
}

impl std::ops::Div for RobustFpt {
    type Output = RobustFpt;
    fn div(self, rhs: RobustFpt) -> RobustFpt {
        RobustFpt::div(&self, &rhs)
    }
}

impl std::ops::Neg for RobustFpt {
    type Output = RobustFpt;
    fn neg(self) -> RobustFpt {
        RobustFpt::neg(&self)
    }
}

/// `robust_dif<robust_fpt<double>>`: a value kept as the difference of two non-negative sums.
#[derive(Clone, Copy, Debug, Default)]
pub struct RobustDif {
    positive_sum: RobustFpt,
    negative_sum: RobustFpt,
}

impl RobustDif {
    pub fn new() -> Self {
        RobustDif { positive_sum: RobustFpt::new(0.0), negative_sum: RobustFpt::new(0.0) }
    }

    pub fn from_pos_neg(pos: RobustFpt, neg: RobustFpt) -> Self {
        RobustDif { positive_sum: pos, negative_sum: neg }
    }

    #[inline]
    pub fn dif(&self) -> RobustFpt {
        self.positive_sum.sub(&self.negative_sum)
    }

    #[inline]
    pub fn pos(&self) -> RobustFpt {
        self.positive_sum
    }

    #[inline]
    pub fn neg_sum(&self) -> RobustFpt {
        self.negative_sum
    }

    /// Unary minus.
    pub fn negate(&self) -> RobustDif {
        RobustDif { positive_sum: self.negative_sum, negative_sum: self.positive_sum }
    }

    /// `operator+=(const T&)`.
    pub fn add_val(&mut self, val: RobustFpt) {
        if !val.is_neg() {
            self.positive_sum.add_assign(&val);
        } else {
            self.negative_sum.sub_assign(&val);
        }
    }

    /// `operator+=(const robust_dif&)`.
    pub fn add_dif(&mut self, that: &RobustDif) {
        self.positive_sum.add_assign(&that.positive_sum);
        self.negative_sum.add_assign(&that.negative_sum);
    }

    /// `operator-=(const T&)`.
    pub fn sub_val(&mut self, val: RobustFpt) {
        if !val.is_neg() {
            self.negative_sum.add_assign(&val);
        } else {
            self.positive_sum.sub_assign(&val);
        }
    }

    /// `operator-=(const robust_dif&)`.
    pub fn sub_dif(&mut self, that: &RobustDif) {
        self.positive_sum.add_assign(&that.negative_sum);
        self.negative_sum.add_assign(&that.positive_sum);
    }

    /// `operator*=(const T&)`.
    pub fn mul_val(&mut self, val: RobustFpt) {
        if !val.is_neg() {
            self.positive_sum.mul_assign(&val);
            self.negative_sum.mul_assign(&val);
        } else {
            let nv = val.neg();
            self.positive_sum.mul_assign(&nv);
            self.negative_sum.mul_assign(&nv);
            std::mem::swap(&mut self.positive_sum, &mut self.negative_sum);
        }
    }

    /// `operator/=(const T&)`.
    pub fn div_val(&mut self, val: RobustFpt) {
        if !val.is_neg() {
            self.positive_sum.div_assign(&val);
            self.negative_sum.div_assign(&val);
        } else {
            let nv = val.neg();
            self.positive_sum.div_assign(&nv);
            self.negative_sum.div_assign(&nv);
            std::mem::swap(&mut self.positive_sum, &mut self.negative_sum);
        }
    }

    /// `operator+(robust_dif, robust_dif)`.
    pub fn plus(&self, rhs: &RobustDif) -> RobustDif {
        RobustDif::from_pos_neg(self.pos().add(&rhs.pos()), self.neg_sum().add(&rhs.neg_sum()))
    }

    /// `operator*(robust_dif, T)`.
    pub fn times_val(&self, val: RobustFpt) -> RobustDif {
        if !val.is_neg() {
            RobustDif::from_pos_neg(self.pos().mul(&val), self.neg_sum().mul(&val))
        } else {
            RobustDif::from_pos_neg(self.neg_sum().neg().mul(&val), self.pos().neg().mul(&val))
        }
    }

    /// `operator*(T, robust_dif)`.
    pub fn val_times(val: RobustFpt, rhs: &RobustDif) -> RobustDif {
        if !val.is_neg() {
            RobustDif::from_pos_neg(val.mul(&rhs.pos()), val.mul(&rhs.neg_sum()))
        } else {
            RobustDif::from_pos_neg(val.neg().mul(&rhs.neg_sum()), val.neg().mul(&rhs.pos()))
        }
    }
}

/// `robust_sqrt_expr<extended_int<64>, extended_exponent_fpt<double>, type_converter_efpt>`.
pub struct RobustSqrtExpr;

#[inline]
fn is_neg_e(e: &Efpt) -> bool {
    e.is_neg()
}

#[inline]
fn is_pos_e(e: &Efpt) -> bool {
    e.is_pos()
}

impl RobustSqrtExpr {
    /// A[0] * sqrt(B[0]).
    pub fn eval1(a_: &[BigInt], b_: &[BigInt]) -> Efpt {
        let a = a_[0].to_efpt();
        let b = b_[0].to_efpt();
        a * b.sqrt()
    }

    /// A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]).
    pub fn eval2(a_: &[BigInt], b_: &[BigInt]) -> Efpt {
        let a = Self::eval1(a_, b_);
        let b = Self::eval1(&a_[1..], &b_[1..]);
        if (!is_neg_e(&a) && !is_neg_e(&b)) || (!is_pos_e(&a) && !is_pos_e(&b)) {
            return a + b;
        }
        let num = &(&(&a_[0] * &a_[0]) * &b_[0]) - &(&(&a_[1] * &a_[1]) * &b_[1]);
        num.to_efpt() / (a - b)
    }

    /// A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]) + A[2] * sqrt(B[2]).
    pub fn eval3(a_: &[BigInt], b_: &[BigInt]) -> Efpt {
        let a = Self::eval2(a_, b_);
        let b = Self::eval1(&a_[2..], &b_[2..]);
        if (!is_neg_e(&a) && !is_neg_e(&b)) || (!is_pos_e(&a) && !is_pos_e(&b)) {
            return a + b;
        }
        let t_a3 = &(&(&(&a_[0] * &a_[0]) * &b_[0]) + &(&(&a_[1] * &a_[1]) * &b_[1])) - &(&(&a_[2] * &a_[2]) * &b_[2]);
        let t_b3 = BigInt::from_i32(1);
        let t_a4 = (&a_[0] * &a_[1]).mul_i32(2);
        let t_b4 = &b_[0] * &b_[1];
        let ta = [t_a3, t_a4];
        let tb = [t_b3, t_b4];
        Self::eval2(&ta, &tb) / (a - b)
    }

    /// A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]) + A[2] * sqrt(B[2]) + A[3] * sqrt(B[3]).
    pub fn eval4(a_: &[BigInt], b_: &[BigInt]) -> Efpt {
        let a = Self::eval2(a_, b_);
        let b = Self::eval2(&a_[2..], &b_[2..]);
        if (!is_neg_e(&a) && !is_neg_e(&b)) || (!is_pos_e(&a) && !is_pos_e(&b)) {
            return a + b;
        }
        let t_a0 = &(&(&(&(&a_[0] * &a_[0]) * &b_[0]) + &(&(&a_[1] * &a_[1]) * &b_[1])) - &(&(&a_[2] * &a_[2]) * &b_[2]))
            - &(&(&a_[3] * &a_[3]) * &b_[3]);
        let t_b0 = BigInt::from_i32(1);
        let t_a1 = (&a_[0] * &a_[1]).mul_i32(2);
        let t_b1 = &b_[0] * &b_[1];
        let t_a2 = (&a_[2] * &a_[3]).mul_i32(-2);
        let t_b2 = &b_[2] * &b_[3];
        let ta = [t_a0, t_a1, t_a2];
        let tb = [t_b0, t_b1, t_b2];
        Self::eval3(&ta, &tb) / (a - b)
    }
}
