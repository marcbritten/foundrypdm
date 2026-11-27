//! Building a nested sheet from a solver solution: compose transforms, validate
//! the layout independently of the solver, render stock-space geometry, and
//! export DXF.

use crate::dxf;
use crate::geom::{BBox, Point, Transform};
use crate::part::Part;
use crate::solver::{Placed, SolverSolution};
use crate::{GrainDir, NestConfig, NestingCorner};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A small numerical epsilon added to the chord allowance so exact-fit
/// boundaries are not demanded of the solver.
const SEPARATION_EPS: f64 = 0.05;

/// Layout safety buffer (mm) added to the strip offset. The solver's
/// edge-inset for `min_item_separation` is not a clean constant for
/// rotated / concave shapes (a part can poke up to ~1 mm past the virtual
/// strip edge), so this buffer guarantees the nominal parts still respect
/// the requested stock margin.
const LAYOUT_SAFETY: f64 = 1.0;

/// How far (mm) two placed parts may fall short of the requested spacing before a layout is
/// refused: rounding only (the solver works in f32).
const SPACING_TOL: f64 = 1e-3;

/// The stock<->strip mapping derived from the config.
///
/// Part spacing is enforced directionally: the solver outline is the nominal
/// tessellation dilated by the box with half-extents (gap_x/2, gap_y/2), and a
/// small isotropic `separation` covers arc chord error on both sides of a
/// boundary. The offset places the strip so nominal parts sit exactly
/// `edge_margin` from the stock edges.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockMapping {
    /// Solver `min_item_separation` (isotropic epsilon: 2·chord_tol + eps).
    pub separation: f64,
    /// Strip<->stock translation offset (includes the nesting origin).
    pub offset: Point,
    /// Solver `strip_height`.
    pub strip_height: f64,
    /// Required horizontal spacing between parts (nominal).
    pub gap_x: f64,
    /// Required vertical spacing between parts (nominal).
    pub gap_y: f64,
    /// Which corner the fill starts from.
    pub corner: NestingCorner,
}

impl StockMapping {
    pub fn from_config(cfg: &NestConfig) -> Result<Self, SheetError> {
        let margin = cfg.edge_margin;
        let separation = 2.0 * cfg.chord_tol + SEPARATION_EPS;
        // The collision box is symmetric (the larger half-extent on both axes)
        // so it is invariant to the part's rotation: a 90°-turned part would
        // otherwise swap its x/y clearance. This guarantees at least
        // max(gap_x, gap_y) of separation between parts in every direction;
        // `build_sheet` re-checks that distance on the nominal outlines.
        //
        // Margin: the nominal tessellation sits `m` inside the dilated
        // outline; the true part bulges up to `chord_tol` outside the
        // tessellation; and the solver's rotated edge-inset can under-shoot by
        // up to ~LAYOUT_SAFETY. offset = M - m + c + safety puts the true
        // boundary at >= M for all shapes and rotations. The nesting origin
        // shifts the whole layout.
        let m = cfg.gap_x.max(cfg.gap_y) / 2.0;
        let inset = margin - m + cfg.chord_tol + LAYOUT_SAFETY;
        let offset = Point::new(inset + cfg.origin.x, inset + cfg.origin.y);
        let strip_height = cfg.stock_height - 2.0 * offset.y;
        if strip_height <= 0.0 {
            return Err(SheetError::Mapping("strip height non-positive".into()));
        }
        Ok(Self {
            separation,
            offset,
            strip_height,
            gap_x: cfg.gap_x,
            gap_y: cfg.gap_y,
            corner: cfg.nesting_corner,
        })
    }

    /// The conservative solver outline for a part: its nominal outer
    /// tessellation dilated by the symmetric spacing box.
    pub fn solver_outline(&self, part: &crate::part::Part) -> Vec<Point> {
        let m = self.gap_x.max(self.gap_y) / 2.0;
        crate::part::dilate_box(&part.outer.points, m, m)
    }

    /// Whether the nesting corner is reached by reflecting the layout (lower-right, upper-left).
    ///
    /// A reflection on its own would mirror every part, whatever its line allows: a line set to
    /// "Original Only" would be cut as its mirror image, which bends into the opposite-hand part
    /// and puts a one-sided finish face-down. So for these corners the solver nests each part's
    /// mirror image ([`Self::solver_part`], [`Self::solver_rotations`]) and
    /// [`Self::stock_transform`] composes that mirror back in: every copy comes out as drawn, and
    /// only a copy the solver itself flipped (a line that allows mirroring) is mirrored.
    pub fn corner_reflects(&self) -> bool {
        matches!(
            self.corner,
            NestingCorner::LowerRight | NestingCorner::UpperLeft
        )
    }

