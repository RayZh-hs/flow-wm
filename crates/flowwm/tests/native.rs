//! Explicit graphical tests use the same independent host/provider startup as installation.
use std::{path::Path, process::Command};
fn smoke(script: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(
        Command::new("python3")
            .arg(root.join("scripts").join(script))
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
}
#[test]
#[ignore = "requires Xvfb, GTK, grim and xdotool; starts an isolated nested Niri"]
fn desktop_rows_previews_and_optional_interfaces() {
    smoke("smoke_desktop.py");
}
#[test]
#[ignore = "requires Xvfb, GTK, grim and xdotool; starts an isolated nested Niri"]
fn pointer_input_row_restoration_and_workflow_ownership() {
    smoke("smoke_interactions.py");
}
