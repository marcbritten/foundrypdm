//! Assembling parsed DXF primitives into nested-able parts: closed contours with
//! holes, marking chains, and conservative solver outlines.
//!
//! The pipeline is:
//! 1. Convert each primitive into one or more oriented [`Edge`]s (a straight
//!    segment or a counter-clockwise arc). Closed primitives (circle, closed
//!    polyline) become closed contours directly.
//! 2. Chain open edges into contours by joining endpoints within a tolerance.
//! 3. Group closed contours into parts: the largest-area contour is the outer
//!    boundary; smaller contained contours are holes.
//! 4. Tessellate curves into polygons for the solver envelope and for validation.

use crate::dxf::Primitive;
use crate::geom::{self, BBox, Point};
use std::collections::HashMap;

/// Joining tolerance in millimetres for matching open endpoints.
pub const DEFAULT_JOIN_TOL: f64 = 0.01;
/// Maximum chord deviation (mm) when tessellating an arc for the solver outline.
pub const DEFAULT_CHORD_TOL: f64 = 0.05;
/// Additional conservative inflation (mm) added to the solver outline beyond the
/// chord tolerance, to account for arc tessellation error.
pub const DEFAULT_ENVELOPE_INFLATION: f64 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NestingError {
    /// No closed contour with area > 0 was found for a cut layer.
    NoClosedContour,
    /// A closed contour is degenerate (zero area or self-overlap we cannot trust).
    DegenerateContour,
    /// More than one disconnected outer island on the cut layer.
    MultipleOuterIslands,
    /// An open chain on the cut layer that did not close.
    OpenCutChain,
    /// The outer contour is not simple / has too few points after tessellation.
    TooFewPoints,
}

/// An oriented edge: a straight segment or a counter-clockwise arc.
#[derive(Debug, Clone)]
pub enum Edge {
    Line {
        a: Point,
        b: Point,
    },
    Arc {
        center: Point,
        radius: f64,
        start: f64,
        end: f64,
    },
}

impl Edge {
    /// The start (from) endpoint.
    pub fn from(&self) -> Point {
        match self {
            Edge::Line { a, .. } => *a,
            Edge::Arc {
                center,
                radius,
                start,
                ..
            } => arc_point(*center, *radius, *start),
        }
    }

    /// The end (to) endpoint.
    pub fn to(&self) -> Point {
        match self {
            Edge::Line { b, .. } => *b,
            Edge::Arc {
                center,
                radius,
                end,
                ..
            } => arc_point(*center, *radius, *end),
        }
    }

    /// Reverse orientation.
    fn reversed(&self) -> Edge {
        match self {
            Edge::Line { a, b } => Edge::Line { a: *b, b: *a },
            Edge::Arc {
                center,
                radius,
                start,
                end,
            } => Edge::Arc {
                center: *center,
                radius: *radius,
                start: norm_deg(*end),
                end: norm_deg(*start - 360.0),
            },
        }
    }

    /// Tessellate into points (excluding the final repeat of the start).
    fn tessellate(&self, chord_tol: f64, out: &mut Vec<Point>) {
        match self {
            Edge::Line { a, b: _ } => {
                out.push(*a);
            }
            Edge::Arc {
                center,
                radius,
                start,
                end,
            } => {
                let total = ccw_sweep(*start, *end);
                if total <= 1e-9 {
                    out.push(arc_point(*center, *radius, *start));
                    return;
                }
                let n = arc_segments(*radius, total, chord_tol);
                let step = total / n as f64;
                let mut ang = *start;
                for _ in 0..n {
                    out.push(arc_point(*center, *radius, ang));
                    ang += step;
                }
                // Points cover [start, end); a full circle closes cyclically back
                // to `start`, and a partial arc hands off to the next edge.
            }
        }
    }
}

/// A point on a circle at `deg` (CCW from +x), y up.
fn arc_point(center: Point, radius: f64, deg: f64) -> Point {
    let r = deg.to_radians();
    Point::new(center.x + radius * r.cos(), center.y + radius * r.sin())
}

/// A point on an edge's arc at the given angle (public for sheet export).
pub fn edge_point_on_arc(center: Point, radius: f64, deg: f64) -> Point {
    arc_point(center, radius, deg)
}

/// Normalize an angle to [0, 360).
fn norm_deg(a: f64) -> f64 {
    a.rem_euclid(360.0)
}

/// Counter-clockwise sweep from `start` to `end` in (0, 360]. A full turn
/// (`end - start` a positive multiple of 360, e.g. 0 -> 360) yields 360.
fn ccw_sweep(start: f64, end: f64) -> f64 {
    let s = norm_deg(start);
    let e = norm_deg(end);
    let mut d = e - s;
    if d <= 1e-9 {
        d += 360.0;
    }
    d
}

/// Number of segments so that the chord deviation is within `chord_tol`.
fn arc_segments(radius: f64, total_deg: f64, chord_tol: f64) -> usize {
    if radius <= 1e-9 {
        return 1;
    }
    // sagitta = r (1 - cos(theta/2)); solve theta_max.
    let ratio = (chord_tol / radius).clamp(0.0, 1.0);
    let theta_max = 2.0 * (1.0 - ratio).acos(); // radians
    let per_seg_deg = theta_max.to_degrees();
    let n = (total_deg / per_seg_deg).ceil() as usize;
    n.max(2)
}

/// A closed contour made of oriented edges.
#[derive(Debug, Clone)]
pub struct Contour {
    pub edges: Vec<Edge>,
    pub layer: String,
    /// Tessellated polygon (CCW for the outer, CW for holes) — `points`.
    pub points: Vec<Point>,
    pub bbox: BBox,
    /// Signed area (positive if `points` is CCW).
    pub area: f64,
}

impl Contour {
    fn centroid(&self) -> Point {
        geom::centroid(&self.points)
    }

    fn abs_area(&self) -> f64 {
        self.area.abs()
    }