    /// The geometry the solver nests for `part`: the part itself, or its mirror image when the
    /// corner is a reflection (see [`Self::corner_reflects`]).
    pub fn solver_part<'a>(&self, part: &'a Part) -> std::borrow::Cow<'a, Part> {
        if self.corner_reflects() {
            std::borrow::Cow::Owned(crate::part::mirror_part(part))
        } else {
            std::borrow::Cow::Borrowed(part)
        }
    }

    /// The solver rotations (degrees) that come out as `rotations` on the sheet, so a corner never
    /// turns a part to an angle its line doesn't allow. The corner maps a solver rotation θ to
    /// 180° − θ (lower-right: the solver turns the mirror image), −θ (upper-left, likewise) or
    /// θ + 180° (upper-right: the layout's half-turn); each mapping is its own inverse. Sets
    /// closed under a half-turn (grain and step-angle sets) come out the same; an explicit
    /// `allowed_rotations` such as `[0]` would not, without this.
    pub fn solver_rotations(&self, rotations: Vec<f64>) -> Vec<f64> {
        let turn: fn(f64) -> f64 = match self.corner {
            NestingCorner::LowerLeft => return rotations,
            NestingCorner::LowerRight => |r| 180.0 - r,
            NestingCorner::UpperLeft => |r| -r,
            NestingCorner::UpperRight => |r| r - 180.0,
        };
        rotations
            .into_iter()
            .map(|r| turn(r).rem_euclid(360.0))
            .collect()
    }

    /// The stock-space transform of a solver placement, moved to the requested corner. For a
    /// reflecting corner the solver placed the part's mirror image, so the part is mirrored
    /// first and the corner's reflection turns it back.
    pub fn stock_transform(&self, p: &Placed, cfg: &NestConfig) -> Transform {
        let t = solver_to_stock(p, self.offset);
        if self.corner == NestingCorner::LowerLeft {
            t
        } else if self.corner_reflects() {
            MIRROR_X.then(t).then(self.corner_transform(cfg))
        } else {
            t.then(self.corner_transform(cfg))
        }
    }

    /// The whole-layout flip that moves the (lower-left fill) layout to the
    /// requested nesting corner. The identity for the lower-left corner.
    ///
    /// Note: upper-right is a 180° *rotation* about the stock center
    /// ((w−x, h−y), proper — not a reflection); the single-axis corners are
    /// reflections, which [`Self::stock_transform`] cancels per part.
    pub fn corner_transform(&self, cfg: &NestConfig) -> Transform {
        let (w, h) = (cfg.stock_width, cfg.stock_height);
        match self.corner {
            NestingCorner::LowerLeft => Transform::identity(),
            NestingCorner::LowerRight => Transform {
                reflected: true,
                rotation_deg: 180.0,
                tx: w,
                ty: 0.0,
            },
            NestingCorner::UpperLeft => Transform {
                reflected: true,
                rotation_deg: 0.0,
                tx: 0.0,
                ty: h,
            },
            NestingCorner::UpperRight => Transform {
                reflected: false,
                rotation_deg: 180.0,
                tx: w,
                ty: h,
            },
        }
    }
}

/// A mirror across the local x axis (the same reflection as [`crate::part::mirror_part`]).
const MIRROR_X: Transform = Transform {
    reflected: true,
    rotation_deg: 0.0,
    tx: 0.0,
    ty: 0.0,
};

#[derive(Debug, Error)]
pub enum SheetError {
    #[error("{0}")]
    Mapping(String),
    #[error("quantity mismatch for part {0}: placed {1}, expected {2}")]
    QuantityMismatch(usize, usize, usize),
    #[error("unknown solver item id {0}")]
    UnknownItem(u64),
    #[error("part {part} (copy {copy}) exceeds the stock margins in stock space")]
    OutOfBounds { part: usize, copy: usize },
    #[error("parts {a} and {b} are {distance:.3} mm apart, closer than the {required:.3} mm spacing")]
    TooClose {
        a: usize,
        b: usize,
        distance: f64,
        required: f64,
    },
    #[error("solver strip width {0} leaves a part past the right margin")]
    StripTooWide(f64),
}

/// One placed copy on a sheet, in stock coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetPlacement {
    /// Index into the part list.
    pub part_index: usize,
    /// Which copy of that part (0-based).
    pub copy_index: usize,
    /// The stable solver item id used in this sheet's instance.
    pub item_id: u64,
    /// Nominal part -> stock transform.
    pub transform: Transform,
}

/// A validated nested sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetLayout {
    pub placements: Vec<SheetPlacement>,
    /// Achieved solver strip width (strip coords).
    pub strip_width: f64,
    /// Nominal material utilization: sum(part areas) / stock area.
    pub utilization: f64,
    /// The least distance between any two parts' real outlines the layout check proved, mm (on a
    /// router, a cut-out bit wider than this cuts into the next part). Set by [`build_sheet`];
    /// `None` on a layout that didn't come from it, whose DXF then says nothing about the gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_gap_mm: Option<f64>,
}

/// A reference to one demanded copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyRef {
    pub part: usize,
    pub copy: usize,
}

