//! A minimal, explicit DXF reader for laser nesting.
//!
//! DXF is a stream of `(group_code, value)` pairs. This reader supports the
//! geometry a flat-pattern export needs: `LINE`, `CIRCLE`, `ARC`, `LWPOLYLINE`,
//! `POINT`, and simple `INSERT` (block expansion with position/rotation/uniform
//! scale). Entities that can carry cut geometry but are not supported yet
//! (`SPLINE`, `ELLIPSE`, open 2D `POLYLINE`) are rejected with a specific,
//! actionable error; known non-geometry entities are ignored. Layer names come
//! from group 8. Units come from the `$INSUNITS` header variable.

use crate::geom::Point;
use std::collections::HashMap;
use thiserror::Error;

/// The physical units the source DXF was drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    /// Millimetres.
    Mm,
    /// Inches.
    Inch,
    /// The file declared no units; a caller must supply an explicit override.
    Unknown,
}

impl Unit {
    /// Convert a file length to millimetres.
    pub fn to_mm(self, value: f64) -> f64 {
        match self {
            Unit::Mm => value,
            Unit::Inch => value * 25.4,
            Unit::Unknown => value,
        }
    }

    fn from_insunits(code: i32) -> Option<Self> {
        // $INSUNITS group-70 values: 1..9 metric, 10..21 imperial, 22 nanometres, ...
        Some(match code {
            1..=9 => Unit::Mm,
            10..=21 => Unit::Inch,
            _ => Unit::Unknown,
        })
    }
}

/// A planar curve primitive, in the file's native units.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    /// A straight segment (open).
    Line {
        layer: String,
        start: Point,
        end: Point,
    },
    /// A full circle (closed).
    Circle {
        layer: String,
        center: Point,
        radius: f64,
    },
    /// A circular arc (open). Angles in degrees, counter-clockwise.
    Arc {
        layer: String,
        center: Point,
        radius: f64,
        start_deg: f64,
        end_deg: f64,
    },
    /// A polyline (LWPOLYLINE). `bulges` is per-vertex; `None` = straight edge.
    Polyline {
        layer: String,
        points: Vec<Point>,
        bulges: Vec<Option<f64>>,
        closed: bool,
    },
    /// A zero-length point (mark candidate).
    Point { layer: String, pos: Point },
}

/// Convert a parsed DXF to millimetres.
///
/// Unit precedence:
/// 1. `override_unit` — an explicit per-part unit ("mm" / "in" / "inch");
/// 2. the file's `$INSUNITS`, when the override is "unknown" or absent;
/// 3. `default_unit` — the nest-level default, when the file declares no units.
///
/// A file that declares no units and has no default is treated as millimetres.
pub fn to_mm(d: &Dxf, override_unit: &str, default_unit: &str) -> Vec<Primitive> {
    let effective = match parse_unit(override_unit) {
        Some(u) => u,
        None => match d.units {
            Unit::Unknown => parse_unit(default_unit).unwrap_or(Unit::Mm),
            other => other,
        },
    };
    let factor = if effective == Unit::Inch { 25.4 } else { 1.0 };
    if factor == 1.0 {
        return d.primitives.clone();
    }
    d.primitives
        .iter()
        .map(|p| scale_primitive(p, factor))
        .collect()
}

/// Parse a user-facing unit string. "unknown"/"" -> None (defer to the file).
fn parse_unit(s: &str) -> Option<Unit> {
    match s.trim().to_ascii_lowercase().as_str() {
        "mm" | "metric" => Some(Unit::Mm),
        "in" | "inch" | "inches" | "imperial" => Some(Unit::Inch),
        _ => None,
    }
}

