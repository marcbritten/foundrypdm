//! A SolidCraft flat panel through the router path, as the worker runs it with
//! `PDM_ROUTER_LAYERS` on: `sheet.export_dxf`'s file → router layers (`cnc_layers`) → auto
//! layers → assemble, keeping layers → the pinned Sparrow → sheet DXF.
//!
//! The panel is `tests/fixtures/solidcraft-flat-panel.dxf`: a 19 mm panel with an outline, a
//! through hole and one blind round pocket 6.35 mm deep, exported by `solidcraft-cli`
//! (`sheet.export_dxf`). `PDM_FLAT_DXF` names another export instead (with `PDM_POCKET_DEPTH`,
//! default 6.35, its pocket's depth). The Sparrow test skips unless `SPARROW_BIN` names the pinned
//! `sparrow`; `PDM_SHEET_DXF_OUT` (optional) is where it writes the sheet DXF.
//!
//! The panel is nested at the lower-left corner and again at the lower-right, which is reached
//! by reflecting the layout and must still not mirror it.
//!
//! The sheet size, margin and spacing are test values.

use pdm_nesting::batch::{self, NestLine};
use pdm_nesting::dxf::{self, Primitive};
use pdm_nesting::{GrainDir, NestConfig, NestingCorner, cnc_layers, part, sheet};

fn sparrow(
    bin: &std::path::Path,
) -> impl Fn(
    &pdm_nesting::solver::SolverInstance,
) -> Result<pdm_nesting::solver::SolverSolution, pdm_nesting::solver::SolverError>
+ '_ {
    use pdm_nesting::solver::{self, SolverError};
    move |inst| {
        let work = tempfile::tempdir().map_err(|e| SolverError::Malformed(e.to_string()))?;
        std::fs::create_dir_all(work.path().join("output"))
            .map_err(|e| SolverError::Malformed(e.to_string()))?;
        let input = work.path().join("inst.json");
        std::fs::write(&input, inst.to_json())
            .map_err(|e| SolverError::Malformed(e.to_string()))?;
        let out = std::process::Command::new(bin)
            .args(["-i"])
            .arg(&input)
            .args(["-t", "20", "-s", "1", "--workers", "2"])
            .current_dir(work.path())
            .output()
            .map_err(|e| SolverError::Malformed(e.to_string()))?;
        if !out.status.success() {
            return Err(SolverError::Malformed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        let text = std::fs::read_to_string(
            work.path()
                .join("output")
                .join(format!("final_{}.json", inst.name)),
        )
        .map_err(|e| SolverError::Malformed(format!("no output: {e}")))?;
        solver::parse_solution(&text)
    }
}

/// The flat pattern (the fixture, or `PDM_FLAT_DXF`) and its pocket's depth.
fn flat_panel() -> (String, f64) {
    let (path, depth) = match std::env::var_os("PDM_FLAT_DXF") {
        Some(p) => (
            std::path::PathBuf::from(p),
            std::env::var("PDM_POCKET_DEPTH")
                .ok()
                .and_then(|d| d.parse().ok())
                .unwrap_or(6.35),
        ),
        None => (
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/solidcraft-flat-panel.dxf"),
            6.35,
        ),
    };
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("POCKET_TOP_Z"), "the export has a pocket layer");
    (raw, depth)
}

/// The flat-pattern job, then the nest job's part, as `run_laser_nest` assembles a generated flat
/// pattern with router layers on ("mm", layer_cut auto).
fn router_part(raw: &str) -> part::Part {
    let relayered = cnc_layers::relayer_solidcraft_flat(raw).unwrap();
    let d = dxf::parse(&relayered).unwrap();
    let prims = dxf::to_mm(&d, "mm", "mm");
    let (cuts, marks) = part::auto_layers(&d, &[]);
    let mut assembled = part::assemble_keeping_layers(&prims, &cuts, &marks, 0.05, 0.01).unwrap();
    assert_eq!(assembled.parts.len(), 1, "one outer island");
    assembled.parts.pop().unwrap()
}