    /// Whether every point of `self` lies within (or on) `other`.
    fn inside(&self, other: &Contour) -> bool {
        if self.bbox.min_x < other.bbox.min_x - 1e-6
            || self.bbox.min_y < other.bbox.min_y - 1e-6
            || self.bbox.max_x > other.bbox.max_x + 1e-6
            || self.bbox.max_y > other.bbox.max_y + 1e-6
        {
            return false;
        }
        let c = self.centroid();
        geom::point_in_ring(c, &other.points)
    }
}

fn build_contour(edges: &[Edge], layer: &str, chord_tol: f64) -> Option<Contour> {
    build_contour_min(edges, layer, chord_tol, 3)
}

/// Like `build_contour` but with a lower minimum point count (marks such as a
/// single bend line tessellate to one or two points and must be kept).
fn build_contour_min(
    edges: &[Edge],
    layer: &str,
    chord_tol: f64,
    min_pts: usize,
) -> Option<Contour> {
    let mut pts: Vec<Point> = Vec::new();
    for e in edges {
        e.tessellate(chord_tol, &mut pts);
    }
    // The chain should be closed: first point == last point. We dropped the last
    // point of each arc; for a closed chain the final edge's end equals the start.
    if pts.len() < min_pts {
        return None;
    }
    let area = geom::signed_area(&pts);
    let bbox = BBox::from_points(pts.iter().copied())?;
    Some(Contour {
        edges: edges.to_vec(),
        layer: layer.to_string(),
        points: pts,
        bbox,
        area,
    })
}

/// A nested part: one outer boundary, zero or more holes, and optional marks.
#[derive(Debug, Clone)]
pub struct Part {
    pub outer: Contour,
    pub holes: Vec<Contour>,
    pub marks: Vec<Contour>,
    /// Nominal area = outer - holes (mm^2), for utilization.
    pub area: f64,
}

impl Part {
    /// Whether any inner contour is a blind feature (`DEPTH_<mm>`, machined from the top face):
    /// such a part must not be nested mirrored, or its pockets would be cut into the wrong face.
    /// Only [`assemble_keeping_layers`] keeps the layers this reads.
    pub fn has_blind_features(&self) -> bool {
        self.holes
            .iter()
            .chain(&self.marks)
            .any(|c| crate::cnc_layers::is_blind_layer(&c.layer))
    }

    pub fn bbox(&self) -> &BBox {
        &self.outer.bbox
    }

    /// Width/height of the nominal outer bounding box.
    pub fn size(&self) -> (f64, f64) {
        (self.outer.bbox.width(), self.outer.bbox.height())
    }
}

/// Result of assembling parts from primitives.
pub struct Assembled {
    pub parts: Vec<Part>,
    /// Open (unclosed) chains that were found; reported for diagnostics.
    pub open_chains: Vec<Contour>,
}

/// Sentinel value for `layer_cut`: auto-detect cut/mark layers from the file.
pub const LAYER_CUT_AUTO: &str = "auto";

/// Auto-detect cut vs. mark layers following common flat-pattern conventions:
/// layers whose names contain bend/mark/etch/center/ref are marking; every
/// other layer that carries geometry is cut. `user_marks` are honoured first.
/// Returns (cut_layers, mark_layers); cuts fall back to `["cut"]` when no
/// non-mark layer has geometry.
pub fn auto_layers(d: &crate::dxf::Dxf, user_marks: &[String]) -> (Vec<String>, Vec<String>) {
    let mut with_geo: Vec<String> = Vec::new();
    for p in &d.primitives {
        let l = p.layer();
        if !with_geo.iter().any(|x| x.eq_ignore_ascii_case(&l)) {
            with_geo.push(l);
        }
    }
    let is_mark = |l: &str| {
        let s = l.to_ascii_lowercase();
        s.contains("bend")
            || s.contains("mark")
            || s.contains("etch")
            || s.contains("center")
            || s.contains("ref")
    };
    let mut marks: Vec<String> = Vec::new();
    for m in user_marks {
        if !m.is_empty() && !marks.iter().any(|x| x.eq_ignore_ascii_case(m)) {
            marks.push(m.clone());
        }
    }
    for l in with_geo.iter().filter(|l| is_mark(l)) {
        if !marks.iter().any(|x| x.eq_ignore_ascii_case(l)) {
            marks.push(l.clone());
        }
    }
    let cuts: Vec<String> = with_geo
        .into_iter()
        .filter(|l| !marks.iter().any(|m| m.eq_ignore_ascii_case(l)))
        .collect();
    let cuts = if cuts.is_empty() {
        vec!["cut".to_string()]
    } else {
        cuts
    };
    (cuts, marks)
}

/// Convert primitives into parts. `cut_layers` are layers treated as cut
/// geometry (layer names are case-insensitive); `mark_layers` are layers
/// treated as marking (bend/etch lines). Layers in neither are ignored. Units
/// are assumed to already be in mm (callers convert via `dxf::to_mm` first).
/// Every cut contour is named after the first cut layer and every mark after
/// the first mark layer; [`assemble_keeping_layers`] keeps each one's own.
pub fn assemble(
    prims: &[Primitive],
    cut_layers: &[String],
    mark_layers: &[String],
    chord_tol: f64,
    join_tol: f64,
) -> Result<Assembled, NestingError> {
    assemble_layers(prims, cut_layers, mark_layers, chord_tol, join_tol, false)
}

/// As [`assemble`], but every contour keeps the layer it was drawn on, and loose
/// edges are chained layer by layer, so two layers' edges are never joined. A
/// sheet's DXF then says which contour is the outline, which goes through and
/// how deep a blind one goes (router layers, see [`crate::cnc_layers`]). A blind
/// feature (`DEPTH_<mm>`) leaves material under it, so it doesn't come off the
/// part's area.
pub fn assemble_keeping_layers(
    prims: &[Primitive],
    cut_layers: &[String],
    mark_layers: &[String],
    chord_tol: f64,
    join_tol: f64,
) -> Result<Assembled, NestingError> {
    assemble_layers(prims, cut_layers, mark_layers, chord_tol, join_tol, true)
}