/// Scale all coordinates (and radii) of a primitive by `f`.
fn scale_primitive(p: &Primitive, f: f64) -> Primitive {
    let spt = |pt: &Point| Point::new(pt.x * f, pt.y * f);
    match p {
        Primitive::Line { layer, start, end } => Primitive::Line {
            layer: layer.clone(),
            start: spt(start),
            end: spt(end),
        },
        Primitive::Circle {
            layer,
            center,
            radius,
        } => Primitive::Circle {
            layer: layer.clone(),
            center: spt(center),
            radius: radius * f,
        },
        Primitive::Arc {
            layer,
            center,
            radius,
            start_deg,
            end_deg,
        } => Primitive::Arc {
            layer: layer.clone(),
            center: spt(center),
            radius: radius * f,
            start_deg: *start_deg,
            end_deg: *end_deg,
        },
        Primitive::Polyline {
            layer,
            points,
            bulges,
            closed,
        } => Primitive::Polyline {
            layer: layer.clone(),
            points: points.iter().map(spt).collect(),
            bulges: bulges.clone(),
            closed: *closed,
        },
        Primitive::Point { layer, pos } => Primitive::Point {
            layer: layer.clone(),
            pos: spt(pos),
        },
    }
}

impl Primitive {
    /// The layer this primitive lives on.
    pub fn layer(&self) -> String {
        match self {
            Self::Line { layer, .. }
            | Self::Circle { layer, .. }
            | Self::Arc { layer, .. }
            | Self::Polyline { layer, .. }
            | Self::Point { layer, .. } => layer.clone(),
        }
    }
}

/// A parsed DXF document.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Dxf {
    pub units: Unit,
    pub layers: Vec<String>,
    pub primitives: Vec<Primitive>,
}

#[derive(Debug, Error, PartialEq)]
pub enum DxfError {
    #[error("bad group code at line {0}: not an integer")]
    BadGroupCode(usize),
    #[error("unexpected end of file")]
    Eof,
    #[error("unsupported cut entity '{kind}' on layer '{layer}'")]
    UnsupportedEntity { kind: String, layer: String },
    #[error("polyline on layer '{layer}' has fewer than 2 vertices")]
    PolylineTooShort { layer: String },
    #[error("arc on layer '{layer}' has zero or negative radius")]
    BadArcRadius { layer: String },
    #[error("circle on layer '{layer}' has zero or negative radius")]
    BadCircleRadius { layer: String },
    #[error("block '{block}' referenced by INSERT not defined")]
    MissingBlock { block: String },
    #[error("INSERT recursion limit reached at block '{block}'")]
    InsertRecursion { block: String },
    #[error("non-finite coordinate in entity on layer '{layer}'")]
    NonFinite { layer: String },
}

type Result<T> = std::result::Result<T, DxfError>;

const INSERT_DEPTH_LIMIT: usize = 16;

#[derive(Debug, Clone, Default)]
struct Raw {
    kind: String,
    layer: String,
    block: Option<String>,
    nums: HashMap<i32, f64>,
    // LWPOLYLINE vertices in order as (x, y, bulge); bulge is per-vertex.
    verts: Vec<(f64, f64, Option<f64>)>,
}

impl Raw {
    fn n(&self, c: i32) -> Option<f64> {
        self.nums.get(&c).copied()
    }
    fn n_def(&self, c: i32, d: f64) -> f64 {
        self.n(c).unwrap_or(d)
    }
}

