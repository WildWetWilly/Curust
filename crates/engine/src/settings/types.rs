//! `settings/types/*`: facades around doubles used by the settings system.
//!
//! In C++ `Ratio`, `Velocity`, `Acceleration` and `Temperature` behave exactly like `double`, so
//! they are plain `f64` aliases here. `Duration` clamps negative values to zero and the angle
//! types use clock arithmetic, so those are newtypes.

use std::f64::consts::PI;
use std::ops::{Add, AddAssign, Mul, Sub, SubAssign};

pub type Ratio = f64;
pub type Velocity = f64;
pub type Acceleration = f64;
pub type Temperature = f64;

/// Layer index (may be negative for raft layers). `Settings::get<LayerIndex>` subtracts one.
pub type LayerIndex = i64;

pub const TAU: f64 = 2.0 * PI;

/// A duration in seconds; never negative.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Duration(f64);

impl Duration {
    #[inline]
    pub fn new(value: f64) -> Self {
        Duration(if value > 0.0 { value } else { 0.0 })
    }
    #[inline]
    pub fn value(&self) -> f64 {
        self.0
    }
}

impl From<f64> for Duration {
    fn from(v: f64) -> Self {
        Duration::new(v)
    }
}
impl From<Duration> for f64 {
    fn from(d: Duration) -> f64 {
        d.0
    }
}
impl Add for Duration {
    type Output = Duration;
    fn add(self, o: Duration) -> Duration {
        Duration::new(self.0 + o.0)
    }
}
impl Sub for Duration {
    type Output = Duration;
    fn sub(self, o: Duration) -> Duration {
        Duration::new(self.0 - o.0)
    }
}
impl AddAssign for Duration {
    /// Note: like C++, `+=` does not clamp.
    fn add_assign(&mut self, o: Duration) {
        self.0 += o.0;
    }
}
impl SubAssign for Duration {
    /// Note: like C++, `-=` does not clamp.
    fn sub_assign(&mut self, o: Duration) {
        self.0 -= o.0;
    }
}
impl Mul<Duration> for f64 {
    type Output = Duration;
    fn mul(self, d: Duration) -> Duration {
        Duration::new(self * d.0)
    }
}

#[inline]
fn wrap(value: f64, modulo: f64) -> f64 {
    // std::fmod(std::fmod(value, m) + m, m); Rust's % on floats is fmod.
    ((value % modulo) + modulo) % modulo
}

/// An angle in degrees, always in [0, 360).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct AngleDegrees(f64);

impl AngleDegrees {
    #[inline]
    pub fn new(value: f64) -> Self {
        AngleDegrees(wrap(value, 360.0))
    }
    #[inline]
    pub fn value(&self) -> f64 {
        self.0
    }
}

impl From<AngleRadians> for AngleDegrees {
    /// `AngleDegrees(const AngleRadians&)`: `value * 360 / TAU` (no normalisation).
    fn from(r: AngleRadians) -> Self {
        AngleDegrees(r.0 * 360.0 / TAU)
    }
}
impl From<f64> for AngleDegrees {
    fn from(v: f64) -> Self {
        AngleDegrees::new(v)
    }
}
impl Add for AngleDegrees {
    type Output = AngleDegrees;
    /// Note: the C++ operator returns a double which is converted back through the constructor, so the
    /// value is wrapped twice.
    fn add(self, o: AngleDegrees) -> AngleDegrees {
        AngleDegrees::new(wrap(self.0 + o.0, 360.0))
    }
}
impl Sub for AngleDegrees {
    type Output = AngleDegrees;
    /// Note: wrapped twice, like the C++ operator (see `Add`).
    fn sub(self, o: AngleDegrees) -> AngleDegrees {
        AngleDegrees::new(wrap(self.0 - o.0, 360.0))
    }
}
impl AddAssign for AngleDegrees {
    fn add_assign(&mut self, o: AngleDegrees) {
        self.0 = wrap(self.0 + o.0, 360.0);
    }
}
impl SubAssign for AngleDegrees {
    fn sub_assign(&mut self, o: AngleDegrees) {
        self.0 = wrap(self.0 - o.0, 360.0);
    }
}

/// An angle in radians, always in [0, 2*pi).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct AngleRadians(f64);

impl AngleRadians {
    #[inline]
    pub fn new(value: f64) -> Self {
        AngleRadians(wrap(value, TAU))
    }
    #[inline]
    pub fn value(&self) -> f64 {
        self.0
    }
}

impl From<AngleDegrees> for AngleRadians {
    /// `AngleRadians(const AngleDegrees&)`: `value * TAU / 360` (no normalisation).
    fn from(d: AngleDegrees) -> Self {
        AngleRadians(d.0 * TAU / 360.0)
    }
}
impl From<f64> for AngleRadians {
    fn from(v: f64) -> Self {
        AngleRadians::new(v)
    }
}
impl Add for AngleRadians {
    type Output = AngleRadians;
    /// Note: wrapped twice, like the C++ operator (the returned double goes through the constructor).
    fn add(self, o: AngleRadians) -> AngleRadians {
        AngleRadians::new(wrap(self.0 + o.0, TAU))
    }
}
impl Sub for AngleRadians {
    type Output = AngleRadians;
    /// Note: wrapped twice, like the C++ operator.
    fn sub(self, o: AngleRadians) -> AngleRadians {
        AngleRadians::new(wrap(self.0 - o.0, TAU))
    }
}
impl AddAssign for AngleRadians {
    fn add_assign(&mut self, o: AngleRadians) {
        self.0 = wrap(self.0 + o.0, TAU);
    }
}
impl SubAssign for AngleRadians {
    fn sub_assign(&mut self, o: AngleRadians) {
        self.0 = wrap(self.0 - o.0, TAU);
    }
}

/// `boundedTan`: tangent limited to slightly less than pi/2.
#[inline]
pub fn bounded_tan(angle: AngleRadians) -> f64 {
    angle.0.min(PI / 2.0 - 0.001).tan()
}