fn assemble_layers(
    prims: &[Primitive],
    cut_layers: &[String],
    mark_layers: &[String],
    chord_tol: f64,
    join_tol: f64,
    keep_layers: bool,
) -> Result<Assembled, NestingError> {
    let cut_set: Vec<String> = cut_layers.iter().map(|s| s.to_ascii_lowercase()).collect();
    let mark_set: Vec<String> = mark_layers.iter().map(|s| s.to_ascii_lowercase()).collect();
    let cut_attr = cut_layers.first().cloned().unwrap_or_default();

    // 1. Build closed contours from closed primitives on the cut layers; loose
    //    edges are collected per layer name. Without `keep_layers` every cut
    //    contour takes the first cut layer's name (so all loose edges chain
    //    together) and every mark the first mark layer's.
    let mut closed: Vec<(Vec<Edge>, String)> = Vec::new();
    let mut open_by_layer: Vec<(String, Vec<Edge>)> = Vec::new();
    let mut mark_chains: Vec<(Vec<Edge>, String)> = Vec::new();
    let mut mark_attr = String::new();

    for p in prims {
        let layer = p.layer();
        let is_cut = cut_set.contains(&layer.to_ascii_lowercase());
        let is_mark = mark_set.contains(&layer.to_ascii_lowercase());
        let (edges, is_closed) = primitive_to_edges(p);
        match (is_cut, is_closed, is_mark) {
            (true, true, _) => {
                let name = if keep_layers { layer } else { cut_attr.clone() };
                closed.push((edges, name));
            }
            (true, false, _) => {
                let name = if keep_layers { layer } else { cut_attr.clone() };
                match open_by_layer.iter_mut().find(|(l, _)| *l == name) {
                    Some((_, es)) => es.extend(edges),
                    None => open_by_layer.push((name, edges)),
                }
            }
            (false, _, true) if !edges.is_empty() => {
                if mark_attr.is_empty() {
                    mark_attr = layer.clone();
                }
                let name = if keep_layers { layer } else { mark_attr.clone() };
                mark_chains.push((edges, name));
            }
            _ => {}
        }
    }

    // 2. Chain open cut edges into closed/open contours, layer by layer.
    let mut open_chains: Vec<(Vec<Edge>, String)> = Vec::new();
    for (layer, edges) in &open_by_layer {
        let (cut_chains, open) = chain(edges, join_tol);
        closed.extend(cut_chains.into_iter().map(|c| (c, layer.clone())));
        open_chains.extend(open.into_iter().map(|c| (c, layer.clone())));
    }

    // 3. Materialize contours.
    let mut contours: Vec<Contour> = Vec::new();
    for (c, layer) in closed.iter().chain(&open_chains) {
        if let Some(cont) = build_contour(c, layer, chord_tol) {
            contours.push(cont);
        }
    }
    // Build mark contours (for later transformation + export). Marks may be a
    // single line, so allow as few as one tessellated point.
    let mut marks: Vec<Contour> = Vec::new();
    for (c, layer) in &mark_chains {
        if let Some(cont) = build_contour_min(c, layer, chord_tol, 1) {
            marks.push(cont);
        }
    }

    if contours.iter().all(|c| c.abs_area() < 1e-9) {
        return Err(NestingError::NoClosedContour);
    }

    // 4. Group into parts: sort by area desc; each is an outer unless contained
    //    in an already-chosen outer.
    let mut order: Vec<usize> = (0..contours.len()).collect();
    order.sort_by(|a, b| contours[*b].abs_area().total_cmp(&contours[*a].abs_area()));
    let mut part_outers: Vec<usize> = Vec::new();
    for (rank, &idx) in order.iter().enumerate() {
        let cont = &contours[idx];
        if cont.abs_area() < 1e-9 {
            continue;
        }
        let contained_in_earlier = part_outers
            .iter()
            .any(|&o| cont.inside(&contours[o]) && o != idx);
        if !contained_in_earlier {
            part_outers.push(idx);
        }
        let _ = rank;
    }

    if part_outers.len() > 1 {
        // Multiple disconnected outer islands on the cut layer.
        // Accept only if the caller allows it; here we return all as separate parts.
    }

    let mut parts: Vec<Part> = Vec::new();
    for outer_idx in &part_outers {
        let outer = &contours[*outer_idx];
        let mut holes: Vec<&Contour> = Vec::new();
        for (i, c) in contours.iter().enumerate() {
            if i == *outer_idx || c.abs_area() < 1e-9 {
                continue;
            }
            if c.inside(outer) {
                holes.push(c);
            }
        }
        // Ensure hole winding is opposite to the outer (CW). We store tessellated
        // points; the solver only needs the outer, so winding is advisory.
        // With kept layers, a blind feature (`DEPTH_<mm>`) leaves material under
        // it: not a hole in the area.
        let area = outer.abs_area()
            - holes
                .iter()
                .filter(|h| !(keep_layers && crate::cnc_layers::is_blind_layer(&h.layer)))
                .map(|h| h.abs_area())
                .sum::<f64>();
        parts.push(Part {
            outer: outer.clone(),
            holes: holes.into_iter().cloned().collect(),
            marks: Vec::new(),
            area,
        });
    }

    if parts.is_empty() {
        return Err(NestingError::NoClosedContour);
    }
    // Attach marks to the part whose bbox contains the mark centroid (best effort).
    for m in &marks {
        if let Some(slot) = parts
            .iter_mut()
            .find(|p| p.outer.bbox.contains_point(geom::centroid(&m.points)))
        {
            slot.marks.push(m.clone());
        }
    }

    // Origin-anchor each part (outer + holes + marks together) so rotations
    // about the local origin stay in-bounds of the strip.
    for p in parts.iter_mut() {
        *p = normalize_part(p);
    }

    Ok(Assembled {
        parts,
        open_chains: open_chains
            .iter()
            .filter_map(|(c, layer)| build_contour(c, layer, chord_tol))
            .collect(),
    })
}

