//! Small 2D geometry types used by the engine and renderer.

use serde::{Deserialize, Serialize};
use std::ops::{Add, Div, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

pub const fn v2(x: f64, y: f64) -> Vec2 {
    Vec2 { x, y }
}

impl Vec2 {
    pub const ZERO: Vec2 = v2(0.0, 0.0);
    pub const ONE: Vec2 = v2(1.0, 1.0);

    pub fn splat(v: f64) -> Self {
        v2(v, v)
    }
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn min_elem(self) -> f64 {
        self.x.min(self.y)
    }
    pub fn max_elem(self) -> f64 {
        self.x.max(self.y)
    }
    pub fn avg(self) -> f64 {
        (self.x + self.y) / 2.0
    }
    pub fn mul(self, o: Vec2) -> Vec2 {
        v2(self.x * o.x, self.y * o.y)
    }
    pub fn div(self, o: Vec2) -> Vec2 {
        v2(self.x / o.x, self.y / o.y)
    }
    pub fn aspect(self) -> f64 {
        self.x / self.y
    }
    pub fn lerp(self, o: Vec2, t: f64) -> Vec2 {
        self + (o - self) * t
    }
    pub fn clamp(self, lo: f64, hi: f64) -> Vec2 {
        v2(self.x.clamp(lo, hi), self.y.clamp(lo, hi))
    }
    /// Grow one side so the size has the given aspect ratio.
    pub fn expand_to_aspect(self, aspect: f64) -> Vec2 {
        if self.aspect() > aspect { v2(self.x, self.x / aspect) } else { v2(self.y * aspect, self.y) }
    }
    /// Shrink one side so the size has the given aspect ratio.
    pub fn shrink_to_aspect(self, aspect: f64) -> Vec2 {
        if self.aspect() > aspect { v2(self.y * aspect, self.y) } else { v2(self.x, self.x / aspect) }
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        v2(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        v2(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, s: f64) -> Vec2 {
        v2(self.x * s, self.y * s)
    }
}
impl Div<f64> for Vec2 {
    type Output = Vec2;
    fn div(self, s: f64) -> Vec2 {
        v2(self.x / s, self.y / s)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const UNIT: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_min_size(min: Vec2, size: Vec2) -> Self {
        Self::new(min.x, min.y, size.x, size.y)
    }
    pub fn min(&self) -> Vec2 {
        v2(self.x, self.y)
    }
    pub fn max(&self) -> Vec2 {
        v2(self.x + self.w, self.y + self.h)
    }
    pub fn size(&self) -> Vec2 {
        v2(self.w, self.h)
    }
    pub fn center(&self) -> Vec2 {
        v2(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.x + self.w && p.y <= self.y + self.h
    }
    pub fn inset(&self, d: f64) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    /// Point at normalised coordinates inside the rect.
    pub fn at(&self, s: Vec2) -> Vec2 {
        v2(self.x + s.x * self.w, self.y + s.y * self.h)
    }
    /// Normalised coordinates of a point.
    pub fn scalar_of(&self, p: Vec2) -> Vec2 {
        v2((p.x - self.x) / self.w, (p.y - self.y) / self.h)
    }
    pub fn scale_about_origin(&self, s: f64) -> Rect {
        Rect::new(self.x * s, self.y * s, self.w * s, self.h * s)
    }
    pub fn translate(&self, d: Vec2) -> Rect {
        Rect::new(self.x + d.x, self.y + d.y, self.w, self.h)
    }
    pub fn diagonal(&self) -> f64 {
        self.w.hypot(self.h)
    }
}

/// Map `v` from [a0, a1] to [b0, b1], clamped.
pub fn remap(v: f64, a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    if (a1 - a0).abs() < 1e-12 {
        return b0;
    }
    let t = ((v - a0) / (a1 - a0)).clamp(0.0, 1.0);
    b0 + (b1 - b0) * t
}

/// Piecewise-linear keyframes (clamped at the ends).
pub fn keyframes(v: f64, input: &[f64], output: &[f64]) -> f64 {
    debug_assert_eq!(input.len(), output.len());
    if v <= input[0] {
        return output[0];
    }
    for i in 1..input.len() {
        if v <= input[i] {
            return remap(v, input[i - 1], input[i], output[i - 1], output[i]);
        }
    }
    *output.last().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframes_interpolate() {
        assert!((keyframes(0.5, &[0.0, 1.0], &[0.2, 3.5]) - 1.85).abs() < 1e-12);
        assert_eq!(keyframes(2.0, &[0.0, 1.0], &[0.0, 10.0]), 10.0);
        assert_eq!(keyframes(0.9, &[0.0, 0.05, 0.8, 1.0], &[0.0, 1.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn aspect_fit() {
        let s = v2(1920.0, 1200.0).expand_to_aspect(16.0 / 9.0);
        assert!((s.x - 2133.33).abs() < 0.1 && s.y == 1200.0);
        let s = v2(1920.0, 1200.0).shrink_to_aspect(1.0);
        assert_eq!(s, v2(1200.0, 1200.0));
    }
}
