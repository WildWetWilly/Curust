//! Port of `boost/polygon/detail/voronoi_ctypes.hpp`: ULP comparison, extended exponent
//! floating point numbers and fixed size big integers.

use std::cmp::Ordering;

extern "C" {
    fn frexp(x: f64, exp: *mut i32) -> f64;
    fn ldexp(x: f64, exp: i32) -> f64;
}

#[inline]
pub fn c_frexp(x: f64) -> (f64, i32) {
    let mut e: i32 = 0;
    // SAFETY: plain libm call with a valid out pointer.
    let v = unsafe { frexp(x, &mut e) };
    (v, e)
}

#[inline]
pub fn c_ldexp(x: f64, e: i32) -> f64 {
    // SAFETY: plain libm call.
    unsafe { ldexp(x, e) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UlpResult {
    Less = -1,
    Equal = 0,
    More = 1,
}

/// `ulp_comparison<fpt64>::operator()`.
pub fn ulp_cmp(a: f64, b: f64, max_ulps: u32) -> UlpResult {
    let mut ll_a = a.to_bits();
    let mut ll_b = b.to_bits();
    if ll_a < 0x8000000000000000u64 {
        ll_a = 0x8000000000000000u64 - ll_a;
    }
    if ll_b < 0x8000000000000000u64 {
        ll_b = 0x8000000000000000u64 - ll_b;
    }
    if ll_a > ll_b {
        return if ll_a - ll_b <= max_ulps as u64 { UlpResult::Equal } else { UlpResult::Less };
    }
    if ll_b - ll_a <= max_ulps as u64 {
        UlpResult::Equal
    } else {
        UlpResult::More
    }
}

const MAX_SIGNIFICANT_EXP_DIF: i32 = 54;

/// `extended_exponent_fpt<double>`.
#[derive(Clone, Copy, Debug)]
pub struct Efpt {
    val: f64,
    exp: i32,
}

impl Efpt {
    pub fn new(val: f64) -> Efpt {
        let (v, e) = c_frexp(val);
        Efpt { val: v, exp: e }
    }

    pub fn with_exp(val: f64, exp: i32) -> Efpt {
        let (v, e) = c_frexp(val);
        Efpt { val: v, exp: e.wrapping_add(exp) }
    }

    #[inline]
    pub fn is_pos(&self) -> bool {
        self.val > 0.0
    }

    #[inline]
    pub fn is_neg(&self) -> bool {
        self.val < 0.0
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.val == 0.0
    }

    pub fn neg(&self) -> Efpt {
        Efpt::with_exp(-self.val, self.exp)
    }

    pub fn add(&self, that: &Efpt) -> Efpt {
        if self.val == 0.0 || that.exp > self.exp + MAX_SIGNIFICANT_EXP_DIF {
            return *that;
        }
        if that.val == 0.0 || self.exp > that.exp + MAX_SIGNIFICANT_EXP_DIF {
            return *self;
        }
        if self.exp >= that.exp {
            let exp_dif = self.exp - that.exp;
            let val = c_ldexp(self.val, exp_dif) + that.val;
            Efpt::with_exp(val, that.exp)
        } else {
            let exp_dif = that.exp - self.exp;
            let val = c_ldexp(that.val, exp_dif) + self.val;
            Efpt::with_exp(val, self.exp)
        }
    }

    pub fn sub(&self, that: &Efpt) -> Efpt {
        if self.val == 0.0 || that.exp > self.exp + MAX_SIGNIFICANT_EXP_DIF {
            return Efpt::with_exp(-that.val, that.exp);
        }
        if that.val == 0.0 || self.exp > that.exp + MAX_SIGNIFICANT_EXP_DIF {
            return *self;
        }
        if self.exp >= that.exp {
            let exp_dif = self.exp - that.exp;
            let val = c_ldexp(self.val, exp_dif) - that.val;
            Efpt::with_exp(val, that.exp)
        } else {
            let exp_dif = that.exp - self.exp;
            let val = c_ldexp(-that.val, exp_dif) + self.val;
            Efpt::with_exp(val, self.exp)
        }
    }

    pub fn mul(&self, that: &Efpt) -> Efpt {
        let val = self.val * that.val;
        let exp = self.exp.wrapping_add(that.exp);
        Efpt::with_exp(val, exp)
    }

    pub fn div(&self, that: &Efpt) -> Efpt {
        let val = self.val / that.val;
        let exp = self.exp.wrapping_sub(that.exp);
        Efpt::with_exp(val, exp)
    }

    pub fn sqrt(&self) -> Efpt {
        let mut val = self.val;
        let mut exp = self.exp;
        if exp & 1 != 0 {
            val *= 2.0;
            exp -= 1;
        }
        Efpt::with_exp(val.sqrt(), exp >> 1)
    }

    pub fn d(&self) -> f64 {
        c_ldexp(self.val, self.exp)
    }
}

pub const BIG_N: usize = 64;

/// `extended_int<64>`.
#[derive(Clone)]
pub struct BigInt {
    chunks: [u32; BIG_N],
    count: i32,
}

impl std::fmt::Debug for BigInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BigInt({}, {:?})", self.count, &self.chunks[..self.size()])
    }
}

