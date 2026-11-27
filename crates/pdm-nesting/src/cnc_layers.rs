//! Router layers for a flat panel: which contour is the outline, which goes through, and how
//! deep a blind one goes, as layer names router CAM can read.
//!
//! - `OUTLINE`: the part's outer contour, cut through.
//! - `THROUGH`: an inner contour cut all the way through (a hole, an inside cut-out).
//! - `DEPTH_<mm>`: a blind feature this deep from the top face (the face the part is machined
//!   from, the side away from the spoilboard), e.g. `DEPTH_6.35`.
//!
//! SolidCraft's `sheet.export_dxf` writes the outline on `OUTLINE`, the cut-outs on `CUT` and
//! each pocket on `POCKET_<side>_Z<z0>_TO_<z1>_MM`, with z measured up from the flat's bottom
//! face. [`relayer_solidcraft_flat`] renames those layers in the DXF text, so the depth comes
//! from the model, not from a hand-named layer.

use std::fmt;

/// The outline layer.
pub const OUTLINE: &str = "OUTLINE";
/// The layer of inner contours cut all the way through.
pub const THROUGH: &str = "THROUGH";
/// The prefix of a blind feature's layer; the rest is its depth in mm.
pub const DEPTH_PREFIX: &str = "DEPTH_";

/// Why a flat pattern cannot be put on router layers.
#[derive(Debug, Clone, PartialEq)]
pub enum RelayerError {
    /// A pocket opens on the bottom face, or lies inside the material: it cannot be cut from
    /// the top face in one setup.
    NotFromTop { layer: String },
    /// A pocket layer whose depths cannot be read.
    BadPocketLayer { layer: String },
    /// The file already uses a router layer name for something else.
    Clash { layer: String },
}

impl fmt::Display for RelayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelayerError::NotFromTop { layer } => write!(
                f,
                "pocket layer {layer} is not open on the top face: it cannot be machined from the \
                 top in one setup (flip the part in the model, or machine it in a second setup)"
            ),
            RelayerError::BadPocketLayer { layer } => {
                write!(f, "pocket layer {layer}: its depths cannot be read")
            }
            RelayerError::Clash { layer } => write!(
                f,
                "the flat pattern already has a layer {layer}; it cannot be told apart from the \
                 router layers"
            ),
        }
    }
}

impl std::error::Error for RelayerError {}

/// Whether `layer` is a blind feature's layer (`DEPTH_<mm>`).
pub fn is_blind_layer(layer: &str) -> bool {
    layer
        .trim()
        .to_ascii_uppercase()
        .strip_prefix(DEPTH_PREFIX)
        .and_then(|d| d.parse::<f64>().ok())
        .is_some_and(|d| d.is_finite() && d > 0.0)
}

