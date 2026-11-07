//! Native workspace content supplied by a graph client. Geometry is latched on
//! the canvas surface commit, so its decorations and live windows move together.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Reserved application ID prefix for native node canvases.
pub const PREFIX: &str = "dev.flowwm.node-row.v1.";

/// Bind a canvas to a stable workspace ID.
pub fn app_id(workspace: u64) -> String {
    format!("{PREFIX}{workspace}")
}

/// Parse a native canvas binding, rejecting zero and malformed IDs.
pub fn workspace_id(app_id: &str) -> Option<u64> {
    app_id
        .strip_prefix(PREFIX)?
        .parse()
        .ok()
        .filter(|&id| id != 0)
}

/// Presentation of one application tile in a node row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct NodeWindow {
    /// The existing niri window ID.
    pub id: u64,
    /// Rectangle relative to the canvas, in logical pixels.
    pub bounds: (f64, f64, f64, f64),
    /// Expanded windows receive normal application input; previews belong to the graph.
    pub expanded: bool,
    /// Preview opacity, including graph search dimming.
    pub opacity: f32,
}

/// Geometry committed with a native graph canvas buffer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct NodeRowScene {
    /// Live previews and expanded application tiles.
    pub windows: Vec<NodeWindow>,
    /// Additional shell insets, in top, right, bottom, left order.
    pub insets: [i32; 4],
}

impl NodeRowScene {
    /// Bound payload size and reject non-finite or degenerate geometry.
    pub fn validate(&self) -> bool {
        let mut ids = HashSet::new();
        self.windows.len() <= 4096
            && self.insets.iter().all(|&v| (0..=16384).contains(&v))
            && self.windows.iter().all(|w| {
                let (x, y, width, height) = w.bounds;
                w.opacity.is_finite()
                    && (0.0..=1.0).contains(&w.opacity)
                    && w.id != 0
                    && ids.insert(w.id)
                    && [x, y, width, height]
                        .iter()
                        .all(|v| v.is_finite() && v.abs() <= 1e7)
                    && width > 0.
                    && height > 0.
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_and_scenes_reject_invalid_ids_geometry_and_duplicate_elements() {
        assert_eq!(workspace_id(&app_id(42)), Some(42));
        for value in [
            app_id(0),
            format!("{PREFIX}bad"),
            format!("{PREFIX}18446744073709551616"),
            "dev.flowwm.Shell".into(),
        ] {
            assert_eq!(workspace_id(&value), None);
        }
        let mut scene = NodeRowScene {
            windows: vec![NodeWindow {
                id: 1,
                bounds: (-12.5, 9., 100., 80.),
                expanded: false,
                opacity: 1.,
            }],
            insets: [28, 0, 0, 0],
        };
        assert!(scene.validate());
        scene.windows.push(scene.windows[0].clone());
        assert!(!scene.validate());
        scene.windows.pop();
        for value in [f64::NAN, f64::INFINITY, 1e9] {
            scene.windows[0].bounds.0 = value;
            assert!(!scene.validate());
        }
        scene.windows[0].bounds.0 = 0.;
        scene.windows[0].bounds.2 = 0.;
        assert!(!scene.validate());
        scene.windows.clear();
        scene.insets[0] = -1;
        assert!(!scene.validate());
    }
}
