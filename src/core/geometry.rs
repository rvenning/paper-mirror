//! Small geometry types shared by the layout, annotation and backend code.
//!
//! Page space is PDF user space in points with the origin at the bottom-left of the
//! (rotated) page box. View space is logical pixels with the origin at the top-left of
//! the scrollable document canvas.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Point) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

/// Axis-aligned rectangle; `x`/`y` is the minimum corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Rectangle spanning two arbitrary corners.
    pub fn from_points(a: Point, b: Point) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self { x, y, w: (a.x - b.x).abs(), h: (a.y - b.y).abs() }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right() && other.x < self.right() && self.y < other.bottom() && other.y < self.bottom()
    }

    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn inflate(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

/// Clockwise page rotation in quarter turns, matching PDF `/Rotate`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn from_degrees(deg: i32) -> Self {
        match deg.rem_euclid(360) {
            90 => Rotation::R90,
            180 => Rotation::R180,
            270 => Rotation::R270,
            _ => Rotation::R0,
        }
    }

    pub fn degrees(self) -> i32 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }

    pub fn rotate_cw(self) -> Self {
        Self::from_degrees(self.degrees() + 90)
    }

    pub fn rotate_ccw(self) -> Self {
        Self::from_degrees(self.degrees() - 90)
    }

    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::R90 | Rotation::R270)
    }
}

/// A 2D affine transform `(a b c d e f)` mapping (x, y) to (a·x + c·y + e, b·x + d·y + f),
/// the same convention as a PDF `cm` matrix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(dx: f32, dy: f32) -> Affine {
        Affine { e: dx, f: dy, ..Affine::IDENTITY }
    }

    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine { a: sx, d: sy, ..Affine::IDENTITY }
    }

    /// Builds the transform that maps the unit basis to three image points: the
    /// origin, the image of (1, 0) and the image of (0, 1).
    pub fn from_basis(origin: Point, x1: Point, y1: Point) -> Affine {
        Affine {
            a: x1.x - origin.x,
            b: x1.y - origin.y,
            c: y1.x - origin.x,
            d: y1.y - origin.y,
            e: origin.x,
            f: origin.y,
        }
    }

    pub fn apply(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    /// `self` after `other`: (self ∘ other)(p) = self(other(p)).
    pub fn then(&self, other: &Affine) -> Affine {
        Affine {
            a: other.a * self.a + other.b * self.c,
            b: other.a * self.b + other.b * self.d,
            c: other.c * self.a + other.d * self.c,
            d: other.c * self.b + other.d * self.d,
            e: other.e * self.a + other.f * self.c + self.e,
            f: other.e * self.b + other.f * self.d + self.f,
        }
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Affine { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) })
    }

    /// Bounding box of a transformed rectangle.
    pub fn apply_rect(&self, r: Rect) -> Rect {
        let pts = [
            self.apply(Point::new(r.x, r.y)),
            self.apply(Point::new(r.right(), r.y)),
            self.apply(Point::new(r.x, r.bottom())),
            self.apply(Point::new(r.right(), r.bottom())),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Average scale factor (for stroke widths under the transform).
    pub fn scale_factor(&self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affine_compose_and_invert() {
        let t = Affine::translate(10.0, 20.0).then(&Affine::scale(2.0, 3.0));
        assert_eq!(t.apply(Point::new(1.0, 1.0)), Point::new(12.0, 23.0));
        let inv = t.inverse().unwrap();
        let p = inv.apply(Point::new(12.0, 23.0));
        assert!((p.x - 1.0).abs() < 1e-5 && (p.y - 1.0).abs() < 1e-5);
        // Display space of an unrotated Letter page -> PDF user space (y flipped).
        let m = Affine::from_basis(Point::new(0.0, 792.0), Point::new(1.0, 792.0), Point::new(0.0, 791.0));
        assert_eq!(m.apply(Point::new(10.0, 20.0)), Point::new(10.0, 772.0));
        assert_eq!(m.apply_rect(Rect::new(0.0, 0.0, 10.0, 10.0)), Rect::new(0.0, 782.0, 10.0, 10.0));
        assert!((m.scale_factor() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rect_from_points_normalises_corners() {
        let r = Rect::from_points(Point::new(10.0, 20.0), Point::new(4.0, 2.0));
        assert_eq!(r, Rect::new(4.0, 2.0, 6.0, 18.0));
    }

    #[test]
    fn intersection_and_union() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        assert!(a.intersection(&Rect::new(20.0, 20.0, 1.0, 1.0)).is_none());
        // Touching edges do not intersect.
        assert!(!a.intersects(&Rect::new(10.0, 0.0, 5.0, 5.0)));
    }

    #[test]
    fn rotation_wraps() {
        assert_eq!(Rotation::R270.rotate_cw(), Rotation::R0);
        assert_eq!(Rotation::R0.rotate_ccw(), Rotation::R270);
        assert_eq!(Rotation::from_degrees(-90), Rotation::R270);
        assert_eq!(Rotation::from_degrees(450), Rotation::R90);
        assert!(Rotation::R90.swaps_axes());
    }
}
