//! Bounded multi-sheet batching around the (single-strip) Sparrow solver.
//!
//! The solver places every demanded copy in one strip and minimizes the strip
//! width. A batch fits a stock sheet iff its solved width and nominal geometry
//! satisfy the stock margins and the part spacing. This module splits demand
//! into per-sheet batches, solving each candidate and keeping the last accepted
//! feasible layout, under one global wall-clock budget.
//!
//! Demand is expressed as [`NestLine`]s: a part (by index) with a quantity and
//! per-line options (allow-mirror, grain). The same part may appear on several
//! lines with different options.

use crate::geom::Point;
use crate::part::Part;
use crate::sheet::{self, CopyRef, SheetLayout, StockMapping};
use crate::solver::{self, SolverInstance, SolverItem, SolverSolution};
use crate::{GrainDir, NestConfig};
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BatchError {
    #[error("total copies {0} exceeds the maximum {1}")]
    TooManyCopies(usize, usize),
    #[error("part {0} does not fit a stock sheet in any allowed orientation")]
    OversizedPart(usize),
    #[error("partial demand: {remaining} copies could not be placed within {sheets} sheets")]
    Partial {
        remaining: usize,
        sheets: usize,
        /// Sheets placed before this size ran out of demand capacity.
        placed: Vec<BatchSheet>,
    },
    #[error("solver: {0}")]
    Solver(#[from] solver::SolverError),
    #[error("sheet: {0}")]
    Sheet(#[from] sheet::SheetError),
}

/// One demand line: part index + quantity + per-line nesting options.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestLine {
    /// Index into the parts slice.
    pub part: usize,
    pub quantity: u64,
    /// Allow the solver to mirror (flip) copies of this line.
    pub allow_mirror: bool,
    /// This line's grain preference relative to the part's long axis.
    pub grain: GrainDir,
    /// Rotation increment in degrees (90 => 0/90/180/270, 45 => 8, 180 => 0/180).
    pub step_angle: f64,
    /// Nesting priority (lower number placed first).
    pub priority: i32,
    /// Nest by the part's bounding box instead of its outline.
    pub use_bounding_box: bool,
    /// Part label (drawn when the config has labels enabled).
    pub label: String,
}

/// A sheet of the batch: its copies, the validated layout, and the raw solution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSheet {
    pub copies: Vec<CopyRef>,
    pub layout: SheetLayout,
    pub strip_width: f64,
}

/// Pre-check that every line's part fits a stock sheet in at least one allowed
/// orientation (nominal bounding box within the usable rectangle).
pub fn precheck(lines: &[NestLine], parts: &[Part], cfg: &NestConfig) -> Result<(), BatchError> {
    let usable_w = cfg.stock_width - 2.0 * cfg.edge_margin;
    let usable_h = cfg.stock_height - 2.0 * cfg.edge_margin;
    for line in lines {
        let p = &parts[line.part];
        let bb = p.outer.bbox;
        let rots = line_rotations(cfg, p, line);
        let fits = rots
            .iter()
            .any(|&r| rotation_fits(bb.width(), bb.height(), r, usable_w, usable_h));
        if !fits {
            return Err(BatchError::OversizedPart(line.part));
        }
    }
    Ok(())
}

fn rotation_fits(w: f64, h: f64, rot_deg: f64, usable_w: f64, usable_h: f64) -> bool {
    let r = (rot_deg / 90.0).round();
    let (ew, eh) = match r.rem_euclid(4.0) {
        0.0 | 2.0 => (w, h),
        _ => (h, w),
    };
    ew <= usable_w && eh <= usable_h
}

/// The rotation base set from a step angle (degrees): `{0, step, 2·step, … <
/// 360}`. A step of 360° (or ≤ 0) means a single orientation.
fn step_set(step: f64) -> Vec<f64> {
    let s = if step > 1.0 { step } else { 90.0 };
    let mut v = Vec::new();
    let mut i = 0.0;
    while i < 360.0 - 1e-6 {
        v.push(i);
        i += s;
    }
    v
}