/// Build and validate a sheet layout from a solver solution.
///
/// `copies` must be exactly the set of copies the solver was asked to place, and
/// `item_id` assignment is `copied[0].part * 1000 + ...` — see
/// [`assign_item_ids`]. For a reflecting nesting corner the solution must come from an
/// instance of the parts' mirror images, as [`crate::batch::build_instance`] builds it
/// (see [`StockMapping::corner_reflects`]).
pub fn build_sheet(
    solution: &SolverSolution,
    item_ids: &[u64],
    parts: &[Part],
    cfg: &NestConfig,
    mapping: &StockMapping,
) -> Result<SheetLayout, SheetError> {
    if solution.placed.len() != item_ids.len() {
        return Err(SheetError::QuantityMismatch(
            0,
            solution.placed.len(),
            item_ids.len(),
        ));
    }
    let eps = 1e-3;
    let mut placed: Vec<(usize, usize, Transform)> = Vec::new();
    let mut seen = vec![0usize; item_ids.len()];
    for p in &solution.placed {
        let pos = item_ids
            .iter()
            .position(|&id| id == p.item_id)
            .ok_or(SheetError::UnknownItem(p.item_id))?;
        seen[pos] += 1;
        let (part, copy) = decode_item_id(p.item_id);
        if part >= parts.len() {
            return Err(SheetError::UnknownItem(p.item_id));
        }
        // Move the whole layout to the requested nesting corner (identity for
        // lower-left), without mirroring any part (see `StockMapping::corner_reflects`).
        let t = mapping.stock_transform(p, cfg);
        placed.push((part, copy, t));
    }
    for (i, &want) in item_ids.iter().enumerate() {
        if seen[i] != 1 {
            return Err(SheetError::QuantityMismatch(i, seen[i], 1));
        }
        let _ = want;
    }

    // Per-placement nominal outline in stock space, for bounds + spacing.
    let outlines: Vec<Vec<Point>> = placed
        .iter()
        .map(|(part, _, t)| {
            let base = &parts[*part].outer.points;
            base.iter().map(|p| t.apply(*p)).collect()
        })
        .collect();

    // Bounds check.
    let (w, h) = (cfg.stock_width, cfg.stock_height);
    let m = cfg.edge_margin;
    for (i, ((part, copy, _), ol)) in placed.iter().zip(outlines.iter()).enumerate() {
        if let Some(bb) = BBox::from_points(ol.iter().copied()) {
            if bb.min_x < m - eps
                || bb.min_y < m - eps
                || bb.max_x > w - m + eps
                || bb.max_y > h - m + eps
            {
                return Err(SheetError::OutOfBounds {
                    part: *part,
                    copy: *copy,
                });
            }
        } else {
            return Err(SheetError::OutOfBounds {
                part: *part,
                copy: *copy,
            });
        }
        let _ = i;
    }

    // Minimum spacing. The solver keeps the dilated outlines (nominal
    // tessellation ⊕ symmetric box of half-extent max(gap_x, gap_y)/2) apart,
    // and every dilation contains the true box dilation (`part::dilate_box`),
    // so the nominal tessellations of a correct layout are at least
    // max(gap_x, gap_y) apart in every direction (and at least `separation`
    // when no spacing is asked for). Re-check that distance here, independently
    // of the solver: the spacing is usually the cutting process's own minimum
    // (kerf, bridge width, cutter), so a closer pair is a bad sheet, not a tight
    // one. The Euclidean distance is shape-independent, whereas a
    // bounding-box check is too strict for concave parts that nest into each
    // other's notches (small bbox gap, large real gap).
    let required = mapping.gap_x.max(mapping.gap_y).max(mapping.separation);
    let boxes: Vec<BBox> = outlines
        .iter()
        .map(|o| BBox::from_points(o.iter().copied()).unwrap_or_default())
        .collect();
    for i in 0..outlines.len() {
        for j in (i + 1)..outlines.len() {
            let (bi, bj) = (&boxes[i], &boxes[j]);
            // If separated by more than the spacing in either axis, the rings
            // are too far apart to violate it — skip the O(n^2) check.
            let x_gap = bi.min_x.max(bj.min_x) - bi.max_x.min(bj.max_x);
            let y_gap = bi.min_y.max(bj.min_y) - bi.max_y.min(bj.max_y);
            if x_gap > required || y_gap > required {
                continue;
            }
            let d = min_ring_distance(&outlines[i], &outlines[j]);
            if d < required - SPACING_TOL {
                return Err(SheetError::TooClose {
                    a: i,
                    b: j,
                    distance: d,
                    required,
                });
            }
        }
    }

    // Strip width sanity: the used width must fit the stock on the fill side.
    // For a horizontally-flipped corner the strip grows leftward from
    // (w - offset.x), so it overflows when it is wider than that span.
    let flipped_x = matches!(
        mapping.corner,
        NestingCorner::LowerRight | NestingCorner::UpperRight
    );
    let over = if flipped_x {
        solution.strip_width - (w - mapping.offset.x)
    } else {
        mapping.offset.x + solution.strip_width - w
    };
    if over > eps {
        return Err(SheetError::StripTooWide(solution.strip_width));
    }

    let utilization: f64 = placed
        .iter()
        .map(|(part, _, _)| parts[*part].area)
        .sum::<f64>()
        / (w * h);

    Ok(SheetLayout {
        placements: placed
            .iter()
            .enumerate()
            .map(|(i, (part, copy, t))| SheetPlacement {
                part_index: *part,
                copy_index: *copy,
                item_id: item_ids[i],
                transform: *t,
            })
            .collect(),
        strip_width: solution.strip_width,
        utilization,
        checked_gap_mm: Some(checked_gap(required, cfg.chord_tol)),
    })
}

/// The least distance between two parts' real outlines that the spacing check above proves, mm.
/// The check measures the straight-segment forms (within [`SPACING_TOL`]), and a curve bulges up
/// to `chord_tol` past its form, on each of the two parts. Rounded down to 0.001 mm.
fn checked_gap(required: f64, chord_tol: f64) -> f64 {
    let gap = required - SPACING_TOL - 2.0 * chord_tol.max(0.0);
    // The 1e-9 keeps float noise (4.8989999…) from rounding a whole thousandth down.
    (gap.max(0.0) * 1000.0 + 1e-9).floor() / 1000.0
}

/// The first line of a sheet DXF the nest checked: a DXF comment (group 999, which readers skip)
/// giving the least gap between parts, so CAM downstream can check a tool against it (on a router,
/// a cut-out bit wider than the gap cuts into the next part). Readers look for `part_gap_mm=`.
pub const NEST_RECORD_PREFIX: &str = "FoundryPDM nest:";