impl BBox {
    fn contains_point(&self, p: Point) -> bool {
        p.x >= self.min_x && p.x <= self.max_x && p.y >= self.min_y && p.y <= self.max_y
    }
}

/// The convex hull of a point set (Andrew's monotone chain), counter-clockwise, without
/// collinear points.
fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut pts: Vec<Point> = points.iter().copied().filter(|p| p.is_finite()).collect();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let chain = |pts: &mut dyn Iterator<Item = Point>| {
        let mut c: Vec<Point> = Vec::new();
        for p in pts {
            while c.len() >= 2 && cross(c[c.len() - 2], c[c.len() - 1], p) <= 0.0 {
                c.pop();
            }
            c.push(p);
        }
        // The last point of each chain starts the other one.
        c.pop();
        c
    };
    let mut hull = chain(&mut pts.iter().copied());
    hull.extend(chain(&mut pts.iter().rev().copied()));
    hull
}

/// The axis-aligned bounding box of `points`, grown by `(hx, hy)`, counter-clockwise. The last
/// resort of [`dilate_box`]: always simple and always at least the requested spacing away.
fn bbox_grown(points: &[Point], hx: f64, hy: f64) -> Vec<Point> {
    let bb = BBox::from_points(points.iter().copied()).unwrap_or_default();
    vec![
        Point::new(bb.min_x - hx, bb.min_y - hy),
        Point::new(bb.max_x + hx, bb.min_y - hy),
        Point::new(bb.max_x + hx, bb.max_y + hy),
        Point::new(bb.min_x - hx, bb.max_y + hy),
    ]
}

/// Minkowski dilation of a polygon by the axis box with HALF extents
/// `(hx, hy)`: `P ⊕ [-hx,hx]×[-hy,hy]`. Used to build the solver's collision
/// outline so that horizontal/vertical part spacing is enforced exactly: two
/// originals whose dilations are disjoint are ≥ `2hx` apart in x when their
/// y-ranges overlap and ≥ `2hy` apart in y when their x-ranges overlap.
///
/// Construction: offset every edge outward by the box support in its normal
/// direction; the dilated vertex at corner i is the intersection of the two
/// adjacent offset lines. That is exact for convex corners and produces the
/// correct notch corner at right-angle reflex corners (the usual sheet-metal
/// case). Winding is handled; consecutive duplicates are removed.
///
/// When the construction is not sane (sharp spikes, a notch narrower than the spacing), the
/// same construction is run on the outline's convex hull, where it is exact: nothing nests into
/// that part's notches, but the spacing still holds. The grown bounding box is the last resort.
/// Every result contains the true dilation, so the spacing is never less than asked for.
pub fn dilate_box(points: &[Point], hx: f64, hy: f64) -> Vec<Point> {
    if (hx <= 0.0 && hy <= 0.0) || points.len() < 3 {
        return points.to_vec();
    }
    offset_by_box(points, points, hx, hy)
        .or_else(|| offset_by_box(&convex_hull(points), points, hx, hy))
        .unwrap_or_else(|| bbox_grown(points, hx, hy))
}

/// The edge-offset construction of [`dilate_box`] on `ring`, or `None` when the result is not
/// sane: not finite, not simple, implausibly small, or not containing every point of `outline`.
fn offset_by_box(ring: &[Point], outline: &[Point], hx: f64, hy: f64) -> Option<Vec<Point>> {
    let points = ring;
    if points.len() < 3 {
        return None;
    }
    let n = points.len();
    let ccw = geom::signed_area(points) >= 0.0;
    // Outward-offset start point and direction of each edge.
    let mut offs: Vec<Point> = Vec::with_capacity(n);
    let mut dirs: Vec<[f64; 2]> = Vec::with_capacity(n);
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let l = dx.hypot(dy);
        if l < 1e-12 {
            offs.push(a);
            dirs.push([1.0, 0.0]);
            continue;
        }
        // Outward normal: for CCW winding the interior is left of the edge, so
        // the outward normal is the right normal (dy, -dx); mirror for CW.
        let (nx, ny) = if ccw {
            (dy / l, -dx / l)
        } else {
            (-dy / l, dx / l)
        };
        let h = hx * nx.abs() + hy * ny.abs();
        offs.push(Point::new(a.x + nx * h, a.y + ny * h));
        dirs.push([dx / l, dy / l]);
    }
    // Dilated vertex i = intersection of the offset lines of edges i-1 and i.
    let mut out: Vec<Point> = Vec::with_capacity(n);
    for i in 0..n {
        let i0 = (i + n - 1) % n;
        out.push(line_line(offs[i0], dirs[i0], offs[i], dirs[i]).unwrap_or(offs[i]));
    }
    // Remove consecutive (and wraparound) duplicates.
    let mut dedup: Vec<Point> = Vec::with_capacity(out.len());
    for p in out {
        let dup = dedup
            .last()
            .is_some_and(|q| (q.x - p.x).abs() < 1e-6 && (q.y - p.y).abs() < 1e-6);
        if !dup {
            dedup.push(p);
        }
    }
    if dedup.len() > 1 {
        let f = dedup[0];
        let l = *dedup.last().unwrap();
        if (f.x - l.x).abs() < 1e-6 && (f.y - l.y).abs() < 1e-6 {
            dedup.pop();
        }
    }
    // Sanity: finite, simple, sane area, and every vertex of the outline lies inside.
    (dedup.len() >= 3
        && dedup.iter().all(|p| p.is_finite())
        && geom::is_simple_ring(&dedup)
        && geom::signed_area(&dedup).abs() > geom::signed_area(points).abs() * 0.5
        && outline.iter().all(|p| geom::point_in_ring(*p, &dedup)))
    .then_some(dedup)
}