/// The rotations allowed for a line: the step-angle base set filtered by the
/// sheet/part grain alignment (or the explicit `cfg.allowed_rotations` when
/// non-empty). The stock *length* axis is vertical (y) in PDM layouts; the
/// part's long axis is its larger bounding-box side; grain alignment keeps the
/// part's relevant axis parallel to the sheet grain axis.
pub fn line_rotations(cfg: &NestConfig, part: &Part, line: &NestLine) -> Vec<f64> {
    if !cfg.allowed_rotations.is_empty() {
        return cfg.allowed_rotations.clone();
    }
    rotate_set(cfg, part, line.grain, line.step_angle)
}

/// Legacy entry point: grain-derived rotations at a 90° step.
pub fn grain_rotations(cfg: &NestConfig, part: &Part, grain: GrainDir) -> Vec<f64> {
    if !cfg.allowed_rotations.is_empty() {
        return cfg.allowed_rotations.clone();
    }
    rotate_set(cfg, part, grain, 90.0)
}

fn rotate_set(cfg: &NestConfig, part: &Part, grain: GrainDir, step: f64) -> Vec<f64> {
    let base = step_set(step);
    if cfg.sheet_grain == GrainDir::Any || grain == GrainDir::Any {
        return base;
    }
    let bb = part.outer.bbox;
    let w = bb.width();
    let h = bb.height();
    if (w - h).abs() < 1e-6 {
        return base; // square: any orientation aligns
    }
    let part_local_long = if w > h { 0.0 } else { 90.0 };
    let sheet_dir = if cfg.sheet_grain == GrainDir::AlongLength {
        90.0
    } else {
        0.0
    };
    let mut target = sheet_dir;
    if grain == GrainDir::AlongWidth {
        target += 90.0; // part's long axis must run across the sheet grain
    }
    target %= 180.0;
    let delta = ((target - part_local_long) % 180.0 + 180.0) % 180.0;
    let filtered: Vec<f64> = base
        .iter()
        .filter(|th| {
            let m = ((**th) % 180.0 + 180.0) % 180.0;
            let diff = (m - delta).abs();
            diff < 1e-3 || (180.0 - diff) < 1e-3
        })
        .copied()
        .collect();
    if filtered.is_empty() { base } else { filtered }
}

/// Expand demand lines into per-copy references (`part` = line index).
pub fn expand_lines(lines: &[NestLine]) -> Vec<CopyRef> {
    let mut v = Vec::new();
    for (line, l) in lines.iter().enumerate() {
        for copy in 0..usize::try_from(l.quantity).unwrap_or(usize::MAX) {
            v.push(CopyRef { part: line, copy });
        }
    }
    v
}

/// Total nominal area of a set of copies.
fn total_area(copies: &[CopyRef], parts: &[Part]) -> f64 {
    copies.iter().map(|c| parts[c.part].area).sum()
}

/// Build a solver instance for a specific set of copies (one sheet candidate).
pub fn build_instance(
    lines: &[NestLine],
    parts: &[Part],
    copies: &[CopyRef],
    cfg: &NestConfig,
    mapping: &StockMapping,
    name: &str,
) -> SolverInstance {
    let item_ids = sheet::assign_item_ids(copies);
    // What the solver nests for each line: the part, or its mirror image when the nesting
    // corner is a reflection, so that the corner turns it back (`StockMapping::corner_reflects`).
    let shapes: Vec<std::borrow::Cow<'_, Part>> = lines
        .iter()
        .map(|l| mapping.solver_part(&parts[l.part]))
        .collect();
    let items: Vec<SolverItem> = copies
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let line = &lines[c.part];
            let part = shapes[c.part].as_ref();
            // Collision shape: the dilated outline, or the dilated bounding box
            // when the line requests box nesting.
            let polygon = if line.use_bounding_box {
                let bb = &part.outer.bbox;
                let m = mapping.gap_x.max(mapping.gap_y) / 2.0;
                crate::part::dilate_box(
                    &[
                        crate::geom::Point::new(bb.min_x, bb.min_y),
                        crate::geom::Point::new(bb.max_x, bb.min_y),
                        crate::geom::Point::new(bb.max_x, bb.max_y),
                        crate::geom::Point::new(bb.min_x, bb.max_y),
                    ],
                    m,
                    m,
                )
            } else {
                mapping.solver_outline(part)
            };
            SolverItem {
                id: item_ids[i],
                part_index: line.part,
                polygon,
                demand: 1,
                // The rotations allowed on the sheet, as the solver must turn this shape.
                rotations: mapping.solver_rotations(line_rotations(cfg, &parts[line.part], line)),
                allow_reflection: line.allow_mirror,
            }
        })
        .collect();
    SolverInstance {
        name: name.to_string(),
        min_item_separation: mapping.separation,
        strip_height: mapping.strip_height,
        items,
    }
}