/// A depth in mm as a layer name: at most three decimals, trailing zeros dropped
/// (`19 − 12.65` → `DEPTH_6.35`).
pub fn depth_layer(depth_mm: f64) -> String {
    let t = format!("{depth_mm:.3}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    format!("{DEPTH_PREFIX}{t}")
}

/// The router layer of one SolidCraft flat-pattern layer: `Ok(Some(new))` to rename it,
/// `Ok(None)` to keep it (bend lines, etching, text and the like).
pub fn router_layer(layer: &str) -> Result<Option<String>, RelayerError> {
    let up = layer.trim().to_ascii_uppercase();
    if up == "CUT" {
        return Ok(Some(THROUGH.to_string()));
    }
    if up == THROUGH || is_blind_layer(&up) {
        return Err(RelayerError::Clash {
            layer: layer.to_string(),
        });
    }
    let Some(rest) = up.strip_prefix("POCKET_") else {
        return Ok(None);
    };
    let bad = || RelayerError::BadPocketLayer {
        layer: layer.to_string(),
    };
    // POCKET_<side>_Z<z0>_TO_<z1>_MM
    let (side, zs) = rest.split_once("_Z").ok_or_else(bad)?;
    let zs = zs.strip_suffix("_MM").ok_or_else(bad)?;
    let (z0, z1) = zs.split_once("_TO_").ok_or_else(bad)?;
    let (z0, z1): (f64, f64) = (
        z0.parse().map_err(|_| bad())?,
        z1.parse().map_err(|_| bad())?,
    );
    if !(z0.is_finite() && z1.is_finite() && z1 > z0) {
        return Err(bad());
    }
    if side != "TOP" {
        return Err(RelayerError::NotFromTop {
            layer: layer.to_string(),
        });
    }
    // Open on the top face (z1 is the sheet's thickness): as deep as from there to its floor.
    Ok(Some(depth_layer(z1 - z0)))
}

/// Rename the layers of a SolidCraft flat-pattern DXF (ASCII, group code / value lines) to the
/// router layers: `CUT` → `THROUGH`, `POCKET_TOP_Z<z0>_TO_<z1>_MM` → `DEPTH_<z1 − z0>`.
/// `OUTLINE` stays. Entity layers (group 8) and the LAYER table's names are renamed alike; line
/// endings are kept. A pocket opening on the bottom face or inside the material is refused.
pub fn relayer_solidcraft_flat(dxf: &str) -> Result<String, RelayerError> {
    let lines: Vec<&str> = dxf.split_inclusive('\n').collect();
    let mut out = String::with_capacity(dxf.len() + 64);
    let mut in_layer_record = false;
    let mut i = 0;
    while i < lines.len() {
        let code_line = lines[i];
        let Some(value_line) = lines.get(i + 1) else {
            out.push_str(code_line);
            break;
        };
        let code = code_line.trim();
        let value = value_line.trim_end_matches(['\r', '\n']);
        let ending = &value_line[value.len()..];
        if code == "0" {
            in_layer_record = value.trim() == "LAYER";
        }
        let names_layer = code == "8" || (code == "2" && in_layer_record);
        out.push_str(code_line);
        match names_layer
            .then(|| router_layer(value))
            .transpose()?
            .flatten()
        {
            Some(new) => {
                out.push_str(&new);
                out.push_str(ending);
            }
            None => out.push_str(value_line),
        }
        i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_from_the_model() {
        assert_eq!(router_layer("OUTLINE").unwrap(), None);
        assert_eq!(router_layer("CUT").unwrap().as_deref(), Some("THROUGH"));
        assert_eq!(
            router_layer("POCKET_TOP_Z12.650000_TO_19.000000_MM")
                .unwrap()
                .as_deref(),
            Some("DEPTH_6.35")
        );
        assert_eq!(
            router_layer("POCKET_TOP_Z9.000000_TO_19.000000_MM")
                .unwrap()
                .as_deref(),
            Some("DEPTH_10")
        );
        assert_eq!(router_layer("BEND_UP").unwrap(), None);
        assert!(matches!(
            router_layer("POCKET_BOTTOM_Z0.000000_TO_0.500000_MM"),
            Err(RelayerError::NotFromTop { .. })
        ));
        assert!(matches!(
            router_layer("POCKET_INTERNAL_Z1.000000_TO_2.000000_MM"),
            Err(RelayerError::NotFromTop { .. })
        ));
        assert!(matches!(
            router_layer("POCKET_TOP_Zx_TO_1_MM"),
            Err(RelayerError::BadPocketLayer { .. })
        ));
        assert!(matches!(
            router_layer("DEPTH_3"),
            Err(RelayerError::Clash { .. })
        ));
    }

    #[test]
    fn renames_entities_and_the_layer_table() {
        let dxf = "0\r\nSECTION\r\n2\r\nTABLES\r\n0\r\nTABLE\r\n2\r\nLAYER\r\n\
                   0\r\nLAYER\r\n2\r\nCUT\r\n70\r\n0\r\n\
                   0\r\nLAYER\r\n2\r\nPOCKET_TOP_Z12.650000_TO_19.000000_MM\r\n70\r\n0\r\n\
                   0\r\nENDTAB\r\n0\r\nENDSEC\r\n\
                   0\r\nSECTION\r\n2\r\nENTITIES\r\n\
                   0\r\nCIRCLE\r\n8\r\nCUT\r\n10\r\n1\r\n20\r\n1\r\n40\r\n5\r\n\
                   0\r\nCIRCLE\r\n8\r\nPOCKET_TOP_Z12.650000_TO_19.000000_MM\r\n10\r\n1\r\n20\r\n1\r\n40\r\n5\r\n\
                   0\r\nENDSEC\r\n0\r\nEOF\r\n";
        let out = relayer_solidcraft_flat(dxf).unwrap();
        assert!(
            !out.contains("POCKET_") && !out.contains("\nCUT\r"),
            "{out}"
        );
        assert_eq!(out.matches("\r\nTHROUGH\r\n").count(), 2, "{out}");
        assert_eq!(out.matches("\r\nDEPTH_6.35\r\n").count(), 2, "{out}");
        // The table's own name ("2 / LAYER" after "0 / TABLE") is not a layer.
        assert!(out.contains("TABLE\r\n2\r\nLAYER\r\n"), "{out}");
        let parsed = crate::dxf::parse(&out).unwrap();
        let mut layers: Vec<String> = parsed.primitives.iter().map(|p| p.layer()).collect();
        layers.sort();
        assert_eq!(layers, ["DEPTH_6.35", "THROUGH"]);
    }

    #[test]
    fn blind_layers() {
        assert!(is_blind_layer("DEPTH_6.35") && is_blind_layer("depth_1"));
        assert!(
            !is_blind_layer("DEPTH_0") && !is_blind_layer("DEPTH_x") && !is_blind_layer("THROUGH")
        );
        assert_eq!(depth_layer(19.0 - 12.65), "DEPTH_6.35");
    }
}