/// Intersection of the infinite lines p0 + t·d0 and p1 + s·d1.
fn line_line(p0: Point, d0: [f64; 2], p1: Point, d1: [f64; 2]) -> Option<Point> {
    let denom = d0[0] * d1[1] - d0[1] * d1[0];
    if denom.abs() < 1e-12 {
        return None;
    }
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    let t = (dx * d1[1] - dy * d1[0]) / denom;
    Some(Point::new(p0.x + t * d0[0], p0.y + t * d0[1]))
}

/// Translate a contour's tessellation and edges by (dx, dy).
fn translate_contour(c: &Contour, dx: f64, dy: f64) -> Contour {
    let tr = |p: Point| Point::new(p.x + dx, p.y + dy);
    Contour {
        edges: c
            .edges
            .iter()
            .map(|e| match e {
                Edge::Line { a, b } => Edge::Line {
                    a: tr(*a),
                    b: tr(*b),
                },
                Edge::Arc {
                    center,
                    radius,
                    start,
                    end,
                } => Edge::Arc {
                    center: tr(*center),
                    radius: *radius,
                    start: *start,
                    end: *end,
                },
            })
            .collect(),
        layer: c.layer.clone(),
        points: c.points.iter().map(|p| tr(*p)).collect(),
        bbox: crate::geom::BBox {
            min_x: c.bbox.min_x + dx,
            max_x: c.bbox.max_x + dx,
            min_y: c.bbox.min_y + dy,
            max_y: c.bbox.max_y + dy,
        },
        area: c.area,
    }
}

/// Origin-anchor a part: translate it so its outer bounding-box minimum corner
/// is at (0, 0). Nesting solvers rotate parts about their local origin, so a
/// part whose geometry straddles the origin would poke past the strip edge when
/// rotated; anchoring at (0,0) keeps every rotation in the positive quadrant.
pub fn normalize_part(part: &Part) -> Part {
    let (dx, dy) = (-part.outer.bbox.min_x, -part.outer.bbox.min_y);
    Part {
        outer: translate_contour(&part.outer, dx, dy),
        holes: part
            .holes
            .iter()
            .map(|h| translate_contour(h, dx, dy))
            .collect(),
        marks: part
            .marks
            .iter()
            .map(|m| translate_contour(m, dx, dy))
            .collect(),
        area: part.area,
    }
}

/// Mirror an edge across the x axis (y -> -y), preserving orientation.
pub fn mirror_edge(e: &Edge) -> Edge {
    let flip = |p: Point| Point::new(p.x, -p.y);
    match e {
        Edge::Line { a, b } => Edge::Line {
            a: flip(*a),
            b: flip(*b),
        },
        Edge::Arc {
            center,
            radius,
            start,
            end,
        } => {
            // Reflection negates angles; keep the CCW sweep.
            let sweep = ccw_sweep(*start, *end);
            Edge::Arc {
                center: flip(*center),
                radius: *radius,
                start: -end,
                end: -end + sweep,
            }
        }
    }
}

/// Return a copy of the part mirrored across its local x axis. Used for
/// "mirror of the original" nest lines.
pub fn mirror_part(part: &Part) -> Part {
    Part {
        outer: Contour {
            edges: part.outer.edges.iter().map(mirror_edge).collect(),
            layer: part.outer.layer.clone(),
            points: part
                .outer
                .points
                .iter()
                .map(|p| Point::new(p.x, -p.y))
                .collect(),
            bbox: {
                let bb = &part.outer.bbox;
                crate::geom::BBox {
                    min_x: bb.min_x,
                    max_x: bb.max_x,
                    min_y: -bb.max_y,
                    max_y: -bb.min_y,
                }
            },
            area: -part.outer.area,
        },
        holes: part
            .holes
            .iter()
            .map(|h| Contour {
                edges: h.edges.iter().map(mirror_edge).collect(),
                layer: h.layer.clone(),
                points: h.points.iter().map(|p| Point::new(p.x, -p.y)).collect(),
                bbox: crate::geom::BBox {
                    min_x: h.bbox.min_x,
                    max_x: h.bbox.max_x,
                    min_y: -h.bbox.max_y,
                    max_y: -h.bbox.min_y,
                },
                area: -h.area,
            })
            .collect(),
        marks: part
            .marks
            .iter()
            .map(|m| Contour {
                edges: m.edges.iter().map(mirror_edge).collect(),
                layer: m.layer.clone(),
                points: m.points.iter().map(|p| Point::new(p.x, -p.y)).collect(),
                bbox: crate::geom::BBox {
                    min_x: m.bbox.min_x,
                    max_x: m.bbox.max_x,
                    min_y: -m.bbox.max_y,
                    max_y: -m.bbox.min_y,
                },
                area: -m.area,
            })
            .collect(),
        area: part.area,
    }
}

fn primitive_to_edges(p: &Primitive) -> (Vec<Edge>, bool) {
    match p {
        Primitive::Line { start, end, .. } => (vec![Edge::Line { a: *start, b: *end }], false),
        Primitive::Circle { center, radius, .. } => (
            vec![Edge::Arc {
                center: *center,
                radius: *radius,
                start: 0.0,
                end: 360.0,
            }],
            true,
        ),
        Primitive::Arc {
            center,
            radius,
            start_deg,
            end_deg,
            ..
        } => (
            vec![Edge::Arc {
                center: *center,
                radius: *radius,
                start: *start_deg,
                end: *end_deg,
            }],
            false,
        ),
        Primitive::Polyline {
            points,
            bulges,
            closed,
            ..
        } => {
            let mut edges = Vec::new();
            let n = points.len();
            let pairs = if *closed { n } else { n - 1 };
            for i in 0..pairs {
                let a = points[i];
                let b = points[(i + 1) % n];
                let bulge = bulges.get(i).copied().flatten();
                if let Some(k) = bulge {
                    if k.abs() > 1e-9 {
                        let (center, radius, s, e) = bulge_to_arc(a, b, k);
                        edges.push(Edge::Arc {
                            center,
                            radius,
                            start: s,
                            end: e,
                        });
                    } else {
                        edges.push(Edge::Line { a, b });
                    }
                } else {
                    edges.push(Edge::Line { a, b });
                }
            }
            (edges, *closed)
        }
        Primitive::Point { .. } | Primitive::Text { .. } => (Vec::new(), false),
    }
}