fn nest_record(gap_mm: f64) -> String {
    format!(
        "999\n{NEST_RECORD_PREFIX} part_gap_mm={gap_mm:.3} (the least distance between parts the nest checked, curves included)\n"
    )
}

/// Map a solver placement into a stock-space transform.
pub fn solver_to_stock(p: &Placed, offset: Point) -> Transform {
    crate::solver::placed_transform(p, offset)
}

/// Assign a stable, decodable item id to each copy. Layout: high bytes = part
/// index, low bytes = copy index. `part` fits in the upper 32 bits, `copy` in
/// the lower 32 bits.
pub fn assign_item_ids(copies: &[CopyRef]) -> Vec<u64> {
    copies
        .iter()
        .map(|c| ((c.part as u64) << 32) | (c.copy as u64))
        .collect()
}

pub fn decode_item_id(id: u64) -> (usize, usize) {
    ((id >> 32) as usize, (id & 0xFFFF_FFFF) as usize)
}

/// Minimum distance between two closed rings (tessellated), in mm.
fn min_ring_distance(a: &[Point], b: &[Point]) -> f64 {
    let na = a.len();
    let nb = b.len();
    let mut best = f64::INFINITY;
    for i in 0..na {
        let p1 = a[i];
        let p2 = a[(i + 1) % na];
        for j in 0..nb {
            let q1 = b[j];
            let q2 = b[(j + 1) % nb];
            best = best.min(seg_seg_dist(p1, p2, q1, q2));
        }
    }
    best
}

/// Minimum distance between segments p1-p2 and q1-q2.
fn seg_seg_dist(p1: Point, p2: Point, q1: Point, q2: Point) -> f64 {
    // If the segments intersect, distance is 0. Otherwise min of the 4
    // point-to-segment distances.
    if segments_intersect(p1, p2, q1, q2) {
        return 0.0;
    }
    [
        point_seg_dist(p1, q1, q2),
        point_seg_dist(p2, q1, q2),
        point_seg_dist(q1, p1, p2),
        point_seg_dist(q2, p1, p2),
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min)
}

fn point_seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    if len2 < 1e-18 {
        return p.minus(a).len();
    }
    let t = ((p.x - a.x) * abx + (p.y - a.y) * aby) / len2;
    let t = t.clamp(0.0, 1.0);
    let proj = Point::new(a.x + t * abx, a.y + t * aby);
    p.minus(proj).len()
}

