//! Portable environment values and row-setting overlays.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub type Env = BTreeMap<String, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RowSettingDefinition {
    pub name: String,
    pub label: String,
    pub default: String,
}

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && !name.contains(['=', '\0']),
        "Invalid environment variable name: {name}"
    );
    Ok(())
}

pub fn validate(env: &Env) -> Result<()> {
    for (name, value) in env {
        validate_name(name)?;
        ensure!(
            !value.contains('\0'),
            "Environment value for {name} contains NUL"
        );
    }
    Ok(())
}

pub fn validate_row_settings(settings: &Env) -> Result<()> {
    validate(settings)?;
    let mut names = BTreeSet::new();
    for name in settings.keys() {
        crate::graph::validate_name(name)?;
        ensure!(
            names.insert(name.to_ascii_uppercase()),
            "Row setting names collide: {name}"
        );
    }
    Ok(())
}

pub fn row_environment(session: &Env, settings: &Env) -> Result<Env> {
    validate_row_settings(settings)?;
    let mut env = session.clone();
    for (name, value) in settings {
        env.insert(
            format!("FLOWWM_ROWSET_{}", name.to_ascii_uppercase()),
            value.clone(),
        );
    }
    Ok(env)
}