/// Convert a bulge (tangent ratio) between two points into an arc.
/// `bulge = tan(theta/4)` where theta is the included angle (positive CCW).
fn bulge_to_arc(a: Point, b: Point, bulge: f64) -> (Point, f64, f64, f64) {
    let chord = (b.x - a.x).hypot(b.y - a.y);
    // Included angle.
    let theta = 4.0 * bulge.atan();
    // Radius = chord / (2 sin(theta/2)).
    let r = if theta.abs() < 1e-9 {
        chord / 2.0 * 1e6 // near-straight; approximate with a large-radius arc
    } else {
        chord / (2.0 * (theta / 2.0).sin()).abs()
    };
    // Midpoint of chord and the arc center offset.
    let mx = (a.x + b.x) / 2.0;
    let my = (a.y + b.y) / 2.0;
    let dist = (chord / 2.0).powi(2);
    let h = (r * r - dist).max(0.0).sqrt();
    // Unit vector from a to b.
    let ux = (b.x - a.x) / chord;
    let uy = (b.y - a.y) / chord;
    // Perpendicular (left normal for CCW theta>0).
    let side = if theta > 0.0 { 1.0 } else { -1.0 };
    let cx = mx + side * (-uy) * h;
    let cy = my + side * ux * h;
    let center = Point::new(cx, cy);
    let sa = ((a.y - cy).atan2(a.x - cx)).to_degrees();
    let sb = ((b.y - cy).atan2(b.x - cx)).to_degrees();
    (center, r, sa, sb)
}

/// Chain open edges into closed/open contour edge lists. Returns
/// `(closed_chains, open_chains)`; each chain is a head-to-tail oriented
/// `Vec<Edge>`.
fn chain(edges: &[Edge], tol: f64) -> (Vec<Vec<Edge>>, Vec<Vec<Edge>>) {
    if edges.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let n = edges.len();
    // Snap endpoints to a grid for hashing.
    let cell = tol.max(1e-6);
    let key =
        |p: Point| -> (i64, i64) { ((p.x / cell).round() as i64, (p.y / cell).round() as i64) };
    // Snapped endpoint -> edge indices having that endpoint.
    let mut at_end: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        at_end.entry(key(e.from())).or_default().push(i);
        at_end.entry(key(e.to())).or_default().push(i);
    }
    let mut used = vec![false; n];
    let mut closed_chains: Vec<Vec<Edge>> = Vec::new();
    let mut open_chains: Vec<Vec<Edge>> = Vec::new();

    for start in 0..n {
        if used[start] {
            continue;
        }
        let mut chain: Vec<Edge> = Vec::new();
        chain.push(edges[start].clone());
        used[start] = true;
        let mut cur = edges[start].to();
        let mut closed = false;
        loop {
            let ck = key(cur);
            let mut next: Option<usize> = None;
            if let Some(cands) = at_end.get(&ck) {
                for &ci in cands {
                    let matches = approx_pt(edges[ci].from(), cur, tol)
                        || approx_pt(edges[ci].to(), cur, tol);
                    if !matches {
                        continue;
                    }
                    if ci == start {
                        // Only close once we have moved at least one edge.
                        if chain.len() > 1 {
                            next = Some(ci);
                            break;
                        }
                        continue;
                    }
                    if used[ci] {
                        continue;
                    }
                    next = Some(ci);
                    break;
                }
            }
            let Some(ci) = next else {
                break;
            };
            used[ci] = true;
            if ci == start {
                closed = true;
                break;
            }
            if approx_pt(edges[ci].from(), cur, tol) {
                chain.push(edges[ci].clone());
                cur = edges[ci].to();
            } else {
                chain.push(edges[ci].reversed());
                cur = edges[ci].from();
            }
        }
        if closed {
            closed_chains.push(chain);
        } else {
            open_chains.push(chain);
        }
    }
    (closed_chains, open_chains)
}

