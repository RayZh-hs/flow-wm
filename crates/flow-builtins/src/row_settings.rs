//! Builtin row-settings extension and configurable terminal launch adapters.
use flow_core::{environment::RowSettingDefinition, providers::Registry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn directory(
    properties: &flow_core::graph::Record,
    env: &flow_core::environment::Env,
) -> std::path::PathBuf {
    properties
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            env.get("FLOWWM_ROWSET_PWD")
                .map(String::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(".")
        .into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Extension {
    pub pwd: Option<String>,
    pub terminals: BTreeMap<String, Vec<String>>,
}
impl Default for Extension {
    fn default() -> Self {
        Self {
            pwd: None,
            terminals: [
                ("alacritty", vec!["--working-directory"]),
                ("kitty", vec!["--directory"]),
                ("foot", vec!["--working-directory"]),
                ("wezterm", vec!["start", "--cwd"]),
            ]
            .into_iter()
            .map(|(name, args)| (name.into(), args.into_iter().map(str::to_owned).collect()))
            .collect(),
        }
    }
}
impl Extension {
    pub fn install(&self, registry: &mut Registry) {
        let default = self
            .pwd
            .clone()
            .or_else(|| registry.session_environment.get("HOME").cloned())
            .unwrap_or_else(|| "/".into());
        registry.row_settings.insert(
            "pwd".into(),
            RowSettingDefinition {
                name: "pwd".into(),
                label: "Working directory".into(),
                default,
            },
        );
    }
    pub fn terminal_arguments(
        &self,
        executable: &str,
        env: &flow_core::environment::Env,
    ) -> Vec<String> {
        let executable = std::path::Path::new(executable)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(executable);
        let Some(args) = self.terminals.get(executable) else {
            return vec![];
        };
        let Some(pwd) = env.get("FLOWWM_ROWSET_PWD").filter(|s| !s.is_empty()) else {
            return vec![];
        };
        let mut args = args.clone();
        if !args.is_empty() {
            args.push(pwd.clone());
        }
        args
    }
}
