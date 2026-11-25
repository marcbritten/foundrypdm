//! Plane geometry: points, rigid transforms, curve tessellation, polygons.
//!
//! All internal geometry is in millimetres, y up (DXF convention). Angles are
//! degrees counter-clockwise from the +x axis. This module is pure and allocation
//! light enough to run inline; callers that do heavy work should keep it off the
//! async executor threads (the worker wraps it in `spawn_blocking`).

use serde::{Deserialize, Serialize};

/// A 2D point in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Whether the point is finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Vector from `o` to `self`.
    pub fn minus(self, o: Point) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

/// A 2D vector.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn len(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

/// A rigid (plus optional mirror) 2D transform, matching the solver's
/// `reflected` / `rotation` / `translation` composition order:
/// reflect across x, then rotate CCW, then translate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Transform {
    /// Mirror across the local x axis before rotation.
    pub reflected: bool,
    /// Rotation in degrees (counter-clockwise, y up).
    pub rotation_deg: f64,
    /// Translation applied after rotation.
    pub tx: f64,
    pub ty: f64,
}

impl Transform {
    pub const fn identity() -> Self {
        Self {
            reflected: false,
            rotation_deg: 0.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Apply this transform to a point.
    pub fn apply(self, p: Point) -> Point {
        let (x, y) = if self.reflected {
            (p.x, -p.y)
        } else {
            (p.x, p.y)
        };
        let r = self.rotation_deg.to_radians();
        let (cr, sr) = (r.cos(), r.sin());
        Point::new(cr * x - sr * y + self.tx, sr * x + cr * y + self.ty)
    }

    /// Compose `other` after `self` (apply self, then other): `other ∘ self`.
    /// Used to fold the stock offset into a solver transform.
    pub fn then(self, other: Transform) -> Transform {
        // Evaluate the composition on the origin and the two basis vectors, then
        // recover (reflected, rotation, translation) from the signed-rotation
        // matrix it induces.
        let p0 = other.apply(self.apply(Point::new(0.0, 0.0)));
        let px = other.apply(self.apply(Point::new(1.0, 0.0)));
        let py = other.apply(self.apply(Point::new(0.0, 1.0)));
        let ax = Vec2::new(px.x - p0.x, px.y - p0.y);
        let ay = Vec2::new(py.x - p0.x, py.y - p0.y);
        let cross = ax.x * ay.y - ax.y * ay.x;
        Transform {
            reflected: cross < 0.0,
            rotation_deg: f64::atan2(ax.y, ax.x).to_degrees(),
            tx: p0.x,
            ty: p0.y,
        }
    }
}

/// Bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct BBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BBox {
    pub fn from_points(pts: impl IntoIterator<Item = Point>) -> Option<Self> {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut any = false;
        for p in pts {
            if !p.is_finite() {
                return None;
            }
            any = true;
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
        any.then_some(Self {
            min_x,
            min_y,
            max_x,
            max_y,
        })
    }

    pub fn width(self) -> f64 {
        self.max_x - self.min_x
    }
    pub fn height(self) -> f64 {
        self.max_y - self.min_y
    }
}

/// Whether `p` is inside (or on) the closed ring `poly`, using ray casting.
pub fn point_in_ring(p: Point, poly: &[Point]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let pi = poly[i];
        let pj = poly[j];
        let intersects = (pi.y > p.y) != (pj.y > p.y)
            && (p.x < (pj.x - pi.x) * (p.y - pi.y) / (pj.y - pi.y) + pi.x);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Shoelace signed area of a ring (positive for counter-clockwise).
pub fn signed_area(ring: &[Point]) -> f64 {
    let mut a = 0.0;
    let n = ring.len();
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    a / 2.0
}

/// Whether a closed ring is simple (no self-intersections). O(n^2) with bbox
/// pruning; used to gate offset-polygon construction where n is modest.
pub fn is_simple_ring(pts: &[Point]) -> bool {
    let n = pts.len();
    if n < 4 {
        return true;
    }
    fn orient(a: Point, b: Point, c: Point) -> f64 {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    }
    fn segs_cross(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
        let d1 = orient(q1, q2, p1);
        let d2 = orient(q1, q2, p2);
        let d3 = orient(p1, p2, q1);
        let d4 = orient(p1, p2, q2);
        ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
            && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    }
    for i in 0..n {
        let p1 = pts[i];
        let p2 = pts[(i + 1) % n];
        let bi = (
            p1.x.min(p2.x),
            p1.x.max(p2.x),
            p1.y.min(p2.y),
            p1.y.max(p2.y),
        );
        for j in (i + 1)..n {
            if j == i + 1 || (j == 0 && i == n - 1) {
                continue; // adjacent edges share a vertex
            }
            let q1 = pts[j];
            let q2 = pts[(j + 1) % n];
            let bj = (
                q1.x.min(q2.x),
                q1.x.max(q2.x),
                q1.y.min(q2.y),
                q1.y.max(q2.y),
            );
            if bi.1 < bj.0 || bj.1 < bi.0 || bi.3 < bj.2 || bj.3 < bi.2 {
                continue; // disjoint bboxes
            }
            if segs_cross(p1, p2, q1, q2) {
                return false;
            }
        }
    }
    true
}

/// Centroid of a (possibly non-uniform) polygon ring.
pub fn centroid(ring: &[Point]) -> Point {
    let n = ring.len();
    if n == 0 {
        return Point::new(0.0, 0.0);
    }
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut a = 0.0;
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        let f = p.x * q.y - q.x * p.y;
        a += f;
        cx += (p.x + q.x) * f;
        cy += (p.y + q.y) * f;
    }
    if a.abs() < 1e-12 {
        // Degenerate: fall back to the mean of the vertices.
        let mut sx = 0.0;
        let mut sy = 0.0;
        for p in ring {
            sx += p.x;
            sy += p.y;
        }
        return Point::new(sx / n as f64, sy / n as f64);
    }
    a *= 3.0;
    Point::new(cx / a, cy / a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn transform_apply_matches_documented_order() {
        // Rotate 90deg CCW about origin: (1,0) -> (0,1).
        let t = Transform {
            reflected: false,
            rotation_deg: 90.0,
            tx: 10.0,
            ty: 20.0,
        };
        let p = t.apply(Point::new(1.0, 0.0));
        assert!(approx(p.x, 10.0) && approx(p.y, 21.0));
    }

    #[test]
    fn transform_reflect_then_rotate() {
        // reflect (x,-y) then rotate 90: point (1,2) -> reflect (1,-2) -> rot90 (2,1)
        let t = Transform {
            reflected: true,
            rotation_deg: 90.0,
            tx: 0.0,
            ty: 0.0,
        };
        let p = t.apply(Point::new(1.0, 2.0));
        assert!(approx(p.x, 2.0) && approx(p.y, 1.0));
    }

    #[test]
    fn transform_then_composition_is_rigid() {
        let a = Transform {
            reflected: false,
            rotation_deg: 90.0,
            tx: 3.0,
            ty: 4.0,
        };
        let b = Transform {
            reflected: false,
            rotation_deg: 0.0,
            tx: 10.0,
            ty: 0.0,
        };
        let c = a.then(b);
        // a then b == a.translate + b.translate with same rotation.
        let p = Point::new(1.0, 0.0);
        let lhs = c.apply(p);
        let rhs = b.apply(a.apply(p));
        assert!(approx(lhs.x, rhs.x) && approx(lhs.y, rhs.y));
    }

    #[test]
    fn signed_area_ccw_positive() {
        let square = [
            Point::new(0.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(2.0, 2.0),
            Point::new(0.0, 2.0),
        ];
        assert!(approx(signed_area(&square), 4.0));
        let rev: Vec<Point> = square.iter().rev().copied().collect();
        assert!(approx(signed_area(&rev), -4.0));
    }

    #[test]
    fn point_in_ring_basic() {
        let square = [
            Point::new(0.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(2.0, 2.0),
            Point::new(0.0, 2.0),
        ];
        assert!(point_in_ring(Point::new(1.0, 1.0), &square));
        assert!(!point_in_ring(Point::new(3.0, 1.0), &square));
    }
}
