//! One niri configuration for the compositor, session environment, and graph.
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::OnceLock};

pub fn directory() -> PathBuf {
    if let Some(path) = std::env::var_os("FLOWWM_CONFIG_DIR") {
        return path.into();
    }
    directories::BaseDirs::new()
        .expect("Home directory")
        .config_dir()
        .join("flow-wm")
}

pub fn defaults() -> niri_config::Config {
    let text = include_str!("../../../config/compositor.kdl")
        .lines()
        .filter(|line| !line.starts_with("include "))
        .collect::<Vec<_>>()
        .join("\n");
    niri_config::Config::parse_mem(&text).expect("Bundled configuration")
}

pub fn load() -> anyhow::Result<niri_config::Config> {
    let path = directory().join("config.kdl");
    if path.exists() {
        niri_config::Config::load(&path)
            .config
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    } else {
        Ok(defaults())
    }
}

pub fn flow() -> &'static niri_config::flow::Flow {
    static FLOW: OnceLock<niri_config::flow::Flow> = OnceLock::new();
    FLOW.get_or_init(|| match load() {
        Ok(config) => {
            let mut flow = config.flow;
            let scale = if config.animations.off {
                0.
            } else {
                config.animations.slowdown
            };
            flow.motion_ms = Some((flow.motion_ms.unwrap_or(220) as f64 * scale) as u32);
            flow.removal_ms = Some((flow.removal_ms.unwrap_or(180) as f64 * scale) as u32);
            flow
        }
        Err(error) => {
            eprintln!("FlowWM configuration: {error:#}");
            defaults().flow
        }
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Shortcut {
    pub keys: Vec<String>,
    pub action: String,
    pub description: String,
    pub scope: String,
}

pub fn key_name(key: niri_config::Key) -> String {
    use niri_config::{Modifiers as M, Trigger};
    let mut parts = vec![];
    for (flag, name) in [
        (M::COMPOSITOR, "Mod"),
        (M::SUPER, "Super"),
        (M::CTRL, "Ctrl"),
        (M::SHIFT, "Shift"),
        (M::ALT, "Alt"),
    ] {
        if key.modifiers.contains(flag) {
            parts.push(name.to_string());
        }
    }
    parts.push(match key.trigger {
        Trigger::Keysym(sym) => sym.name().unwrap_or("Unknown").to_string(),
        other => format!("{other:?}"),
    });
    parts.join("+")
}

pub fn graph_accelerator(key: &str) -> String {
    let mut parts: Vec<_> = key.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let mut result = String::new();
    for modifier in parts {
        result.push_str(match modifier.to_ascii_lowercase().as_str() {
            "mod" | "super" | "win" => "<Super>",
            "ctrl" | "control" => "<Control>",
            "shift" => "<Shift>",
            "alt" => "<Alt>",
            _ => "",
        });
    }
    result.push_str(key);
    result
}

pub fn shortcuts() -> Vec<Shortcut> {
    let config = match load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("FlowWM configuration: {error:#}");
            return vec![];
        }
    };
    let mut result: Vec<_> = config
        .binds
        .0
        .into_iter()
        .filter_map(|bind| {
            let description = bind.hotkey_overlay_title.flatten()?;
            Some(Shortcut {
                keys: vec![key_name(bind.key)],
                action: format!("{:?}", bind.action),
                description,
                scope: "global".into(),
            })
        })
        .collect();
    let defaults = defaults().flow;
    for bind in config
        .flow
        .binds
        .or(defaults.binds)
        .unwrap_or_default()
        .bindings
    {
        result.push(Shortcut {
            keys: vec![bind.key],
            action: bind.action,
            description: bind.title,
            scope: "graph".into(),
        });
    }
    result
}