fn segments_intersect(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
    fn orient(a: Point, b: Point, c: Point) -> f64 {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    }
    let d1 = orient(q1, q2, p1);
    let d2 = orient(q1, q2, p2);
    let d3 = orient(p1, p2, q1);
    let d4 = orient(p1, p2, q2);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Render the stock-space primitives (outer + holes + marks) for a sheet.
pub fn sheet_primitives(layout: &SheetLayout, parts: &[Part]) -> Vec<dxf::Primitive> {
    let mut prims: Vec<dxf::Primitive> = Vec::new();
    for pl in &layout.placements {
        let part = &parts[pl.part_index];
        for c in &part.outer.edges {
            prims.push(transform_edge_to_primitive(
                c,
                &pl.transform,
                &part.outer.layer,
            ));
        }
        for hole in &part.holes {
            for c in &hole.edges {
                prims.push(transform_edge_to_primitive(c, &pl.transform, &hole.layer));
            }
        }
        for mark in &part.marks {
            for c in &mark.edges {
                prims.push(transform_edge_to_primitive(c, &pl.transform, &mark.layer));
            }
        }
    }
    prims
}

fn transform_edge_to_primitive(
    e: &crate::part::Edge,
    t: &Transform,
    layer: &str,
) -> dxf::Primitive {
    match e {
        crate::part::Edge::Line { a, b } => {
            let a2 = t.apply(*a);
            let b2 = t.apply(*b);
            dxf::Primitive::Line {
                layer: layer.to_string(),
                start: a2,
                end: b2,
            }
        }
        crate::part::Edge::Arc {
            center,
            radius,
            start,
            end,
        } => {
            // A full turn exports as a circle (its start/end coincide).
            if full_turn(*start, *end) {
                let c2 = t.apply(*center);
                let p1 = t.apply(Point::new(center.x + radius, center.y));
                let r = c2.minus(p1).len();
                return dxf::Primitive::Circle {
                    layer: layer.to_string(),
                    center: c2,
                    radius: r,
                };
            }
            // Apply the transform to the center and two points on the arc to
            // recover the transformed center/radius/angles.
            let c2 = t.apply(*center);
            let p0 = crate::part::edge_point_on_arc(*center, *radius, *start);
            let p1 = crate::part::edge_point_on_arc(*center, *radius, *end);
            let p02 = t.apply(p0);
            let p12 = t.apply(p1);
            let mid = crate::part::edge_point_on_arc(*center, *radius, (*start + *end) / 2.0);
            let mid2 = t.apply(mid);
            // Reconstruct the circle through the three transformed points.
            match circle_from_three(c2, p02, p12) {
                Some((cc, r)) => {
                    let s = angle_of(cc, p02);
                    let e2 = angle_of(cc, p12);
                    let _ = mid2;
                    // Determine sweep direction (CCW) via the mid point.
                    let m_ang = angle_of(cc, mid2);
                    let (s, e2, _) = orient_arc(s, e2, m_ang);
                    dxf::Primitive::Arc {
                        layer: layer.to_string(),
                        center: cc,
                        radius: r,
                        start_deg: s,
                        end_deg: e2,
                    }
                }
                None => {
                    // Degenerate (very small arc) -> treat as a polyline of the
                    // arc's points.
                    dxf::Primitive::Line {
                        layer: layer.to_string(),
                        start: p02,
                        end: p12,
                    }
                }
            }
        }
    }
}

/// Whether an arc from `start` to `end` (CCW) is a full turn. The representation
/// keeps `end - start` equal to the sweep, so a full turn reads as ~360.
fn full_turn(start: f64, end: f64) -> bool {
    end - start >= 360.0 - 1e-6
}

fn circle_from_three(a: Point, b: Point, c: Point) -> Option<(Point, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let a2 = a.x * a.x + a.y * a.y;
    let b2 = b.x * b.x + b.y * b.y;
    let c2 = c.x * c.x + c.y * c.y;
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = Point::new(ux, uy);
    let r = center.minus(a).len();
    Some((center, r))
}

fn angle_of(center: Point, p: Point) -> f64 {
    (p.y - center.y)
        .atan2(p.x - center.x)
        .to_degrees()
        .rem_euclid(360.0)
}

/// Given start, end, and a mid angle (all CCW from start), normalize to a
/// counter-clockwise (start, end) pair with the mid lying on the sweep.
fn orient_arc(start: f64, end: f64, mid: f64) -> (f64, f64, bool) {
    let norm = |a: f64| a.rem_euclid(360.0);
    let s = norm(start);
    let e = norm(end);
    let m = norm(mid);
    // The CCW sweep from s to e is (e - s) mod 360 (0 -> 360 treated as full).
    let sweep = if e >= s { e - s } else { e - s + 360.0 };
    // mid should be within (s, s+sweep).
    let rel = norm(m - s);
    if sweep <= 1e-9 {
        return (s, s + 360.0, true);
    }
    if rel <= sweep + 1e-6 {
        (s, s + sweep, true)
    } else {
        // mid is on the other side: the arc actually goes the long way.
        (s, s + (360.0 - sweep), false)
    }
}

/// Render the full sheet as a DXF string (mm, $INSUNITS=4) with a stock outline.
pub fn export_sheet_dxf(
    layout: &SheetLayout,
    parts: &[Part],
    labels: &[String],
    cfg: &NestConfig,
) -> String {
    let mut prims = sheet_primitives(layout, parts);
    // Optional part labels (text, not cutting geometry).
    if cfg.add_part_labels {
        for pl in &layout.placements {
            let part = &parts[pl.part_index];
            let text = labels
                .get(pl.part_index)
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| label_for(pl));
            let c = pl
                .transform
                .apply(crate::geom::centroid(&part.outer.points));
            prims.push(dxf::Primitive::Text {
                layer: "LABELS".into(),
                pos: c,
                height: 3.0,
                content: text,
            });
        }
    }
    let body = dxf::write(&prims, Some((cfg.stock_width, cfg.stock_height)), "STOCK");
    // Only a layout the spacing check passed says what gap it keeps.
    match layout.checked_gap_mm {
        Some(gap) => nest_record(gap) + &body,
        None => body,
    }
}

/// Render the sheet as a preview SVG: stock outline, nominal outer contours,
/// holes (dashed), and a per-copy label. Labels and the stock border are drawn
/// as overlays (not cutting geometry).
pub fn export_sheet_svg(
    layout: &SheetLayout,
    parts: &[Part],
    labels: &[String],
    cfg: &NestConfig,
) -> String {
    let w = cfg.stock_width;
    let h = cfg.stock_height;
    let pad = 8.0;
    let neg_pad = -pad;
    let sw = w + 2.0 * pad;
    let sh = h + 2.0 * pad;
    let mut s = String::new();
    s.push_str(&format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" \
         viewBox=\"{neg_pad:.0} {neg_pad:.0} {sw:.0} {sh:.0}\">\n"
    ));
    // Stock border.
    s.push_str(&format!(
        "<rect x=\"0\" y=\"0\" width=\"{w:.1}\" height=\"{h:.1}\" fill=\"#fafafa\" stroke=\"#999\" stroke-width=\"1\"/>\n"
    ));
    // Grain direction indicator (top-left corner, low emphasis).
    match cfg.sheet_grain {
        GrainDir::AlongLength => {
            s.push_str(
                "<g opacity=\"0.5\"><line x1=\"10\" y1=\"10\" x2=\"10\" y2=\"54\" stroke=\"#6a5\" stroke-width=\"1.5\"/>
                 <polygon points=\"10,58 7,50 13,50\" fill=\"#6a5\"/>
                 <polygon points=\"10,6 7,14 13,14\" fill=\"#6a5\"/>
                 <text x=\"16\" y=\"36\" font-size=\"9\" fill=\"#6a5\">GRAIN (length)</text></g>\n",
            );
        }
        GrainDir::AlongWidth => {
            s.push_str(
                "<g opacity=\"0.5\"><line x1=\"10\" y1=\"10\" x2=\"54\" y2=\"10\" stroke=\"#6a5\" stroke-width=\"1.5\"/>
                 <polygon points=\"58,10 50,7 50,13\" fill=\"#6a5\"/>
                 <polygon points=\"6,10 14,7 14,13\" fill=\"#6a5\"/>
                 <text x=\"16\" y=\"24\" font-size=\"9\" fill=\"#6a5\">GRAIN (width)</text></g>\n",
            );
        }
        GrainDir::Any => {}
    }
    for pl in &layout.placements {
        let part = &parts[pl.part_index];
        // Outer contour.
        let outer: String = part
            .outer
            .points
            .iter()
            .map(|p| {
                let t = pl.transform.apply(*p);
                format!("{:.2},{:.2}", t.x, t.y)
            })
            .collect::<Vec<_>>()
            .join(" ");
        s.push_str(&format!(
            "<polygon points=\"{}\" fill=\"rgba(40,120,200,0.15)\" stroke=\"#1f6feb\" stroke-width=\"1.2\"/>\n",
            outer
        ));
        // Holes.
        for hole in &part.holes {
            let pts: String = hole
                .points
                .iter()
                .map(|p| {
                    let t = pl.transform.apply(*p);
                    format!("{:.2},{:.2}", t.x, t.y)
                })
                .collect::<Vec<_>>()
                .join(" ");
            s.push_str(&format!(
                "<polygon points=\"{}\" fill=\"rgba(255,255,255,0.9)\" stroke=\"#1f6feb\" stroke-width=\"0.8\" stroke-dasharray=\"3,2\"/>\n",
                pts
            ));
        }
        // Label at the part centroid (the line's label when set, else P#-C#).
        let c = pl
            .transform
            .apply(crate::geom::centroid(&part.outer.points));
        let text = labels
            .get(pl.part_index)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| label_for(pl));
        s.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10\" fill=\"#333\" text-anchor=\"middle\">{}</text>\n",
            c.x,
            c.y,
            text
        ));
    }
    s.push_str("</svg>\n");
    s
}

