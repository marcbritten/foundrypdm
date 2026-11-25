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
