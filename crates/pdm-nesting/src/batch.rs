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
    let items: Vec<SolverItem> = copies
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let line = &lines[c.part];
            let part = &parts[line.part];
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
                rotations: line_rotations(cfg, part, line),
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
        sheet::build_sheet(&sol, &ids, parts, cfg, &mapping)
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
}