fn label_for(pl: &SheetPlacement) -> String {
    format!("P{}-C{}", pl.part_index + 1, pl.copy_index + 1)
}

/// Render the assembled parts (untransformed, local coordinates) as a preview
/// SVG: outer profile (green), holes (red, dashed), marks (grey). Used for the
/// flat-pattern thumbnails in the nest part picker.
pub fn render_parts_preview(parts: &[Part], width: f64, height: f64) -> String {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in parts {
        min_x = min_x.min(p.outer.bbox.min_x);
        min_y = min_y.min(p.outer.bbox.min_y);
        max_x = max_x.max(p.outer.bbox.max_x);
        max_y = max_y.max(p.outer.bbox.max_y);
    }
    if !min_x.is_finite() {
        return format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.0} {height:.0}\"><rect width=\"{width:.0}\" height=\"{height:.0}\" fill=\"#111\"/></svg>"
        );
    }
    let span_x = (max_x - min_x).max(1e-6);
    let span_y = (max_y - min_y).max(1e-6);
    let pad = 12.0;
    let scale = ((width - 2.0 * pad) / span_x).min((height - 2.0 * pad) / span_y);
    let ox = pad + (width - 2.0 * pad - span_x * scale) / 2.0 - min_x * scale;
    let oy = pad + (height - 2.0 * pad - span_y * scale) / 2.0 + min_y * scale;
    let tr = |p: &Point| format!("{:.2},{:.2}", p.x * scale + ox, height - (p.y * scale + oy));
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.0} {height:.0}\">
         <rect width=\"{width:.0}\" height=\"{height:.0}\" fill=\"#111\"/>"
    );
    for p in parts {
        // Outer profile.
        let pts: Vec<String> = p.outer.points.iter().map(&tr).collect();
        s.push_str(&format!(
            "<polygon points=\"{}\" fill=\"none\" stroke=\"#3c3\" stroke-width=\"1.5\"/>",
            pts.join(" ")
        ));
        // Holes.
        for h in &p.holes {
            let hpts: Vec<String> = h.points.iter().map(&tr).collect();
            s.push_str(&format!(
                "<polygon points=\"{}\" fill=\"none\" stroke=\"#c33\" stroke-width=\"1\" stroke-dasharray=\"3,2\"/>",
                hpts.join(" ")
            ));
        }
        // Marks.
        for m in &p.marks {
            let mpts: Vec<String> = m.points.iter().map(&tr).collect();
            s.push_str(&format!(
                "<polyline points=\"{}\" fill=\"none\" stroke=\"#888\" stroke-width=\"1\"/>",
                mpts.join(" ")
            ));
        }
    }
    s.push_str("</svg>");
    s
}

/// Convenience: the part's nominal outer polygon in its local (untransformed)
/// coordinates, for building solver instances.
pub fn part_outline(part: &Part) -> &[Point] {
    &part.outer.points
}

