//! Laser nesting: DXF ingestion, Sparrow solver I/O, placement transforms, sheet export.
//!
//! This crate is pure (no async, no I/O beyond JSON strings and in-memory
//! geometry) so it can be unit-tested and run inside `spawn_blocking`. The
//! worker crate owns the Sparrow subprocess and blob storage; it calls into the
//! functions here to build solver inputs, validate solutions, and render sheet
//! DXF.
//!
//! See `docs/sparrow-compatibility.md` for the pinned upstream and the verified
//! Sparrow I/O contract, and `docs/sparrow-laser-implementation-plan.md` for the
//! overall integration plan.

pub mod batch;
pub mod dxf;
pub mod geom;
pub mod part;
pub mod sheet;
pub mod solver;

use thiserror::Error;

/// Top-level error for the nesting pipeline.
#[derive(Debug, Error)]
pub enum NestError {
    #[error("dxf: {0}")]
    Dxf(#[from] dxf::DxfError),
    #[error("solver: {0}")]
    Solver(#[from] solver::SolverError),
    #[error("sheet: {0}")]
    Sheet(#[from] sheet::SheetError),
    #[error("batch: {0}")]
    Batch(#[from] batch::BatchError),
    #[error("part: {0}")]
    Part(String),
}

/// Grain direction relative to a part or sheet's long axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrainDir {
    /// Grain runs along the long axis (the default intent for sheet metal).
    #[default]
    AlongLength,
    /// Grain runs across the long axis.
    AlongWidth,
    /// No grain preference; any rotation is allowed.
    Any,
}

/// The corner of the stock from which the layout fill starts. The solver packs
/// from the lower-left; other corners are realised by a whole-layout flip,
/// which preserves relative grain alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NestingCorner {
    #[default]
    LowerLeft,
    LowerRight,
    UpperLeft,
    UpperRight,
}

/// Nesting configuration for a single run.
///
/// The stock rectangle is `stock_width` (x) by `stock_height` (y); the PDM API
/// presents these as the sheet's *width* and *length* respectively, so the
/// length axis is vertical in layouts and previews.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestConfig {
    /// Stock sheet width in mm.
    pub stock_width: f64,
    /// Stock sheet length (height) in mm.
    pub stock_height: f64,
    /// Minimum distance from any part's nominal boundary to a stock edge.
    pub edge_margin: f64,
    /// Minimum horizontal spacing between parts (x-axis clearance).
    pub gap_x: f64,
    /// Minimum vertical spacing between parts (y-axis clearance).
    pub gap_y: f64,
    /// The sheet's grain direction (drives per-part rotation restrictions).
    #[serde(default)]
    pub sheet_grain: GrainDir,
    /// Which corner the fill starts from (default lower-left).
    #[serde(default)]
    pub nesting_corner: NestingCorner,
    /// Nesting origin offset from the stock lower-left corner (mm).
    #[serde(default)]
    pub origin: crate::geom::Point,
    /// Draw part labels (P1, P2, …) on the exported DXF + preview.
    #[serde(default)]
    pub add_part_labels: bool,
    /// Max chord deviation (mm) when tessellating arcs for the solver outline.
    #[serde(default = "default_chord")]
    pub chord_tol: f64,
    /// Endpoint-join tolerance (mm) when chaining open entities.
    #[serde(default = "default_join")]
    pub join_tol: f64,
    /// Total wall-clock budget (seconds) for the whole batch.
    #[serde(default = "default_budget")]
    pub time_budget_sec: u32,
    /// Per-attempt solver time limit (seconds).
    #[serde(default = "default_attempt")]
    pub attempt_limit_sec: u32,
    /// Maximum number of sheets to produce.
    #[serde(default = "default_max_sheets")]
    pub max_sheets: u32,
    /// Maximum total copies across all parts.
    #[serde(default = "default_max_copies")]
    pub max_copies: u32,
    /// Allowed rotations (degrees, CCW) for every part in this run.
    #[serde(default = "default_rotations")]
    pub allowed_rotations: Vec<f64>,
    /// Maximum solver attempts per sheet during greedy packing.
    #[serde(default = "default_attempts")]
    pub max_attempts_per_sheet: u32,
}

fn default_chord() -> f64 {
    part::DEFAULT_CHORD_TOL
}
fn default_join() -> f64 {
    part::DEFAULT_JOIN_TOL
}
fn default_budget() -> u32 {
    600
}
fn default_attempt() -> u32 {
    60
}
fn default_max_sheets() -> u32 {
    16
}
fn default_max_copies() -> u32 {
    400
}
fn default_rotations() -> Vec<f64> {
    vec![0.0, 90.0, 180.0, 270.0]
}
fn default_attempts() -> u32 {
    60
}

impl Default for NestConfig {
    fn default() -> Self {
        Self {
            stock_width: 1000.0,
            stock_height: 2000.0,
            edge_margin: 10.0,
            gap_x: 5.0,
            gap_y: 5.0,
            sheet_grain: GrainDir::AlongLength,
            nesting_corner: NestingCorner::LowerLeft,
            origin: crate::geom::Point::new(0.0, 0.0),
            add_part_labels: false,
            chord_tol: part::DEFAULT_CHORD_TOL,
            join_tol: part::DEFAULT_JOIN_TOL,
            time_budget_sec: default_budget(),
            attempt_limit_sec: default_attempt(),
            max_sheets: default_max_sheets(),
            max_copies: default_max_copies(),
            allowed_rotations: default_rotations(),
            max_attempts_per_sheet: default_attempts(),
        }
    }
}

impl NestConfig {
    /// Validate the configuration for internal consistency.
    pub fn validate(&self) -> Result<(), NestError> {
        use NestError::Part as E;
        if !(self.stock_width.is_finite() && self.stock_width > 0.0)
            || !(self.stock_height.is_finite() && self.stock_height > 0.0)
        {
            return Err(E("stock dimensions must be finite and positive".into()));
        }
        if !(self.edge_margin.is_finite() && self.edge_margin >= 0.0) {
            return Err(E("edge margin must be finite and >= 0".into()));
        }
        if !(self.gap_x.is_finite() && self.gap_x >= 0.0)
            || !(self.gap_y.is_finite() && self.gap_y >= 0.0)
        {
            return Err(E("part spacing must be finite and >= 0".into()));
        }
        if !(self.origin.x.is_finite() && self.origin.y.is_finite()) {
            return Err(E("nesting origin must be finite".into()));
        }
        if self.edge_margin * 2.0 + self.gap_x.max(self.gap_y) >= self.stock_height
            || self.edge_margin * 2.0 + self.gap_x.max(self.gap_y) >= self.stock_width
        {
            return Err(E(
                "stock too small for the requested margins and spacing".into()
            ));
        }
        Ok(())
    }
}