/// Run the batch. Returns one layout per sheet. Fails with `Partial` if copies
/// remain after `max_sheets` (or the budget is exhausted without progress).
pub fn run_batch<F: Fn(&SolverInstance) -> Result<SolverSolution, solver::SolverError>>(
    lines: &[NestLine],
    parts: &[Part],
    cfg: &NestConfig,
    oracle: F,
) -> Result<Vec<BatchSheet>, BatchError> {
    cfg.validate()
        .map_err(|e| BatchError::Sheet(sheet::SheetError::Mapping(e.to_string())))?;
    let mapping = StockMapping::from_config(cfg)?;
    precheck(lines, parts, cfg)?;

    let mut remaining = expand_lines(lines);
    if remaining.len() > cfg.max_copies as usize {
        return Err(BatchError::TooManyCopies(
            remaining.len(),
            cfg.max_copies as usize,
        ));
    }
    // Sort by priority (lower first), then area descending, then line/copy for
    // determinism.
    remaining.sort_by(|a, b| {
        lines[a.part]
            .priority
            .cmp(&lines[b.part].priority)
            .then(
                parts[lines[b.part].part]
                    .area
                    .total_cmp(&parts[lines[a.part].part].area),
            )
            .then(a.part.cmp(&b.part))
            .then(a.copy.cmp(&b.copy))
    });

    let start = Instant::now();
    let budget = std::time::Duration::from_secs(cfg.time_budget_sec as u64);
    let mut sheets: Vec<BatchSheet> = Vec::new();
    let usable =
        (cfg.stock_width - 2.0 * cfg.edge_margin) * (cfg.stock_height - 2.0 * cfg.edge_margin);
    let pack_factor = 0.85;

    // Solve a candidate set; Ok = a validated layout that fits the stock.
    let try_solve = |copies: &[CopyRef], sheet_no: usize| -> Result<(SheetLayout, f64), ()> {
        let name = format!("sheet{sheet_no}");
        let inst = build_instance(lines, parts, copies, cfg, &mapping, &name);
        let sol = oracle(&inst).map_err(|_| ())?;
        let ids = sheet::assign_item_ids(copies);
        // Fill from the corner on both axes (see `snap_down`). Should the moved layout not
        // validate, the solver's own is used.
        let snapped = snap_down(&sol, &inst, mapping.separation);
        sheet::build_sheet(&snapped, &ids, parts, cfg, &mapping)
            .or_else(|_| sheet::build_sheet(&sol, &ids, parts, cfg, &mapping))
            .map(|l| (l, sol.strip_width))
            .map_err(|_| ())
    };

    for _ in 0..cfg.max_sheets {
        if remaining.is_empty() {
            break;
        }
        if start.elapsed() > budget {
            break;
        }
        let placed: Option<(Vec<CopyRef>, SheetLayout, f64)> =
            if total_area(&remaining, parts) <= usable * pack_factor {
                // Everything left plausibly fits one sheet: solve it all at once.
                match try_solve(&remaining, sheets.len()) {
                    Ok((layout, sw)) => Some((remaining.clone(), layout, sw)),
                    Err(()) => None,
                }
            } else {
                None
            };
        let placed = match placed {
            Some(p) => Some(p),
            None => {
                // Split: take the largest sheet-sized prefix, dropping copies
                // until it validates.
                let mut k = 1usize;
                let mut acc = parts[lines[remaining[0].part].part].area;
                while k < remaining.len() && acc <= usable * pack_factor {
                    k += 1;
                    acc += parts[lines[remaining[k - 1].part].part].area;
                }
                let mut found = None;
                for kk in (1..=k).rev() {
                    if start.elapsed() > budget {
                        break;
                    }
                    let cand = &remaining[..kk];
                    if try_solve(cand, sheets.len()).is_ok_and(|(layout, sw)| {
                        found = Some((cand.to_vec(), layout, sw));
                        true
                    }) {
                        break;
                    }
                }
                found
            }
        };
        let Some((cand, layout, strip_width)) = placed else {
            break;
        };
        remaining = remaining[cand.len()..].to_vec();
        sheets.push(BatchSheet {
            copies: cand,
            layout,
            strip_width,
        });
    }

    if remaining.is_empty() {
        Ok(sheets)
    } else {
        Err(BatchError::Partial {
            remaining: remaining.len(),
            sheets: sheets.len(),
            placed: sheets,
        })
    }
}