/// Total nominal demand area across all copies (for utilization reporting).
pub fn demand_area(parts: &[Part], copies: &[CopyRef]) -> f64 {
    copies.iter().map(|c| parts[c.part].area).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dxf::Primitive as P;
    use crate::part::assemble;

    fn square_part() -> Part {
        let prim = P::Polyline {
            layer: "cut".into(),
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 10.0),
                Point::new(0.0, 10.0),
            ],
            bulges: vec![None; 4],
            closed: true,
        };
        assemble(&[prim], &["cut".to_string()], &[], 0.05, 0.01)
            .unwrap()
            .parts
            .pop()
            .unwrap()
    }

    #[test]
    fn item_id_roundtrip() {
        let id = ((7u64) << 32) | 3u64;
        let (p, c) = decode_item_id(id);
        assert_eq!(p, 7);
        assert_eq!(c, 3);
    }

    #[test]
    fn mapping_derivation() {
        let cfg = NestConfig {
            stock_width: 200.0,
            stock_height: 100.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            chord_tol: 0.05,
            ..Default::default()
        };
        let m = StockMapping::from_config(&cfg).unwrap();
        // S = 2*chord_tol + 0.05 = 0.15; inset = M - gap_max/2 + c + safety
        // = 10 - 2.5 + 0.05 + 1.0 = 8.55.
        assert!((m.separation - 0.15).abs() < 1e-9);
        assert!((m.offset.x - 8.55).abs() < 1e-9);
        assert!((m.offset.y - 8.55).abs() < 1e-9);
        assert!((m.strip_height - (100.0 - 2.0 * 8.55)).abs() < 1e-9);
    }

    fn bounds_cfg() -> (NestConfig, StockMapping) {
        let cfg = NestConfig {
            stock_width: 100.0,
            stock_height: 100.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            chord_tol: 0.05,
            ..Default::default()
        };
        let mapping = StockMapping::from_config(&cfg).unwrap();
        (cfg, mapping)
    }

    #[test]
    fn build_sheet_accepts_separation_inset_placement() {
        let part = square_part();
        let parts = vec![part];
        let (cfg, mapping) = bounds_cfg();
        // A 10x10 part placed at strip (5.1, 5.1) lands at stock [13.65, 23.65]
        // (offset 8.55), inside the 10 mm margins.
        let solution = SolverSolution {
            strip_width: 25.0,
            placed: vec![Placed {
                item_id: assign_item_ids(&[CopyRef { part: 0, copy: 0 }])[0],
                reflected: false,
                rotation_deg: 0.0,
                translation: [5.1, 5.1],
            }],
        };
        let ids = assign_item_ids(&[CopyRef { part: 0, copy: 0 }]);
        let layout = build_sheet(&solution, &ids, &parts, &cfg, &mapping).unwrap();
        assert_eq!(layout.placements.len(), 1);
    }

    #[test]
    fn build_sheet_rejects_placement_at_strip_origin() {
        let part = square_part();
        let parts = vec![part];
        let (cfg, mapping) = bounds_cfg();
        // A placement at the strip origin (not inset) is closer to the stock edge
        // than the mapping allows -> out of bounds.
        let solution = SolverSolution {
            strip_width: 25.0,
            placed: vec![Placed {
                item_id: assign_item_ids(&[CopyRef { part: 0, copy: 0 }])[0],
                reflected: false,
                rotation_deg: 0.0,
                translation: [0.0, 0.0],
            }],
        };
        let ids = assign_item_ids(&[CopyRef { part: 0, copy: 0 }]);
        assert!(matches!(
            build_sheet(&solution, &ids, &parts, &cfg, &mapping),
            Err(SheetError::OutOfBounds { .. })
        ));
    }

    /// Two 10 x 10 squares side by side, `gap` mm apart in stock space, on the 100 x 100 stock
    /// with 5 mm spacing; the second one flipped when `flip_second`.
    fn squares(gap: f64, flip_second: bool) -> Result<SheetLayout, SheetError> {
        let parts = vec![square_part()];
        let (cfg, mapping) = bounds_cfg();
        let copies = [CopyRef { part: 0, copy: 0 }, CopyRef { part: 0, copy: 1 }];
        let ids = assign_item_ids(&copies);
        // A flip mirrors across the local x axis (y -> -y), so a flipped square sits 10 mm
        // lower for the same translation: lift it back level with the first.
        let at = |id: u64, x: f64, reflected: bool| Placed {
            item_id: id,
            reflected,
            rotation_deg: 0.0,
            translation: [x, if reflected { 15.1 } else { 5.1 }],
        };
        let solution = SolverSolution {
            strip_width: 40.0,
            placed: vec![
                at(ids[0], 5.1, false),
                at(ids[1], 5.1 + 10.0 + gap, flip_second),
            ],
        };
        build_sheet(&solution, &ids, &parts, &cfg, &mapping)
    }

    fn two_squares(gap: f64) -> Result<SheetLayout, SheetError> {
        squares(gap, false)
    }

    #[test]
    fn build_sheet_checks_the_spacing_of_flipped_parts() {
        assert!(matches!(
            squares(3.0, true),
            Err(SheetError::TooClose { .. })
        ));
        let layout = squares(5.0, true).unwrap();
        assert!(layout.placements[1].transform.reflected);
    }

    #[test]
    fn build_sheet_rejects_parts_closer_than_the_spacing() {
        // 3 mm apart: well over the solver's 0.15 mm separation, but under the 5 mm spacing.
        match two_squares(3.0) {
            Err(SheetError::TooClose { distance, required, .. }) => {
                assert!((distance - 3.0).abs() < 1e-9, "{distance}");
                assert!((required - 5.0).abs() < 1e-9, "{required}");
            }
            other => panic!("expected TooClose, got {other:?}"),
        }
    }

    #[test]
    fn build_sheet_accepts_parts_at_the_spacing() {
        assert_eq!(two_squares(5.0).unwrap().placements.len(), 2);
    }

    #[test]
    fn export_sheet_dxf_writes_mm() {
        let part = square_part();
        let parts = vec![part];
        let cfg = NestConfig {
            stock_width: 100.0,
            stock_height: 100.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            chord_tol: 0.05,
            ..Default::default()
        };
        let mapping = StockMapping::from_config(&cfg).unwrap();
        let ids = assign_item_ids(&[CopyRef { part: 0, copy: 0 }]);
        let solution = SolverSolution {
            strip_width: 30.0,
            placed: vec![Placed {
                item_id: ids[0],
                reflected: false,
                rotation_deg: 0.0,
                translation: [10.0, 10.0],
            }],
        };
        let layout = build_sheet(&solution, &ids, &parts, &cfg, &mapping).unwrap();
        let dxf_text = export_sheet_dxf(&layout, &parts, &[], &cfg);
        let parsed = dxf::parse(&dxf_text).unwrap();
        assert_eq!(parsed.units, dxf::Unit::Mm);
        // stock + one part outline (4 edges) -> >= 5 primitives.
        assert!(parsed.primitives.len() >= 5);
        // The gap the check proved leads the file, as a DXF comment: 5 mm checked, less
        // 0.001 mm for rounding and 0.05 mm of curve on each of two parts.
        assert_eq!(layout.checked_gap_mm, Some(4.899));
        assert!(
            dxf_text.starts_with("999\nFoundryPDM nest: part_gap_mm=4.899 (the least distance between parts the nest checked, curves included)\n0\nSECTION\n"),
            "{}",
            &dxf_text[..dxf_text.len().min(160)]
        );
    }

    #[test]
    fn a_layout_the_check_didnt_make_says_nothing_of_its_gap() {
        // An older stored layout has no gap, so its DXF carries no record.
        let old = r#"{"placements":[],"stripWidth":0.0,"utilization":0.0}"#;
        let layout: SheetLayout = serde_json::from_str(old).unwrap();
        assert_eq!(layout.checked_gap_mm, None);
        let cfg = NestConfig { stock_width: 100.0, stock_height: 100.0, ..Default::default() };
        let dxf_text = export_sheet_dxf(&layout, &[], &[], &cfg);
        assert!(dxf_text.starts_with("0\nSECTION\n"), "{}", &dxf_text[..40]);
        assert!(!dxf_text.contains(NEST_RECORD_PREFIX));
        // The record is left out of the JSON when there is none, and kept when there is one.
        assert!(!serde_json::to_string(&layout).unwrap().contains("checkedGapMm"));
    }

    #[test]
    fn the_checked_gap_allows_for_curves_and_rounding() {
        assert_eq!(checked_gap(9.525, 0.05), 9.424);
        assert_eq!(checked_gap(6.35, 0.05), 6.249);
        assert_eq!(checked_gap(0.0, 0.05), 0.0);
        assert_eq!(checked_gap(10.0, 0.0), 9.999);
    }

    #[test]
    fn corner_transform_flips_to_requested_corner() {
        let cfg = NestConfig {
            stock_width: 100.0,
            stock_height: 200.0,
            ..Default::default()
        };
        let map = |c: crate::NestingCorner| StockMapping {
            separation: 0.0,
            offset: Point::new(0.0, 0.0),
            strip_height: 100.0,
            gap_x: 0.0,
            gap_y: 0.0,
            corner: c,
        };
        // Upper-right is a 180° rotation about the stock center: interior
        // points must map to (100−x, 200−y), not (100−x, 200+y).
        let ur = map(crate::NestingCorner::UpperRight);
        let p = ur.corner_transform(&cfg).apply(Point::new(20.0, 50.0));
        assert!((p.x - 80.0).abs() < 1e-6 && (p.y - 150.0).abs() < 1e-6);
        // Lower-right mirrors x only; upper-left mirrors y only.
        let lr = map(crate::NestingCorner::LowerRight)
            .corner_transform(&cfg)
            .apply(Point::new(20.0, 50.0));
        assert!((lr.x - 80.0).abs() < 1e-6 && (lr.y - 50.0).abs() < 1e-6);
        let ul = map(crate::NestingCorner::UpperLeft)
            .corner_transform(&cfg)
            .apply(Point::new(20.0, 50.0));
        assert!((ul.x - 20.0).abs() < 1e-6 && (ul.y - 150.0).abs() < 1e-6);
        // The lower-left corner stays the identity.
        let ll = map(crate::NestingCorner::LowerLeft)
            .corner_transform(&cfg)
            .apply(Point::new(3.0, 4.0));
        assert!((ll.x - 3.0).abs() < 1e-6 && (ll.y - 4.0).abs() < 1e-6);
    }

    #[test]
    fn add_part_labels_writes_text() {
        let part = square_part();
        let parts = vec![part];
        let cfg = NestConfig {
            stock_width: 100.0,
            stock_height: 100.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            chord_tol: 0.05,
            add_part_labels: true,
            ..Default::default()
        };
        let mapping = StockMapping::from_config(&cfg).unwrap();
        let ids = assign_item_ids(&[CopyRef { part: 0, copy: 0 }]);
        let solution = SolverSolution {
            strip_width: 30.0,
            placed: vec![Placed {
                item_id: ids[0],
                reflected: false,
                rotation_deg: 0.0,
                translation: [10.0, 10.0],
            }],
        };
        let layout = build_sheet(&solution, &ids, &parts, &cfg, &mapping).unwrap();
        let dxf_text = export_sheet_dxf(&layout, &parts, &["BRK".to_string()], &cfg);
        let parsed = dxf::parse(&dxf_text).unwrap();
        let has_label = parsed
            .primitives
            .iter()
            .any(|p| matches!(p, dxf::Primitive::Text { layer, content, .. } if layer == "LABELS" && content == "BRK"));
        assert!(has_label, "expected a LABELS text primitive");
    }
}
