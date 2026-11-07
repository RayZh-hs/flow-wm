//! Versioned workspace-local layer surfaces. The compositor applies workspace
//! geometry directly, without per-frame IPC traffic or client-side animations.

/// Namespace prefix understood by compositors supporting version 1.
pub const PREFIX: &str = "niri-workspace-v1:";

/// Make a layer namespace attached to one stable workspace ID.
pub fn namespace(workspace_id: u64, name: &str) -> String {
    format!("{PREFIX}{workspace_id}:{name}")
}

/// Parse a workspace-local layer namespace. Zero and empty names are invalid.
pub fn workspace_id(namespace: &str) -> Option<u64> {
    let (id, name) = namespace.strip_prefix(PREFIX)?.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    id.parse().ok().filter(|&id| id != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_bind_stable_workspace_ids_and_reject_malformed_bindings() {
        assert_eq!(workspace_id(&namespace(42, "flowwm")), Some(42));
        for value in [
            "flowwm",
            "niri-workspace-v1:0:flowwm",
            "niri-workspace-v1:42:",
            "niri-workspace-v1:42",
            "niri-workspace-v1:overflow:flowwm",
        ] {
            assert_eq!(workspace_id(value), None);
        }
    }
}