impl Default for BigInt {
    fn default() -> Self {
        BigInt { chunks: [0; BIG_N], count: 0 }
    }
}

impl From<i64> for BigInt {
    fn from(that: i64) -> Self {
        BigInt::from_i64(that)
    }
}

impl From<i32> for BigInt {
    fn from(that: i32) -> Self {
        BigInt::from_i32(that)
    }
}

impl BigInt {
    pub fn from_i32(that: i32) -> BigInt {
        let mut r = BigInt::default();
        if that > 0 {
            r.chunks[0] = that as u32;
            r.count = 1;
        } else if that < 0 {
            r.chunks[0] = that.wrapping_neg() as u32;
            r.count = -1;
        } else {
            r.count = 0;
        }
        r
    }

    pub fn from_i64(that: i64) -> BigInt {
        let mut r = BigInt::default();
        if that > 0 {
            r.chunks[0] = that as u32;
            r.chunks[1] = (that >> 32) as u32;
            r.count = if r.chunks[1] != 0 { 2 } else { 1 };
        } else if that < 0 {
            let that = that.wrapping_neg();
            r.chunks[0] = that as u32;
            r.chunks[1] = (that >> 32) as u32;
            r.count = if r.chunks[1] != 0 { -2 } else { -1 };
        } else {
            r.count = 0;
        }
        r
    }

    #[inline]
    pub fn is_pos(&self) -> bool {
        self.count > 0
    }

    #[inline]
    pub fn is_neg(&self) -> bool {
        self.count < 0
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.count == 0
    }

    #[inline]
    pub fn count(&self) -> i32 {
        self.count
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.count.unsigned_abs() as usize
    }

    pub fn chunks(&self) -> &[u32] {
        &self.chunks
    }

    pub fn cmp_lt(&self, that: &BigInt) -> bool {
        if self.count != that.count {
            return self.count < that.count;
        }
        let mut i = self.size();
        if i == 0 {
            return false;
        }
        loop {
            i -= 1;
            if self.chunks[i] != that.chunks[i] {
                return (self.chunks[i] < that.chunks[i]) ^ (self.count < 0);
            }
            if i == 0 {
                break;
            }
        }
        false
    }

    pub fn eq(&self, that: &BigInt) -> bool {
        if self.count != that.count {
            return false;
        }
        for i in 0..self.size() {
            if self.chunks[i] != that.chunks[i] {
                return false;
            }
        }
        true
    }

    pub fn cmp(&self, that: &BigInt) -> Ordering {
        if self.cmp_lt(that) {
            Ordering::Less
        } else if that.cmp_lt(self) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }

    pub fn neg(&self) -> BigInt {
        let mut r = self.clone();
        r.count = -r.count;
        r
    }

    pub fn add(&self, e2: &BigInt) -> BigInt {
        let e1 = self;
        if e1.count == 0 {
            return e2.clone();
        }
        if e2.count == 0 {
            return e1.clone();
        }
        let mut r = BigInt::default();
        if (e1.count > 0) ^ (e2.count > 0) {
            r.dif_chunks(&e1.chunks, e1.size(), &e2.chunks, e2.size(), false);
        } else {
            r.add_chunks(&e1.chunks, e1.size(), &e2.chunks, e2.size());
        }
        if e1.count < 0 {
            r.count = -r.count;
        }
        r
    }

