//! Sparrow (jagua-rs SPP) I/O: build the instance JSON, parse the solution JSON,
//! and compose the solver transform with the stock offset into a [`geom::Transform`].
//!
//! See `docs/sparrow-compatibility.md` for the pinned revision and the verified
//! input/output contract this module targets.

use crate::geom::{Point, Transform};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SolverError {
    #[error("solver output missing or malformed: {0}")]
    Malformed(String),
    #[error("solver reported an unknown item id {0}")]
    UnknownItemId(u64),
    #[error("solver placed item {id} {got} times, expected {want}")]
    BadCount { id: u64, got: usize, want: u64 },
}

/// One item to send to the solver (a single part shape with a demand).
#[derive(Debug, Clone)]
pub struct SolverItem {
    /// Stable unique id within this instance.
    pub id: u64,
    /// Index into the caller's part list (for mapping results back).
    pub part_index: usize,
    /// Conservative outer outline in mm (the tessellated nominal polygon
    /// dilated by the spacing box).
    pub polygon: Vec<Point>,
    /// Number of copies to place.
    pub demand: u64,
    /// Allowed rotations in degrees (CCW). Must be non-empty.
    pub rotations: Vec<f64>,
    /// Allow the solver to mirror (x-axis reflection) copies of this item.
    pub allow_reflection: bool,
}

/// The full solver instance we hand to Sparrow.
#[derive(Debug, Clone)]
pub struct SolverInstance {
    pub name: String,
    pub min_item_separation: f64,
    pub strip_height: f64,
    pub items: Vec<SolverItem>,
}

impl SolverInstance {
    /// Serialize to the Sparrow input JSON.
    pub fn to_json(&self) -> String {
        use serde_json::{Value, json};
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|it| {
                let data: Vec<Value> = it
                    .polygon
                    .iter()
                    .map(|p| json!([p.x as f32, p.y as f32]))
                    .collect();
                let angles: Vec<Value> = it.rotations.iter().map(|a| json!(*a as f32)).collect();
                let orientation = if it.allow_reflection {
                    json!({
                        "rotation": { "mode": "discrete", "angles": angles },
                        "reflection_axes": [0.0],
                    })
                } else {
                    json!({ "rotation": { "mode": "discrete", "angles": angles } })
                };
                json!({
                    "id": it.id,
                    "demand": it.demand,
                    "orientation": orientation,
                    "shape": { "type": "simple_polygon", "data": data },
                })
            })
            .collect();
        let v = json!({
            "name": self.name,
            "min_item_separation": self.min_item_separation as f32,
            "strip_height": self.strip_height as f32,
            "items": items,
        });
        v.to_string()
    }
}

/// A placed item from the solver, in strip coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub item_id: u64,
    #[serde(default)]
    pub reflected: bool,
    pub rotation_deg: f64,
    #[serde(rename = "translation")]
    pub translation: [f64; 2],
}

/// Parsed solver solution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverSolution {
    pub strip_width: f64,
    pub placed: Vec<Placed>,
}

#[derive(Deserialize)]
struct RawSolution {
    solution: RawInner,
}
#[derive(Deserialize)]
struct RawInner {
    strip_width: f64,
    layout: RawLayout,
}
#[derive(Deserialize)]
struct RawLayout {
    placed_items: Vec<RawPlaced>,
}
#[derive(Deserialize)]
struct RawPlaced {
    item_id: u64,
    transformation: RawTransform,
}
#[derive(Deserialize)]
struct RawTransform {
    #[serde(default)]
    reflected: bool,
    rotation: f64,
    translation: [f64; 2],
}

/// Parse the `output/final_{name}.json` that Sparrow writes.
pub fn parse_solution(json: &str) -> Result<SolverSolution, SolverError> {
    let raw: RawSolution =
        serde_json::from_str(json).map_err(|e| SolverError::Malformed(e.to_string()))?;
    let placed: Vec<Placed> = raw
        .solution
        .layout
        .placed_items
        .into_iter()
        .map(|p| Placed {
            item_id: p.item_id,
            reflected: p.transformation.reflected,
            rotation_deg: p.transformation.rotation,
            translation: [
                p.transformation.translation[0],
                p.transformation.translation[1],
            ],
        })
        .collect();
    Ok(SolverSolution {
        strip_width: raw.solution.strip_width,
        placed,
    })
}