/// Move a solved layout down the strip until its lowest item sits `floor` above the strip's
/// bottom edge.
///
/// Sparrow shortens the strip along x but leaves the layout's height in the strip open: the parts
/// sit a steady distance from the fill-side edge, but can sit anywhere between the strip's bottom
/// and top edges. So a nest starts from the chosen corner's side (left or right) but floats
/// between its top and bottom, leaving the off-cut in two pieces. The corner flip in
/// `sheet::build_sheet` turns the strip's bottom edge into the chosen corner's horizontal edge, so
/// pressing the layout down here puts it into that corner. The shift is uniform, so every distance
/// between parts is kept; a layout already at or below `floor` is left as it is. `floor` is the
/// solver's own clearance (`StockMapping::separation`), the gap it leaves at the fill-side edge.
pub fn snap_down(sol: &SolverSolution, inst: &SolverInstance, floor: f64) -> SolverSolution {
    let lowest = sol
        .placed
        .iter()
        .filter_map(|p| {
            let item = inst.items.iter().find(|i| i.id == p.item_id)?;
            let t = crate::geom::Transform {
                reflected: p.reflected,
                rotation_deg: p.rotation_deg,
                tx: p.translation[0],
                ty: p.translation[1],
            };
            item.polygon.iter().map(|q| t.apply(*q).y).reduce(f64::min)
        })
        .reduce(f64::min);
    let mut out = sol.clone();
    if let Some(drop) = lowest.map(|low| low - floor).filter(|d| *d > 0.0) {
        for p in &mut out.placed {
            p.translation[1] -= drop;
        }
    }
    out
}

/// A trivial in-memory oracle for tests: greedily axis-aligned packs the copies
/// into a strip and reports the width. Not a real optimizer, but exercises the
/// batching/validation paths deterministically.
#[allow(dead_code)]
pub fn mock_oracle(
    mapping: &StockMapping,
) -> impl Fn(&SolverInstance) -> Result<SolverSolution, solver::SolverError> {
    move |inst: &SolverInstance| -> Result<SolverSolution, solver::SolverError> {
        let mut x = mapping.separation;
        let mut placed = Vec::new();
        for it in &inst.items {
            let bb = bbox_of(&it.polygon);
            placed.push(solver::Placed {
                item_id: it.id,
                reflected: false,
                rotation_deg: 0.0,
                translation: [x, mapping.separation],
            });
            x += bb.1 + mapping.separation;
        }
        Ok(SolverSolution {
            strip_width: x,
            placed,
        })
    }
}

