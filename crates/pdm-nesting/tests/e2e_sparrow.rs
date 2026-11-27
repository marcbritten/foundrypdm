//! End-to-end test against the real pinned Sparrow binary.
//!
//! Skips (rather than fails) when the binary is not available, so the unit-test
//! suite stays green in minimal environments. Set `SPARROW_BIN` to point at the
//! pinned `sparrow` executable to run it. It asserts feasibility, quantity, and
//! nominal dimension preservation — not an exact layout.

use pdm_nesting::NestConfig;
use pdm_nesting::batch;
use pdm_nesting::dxf::{self, Primitive};
use pdm_nesting::geom::Point;
use pdm_nesting::part::{self, Part};
use pdm_nesting::sheet;
use pdm_nesting::solver;

fn sparrow_bin() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("SPARROW_BIN")
        && !p.is_empty()
    {
        return Some(p.into());
    }
    // The pinned build produced during the compatibility spike, if present.
    let default = std::path::PathBuf::from("/tmp/opencode/sparrow-src/target/release/sparrow");
    default.is_file().then_some(default)
}

/// One Sparrow run of an instance (the pinned binary at `bin`), as the worker runs it.
fn run_sparrow(
    bin: &std::path::Path,
    inst: &solver::SolverInstance,
) -> Result<solver::SolverSolution, solver::SolverError> {
    let bad = |e: String| solver::SolverError::Malformed(e);
    let work = tempfile::tempdir().map_err(|e| bad(e.to_string()))?;
    std::fs::create_dir_all(work.path().join("output")).map_err(|e| bad(e.to_string()))?;
    let input = work.path().join("inst.json");
    std::fs::write(&input, inst.to_json()).map_err(|e| bad(e.to_string()))?;
    let out = std::process::Command::new(bin)
        .arg("-i")
        .arg(&input)
        .args(["-t", "20", "-s", "1", "--workers", "2"])
        .current_dir(work.path())
        .output()
        .map_err(|e| bad(e.to_string()))?;
    if !out.status.success() {
        return Err(bad(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    let text = std::fs::read_to_string(
        work.path()
            .join("output")
            .join(format!("final_{}.json", inst.name)),
    )
    .map_err(|e| bad(format!("no output: {e}")))?;
    solver::parse_solution(&text)
}

#[test]
fn e2e_sparrow_no_nesting_corner_mirrors_a_part() {
    // An L (no mirror symmetry) on a line that may not be mirrored, nested from the two corners
    // the layout is reflected to reach. Every copy must come out as drawn: unreflected, with its
    // outline's orientation (signed area) unchanged. Test values.
    let Some(bin) = sparrow_bin() else {
        eprintln!("SPARROW_BIN not found; skipping e2e sparrow test");
        return;
    };
    let outer = Primitive::Polyline {
        layer: "cut".into(),
        points: vec![
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(50.0, 12.0),
            Point::new(12.0, 12.0),
            Point::new(12.0, 44.0),
            Point::new(0.0, 44.0),
        ],
        bulges: vec![None; 6],
        closed: true,
    };
    let ell = part::assemble(&[outer], &["cut".to_string()], &[], 0.05, 0.01)
        .unwrap()
        .parts
        .pop()
        .unwrap();
    let signed_area = |pts: &[Point]| -> f64 {
        let n = pts.len();
        (0..n)
            .map(|i| pts[i].x * pts[(i + 1) % n].y - pts[(i + 1) % n].x * pts[i].y)
            .sum::<f64>()
            / 2.0
    };
    let drawn = signed_area(&ell.outer.points);
    let parts = vec![ell];
    let lines = vec![batch::NestLine {
        part: 0,
        quantity: 4,
        allow_mirror: false,
        grain: pdm_nesting::GrainDir::Any,
        step_angle: 90.0,
        priority: 1,
        use_bounding_box: false,
        label: String::new(),
    }];
    for corner in [
        pdm_nesting::NestingCorner::LowerRight,
        pdm_nesting::NestingCorner::UpperLeft,
    ] {
        let cfg = NestConfig {
            stock_width: 300.0,
            stock_height: 200.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            nesting_corner: corner,
            time_budget_sec: 60,
            attempt_limit_sec: 30,
            max_sheets: 1,
            ..Default::default()
        };
        let sheets = batch::run_batch(&lines, &parts, &cfg, |inst| run_sparrow(&bin, inst))
            .unwrap_or_else(|e| panic!("{corner:?}: {e}"));
        let placements = &sheets[0].layout.placements;
        assert_eq!(placements.len(), 4, "{corner:?}: all copies placed");
        let mirrored = placements
            .iter()
            .filter(|pl| {
                let placed: Vec<Point> = parts[0]
                    .outer
                    .points
                    .iter()
                    .map(|q| pl.transform.apply(*q))
                    .collect();
                pl.transform.reflected || signed_area(&placed) * drawn < 0.0
            })
            .count();
        eprintln!("{corner:?}: {} copies, {mirrored} mirrored", placements.len());
        assert_eq!(mirrored, 0, "{corner:?}: a copy of a no-mirror line came out mirrored");
    }
}

#[test]
fn e2e_sparrow_nests_and_exports() {
    let bin = match sparrow_bin() {
        Some(b) => b,
        None => {
            eprintln!("SPARROW_BIN not found; skipping e2e sparrow test");
            return;
        }
    };

    // Part A: 60x40 rectangle with a central r=8 hole.
    let outer_a = Primitive::Polyline {
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
    let hole_a = Primitive::Circle {
        layer: "cut".into(),
        center: Point::new(30.0, 20.0),
        radius: 8.0,
    };
    let a = part::assemble(&[outer_a, hole_a], &["cut".to_string()], &[], 0.05, 0.01)
        .unwrap()
        .parts
        .pop()
        .unwrap();
    assert_eq!(a.holes.len(), 1, "part A should have one hole");

    // Part B: an asymmetric L-shape.
    let outer_b = Primitive::Polyline {
        layer: "cut".into(),
        points: vec![
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(50.0, 12.0),
            Point::new(12.0, 12.0),
            Point::new(12.0, 44.0),
            Point::new(0.0, 44.0),
        ],
        bulges: vec![None; 6],
        closed: true,
    };
    let b = part::assemble(&[outer_b], &["cut".to_string()], &[], 0.05, 0.01)
        .unwrap()
        .parts
        .pop()
        .unwrap();

    let parts: Vec<Part> = vec![a, b];
    // A x2, B x3 -> 5 copies.
    let lines = vec![
        batch::NestLine {
            part: 0,
            quantity: 2,
            allow_mirror: false,
            grain: pdm_nesting::GrainDir::Any,
            step_angle: 90.0,
            priority: 1,
            use_bounding_box: false,
            label: String::new(),
        },
        batch::NestLine {
            part: 1,
            quantity: 3,
            allow_mirror: false,
            grain: pdm_nesting::GrainDir::Any,
            step_angle: 90.0,
            priority: 1,
            use_bounding_box: false,
            label: String::new(),
        },
    ];

    let cfg = NestConfig {
        stock_width: 300.0,
        stock_height: 200.0,
        edge_margin: 10.0,
        gap_x: 5.0,
        gap_y: 5.0,
        time_budget_sec: 60,
        attempt_limit_sec: 30,
        max_sheets: 4,
        max_copies: 50,
        ..Default::default()
    };
    cfg.validate().unwrap();
    let mapping = sheet::StockMapping::from_config(&cfg).unwrap();

    let copies = batch::expand_lines(&lines);
    assert_eq!(copies.len(), 5);

    // Single-sheet solve of the whole demand (fits: 5 small parts on 300x200).
    let inst = batch::build_instance(&lines, &parts, &copies, &cfg, &mapping, "e2e");
    let run =
        |inst: &solver::SolverInstance| -> Result<solver::SolverSolution, solver::SolverError> {
            let work =
                tempfile::tempdir().map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
            std::fs::create_dir_all(work.path().join("output"))
                .map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
            let input = work.path().join("inst.json");
            std::fs::write(&input, inst.to_json())
                .map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
            let out = std::process::Command::new(&bin)
                .arg("-i")
                .arg(&input)
                .arg("-t")
                .arg("20")
                .arg("-s")
                .arg("1")
                .arg("--workers")
                .arg("2")
                .current_dir(work.path())
                .output()
                .map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
            if !out.status.success() {
                return Err(solver::SolverError::Malformed(format!(
                    "sparrow exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            let json_path = work
                .path()
                .join("output")
                .join(format!("final_{}.json", inst.name));
            let text = std::fs::read_to_string(&json_path)
                .map_err(|e| solver::SolverError::Malformed(format!("no output: {e}")))?;
            let sol = solver::parse_solution(&text)?;
            // Keep the tempdir alive by leaking its path holder.
            let _ = json_path;
            std::mem::forget(work);
            Ok(sol)
        };

    let solution = run(&inst).expect("sparrow run");
    assert_eq!(solution.placed.len(), 5, "all 5 copies must be placed");

    let ids = sheet::assign_item_ids(&copies);
    let layout = sheet::build_sheet(&solution, &ids, &parts, &cfg, &mapping).expect("valid sheet");
    assert_eq!(layout.placements.len(), 5);

    // Export + re-parse: verify a known dimension survives (part A width = 60).
    let dxf_text = sheet::export_sheet_dxf(&layout, &parts, &[], &cfg);
    let parsed = dxf::parse(&dxf_text).expect("exported dxf parses");
    assert_eq!(parsed.units, dxf::Unit::Mm);
    // The stock outline is a 300x200 polyline.
    let stock = parsed
        .primitives
        .iter()
        .find(|p| matches!(p, Primitive::Polyline { layer, closed, .. } if layer == "STOCK" && *closed))
        .expect("stock outline present");
    if let Primitive::Polyline { points, .. } = stock {
        let bb = pointset_bbox(points);
        assert!(
            (bb.0 - 300.0).abs() < 1e-3 && (bb.1 - 200.0).abs() < 1e-3,
            "stock dims"
        );
    }
    // Count circles: part A x2 -> 2 holes -> 2 CIRCLE primitives.
    let circles = parsed
        .primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Circle { .. }))
        .count();
    assert_eq!(circles, 2, "two hole circles preserved");
    // Verify a hole radius is ~8mm.
    if let Some(Primitive::Circle { radius, .. }) = parsed
        .primitives
        .iter()
        .find(|p| matches!(p, Primitive::Circle { .. }))
    {
        assert!(
            (radius - 8.0).abs() < 0.1,
            "hole radius preserved, got {radius}"
        );
    }
}

/// One run of the pinned Sparrow on an instance.
fn solve(
    bin: &std::path::Path,
    inst: &solver::SolverInstance,
) -> Result<solver::SolverSolution, solver::SolverError> {
    let work = tempfile::tempdir().map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
    std::fs::create_dir_all(work.path().join("output"))
        .map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
    let input = work.path().join("inst.json");
    std::fs::write(&input, inst.to_json())
        .map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
    let out = std::process::Command::new(bin)
        .arg("-i")
        .arg(&input)
        .args(["-t", "20", "-s", "1", "--workers", "2"])
        .current_dir(work.path())
        .output()
        .map_err(|e| solver::SolverError::Malformed(e.to_string()))?;
    if !out.status.success() {
        return Err(solver::SolverError::Malformed(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let text = std::fs::read_to_string(
        work.path()
            .join("output")
            .join(format!("final_{}.json", inst.name)),
    )
    .map_err(|e| solver::SolverError::Malformed(format!("no output: {e}")))?;
    solver::parse_solution(&text)
}

fn polyline_part(points: Vec<Point>, holes: Vec<Primitive>) -> Part {
    let n = points.len();
    let mut prims = vec![Primitive::Polyline {
        layer: "cut".into(),
        points,
        bulges: vec![None; n],
        closed: true,
    }];
    prims.extend(holes);
    part::assemble(&prims, &["cut".to_string()], &[], 0.05, 0.01)
        .unwrap()
        .parts
        .pop()
        .unwrap()
}

fn seg_dist(p1: Point, p2: Point, q: Point) -> f64 {
    let (dx, dy) = (p2.x - p1.x, p2.y - p1.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-18 {
        0.0
    } else {
        (((q.x - p1.x) * dx + (q.y - p1.y) * dy) / l2).clamp(0.0, 1.0)
    };
    (q.x - p1.x - t * dx).hypot(q.y - p1.y - t * dy)
}

/// The smallest distance between the nominal outlines of any two placed parts (vertex to edge
/// both ways; the rings don't cross in a valid layout).
fn min_part_distance(layout: &sheet::SheetLayout, parts: &[Part]) -> f64 {
    let rings: Vec<Vec<Point>> = layout
        .placements
        .iter()
        .map(|pl| {
            parts[pl.part_index]
                .outer
                .points
                .iter()
                .map(|p| pl.transform.apply(*p))
                .collect()
        })
        .collect();
    let mut best = f64::INFINITY;
    for i in 0..rings.len() {
        for j in 0..rings.len() {
            if i == j {
                continue;
            }
            let (a, b) = (&rings[i], &rings[j]);
            for k in 0..a.len() {
                let (p1, p2) = (a[k], a[(k + 1) % a.len()]);
                for &q in b {
                    best = best.min(seg_dist(p1, p2, q));
                }
            }
        }
    }
    best
}

/// The real solver free to turn every part and to flip the L (it may or may not use the flip),
/// with a slotted part whose slot is narrower than the spacing (its solver outline comes from
/// the convex-hull fallback): every pair of parts stays at least the spacing apart, and the
/// sheet check accepts the solver's layout. Flipped placements are covered deterministically by
/// `sheet::tests::build_sheet_checks_the_spacing_of_flipped_parts`.
#[test]
fn e2e_sparrow_keeps_the_spacing_with_turning_and_flipping_allowed() {
    let Some(bin) = sparrow_bin() else {
        eprintln!("SPARROW_BIN not found; skipping e2e sparrow spacing test");
        return;
    };
    let rect = polyline_part(
        vec![
            Point::new(0.0, 0.0),
            Point::new(60.0, 0.0),
            Point::new(60.0, 40.0),
            Point::new(0.0, 40.0),
        ],
        vec![Primitive::Circle {
            layer: "cut".into(),
            center: Point::new(30.0, 20.0),
            radius: 8.0,
        }],
    );
    let ell = polyline_part(
        vec![
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(50.0, 12.0),
            Point::new(12.0, 12.0),
            Point::new(12.0, 44.0),
            Point::new(0.0, 44.0),
        ],
        vec![],
    );
    // A 2 mm slot: narrower than the spacing below.
    let slotted = polyline_part(
        vec![
            Point::new(0.0, 0.0),
            Point::new(40.0, 0.0),
            Point::new(40.0, 20.0),
            Point::new(21.0, 20.0),
            Point::new(21.0, 8.0),
            Point::new(19.0, 8.0),
            Point::new(19.0, 20.0),
            Point::new(0.0, 20.0),
        ],
        vec![],
    );
    let parts = vec![rect, ell, slotted];
    let line = |part: usize, quantity: u64, allow_mirror: bool, step_angle: f64, use_bounding_box: bool| {
        batch::NestLine {
            part,
            quantity,
            allow_mirror,
            grain: pdm_nesting::GrainDir::Any,
            step_angle,
            priority: 1,
            use_bounding_box,
            label: String::new(),
        }
    };
    // The rectangle nests by its bounding box; the L turns in 45° steps and may flip.
    let lines = vec![
        line(0, 3, false, 90.0, true),
        line(1, 4, true, 45.0, false),
        line(2, 4, false, 90.0, false),
    ];
    // Test values: a 6.35 mm gap, filled from the upper-right corner with an offset origin.
    let cfg = NestConfig {
        stock_width: 500.0,
        stock_height: 300.0,
        edge_margin: 10.0,
        gap_x: 6.35,
        gap_y: 6.35,
        nesting_corner: pdm_nesting::NestingCorner::UpperRight,
        origin: Point::new(5.0, 3.0),
        allowed_rotations: vec![],
        time_budget_sec: 60,
        attempt_limit_sec: 20,
        max_sheets: 1,
        ..Default::default()
    };
    cfg.validate().unwrap();
    let mapping = sheet::StockMapping::from_config(&cfg).unwrap();
    let copies = batch::expand_lines(&lines);
    let inst = batch::build_instance(&lines, &parts, &copies, &cfg, &mapping, "spacing");
    let solution = solve(&bin, &inst).expect("sparrow run");
    assert_eq!(solution.placed.len(), copies.len(), "every copy placed");
    let ids = sheet::assign_item_ids(&copies);
    let layout = sheet::build_sheet(&solution, &ids, &parts, &cfg, &mapping)
        .expect("the solver's layout keeps the spacing");
    let d = min_part_distance(&layout, &parts);
    eprintln!(
        "closest parts: {d:.4} mm apart (spacing 6.35); turned {}, flipped {}",
        layout
            .placements
            .iter()
            .filter(|p| p.transform.rotation_deg.rem_euclid(360.0) > 1e-6)
            .count(),
        layout.placements.iter().filter(|p| p.transform.reflected).count()
    );
    assert!(d >= 6.35 - 1e-3, "parts {d} mm apart");
    // The sheet DXF leads with the gap the check proved: never more than the parts' real
    // distance, and short of the spacing only by the check's allowances.
    let gap = layout.checked_gap_mm.expect("a checked layout records its gap");
    let dxf_text = sheet::export_sheet_dxf(&layout, &parts, &[], &cfg);
    eprintln!("recorded gap: {gap:.3} mm; {}", dxf_text.lines().nth(1).unwrap_or(""));
    assert!(
        dxf_text.starts_with(&format!("999
{} part_gap_mm={gap:.3} ", sheet::NEST_RECORD_PREFIX)),
        "{}",
        &dxf_text[..dxf_text.len().min(160)]
    );
    assert!(gap <= d + 1e-9 && gap >= 6.35 - 0.2, "recorded {gap} mm, parts {d} mm apart");
}

fn pointset_bbox(pts: &[Point]) -> (f64, f64) {
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

#[test]
fn e2e_sparrow_the_layout_sits_in_the_nesting_corner() {
    // Four L shapes that fill neither the sheet's length nor its height, nested from each corner.
    // The layout must sit in the chosen corner on both axes: as far from that corner's horizontal
    // edge as from its fill-side edge, not floating up the strip. Test values.
    use pdm_nesting::NestingCorner::{LowerLeft, LowerRight, UpperLeft, UpperRight};
    let Some(bin) = sparrow_bin() else {
        eprintln!("SPARROW_BIN not found; skipping e2e sparrow test");
        return;
    };
    let outer = Primitive::Polyline {
        layer: "cut".into(),
        points: vec![
            Point::new(0.0, 0.0),
            Point::new(240.0, 0.0),
            Point::new(240.0, 60.0),
            Point::new(60.0, 60.0),
            Point::new(60.0, 160.0),
            Point::new(0.0, 160.0),
        ],
        bulges: vec![None; 6],
        closed: true,
    };
    let ell = part::assemble(&[outer], &["cut".to_string()], &[], 0.05, 0.01)
        .unwrap()
        .parts
        .pop()
        .unwrap();
    let parts = vec![ell];
    let line = |part: usize, quantity: u64| batch::NestLine {
        part,
        quantity,
        allow_mirror: false,
        grain: pdm_nesting::GrainDir::Any,
        step_angle: 90.0,
        priority: 1,
        use_bounding_box: false,
        label: String::new(),
    };
    let lines = vec![line(0, 4)];
    let (w, h) = (1600.0, 1000.0);
    let mut off = Vec::new();
    for corner in [LowerLeft, LowerRight, UpperLeft, UpperRight] {
        let cfg = NestConfig {
            stock_width: w,
            stock_height: h,
            edge_margin: 10.0,
            gap_x: 6.35,
            gap_y: 6.35,
            nesting_corner: corner,
            time_budget_sec: 60,
            attempt_limit_sec: 30,
            max_sheets: 1,
            ..Default::default()
        };
        let sheets = batch::run_batch(&lines, &parts, &cfg, |inst| run_sparrow(&bin, inst))
            .unwrap_or_else(|e| panic!("{corner:?}: {e}"));
        let layout = &sheets[0].layout;
        assert_eq!(layout.placements.len(), 4, "{corner:?}: all copies placed");
        let pts: Vec<Point> = layout
            .placements
            .iter()
            .flat_map(|pl| {
                parts[pl.part_index]
                    .outer
                    .points
                    .iter()
                    .map(move |p| pl.transform.apply(*p))
            })
            .collect();
        let (min_x, max_x) = pts
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.x), b.max(p.x)));
        let (min_y, max_y) = pts
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.y), b.max(p.y)));
        let (side_gap, edge_gap) = match corner {
            LowerLeft => (min_x, min_y),
            LowerRight => (w - max_x, min_y),
            UpperLeft => (min_x, h - max_y),
            UpperRight => (w - max_x, h - max_y),
        };
        eprintln!(
            "{corner:?}: {side_gap:.2} mm from the fill side, {edge_gap:.2} mm from the corner's edge"
        );
        if (side_gap - edge_gap).abs() > 0.5 {
            off.push(format!("{corner:?}: {side_gap:.2} vs {edge_gap:.2} mm"));
        }
    }
    assert!(off.is_empty(), "the layout floats off the corner's edge: {off:?}");
}