    pub fn sub(&self, e2: &BigInt) -> BigInt {
        let e1 = self;
        if e1.count == 0 {
            let mut r = e2.clone();
            r.count = -r.count;
            return r;
        }
        if e2.count == 0 {
            return e1.clone();
        }
        let mut r = BigInt::default();
        if (e1.count > 0) ^ (e2.count > 0) {
            r.add_chunks(&e1.chunks, e1.size(), &e2.chunks, e2.size());
        } else {
            r.dif_chunks(&e1.chunks, e1.size(), &e2.chunks, e2.size(), false);
        }
        if e1.count < 0 {
            r.count = -r.count;
        }
        r
    }

    pub fn mul(&self, e2: &BigInt) -> BigInt {
        let e1 = self;
        let mut r = BigInt::default();
        if e1.count == 0 || e2.count == 0 {
            r.count = 0;
            return r;
        }
        r.mul_chunks(&e1.chunks, e1.size(), &e2.chunks, e2.size());
        if (e1.count > 0) ^ (e2.count > 0) {
            r.count = -r.count;
        }
        r
    }

    /// `operator*(int32)` / `operator*(int64)`.
    pub fn mul_i64(&self, that: i64) -> BigInt {
        self.mul(&BigInt::from_i64(that))
    }

    pub fn mul_i32(&self, that: i32) -> BigInt {
        self.mul(&BigInt::from_i32(that))
    }

    pub fn p(&self) -> (f64, i32) {
        let mut ret: (f64, i32) = (0.0, 0);
        let sz = self.size();
        if sz == 0 {
            return ret;
        } else if sz == 1 {
            ret.0 = self.chunks[0] as f64;
        } else if sz == 2 {
            ret.0 = self.chunks[1] as f64 * 4294967296.0 + self.chunks[0] as f64;
        } else {
            for i in 1..=3 {
                ret.0 *= 4294967296.0;
                ret.0 += self.chunks[sz - i] as f64;
            }
            ret.1 = ((sz - 3) << 5) as i32;
        }
        if self.count < 0 {
            ret.0 = -ret.0;
        }
        ret
    }

    pub fn d(&self) -> f64 {
        let p = self.p();
        c_ldexp(p.0, p.1)
    }

    /// `type_converter_efpt`.
    pub fn to_efpt(&self) -> Efpt {
        let p = self.p();
        Efpt::with_exp(p.0, p.1)
    }

    fn add_chunks(&mut self, c1: &[u32], sz1: usize, c2: &[u32], sz2: usize) {
        if sz1 < sz2 {
            self.add_chunks(c2, sz2, c1, sz1);
            return;
        }
        self.count = sz1 as i32;
        let mut temp: u64 = 0;
        for i in 0..sz2 {
            temp = temp.wrapping_add((c1[i] as u64).wrapping_add(c2[i] as u64));
            self.chunks[i] = temp as u32;
            temp >>= 32;
        }
        for i in sz2..sz1 {
            temp = temp.wrapping_add(c1[i] as u64);
            self.chunks[i] = temp as u32;
            temp >>= 32;
        }
        if temp != 0 && self.count != BIG_N as i32 {
            self.chunks[self.count as usize] = temp as u32;
            self.count += 1;
        }
    }

    fn dif_chunks(&mut self, c1: &[u32], mut sz1: usize, c2: &[u32], mut sz2: usize, rec: bool) {
        if sz1 < sz2 {
            self.dif_chunks(c2, sz2, c1, sz1, true);
            self.count = -self.count;
            return;
        } else if sz1 == sz2 && !rec {
            loop {
                sz1 -= 1;
                if c1[sz1] < c2[sz1] {
                    sz1 += 1;
                    self.dif_chunks(c2, sz1, c1, sz1, true);
                    self.count = -self.count;
                    return;
                } else if c1[sz1] > c2[sz1] {
                    sz1 += 1;
                    break;
                }
                if sz1 == 0 {
                    break;
                }
            }
            if sz1 == 0 {
                self.count = 0;
                return;
            }
            sz2 = sz1;
        }
        self.count = (sz1 - 1) as i32;
        let mut flag = false;
        for i in 0..sz2 {
            self.chunks[i] = c1[i].wrapping_sub(c2[i]).wrapping_sub(flag as u32);
            flag = (c1[i] < c2[i]) || ((c1[i] == c2[i]) && flag);
        }
        for i in sz2..sz1 {
            self.chunks[i] = c1[i].wrapping_sub(flag as u32);
            flag = c1[i] == 0 && flag;
        }
        if self.chunks[self.count as usize] != 0 {
            self.count += 1;
        }
    }