#[test]
fn a_solidcraft_flat_pattern_takes_router_layers() {
    // No solver needed: the export's layers become router layers, and the part keeps them.
    let (raw, depth) = flat_panel();
    let p = router_part(&raw);
    assert_eq!(p.outer.layer, "OUTLINE");
    let mut holes: Vec<&str> = p.holes.iter().map(|h| h.layer.as_str()).collect();
    holes.sort();
    holes.dedup();
    let want_depth = cnc_layers::depth_layer(depth);
    let mut want = vec!["THROUGH", want_depth.as_str()];
    want.sort();
    assert_eq!(holes, want);
    assert!(p.has_blind_features());
}

#[test]
fn a_solidcraft_panel_nests_with_its_router_layers() {
    let Some(bin) = std::env::var_os("SPARROW_BIN") else {
        eprintln!("skipped: set SPARROW_BIN");
        return;
    };
    let (raw, depth) = flat_panel();
    let p = router_part(&raw);
    assert!(p.has_blind_features());
    let lines = [NestLine {
        part: 0,
        quantity: 1,
        allow_mirror: !p.has_blind_features(),
        grain: GrainDir::Any,
        step_angle: 90.0,
        priority: 1,
        use_bounding_box: false,
        label: String::new(),
    }];
    let parts = vec![p];
    let drawn = signed_area(&parts[0].outer.points);
    // Lower-left, and lower-right: a corner reached by reflecting the layout, which must not
    // mirror the panel (its pocket would land on the bottom face).
    for corner in [NestingCorner::LowerLeft, NestingCorner::LowerRight] {
        let cfg = NestConfig {
            stock_width: 2438.4,
            stock_height: 1219.2,
            edge_margin: 10.0,
            gap_x: 10.0,
            gap_y: 10.0,
            sheet_grain: GrainDir::Any,
            nesting_corner: corner,
            time_budget_sec: 120,
            attempt_limit_sec: 30,
            max_sheets: 1,
            ..Default::default()
        };
        let sheets =
            batch::run_batch(&lines, &parts, &cfg, sparrow(std::path::Path::new(&bin))).unwrap();
        assert_eq!(sheets.len(), 1);
        for pl in &sheets[0].layout.placements {
            assert!(!pl.transform.reflected, "{corner:?}: never mirrored");
            let placed: Vec<_> = parts[0]
                .outer
                .points
                .iter()
                .map(|q| pl.transform.apply(*q))
                .collect();
            assert!(
                signed_area(&placed) * drawn > 0.0,
                "{corner:?}: the outline keeps its orientation"
            );
        }
        let out = sheet::export_sheet_dxf(&sheets[0].layout, &parts, &[], &cfg);
        let out_path =
            std::env::var_os("PDM_SHEET_DXF_OUT").filter(|_| corner == NestingCorner::LowerLeft);
        if let Some(path) = out_path {
            std::fs::write(path, &out).unwrap();
        }
        check_router_layers(&out, depth);
    }
}

/// Signed area (shoelace): its sign is the outline's orientation.
fn signed_area(pts: &[pdm_nesting::geom::Point]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        / 2.0
}

/// The sheet keeps the three router layers, with the model's depth.
fn check_router_layers(out: &str, depth: f64) {
    let s = dxf::parse(out).unwrap();
    let mut layers: Vec<String> = s.primitives.iter().map(|p| p.layer()).collect();
    layers.sort();
    layers.dedup();
    let want_depth = cnc_layers::depth_layer(depth);
    let mut want = vec![
        "OUTLINE".to_string(),
        "STOCK".into(),
        "THROUGH".into(),
        want_depth.clone(),
    ];
    want.sort();
    assert_eq!(layers, want, "{out}");
    let circles = |layer: &str| -> Vec<f64> {
        s.primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::Circle {
                    layer: l, radius, ..
                } if l == layer => Some(*radius),
                _ => None,
            })
            .collect()
    };
    assert_eq!(circles(&want_depth).len(), 1, "one blind round pocket");
    assert!(!circles("THROUGH").is_empty(), "a through hole");
    eprintln!(
        "sheet: layers {layers:?}, pocket r {:?}, holes r {:?}",
        circles(&want_depth),
        circles("THROUGH")
    );
}