/// Compose the solver placement (reflected, rotation, translation) with a
/// translation offset (the stock mapping) into a single [`Transform`].
pub fn placed_transform(p: &Placed, offset: Point) -> Transform {
    let solver = Transform {
        reflected: p.reflected,
        rotation_deg: p.rotation_deg,
        tx: p.translation[0],
        ty: p.translation[1],
    };
    let offset_t = Transform {
        reflected: false,
        rotation_deg: 0.0,
        tx: offset.x,
        ty: offset.y,
    };
    // Apply the solver transform first, then the stock offset: offset ∘ solver.
    solver.then(offset_t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_instance_json_shape() {
        let inst = SolverInstance {
            name: "n".into(),
            min_item_separation: 5.0,
            strip_height: 100.0,
            items: vec![SolverItem {
                id: 7,
                part_index: 0,
                polygon: vec![
                    Point::new(0.0, 0.0),
                    Point::new(10.0, 0.0),
                    Point::new(10.0, 10.0),
                ],
                demand: 3,
                rotations: vec![0.0, 90.0],
                allow_reflection: false,
            }],
        };
        let json = inst.to_json();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["name"], "n");
        assert_eq!(v["min_item_separation"], 5.0);
        assert_eq!(v["strip_height"], 100.0);
        assert_eq!(v["items"][0]["id"], 7);
        assert_eq!(v["items"][0]["demand"], 3);
        assert_eq!(v["items"][0]["orientation"]["rotation"]["mode"], "discrete");
        assert_eq!(v["items"][0]["shape"]["type"], "simple_polygon");
        // No reflection by default.
        assert!(
            v["items"][0]["orientation"]
                .get("reflection_axes")
                .is_none()
        );
    }

    #[test]
    fn build_instance_includes_reflection_when_allowed() {
        let inst = SolverInstance {
            name: "n".into(),
            min_item_separation: 0.0,
            strip_height: 100.0,
            items: vec![SolverItem {
                id: 7,
                part_index: 0,
                polygon: vec![
                    Point::new(0.0, 0.0),
                    Point::new(10.0, 0.0),
                    Point::new(10.0, 10.0),
                ],
                demand: 1,
                rotations: vec![0.0],
                allow_reflection: true,
            }],
        };
        let v: serde_json::Value = serde_json::from_str(&inst.to_json()).unwrap();
        let axes = v["items"][0]["orientation"]["reflection_axes"]
            .as_array()
            .expect("reflection_axes");
        assert_eq!(axes.len(), 1);
        assert!(axes[0].as_f64().unwrap().abs() < 1e-9);
    }

    #[test]
    fn parse_solution_roundtrip() {
        let json = r#"{
          "name": "n",
          "solution": {
            "strip_width": 42.0,
            "layout": { "container_id": 0,
              "placed_items": [
                { "item_id": 1, "transformation": {"rotation": 90.0, "translation": [3.0, 4.0]} },
                { "item_id": 2, "transformation": {"reflected": true, "rotation": 0.0, "translation": [5.0, 6.0]} }
              ],
              "density": 0.5 },
            "density": 0.5,
            "run_time_sec": 1
          }
        }"#;
        let s = parse_solution(json).unwrap();
        assert_eq!(s.strip_width, 42.0);
        assert_eq!(s.placed.len(), 2);
        assert_eq!(s.placed[0].rotation_deg, 90.0);
        assert!(!s.placed[0].reflected);
        assert!(s.placed[1].reflected);
    }

    #[test]
    fn placed_transform_matches_manual() {
        // rotate 90 then translate by (3,4), then offset (10,0).
        let p = Placed {
            item_id: 1,
            reflected: false,
            rotation_deg: 90.0,
            translation: [3.0, 4.0],
        };
        let t = placed_transform(&p, Point::new(10.0, 0.0));
        let pt = t.apply(Point::new(1.0, 0.0));
        // manual: rotate90 (1,0)->(0,1); + (3,4) -> (3,5); + offset (10,0) -> (13,5)
        assert!((pt.x - 13.0).abs() < 1e-9 && (pt.y - 5.0).abs() < 1e-9);
    }
}