    fn mul_chunks(&mut self, c1: &[u32], sz1: usize, c2: &[u32], sz2: usize) {
        let mut cur: u64 = 0;
        let mut nxt: u64;
        let mut tmp: u64;
        self.count = BIG_N.min(sz1 + sz2 - 1) as i32;
        for shift in 0..self.count as usize {
            nxt = 0;
            for first in 0..=shift {
                if first >= sz1 {
                    break;
                }
                let second = shift - first;
                if second >= sz2 {
                    continue;
                }
                tmp = (c1[first] as u64).wrapping_mul(c2[second] as u64);
                cur = cur.wrapping_add(tmp as u32 as u64);
                nxt = nxt.wrapping_add(tmp >> 32);
            }
            self.chunks[shift] = cur as u32;
            cur = nxt.wrapping_add(cur >> 32);
        }
        if cur != 0 && self.count != BIG_N as i32 {
            self.chunks[self.count as usize] = cur as u32;
            self.count += 1;
        }
    }
}

// Operator sugar so the predicate code can mirror the C++ expressions. Every operator creates
// a fresh value, exactly like the C++ binary operators.
macro_rules! big_binop {
    ($tr:ident, $f:ident, $m:ident) => {
        impl std::ops::$tr<&BigInt> for &BigInt {
            type Output = BigInt;
            fn $f(self, rhs: &BigInt) -> BigInt {
                self.$m(rhs)
            }
        }
        impl std::ops::$tr<BigInt> for BigInt {
            type Output = BigInt;
            fn $f(self, rhs: BigInt) -> BigInt {
                (&self).$m(&rhs)
            }
        }
        impl std::ops::$tr<&BigInt> for BigInt {
            type Output = BigInt;
            fn $f(self, rhs: &BigInt) -> BigInt {
                (&self).$m(rhs)
            }
        }
        impl std::ops::$tr<BigInt> for &BigInt {
            type Output = BigInt;
            fn $f(self, rhs: BigInt) -> BigInt {
                self.$m(&rhs)
            }
        }
    };
}
big_binop!(Add, add, add);
big_binop!(Sub, sub, sub);
big_binop!(Mul, mul, mul);

impl std::ops::Mul<i64> for &BigInt {
    type Output = BigInt;
    fn mul(self, rhs: i64) -> BigInt {
        self.mul_i64(rhs)
    }
}

impl std::ops::Mul<i64> for BigInt {
    type Output = BigInt;
    fn mul(self, rhs: i64) -> BigInt {
        self.mul_i64(rhs)
    }
}

impl std::ops::Neg for &BigInt {
    type Output = BigInt;
    fn neg(self) -> BigInt {
        BigInt::neg(self)
    }
}

impl std::ops::Neg for BigInt {
    type Output = BigInt;
    fn neg(self) -> BigInt {
        BigInt::neg(&self)
    }
}

impl std::ops::Add for Efpt {
    type Output = Efpt;
    fn add(self, rhs: Efpt) -> Efpt {
        Efpt::add(&self, &rhs)
    }
}

impl std::ops::Sub for Efpt {
    type Output = Efpt;
    fn sub(self, rhs: Efpt) -> Efpt {
        Efpt::sub(&self, &rhs)
    }
}

impl std::ops::Mul for Efpt {
    type Output = Efpt;
    fn mul(self, rhs: Efpt) -> Efpt {
        Efpt::mul(&self, &rhs)
    }
}

impl std::ops::Div for Efpt {
    type Output = Efpt;
    fn div(self, rhs: Efpt) -> Efpt {
        Efpt::div(&self, &rhs)
    }
}
