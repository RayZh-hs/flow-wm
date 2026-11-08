//! Shared configuration and shortcut registry for the shell, installer, and help UI.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn directory() -> PathBuf {
    if let Some(path) = std::env::var_os("FLOWWM_CONFIG_DIR") {
        return path.into();
    }
    directories::BaseDirs::new()
        .expect("Home directory")
        .config_dir()
        .join("flow-wm")
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shortcut {
    pub keys: Vec<String>,
    pub action: String,
    pub description: String,
    pub scope: String,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub niri: Option<String>,
    #[serde(default)]
    pub repeat: Option<bool>,
    #[serde(default)]
    pub allow_when_locked: bool,
    #[serde(default)]
    pub allow_inhibiting: Option<bool>,
    #[serde(default)]
    pub cooldown_ms: Option<u32>,
}
pub fn shortcuts() -> Vec<Shortcut> {
    let defaults = include_str!("../../../config/keybindings.json");
    match std::fs::read_to_string(directory().join("keybindings.json")) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Invalid shortcut registry: {error}; using defaults");
                serde_json::from_str(defaults).unwrap()
            }
        },
        Err(_) => serde_json::from_str(defaults).unwrap(),
    }
}