fn approx_pt(a: Point, b: Point, tol: f64) -> bool {
    (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prim(points: Vec<Point>, closed: bool) -> Primitive {
        let n = points.len();
        Primitive::Polyline {
            layer: "cut".into(),
            points,
            bulges: vec![None; n],
            closed,
        }
    }

    #[test]
    fn arc_point_and_sweep() {
        let c = Point::new(0.0, 0.0);
        let p = arc_point(c, 1.0, 90.0);
        assert!((p.x - 0.0).abs() < 1e-9 && (p.y - 1.0).abs() < 1e-9);
        assert!((ccw_sweep(350.0, 10.0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn assemble_square_with_circle_hole() {
        let sq = prim(
            vec![
                Point::new(0.0, 0.0),
                Point::new(100.0, 0.0),
                Point::new(100.0, 100.0),
                Point::new(0.0, 100.0),
            ],
            true,
        );
        let hole = Primitive::Circle {
            layer: "cut".into(),
            center: Point::new(50.0, 50.0),
            radius: 10.0,
        };
        let a = assemble(&[sq, hole], &["cut".to_string()], &[], 0.05, 0.01).unwrap();
        assert_eq!(a.parts.len(), 1);
        let p = &a.parts[0];
        assert_eq!(p.holes.len(), 1);
        // outer area ~10000, hole ~ pi*100 ~ 314.
        assert!((p.area - (10000.0 - std::f64::consts::PI * 100.0)).abs() < 5.0);
    }

    #[test]
    fn two_open_lines_are_not_a_closed_part() {
        let l1 = Primitive::Line {
            layer: "cut".into(),
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
        };
        let l2 = Primitive::Line {
            layer: "cut".into(),
            start: Point::new(10.0, 0.0),
            end: Point::new(10.0, 10.0),
        };
        // An L made of two open lines does not enclose area -> not a nestable part.
        assert!(matches!(
            assemble(&[l1, l2], &["cut".to_string()], &[], 0.05, 0.01),
            Err(NestingError::NoClosedContour)
        ));
    }

    #[test]
    fn closed_chains_become_parts() {
        // A square made of 4 lines (open primitives) that chain into a closed contour.
        let lines = vec![
            Primitive::Line {
                layer: "cut".into(),
                start: Point::new(0.0, 0.0),
                end: Point::new(10.0, 0.0),
            },
            Primitive::Line {
                layer: "cut".into(),
                start: Point::new(10.0, 0.0),
                end: Point::new(10.0, 10.0),
            },
            Primitive::Line {
                layer: "cut".into(),
                start: Point::new(10.0, 10.0),
                end: Point::new(0.0, 10.0),
            },
            Primitive::Line {
                layer: "cut".into(),
                start: Point::new(0.0, 10.0),
                end: Point::new(0.0, 0.0),
            },
        ];
        let a = assemble(&lines, &["cut".to_string()], &[], 0.05, 0.01).unwrap();
        assert_eq!(a.parts.len(), 1);
        assert!((a.parts[0].area - 100.0).abs() < 1.0);
    }

    #[test]
    fn auto_layers_split_cut_and_bend() {
        // Mirrors a SolidCraft flat pattern: OUTLINE (outer), CUT (hole),
        // BEND_UP (mark), mixed case.
        let d = crate::dxf::parse(
            "0\nSECTION\n2\nENTITIES\n\
             0\nLWPOLYLINE\n8\nOUTLINE\n90\n4\n70\n1\n\
             10\n0.0\n20\n0.0\n43\n0.0\n\
             10\n60.0\n20\n0.0\n43\n0.0\n\
             10\n60.0\n20\n40.0\n43\n0.0\n\
             10\n0.0\n20\n40.0\n43\n0.0\n\
             0\nCIRCLE\n8\nCUT\n10\n30.0\n20\n20.0\n30\n0.0\n40\n8.0\n\
             0\nLINE\n8\nBEND_UP\n10\n0.0\n20\n5.0\n30\n0.0\n11\n60.0\n21\n5.0\n31\n0.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let (cuts, marks) = auto_layers(&d, &[]);
        assert!(cuts.iter().any(|c| c.eq_ignore_ascii_case("OUTLINE")));
        assert!(cuts.iter().any(|c| c.eq_ignore_ascii_case("CUT")));
        assert!(!cuts.iter().any(|c| c.eq_ignore_ascii_case("BEND_UP")));
        assert!(marks.iter().any(|m| m.eq_ignore_ascii_case("BEND_UP")));

        let prims = crate::dxf::to_mm(&d, "unknown", "mm");
        let a = assemble(&prims, &cuts, &marks, 0.05, 0.01).unwrap();
        assert_eq!(a.parts.len(), 1);
        assert_eq!(a.parts[0].holes.len(), 1, "the CUT circle is a hole");
        assert!((a.parts[0].area - (60.0 * 40.0 - std::f64::consts::PI * 64.0)).abs() < 6.0);
        assert!(
            !a.parts[0].marks.is_empty(),
            "the BEND_UP line must be kept as a mark"
        );
    }

    #[test]
    fn dilate_box_expands_by_half_extents() {
        let square = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let d = dilate_box(&square, 1.0, 2.0);
        let bb = crate::geom::BBox::from_points(d.iter().copied()).unwrap();
        // A square dilated by box (2 x 4) full extents grows 1/2 mm on each side.
        assert!((bb.min_x - (-1.0)).abs() < 0.05, "min_x {}", bb.min_x);
        assert!((bb.max_x - 11.0).abs() < 0.05, "max_x {}", bb.max_x);
        assert!((bb.min_y - (-2.0)).abs() < 0.05, "min_y {}", bb.min_y);
        assert!((bb.max_y - 12.0).abs() < 0.05, "max_y {}", bb.max_y);
        // Area = 12 * 14 = 168.
        assert!((crate::geom::signed_area(&d) - 168.0).abs() < 2.0);
    }

    #[test]
    fn dilate_box_keeps_the_spacing_when_a_notch_is_narrower_than_it() {
        // 40 x 20 with a 1 mm slot 10 deep from the top. Grown by 2 mm the slot walls cross, so
        // the edge-offset construction is not sane and the convex hull is grown instead.
        let slotted = vec![
            Point::new(0.0, 0.0),
            Point::new(40.0, 0.0),
            Point::new(40.0, 20.0),
            Point::new(20.5, 20.0),
            Point::new(20.5, 10.0),
            Point::new(19.5, 10.0),
            Point::new(19.5, 20.0),
            Point::new(0.0, 20.0),
        ];
        let (hx, hy) = (2.0, 2.0);
        let d = dilate_box(&slotted, hx, hy);
        assert!(crate::geom::is_simple_ring(&d), "{d:?}");
        assert!(
            (crate::geom::signed_area(&d).abs() - 44.0 * 24.0).abs() < 1e-6,
            "the hull (the 40 x 20 rectangle) grown by 2 mm: {d:?}"
        );
        // Every point of the outline, moved anywhere within the box, is still inside.
        for p in &slotted {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let q = Point::new(p.x + sx * hx * 0.999, p.y + sy * hy * 0.999);
                assert!(crate::geom::point_in_ring(q, &d), "{q:?} is outside {d:?}");
            }
        }
    }

    #[test]
    fn convex_hull_drops_notches_and_collinear_points() {
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(5.0, 3.0),
            Point::new(0.0, 10.0),
        ];
        let h = convex_hull(&pts);
        assert_eq!(h.len(), 4, "{h:?}");
        assert!(crate::geom::signed_area(&h) > 0.0, "counter-clockwise");
        assert!((crate::geom::signed_area(&h) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn dilate_box_zero_is_identity() {
        let square = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let d = dilate_box(&square, 0.0, 0.0);
        assert_eq!(d, square);
    }

    #[test]
    fn mirror_part_flips_y_and_hole() {
        let outer = Primitive::Polyline {
            layer: "cut".into(),
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(60.0, 0.0),
                Point::new(60.0, 40.0),
                Point::new(0.0, 40.0),
            ],
            bulges: vec![None; 4],
            closed: true,
        };
        let hole = Primitive::Circle {
            layer: "cut".into(),
            center: Point::new(30.0, 20.0),
            radius: 8.0,
        };
        let p = assemble(&[outer, hole], &["cut".to_string()], &[], 0.05, 0.01)
            .unwrap()
            .parts
            .pop()
            .unwrap();
        let m = mirror_part(&p);
        let bb = &m.outer.bbox;
        assert!((bb.min_y - (-40.0)).abs() < 1e-6);
        assert!((bb.max_y - 0.0).abs() < 1e-6);
        // Hole center flipped to y=-20.
        match &m.holes[0].edges[0] {
            Edge::Arc { center, radius, .. } => {
                assert!((center.y - (-20.0)).abs() < 1e-6);
                assert!((radius - 8.0).abs() < 1e-6);
            }
            _ => panic!("expected arc hole"),
        }
        // Area preserved.
        assert!((m.area - p.area).abs() < 1.0);
    }

    #[test]
    fn assemble_is_case_insensitive_on_layers() {
        // Layer "CUT" (uppercase) is matched by cut layer "cut".
        let d = crate::dxf::parse(
            "0\nSECTION\n2\nENTITIES\n\
             0\nLWPOLYLINE\n8\nCUT\n90\n4\n70\n1\n\
             10\n0.0\n20\n0.0\n43\n0.0\n\
             10\n10.0\n20\n0.0\n43\n0.0\n\
             10\n10.0\n20\n10.0\n43\n0.0\n\
             10\n0.0\n20\n10.0\n43\n0.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let prims = crate::dxf::to_mm(&d, "unknown", "mm");
        let a = assemble(&prims, &["cut".to_string()], &[], 0.05, 0.01).unwrap();
        assert_eq!(a.parts.len(), 1);
    }

    /// An outline drawn as four LINEs on OUTLINE, a through hole on THROUGH and a
    /// blind pocket on DEPTH_6.35 (router layers).
    fn router_panel() -> (Vec<Primitive>, Vec<String>, Vec<String>) {
        let mut e = String::from("0\nSECTION\n2\nENTITIES\n");
        for (a, b) in [
            ((0, 0), (100, 0)),
            ((100, 0), (100, 60)),
            ((100, 60), (0, 60)),
            ((0, 60), (0, 0)),
        ] {
            e.push_str(&format!(
                "0\nLINE\n8\nOUTLINE\n10\n{}\n20\n{}\n11\n{}\n21\n{}\n",
                a.0, a.1, b.0, b.1
            ));
        }
        e.push_str("0\nCIRCLE\n8\nTHROUGH\n10\n20\n20\n20\n40\n5\n");
        e.push_str("0\nCIRCLE\n8\nDEPTH_6.35\n10\n60\n20\n30\n40\n17.5\n");
        e.push_str("0\nENDSEC\n0\nEOF\n");
        let d = crate::dxf::parse(&e).unwrap();
        let (cuts, marks) = auto_layers(&d, &[]);
        (crate::dxf::to_mm(&d, "mm", "mm"), cuts, marks)
    }

    #[test]
    fn each_contour_keeps_its_router_layer() {
        let (prims, cuts, marks) = router_panel();
        let a = assemble_keeping_layers(&prims, &cuts, &marks, 0.05, 0.01).unwrap();
        assert_eq!(a.parts.len(), 1);
        let p = &a.parts[0];
        assert_eq!(p.outer.layer, "OUTLINE");
        let mut holes: Vec<&str> = p.holes.iter().map(|h| h.layer.as_str()).collect();
        holes.sort();
        assert_eq!(holes, ["DEPTH_6.35", "THROUGH"]);
        assert!(p.has_blind_features());
        // The pocket leaves material: only the through hole comes off the area.
        let want = 100.0 * 60.0 - std::f64::consts::PI * 25.0;
        assert!((p.area - want).abs() < 1.0, "{} vs {want}", p.area);
        // A mirror keeps the layers.
        let m = mirror_part(p);
        assert_eq!(m.outer.layer, "OUTLINE");
        assert!(m.has_blind_features());
    }

    #[test]
    fn assemble_still_names_every_contour_after_the_first_cut_layer() {
        // The same drawing through plain `assemble`: as before, one layer name for
        // every cut contour, and every hole off the area.
        let (prims, cuts, marks) = router_panel();
        let a = assemble(&prims, &cuts, &marks, 0.05, 0.01).unwrap();
        let p = &a.parts[0];
        let first = cuts[0].as_str();
        assert!(p.holes.iter().chain([&p.outer]).all(|c| c.layer == first), "{cuts:?}");
        assert!(!p.has_blind_features());
        // Both circles come off the area (their tessellated areas).
        let want = p.outer.abs_area() - p.holes.iter().map(|h| h.abs_area()).sum::<f64>();
        assert_eq!(p.holes.len(), 2);
        assert!((p.area - want).abs() < 1e-6, "{} vs {want}", p.area);
    }
}