/// Parse a DXF document from text.
pub fn parse(input: &str) -> Result<Dxf> {
    let lines: Vec<&str> = input.lines().collect();
    let mut pairs: Vec<(i32, String)> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let code_str = lines[i].trim();
        if code_str.is_empty() {
            i += 1;
            continue;
        }
        let code: i32 = code_str
            .parse()
            .map_err(|_| DxfError::BadGroupCode(i + 1))?;
        i += 1;
        let value = if i < lines.len() {
            let v = lines[i].trim();
            i += 1;
            v.to_string()
        } else {
            return Err(DxfError::Eof);
        };
        pairs.push((code, value));
    }

    // Walk the pairs: track the current section, header variable name, current
    // block, and accumulate the current raw entity until the next group 0.
    let mut units = Unit::Unknown;
    let mut layers: Vec<String> = Vec::new();
    let mut blocks: HashMap<String, Vec<Raw>> = HashMap::new();
    let mut top: Vec<Raw> = Vec::new();

    let mut section = String::new();
    let mut current_block: Option<String> = None;
    let mut hdr_var = String::new();
    let mut raw: Option<Raw> = None;
    let mut pending_section = false;

    let flush = |raw: &mut Option<Raw>,
                 top: &mut Vec<Raw>,
                 blocks: &mut HashMap<String, Vec<Raw>>,
                 current_block: &Option<String>| {
        if let Some(r) = raw.take() {
            match current_block {
                Some(b) => blocks.entry(b.clone()).or_default().push(r),
                None => top.push(r),
            }
        }
    };

    for (code, value) in &pairs {
        if *code == 0 {
            flush(&mut raw, &mut top, &mut blocks, &current_block);
            let k = value.to_ascii_uppercase();
            match k.as_str() {
                "SECTION" => {
                    pending_section = true;
                }
                "ENDSEC" => {
                    section.clear();
                }
                "EOF" => {}
                "TABLE" => {}
                "ENDTAB" => {}
                "BLOCK" => {
                    raw = Some(Raw {
                        kind: "BLOCK".into(),
                        ..Default::default()
                    });
                }
                "ENDBLK" => {
                    current_block = None;
                }
                "LAYER" => {
                    raw = Some(Raw {
                        kind: "LAYER".into(),
                        ..Default::default()
                    });
                }
                "INSERT" | "LINE" | "CIRCLE" | "ARC" | "LWPOLYLINE" | "POINT" | "SPLINE"
                | "ELLIPSE" | "POLYLINE" | "VERTEX" | "SEQEND" | "TEXT" | "MTEXT" => {
                    raw = Some(Raw {
                        kind: k,
                        ..Default::default()
                    });
                }
                _ => {
                    raw = None;
                }
            }
            continue;
        }

        if pending_section && *code == 2 {
            // The group-2 immediately after a SECTION row names the section.
            section = value.to_ascii_uppercase();
            pending_section = false;
            continue;
        }

        match *code {
            9 => {
                // Header variable name.
                if section == "HEADER" {
                    hdr_var = value.clone();
                }
            }
            2 => {
                if let Some(r) = raw.as_mut() {
                    match r.kind.as_str() {
                        "LAYER" | "INSERT" => {
                            r.block = Some(value.clone());
                        }
                        "BLOCK" => {
                            r.block = Some(value.clone());
                            current_block = Some(value.clone());
                        }
                        _ => {}
                    }
                }
            }
            8 => {
                if let Some(r) = raw.as_mut()
                    && !matches!(r.kind.as_str(), "LAYER" | "BLOCK")
                {
                    r.layer = value.clone();
                }
            }
            70 => {
                if section == "HEADER"
                    && hdr_var == "$INSUNITS"
                    && let Ok(c) = value.parse::<i32>()
                {
                    units = Unit::from_insunits(c).unwrap_or(units);
                }
                if let Some(r) = raw.as_mut()
                    && let Ok(n) = value.parse::<f64>()
                {
                    r.nums.insert(70, n);
                }
            }
            10 | 20 => {
                if let Some(r) = raw.as_mut() {
                    if r.kind == "LWPOLYLINE" {
                        if *code == 10 {
                            if let Ok(n) = value.parse::<f64>() {
                                r.verts.push((n, f64::NAN, None));
                            }
                        } else if let Some(last) = r.verts.last_mut()
                            && last.1.is_nan()
                            && let Ok(n) = value.parse::<f64>()
                        {
                            last.1 = n;
                        }
                    } else if let Ok(n) = value.parse::<f64>() {
                        r.nums.insert(*code, n);
                    }
                }
            }
            30 | 11 | 21 | 31 | 12 | 22 | 32 | 13 | 23 | 33 => {
                if let Some(r) = raw.as_mut()
                    && let Ok(n) = value.parse::<f64>()
                {
                    r.nums.insert(*code, n);
                }
            }
            40 | 41 | 42 | 43 | 44 | 50 | 51 | 52 => {
                if let Some(r) = raw.as_mut()
                    && let Ok(n) = value.parse::<f64>()
                {
                    if r.kind == "LWPOLYLINE" && *code == 42 {
                        if let Some(last) = r.verts.last_mut() {
                            last.2 = Some(n);
                        }
                    } else {
                        r.nums.insert(*code, n);
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut raw, &mut top, &mut blocks, &current_block);

    // Collect layer names.
    for r in top.iter().chain(blocks.values().flatten()) {
        if r.kind == "LAYER" {
            if let Some(name) = &r.block
                && !layers.contains(name)
            {
                layers.push(name.clone());
            }
        } else if !r.layer.is_empty() && !layers.contains(&r.layer) {
            layers.push(r.layer.clone());
        }
    }

    let mut primitives = Vec::new();
    expand(top, &blocks, &mut primitives, 0, &mut Vec::new())?;

    Ok(Dxf {
        units,
        layers,
        primitives,
    })
}

fn expand(
    entities: Vec<Raw>,
    blocks: &HashMap<String, Vec<Raw>>,
    out: &mut Vec<Primitive>,
    depth: usize,
    stack: &mut Vec<String>,
) -> Result<()> {
    // First handle POLYLINE/VERTEX grouping: a POLYLINE is followed by VERTEX rows
    // until SEQEND. Reconstruct those as a single polyline primitive.
    let mut idx = 0;
    while idx < entities.len() {
        let ent = &entities[idx];
        match ent.kind.as_str() {
            "POLYLINE" => {
                let layer = ent.layer.clone();
                let mut pts: Vec<Point> = Vec::new();
                let mut j = idx + 1;
                while j < entities.len() && entities[j].kind == "VERTEX" {
                    let e = &entities[j];
                    if let (Some(x), Some(y)) = (e.n(10), e.n(20)) {
                        pts.push(Point::new(x, y));
                    }
                    j += 1;
                }
                if pts.len() < 2 {
                    return Err(DxfError::PolylineTooShort { layer });
                }
                out.push(Primitive::Polyline {
                    layer,
                    points: pts,
                    bulges: vec![None; 0],
                    closed: true,
                });
                idx = j;
                continue;
            }
            "VERTEX" | "SEQEND" => {
                idx += 1;
                continue;
            }
            "LINE" => {
                let layer = ent.layer.clone();
                let start = point2(ent, 10, 20, 30, &layer)?;
                let end = point2(ent, 11, 21, 31, &layer)?;
                out.push(Primitive::Line { layer, start, end });
            }
            "CIRCLE" => {
                let layer = ent.layer.clone();
                let center = point2(ent, 10, 20, 30, &layer)?;
                let radius = ent.n(40).ok_or(DxfError::Eof)?;
                if radius <= 0.0 {
                    return Err(DxfError::BadCircleRadius { layer });
                }
                out.push(Primitive::Circle {
                    layer,
                    center,
                    radius,
                });
            }
            "ARC" => {
                let layer = ent.layer.clone();
                let center = point2(ent, 10, 20, 30, &layer)?;
                let radius = ent.n(40).ok_or(DxfError::Eof)?;
                if radius <= 0.0 {
                    return Err(DxfError::BadArcRadius { layer });
                }
                let start_deg = ent.n(50).ok_or(DxfError::Eof)?;
                let end_deg = ent.n(51).ok_or(DxfError::Eof)?;
                out.push(Primitive::Arc {
                    layer,
                    center,
                    radius,
                    start_deg,
                    end_deg,
                });
            }
            "LWPOLYLINE" => {
                let layer = ent.layer.clone();
                if ent.verts.len() < 2 {
                    return Err(DxfError::PolylineTooShort { layer });
                }
                let points = ent
                    .verts
                    .iter()
                    .map(|(x, y, _)| Point::new(*x, if y.is_nan() { 0.0 } else { *y }))
                    .collect::<Vec<_>>();
                let bulges: Vec<Option<f64>> = ent.verts.iter().map(|(_, _, b)| *b).collect();
                let closed = matches!(ent.n(70), Some(f) if (f as i32 & 1) != 0);
                out.push(Primitive::Polyline {
                    layer,
                    points,
                    bulges,
                    closed,
                });
            }
            "POINT" => {
                let layer = ent.layer.clone();
                let pos = point2(ent, 10, 20, 30, &layer)?;
                out.push(Primitive::Point { layer, pos });
            }
            "TEXT" | "MTEXT" => {
                let layer = ent.layer.clone();
                if let (Some(x), Some(y)) = (ent.n(10), ent.n(20)) {
                    out.push(Primitive::Point {
                        layer,
                        pos: Point::new(x, y),
                    });
                }
            }
            "INSERT" => {
                let block = ent
                    .block
                    .clone()
                    .filter(|b| !b.is_empty())
                    .ok_or(DxfError::Eof)?;
                if depth >= INSERT_DEPTH_LIMIT {
                    return Err(DxfError::InsertRecursion { block });
                }
                let members = blocks.get(&block).ok_or_else(|| DxfError::MissingBlock {
                    block: block.clone(),
                })?;
                let pos = point2(ent, 10, 20, 30, &ent.layer)?;
                let sx = ent.n_def(41, 1.0);
                let sy = ent.n_def(42, 1.0);
                let rot = ent.n_def(51, 0.0);
                if sx == 0.0 || sy == 0.0 {
                    return Err(DxfError::BadArcRadius {
                        layer: ent.layer.clone(),
                    });
                }
                let r = rot.to_radians();
                let (cr, sr) = (r.cos(), r.sin());
                let map = move |p: Point| -> Point {
                    let x = p.x * sx;
                    let y = p.y * sy;
                    Point::new(cr * x - sr * y + pos.x, sr * x + cr * y + pos.y)
                };
                stack.push(block.clone());
                let mut member_prims: Vec<Primitive> = Vec::new();
                expand(members.clone(), blocks, &mut member_prims, depth + 1, stack)?;
                for p in member_prims {
                    out.push(transform_primitive(p, &map));
                }
                stack.pop();
            }
            "SPLINE" | "ELLIPSE" => {
                return Err(DxfError::UnsupportedEntity {
                    kind: ent.kind.clone(),
                    layer: ent.layer.clone(),
                });
            }
            "BLOCK" | "ENDBLK" | "LAYER" => {}
            _ => {}
        }
        idx += 1;
    }
    Ok(())
}

fn point2(ent: &Raw, cx: i32, cy: i32, cz: i32, layer: &str) -> Result<Point> {
    let x = ent.n(cx).ok_or(DxfError::Eof)?;
    let y = ent.n(cy).ok_or(DxfError::Eof)?;
    let z = ent.n(cz).unwrap_or(0.0);
    let p = Point::new(x, y);
    if !p.is_finite() || !z.is_finite() {
        return Err(DxfError::NonFinite {
            layer: layer.into(),
        });
    }
    Ok(p)
}

fn transform_primitive(mut prim: Primitive, f: &impl Fn(Point) -> Point) -> Primitive {
    match &mut prim {
        Primitive::Line { start, end, .. } => {
            *start = f(*start);
            *end = f(*end);
        }
        Primitive::Circle { center, radius, .. } => {
            let c = f(*center);
            let p1 = f(Point::new(center.x + *radius, center.y));
            let r = (p1.x - c.x).hypot(p1.y - c.y);
            *center = c;
            *radius = if r > 0.0 { r } else { *radius };
        }
        Primitive::Arc {
            center,
            radius,
            start_deg,
            end_deg,
            ..
        } => {
            let c = f(*center);
            let p1 = f(Point::new(center.x + *radius, center.y));
            let r = (p1.x - c.x).hypot(p1.y - c.y);
            // Shift the angular range by the transform's rotation, read from the
            // image of the local x-axis.
            let o = f(Point::new(0.0, 0.0));
            let ex = f(Point::new(1.0, 0.0));
            let shift = (ex.x - o.x).atan2(ex.y - o.y) * 180.0 / std::f64::consts::PI;
            *center = c;
            *radius = if r > 0.0 { r } else { *radius };
            *start_deg += shift;
            *end_deg += shift;
        }
        Primitive::Polyline { points, .. } => {
            for p in points.iter_mut() {
                *p = f(*p);
            }
        }
        Primitive::Point { pos, .. } => *pos = f(*pos),
    }
    prim
}

// ---------------------------------------------------------------------------
// DXF writer (R12-compatible, millimetres, $INSUNITS=4).
// ---------------------------------------------------------------------------

/// Write primitives to a DXF string in millimetres. A leading stock rectangle,
/// if given, is emitted on `stock_layer`.
pub fn write(primitives: &[Primitive], stock: Option<(f64, f64)>, stock_layer: &str) -> String {
    let mut s = String::new();
    s.push_str("0\nSECTION\n2\nHEADER\n");
    s.push_str("9\n$ACADVER\n1\nAC1009\n");
    s.push_str("9\n$INSUNITS\n70\n4\n");
    s.push_str("0\nENDSEC\n");
    let mut layers: Vec<String> = Vec::new();
    if stock.is_some() {
        layers.push(stock_layer.to_string());
    }
    for p in primitives {
        let l = p.layer();
        if !layers.contains(&l) {
            layers.push(l);
        }
    }
    s.push_str("0\nSECTION\n2\nTABLES\n");
    s.push_str("0\nTABLE\n2\nLAYER\n70\n");
    s.push_str(&layers.len().to_string());
    s.push('\n');
    for l in &layers {
        s.push_str("0\nLAYER\n2\n");
        s.push_str(l);
        s.push_str("\n70\n0\n62\n7\n6\nCONTINUOUS\n");
    }
    s.push_str("0\nENDTAB\n0\nENDSEC\n");
    s.push_str("0\nSECTION\n2\nENTITIES\n");
    if let Some((w, h)) = stock {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(w, 0.0),
            Point::new(w, h),
            Point::new(0.0, h),
        ];
        push_polyline(&mut s, stock_layer, &pts, &vec![None; pts.len()], true);
    }
    for p in primitives {
        write_primitive(&mut s, p);
    }
    s.push_str("0\nENDSEC\n0\nEOF\n");
    s
}

fn push_polyline(s: &mut String, layer: &str, pts: &[Point], bulges: &[Option<f64>], closed: bool) {
    s.push_str("0\nLWPOLYLINE\n8\n");
    s.push_str(layer);
    s.push_str("\n90\n");
    s.push_str(&pts.len().to_string());
    s.push('\n');
    s.push_str("70\n");
    s.push_str(&(if closed { 1 } else { 0 }).to_string());
    s.push('\n');
    for (i, p) in pts.iter().enumerate() {
        s.push_str("10\n");
        s.push_str(&fmt_num(p.x));
        s.push('\n');
        s.push_str("20\n");
        s.push_str(&fmt_num(p.y));
        s.push('\n');
        if let Some(b) = bulges.get(i).copied().flatten()
            && b.abs() > 1e-9
        {
            s.push_str("42\n");
            s.push_str(&fmt_num(b));
            s.push('\n');
        }
        s.push_str("43\n0.0\n");
    }
}

fn write_primitive(s: &mut String, p: &Primitive) {
    match p {
        Primitive::Line { layer, start, end } => {
            s.push_str("0\nLINE\n8\n");
            s.push_str(layer);
            s.push_str("\n10\n");
            s.push_str(&fmt_num(start.x));
            s.push_str("\n20\n");
            s.push_str(&fmt_num(start.y));
            s.push_str("\n30\n0.0\n11\n");
            s.push_str(&fmt_num(end.x));
            s.push_str("\n21\n");
            s.push_str(&fmt_num(end.y));
            s.push_str("\n31\n0.0\n");
        }
        Primitive::Circle {
            layer,
            center,
            radius,
        } => {
            s.push_str("0\nCIRCLE\n8\n");
            s.push_str(layer);
            s.push_str("\n10\n");
            s.push_str(&fmt_num(center.x));
            s.push_str("\n20\n");
            s.push_str(&fmt_num(center.y));
            s.push_str("\n30\n0.0\n40\n");
            s.push_str(&fmt_num(*radius));
            s.push('\n');
        }
        Primitive::Arc {
            layer,
            center,
            radius,
            start_deg,
            end_deg,
        } => {
            s.push_str("0\nARC\n8\n");
            s.push_str(layer);
            s.push_str("\n10\n");
            s.push_str(&fmt_num(center.x));
            s.push_str("\n20\n");
            s.push_str(&fmt_num(center.y));
            s.push_str("\n30\n0.0\n40\n");
            s.push_str(&fmt_num(*radius));
            s.push_str("\n50\n");
            s.push_str(&fmt_num(*start_deg));
            s.push_str("\n51\n");
            s.push_str(&fmt_num(*end_deg));
            s.push('\n');
        }
        Primitive::Polyline {
            layer,
            points,
            bulges,
            closed,
        } => push_polyline(s, layer, points, bulges, *closed),
        Primitive::Point { layer, pos } => {
            s.push_str("0\nPOINT\n8\n");
            s.push_str(layer);
            s.push_str("\n10\n");
            s.push_str(&fmt_num(pos.x));
            s.push_str("\n20\n");
            s.push_str(&fmt_num(pos.y));
            s.push_str("\n30\n0.0\n");
        }
    }
}

fn fmt_num(v: f64) -> String {
    if v.is_finite() && v == v.round() && v.abs() < 1e15 {
        format!("{:.1}", v)
    } else {
        format!("{}", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_and_circle() {
        let d = parse(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n\
             0\nLINE\n8\ncut\n10\n0.0\n20\n0.0\n30\n0.0\n11\n10.0\n21\n0.0\n31\n0.0\n\
             0\nCIRCLE\n8\ncut\n10\n5.0\n20\n5.0\n30\n0.0\n40\n2.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        assert_eq!(d.units, Unit::Mm);
        assert_eq!(d.primitives.len(), 2);
    }

    #[test]
    fn parse_inch_units() {
        let d = parse(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n10\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n\
             0\nLINE\n8\ncut\n10\n0.0\n20\n0.0\n30\n0.0\n11\n1.0\n21\n0.0\n31\n0.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        assert_eq!(d.units, Unit::Inch);
        assert!((Unit::Inch.to_mm(1.0) - 25.4).abs() < 1e-9);
    }

    #[test]
    fn to_mm_respects_insunits_when_override_unknown() {
        // A 1-inch line in an inch-declared file must become 25.4 mm when the
        // caller does not force a unit.
        let d = parse(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n10\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n\
             0\nLINE\n8\ncut\n10\n0.0\n20\n0.0\n30\n0.0\n11\n1.0\n21\n0.0\n31\n0.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let mm = to_mm(&d, "unknown", "mm");
        match &mm[0] {
            Primitive::Line { end, .. } => assert!((end.x - 25.4).abs() < 1e-6),
            _ => panic!("expected line"),
        }
    }

    #[test]
    fn to_mm_explicit_override_wins_over_insunits() {
        // An mm-declared file, but the caller explicitly says the part is inches.
        let d = parse(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n\
             0\nLINE\n8\ncut\n10\n0.0\n20\n0.0\n30\n0.0\n11\n1.0\n21\n0.0\n31\n0.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let mm = to_mm(&d, "in", "mm");
        match &mm[0] {
            Primitive::Line { end, .. } => assert!((end.x - 25.4).abs() < 1e-6),
            _ => panic!("expected line"),
        }
    }

    #[test]
    fn to_mm_default_applies_to_unitless_file() {
        // No $INSUNITS, no override, but the nest default is inches.
        let d = parse(
            "0\nSECTION\n2\nENTITIES\n\
             0\nLINE\n8\ncut\n10\n0.0\n20\n0.0\n30\n0.0\n11\n1.0\n21\n0.0\n31\n0.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        assert_eq!(d.units, Unit::Unknown);
        let mm = to_mm(&d, "unknown", "in");
        match &mm[0] {
            Primitive::Line { end, .. } => assert!((end.x - 25.4).abs() < 1e-6),
            _ => panic!("expected line"),
        }
    }

    #[test]
    fn parse_lwpolyline_closed_with_bulge() {
        let d = parse(
            "0\nSECTION\n2\nENTITIES\n\
             0\nLWPOLYLINE\n8\ncut\n90\n4\n70\n1\n\
             10\n0.0\n20\n0.0\n43\n0.0\n\
             10\n10.0\n20\n0.0\n42\n1.0\n43\n0.0\n\
             10\n10.0\n20\n10.0\n43\n0.0\n\
             10\n0.0\n20\n10.0\n43\n0.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        match &d.primitives[0] {
            Primitive::Polyline {
                points,
                bulges,
                closed,
                ..
            } => {
                assert!(*closed);
                assert_eq!(points.len(), 4);
                assert_eq!(bulges[1], Some(1.0));
            }
            _ => panic!("expected polyline"),
        }
    }

    #[test]
    fn write_roundtrip() {
        let src = parse(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n\
             0\nCIRCLE\n8\ncut\n10\n5.0\n20\n5.0\n30\n0.0\n40\n2.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let out = write(&src.primitives, Some((50.0, 40.0)), "STOCK");
        let back = parse(&out).unwrap();
        assert_eq!(back.primitives.len(), 2); // stock + circle
        assert_eq!(back.units, Unit::Mm);
    }

    #[test]
    fn reject_spline() {
        let r = parse(
            "0\nSECTION\n2\nENTITIES\n\
             0\nSPLINE\n8\ncut\n\
             0\nENDSEC\n0\nEOF\n",
        );
        assert!(matches!(
            r,
            Err(DxfError::UnsupportedEntity { kind, .. }) if kind == "SPLINE"
        ));
    }

    #[test]
    fn expand_insert_block() {
        let r = parse(
            "0\nSECTION\n2\nBLOCKS\n\
             0\nBLOCK\n2\nB1\n10\n0.0\n20\n0.0\n\
             0\nLINE\n8\ncut\n10\n0.0\n20\n0.0\n30\n0.0\n11\n5.0\n21\n0.0\n31\n0.0\n\
             0\nENDBLK\n0\nENDSEC\n\
             0\nSECTION\n2\nENTITIES\n\
             0\nINSERT\n2\nB1\n10\n10.0\n20\n20.0\n51\n0.0\n41\n1.0\n42\n1.0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        assert_eq!(r.primitives.len(), 1);
        match &r.primitives[0] {
            Primitive::Line { start, end, .. } => {
                assert!((start.x - 10.0).abs() < 1e-9);
                assert!((end.x - 15.0).abs() < 1e-9);
            }
            _ => panic!("expected line from block"),
        }
    }
}