fn bbox_of(pts: &[Point]) -> (f64, f64) {
    let mut minx = f64::INFINITY;
    let mut miny = f64::INFINITY;
    let mut maxx = f64::NEG_INFINITY;
    let mut maxy = f64::NEG_INFINITY;
    for p in pts {
        minx = minx.min(p.x);
        miny = miny.min(p.y);
        maxx = maxx.max(p.x);
        maxy = maxy.max(p.y);
    }
    (maxx - minx, maxy - miny)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dxf::Primitive as P;
    use crate::part::assemble;

    fn line(part: usize, quantity: u64) -> NestLine {
        NestLine {
            part,
            quantity,
            allow_mirror: false,
            grain: GrainDir::Any,
            step_angle: 90.0,
            priority: 1,
            use_bounding_box: false,
            label: String::new(),
        }
    }

    fn rect_part(w: f64, h: f64) -> Part {
        let prim = P::Polyline {
            layer: "cut".into(),
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(w, 0.0),
                Point::new(w, h),
                Point::new(0.0, h),
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
    fn precheck_rejects_oversized() {
        let parts = vec![rect_part(500.0, 500.0)];
        let lines = vec![line(0, 1)];
        let cfg = NestConfig {
            stock_width: 200.0,
            stock_height: 200.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            ..Default::default()
        };
        assert!(matches!(
            precheck(&lines, &parts, &cfg),
            Err(BatchError::OversizedPart(0))
        ));
    }

    #[test]
    fn expand_lines_counts() {
        let lines = vec![line(0, 2), line(1, 3)];
        let c = expand_lines(&lines);
        assert_eq!(c.len(), 5);
        assert_eq!(c[0], CopyRef { part: 0, copy: 0 });
        assert_eq!(c[4], CopyRef { part: 1, copy: 2 });
    }

    #[test]
    fn grain_rotations_align_long_axes() {
        // Part long axis is local x (60 wide, 40 tall).
        let part = rect_part(60.0, 40.0);
        let base = NestConfig {
            sheet_grain: GrainDir::AlongLength,
            allowed_rotations: vec![],
            ..Default::default()
        };
        // Sheet grain along length (y): part along length -> long axis must be y
        // -> 90/270.
        assert_eq!(
            grain_rotations(&base, &part, GrainDir::AlongLength),
            vec![90.0, 270.0]
        );
        // Part grain along width -> long axis along x -> 0/180.
        assert_eq!(
            grain_rotations(&base, &part, GrainDir::AlongWidth),
            vec![0.0, 180.0]
        );
        // Sheet grain along width (x) + part along length -> 0/180.
        let cfg2 = NestConfig {
            sheet_grain: GrainDir::AlongWidth,
            allowed_rotations: vec![],
            ..Default::default()
        };
        assert_eq!(
            grain_rotations(&cfg2, &part, GrainDir::AlongLength),
            vec![0.0, 180.0]
        );
        // Any grain -> all rotations.
        assert_eq!(
            grain_rotations(&base, &part, GrainDir::Any),
            vec![0.0, 90.0, 180.0, 270.0]
        );
        // Explicit override wins.
        let cfg3 = NestConfig {
            allowed_rotations: vec![0.0, 180.0],
            ..Default::default()
        };
        assert_eq!(
            grain_rotations(&cfg3, &part, GrainDir::AlongLength),
            vec![0.0, 180.0]
        );
    }

    #[test]
    fn partial_keeps_placed_sheets() {
        // Two copies of a 60x40 part on a 100x100 stock: the oracle accepts a
        // single-item sheet only, so one copy is placed and one remains.
        let part = rect_part(60.0, 40.0);
        let parts = vec![part];
        let lines = vec![line(0, 2)];
        let cfg = NestConfig {
            stock_width: 100.0,
            stock_height: 100.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            chord_tol: 0.05,
            join_tol: 0.01,
            max_sheets: 1,
            ..Default::default()
        };
        let mapping = sheet::StockMapping::from_config(&cfg).unwrap();
        // The solver inlines the dilated outline (local min -m), so the nominal
        // part starts m past the separation to clear the edge margin.
        let m = cfg.gap_x.max(cfg.gap_y) / 2.0;
        let pos = mapping.separation + m;
        let oracle = |inst: &SolverInstance| {
            if inst.items.len() == 1 {
                Ok(solver::SolverSolution {
                    strip_width: 70.0,
                    placed: vec![solver::Placed {
                        item_id: inst.items[0].id,
                        reflected: false,
                        rotation_deg: 0.0,
                        translation: [pos, pos],
                    }],
                })
            } else {
                Err(solver::SolverError::Malformed("single only".into()))
            }
        };
        match run_batch(&lines, &parts, &cfg, oracle) {
            Ok(_) => panic!("expected Partial"),
            Err(BatchError::Partial {
                remaining,
                sheets,
                placed,
            }) => {
                assert_eq!(remaining, 1);
                assert_eq!(sheets, 1);
                assert_eq!(placed.len(), 1);
                assert_eq!(placed[0].layout.placements.len(), 1);
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn line_rotations_step_angle() {
        let part = rect_part(60.0, 40.0);
        let cfg = NestConfig {
            sheet_grain: GrainDir::Any,
            allowed_rotations: vec![],
            ..Default::default()
        };
        // step 90 -> 4 orientations; grain any -> all.
        let l90 = NestLine {
            part: 0,
            quantity: 1,
            allow_mirror: false,
            grain: GrainDir::Any,
            step_angle: 90.0,
            priority: 1,
            use_bounding_box: false,
            label: String::new(),
        };
        assert_eq!(
            line_rotations(&cfg, &part, &l90),
            vec![0.0, 90.0, 180.0, 270.0]
        );
        // step 45 -> 8 orientations.
        let mut l45 = l90.clone();
        l45.step_angle = 45.0;
        assert_eq!(line_rotations(&cfg, &part, &l45).len(), 8);
        // step 180 -> 2 orientations.
        let mut l180 = l90.clone();
        l180.step_angle = 180.0;
        assert_eq!(line_rotations(&cfg, &part, &l180), vec![0.0, 180.0]);
    }

    #[test]
    fn line_rotations_grain_filters_step() {
        // Part long axis = local x. Sheet grain along length (y) + part along
        // length => long axis must be y => only 90/270 survive a 45° step set.
        let part = rect_part(60.0, 40.0);
        let cfg = NestConfig {
            sheet_grain: GrainDir::AlongLength,
            allowed_rotations: vec![],
            ..Default::default()
        };
        let line = NestLine {
            part: 0,
            quantity: 1,
            allow_mirror: false,
            grain: GrainDir::AlongLength,
            step_angle: 45.0,
            priority: 1,
            use_bounding_box: false,
            label: String::new(),
        };
        assert_eq!(line_rotations(&cfg, &part, &line), vec![90.0, 270.0]);
    }

    /// An L: no mirror symmetry, so a mirrored copy shows in its outline's orientation.
    fn l_part() -> Part {
        let prim = P::Polyline {
            layer: "cut".into(),
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(60.0, 0.0),
                Point::new(60.0, 15.0),
                Point::new(15.0, 15.0),
                Point::new(15.0, 40.0),
                Point::new(0.0, 40.0),
            ],
            bulges: vec![None; 6],
            closed: true,
        };
        assemble(&[prim], &["cut".to_string()], &[], 0.05, 0.01)
            .unwrap()
            .parts
            .pop()
            .unwrap()
    }

    /// Signed area (shoelace): its sign is the outline's orientation.
    fn signed_area(pts: &[Point]) -> f64 {
        let n = pts.len();
        (0..n)
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                a.x * b.y - b.x * a.y
            })
            .sum::<f64>()
            / 2.0
    }

    /// A test oracle: turns every item by its first allowed rotation, flips the ones that allow it
    /// when `flip`, and lines the items up along the strip `sep` apart, `lift` above the strip's
    /// bottom clearance (a layout floating in the strip, as Sparrow can leave it).
    fn row_oracle(
        sep: f64,
        flip: bool,
        lift: f64,
    ) -> impl Fn(&SolverInstance) -> Result<SolverSolution, solver::SolverError> {
        move |inst: &SolverInstance| {
            let mut x = sep;
            let mut placed = Vec::new();
            for it in &inst.items {
                let reflected = flip && it.allow_reflection;
                let rot = it.rotations.first().copied().unwrap_or(0.0);
                let (s, c) = rot.to_radians().sin_cos();
                // The solver's order: reflect across x, then rotate, then translate.
                let turned: Vec<Point> = it
                    .polygon
                    .iter()
                    .map(|p| if reflected { Point::new(p.x, -p.y) } else { *p })
                    .map(|p| Point::new(c * p.x - s * p.y, s * p.x + c * p.y))
                    .collect();
                let min_x = turned.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
                let max_x = turned.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
                let min_y = turned.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
                placed.push(solver::Placed {
                    item_id: it.id,
                    reflected,
                    rotation_deg: rot,
                    translation: [x - min_x, sep + lift - min_y],
                });
                x += max_x - min_x + sep;
            }
            Ok(SolverSolution {
                strip_width: x,
                placed,
            })
        }
    }

    #[test]
    fn no_nesting_corner_mirrors_a_part() {
        // Lower-right and upper-left are reached by reflecting the finished layout. That alone
        // would mirror every part whatever its line allows. At every corner the copies must come
        // out unmirrored, turned by the allowed rotation (30° here, which the corner would
        // otherwise turn into 150°, 330° or 210°), and inside the stock.
        use crate::NestingCorner::{LowerLeft, LowerRight, UpperLeft, UpperRight};
        let parts = vec![l_part()];
        let want = signed_area(&parts[0].outer.points);
        let lines = vec![line(0, 3)];
        for corner in [LowerLeft, LowerRight, UpperLeft, UpperRight] {
            let cfg = NestConfig {
                stock_width: 400.0,
                stock_height: 300.0,
                edge_margin: 10.0,
                gap_x: 5.0,
                gap_y: 5.0,
                chord_tol: 0.05,
                join_tol: 0.01,
                max_sheets: 1,
                allowed_rotations: vec![30.0],
                nesting_corner: corner,
                ..Default::default()
            };
            let mapping = sheet::StockMapping::from_config(&cfg).unwrap();
            let oracle = row_oracle(mapping.separation, false, 0.0);
            let sheets =
                run_batch(&lines, &parts, &cfg, oracle).unwrap_or_else(|e| panic!("{corner:?}: {e}"));
            let placements = &sheets[0].layout.placements;
            assert_eq!(placements.len(), 3, "{corner:?}");
            for pl in placements {
                assert!(!pl.transform.reflected, "{corner:?}: a copy came out mirrored");
                let placed: Vec<Point> = parts[0]
                    .outer
                    .points
                    .iter()
                    .map(|p| pl.transform.apply(*p))
                    .collect();
                let got = signed_area(&placed);
                assert!(
                    (got - want).abs() < 1e-6 * want.abs(),
                    "{corner:?}: outline orientation {got} vs {want}"
                );
                let rot = pl.transform.rotation_deg.rem_euclid(360.0);
                assert!(
                    (rot - 30.0).abs() < 1e-6,
                    "{corner:?}: turned {rot}°, not 30°"
                );
            }
        }
    }

    #[test]
    fn a_reflecting_corner_still_lets_a_line_flip() {
        // A copy the solver flips (its line allows mirroring) stays flipped at a reflecting corner.
        let parts = vec![l_part()];
        let mut l = line(0, 1);
        l.allow_mirror = true;
        let cfg = NestConfig {
            stock_width: 400.0,
            stock_height: 300.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            chord_tol: 0.05,
            join_tol: 0.01,
            max_sheets: 1,
            nesting_corner: crate::NestingCorner::LowerRight,
            ..Default::default()
        };
        let mapping = sheet::StockMapping::from_config(&cfg).unwrap();
        let oracle = row_oracle(mapping.separation, true, 0.0);
        let sheets = run_batch(&[l], &parts, &cfg, oracle).unwrap();
        assert!(sheets[0].layout.placements[0].transform.reflected);
    }

    #[test]
    fn a_floating_layout_is_pressed_into_the_corner() {
        // The solver leaves the layout 100 mm up the strip. At every corner the parts must end up
        // as close to that corner's horizontal edge as to its fill-side edge (both are the
        // solver's own clearance from the strip edge), not 100 mm off it.
        use crate::NestingCorner::{LowerLeft, LowerRight, UpperLeft, UpperRight};
        let parts = vec![l_part()];
        let lines = vec![line(0, 3)];
        let (w, h) = (400.0, 300.0);
        for corner in [LowerLeft, LowerRight, UpperLeft, UpperRight] {
            let cfg = NestConfig {
                stock_width: w,
                stock_height: h,
                edge_margin: 10.0,
                gap_x: 5.0,
                gap_y: 5.0,
                chord_tol: 0.05,
                join_tol: 0.01,
                max_sheets: 1,
                nesting_corner: corner,
                ..Default::default()
            };
            let mapping = sheet::StockMapping::from_config(&cfg).unwrap();
            let oracle = row_oracle(mapping.separation, false, 100.0);
            let sheets =
                run_batch(&lines, &parts, &cfg, oracle).unwrap_or_else(|e| panic!("{corner:?}: {e}"));
            let pts: Vec<Point> = sheets[0]
                .layout
                .placements
                .iter()
                .flat_map(|pl| parts[0].outer.points.iter().map(move |p| pl.transform.apply(*p)))
                .collect();
            let (min_x, max_x) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                (a.min(p.x), b.max(p.x))
            });
            let (min_y, max_y) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                (a.min(p.y), b.max(p.y))
            });
            let (side_gap, edge_gap) = match corner {
                LowerLeft => (min_x, min_y),
                LowerRight => (w - max_x, min_y),
                UpperLeft => (min_x, h - max_y),
                UpperRight => (w - max_x, h - max_y),
            };
            assert!(
                (side_gap - edge_gap).abs() < 1e-6,
                "{corner:?}: {side_gap:.3} mm from the fill side, {edge_gap:.3} mm from the edge"
            );
        }
    }
}
