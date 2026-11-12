//! Builtin environment transforms. Values are complete environments, never patches.
use anyhow::{Context, Result, ensure};
use flow_core::{environment::Env, graph::*, providers::*};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn definitions() -> Vec<Definition> {
    ["set_env", "unset_env", "empty_env", "bake_args_into_env"]
        .into_iter()
        .map(|name| {
            let mut inputs = Vec::new();
            if name != "empty_env" {
                inputs.push(Pin::new("env", Type::Env));
            }
            let mut outputs = vec![Pin::new("env", Type::Env)];
            if name == "bake_args_into_env" {
                let signal = Type::Signal {
                    args: BTreeMap::new(),
                    open: true,
                };
                inputs.push(Pin::new("in", signal.clone()));
                outputs.push(Pin::new("out", signal));
            }
            let properties = match name {
                "set_env" => BTreeMap::from([(
                    "variables".into(),
                    Property {
                        ty: Type::Env,
                        default: Some(json!({})),
                        output: false,
                    },
                )]),
                "unset_env" => BTreeMap::from([(
                    "names".into(),
                    Property {
                        ty: Type::List {
                            item: Box::new(Type::String),
                        },
                        default: Some(json!([])),
                        output: false,
                    },
                )]),
                "bake_args_into_env" => BTreeMap::from([(
                    "mappings".into(),
                    Property {
                        ty: Type::Env,
                        default: Some(json!({})),
                        output: false,
                    },
                )]),
                _ => BTreeMap::new(),
            };
            Definition {
                id: format!("local.{name}"),
                owner: "local".into(),
                behavior: Behavior::Lazy,
                inputs,
                outputs,
                properties,
                capabilities: BTreeSet::new(),
                window: None,
                renderer: None,
            }
        })
        .collect()
}

pub fn read(invocation: &Invocation) -> Result<Value> {
    validate_properties(&invocation.type_id, &invocation.properties)?;
    let mut env: Env = invocation
        .properties
        .get("data_inputs")
        .and_then(|v| v.get("env"))
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()?
        .unwrap_or_else(|| invocation.environment.clone().unwrap_or_default());
    flow_core::environment::validate(&env)?;
    match invocation.type_id.as_str() {
        "local.set_env" => {
            let variables: Env = serde_json::from_value(
                invocation
                    .properties
                    .get("variables")
                    .cloned()
                    .unwrap_or(json!({})),
            )?;
            flow_core::environment::validate(&variables)?;
            env.extend(variables);
        }
        "local.unset_env" => {
            let names: Vec<String> = serde_json::from_value(
                invocation
                    .properties
                    .get("names")
                    .cloned()
                    .unwrap_or(json!([])),
            )?;
            for name in names {
                flow_core::environment::validate_name(&name)?;
                env.remove(&name);
            }
        }
        "local.empty_env" => env.clear(),
        "local.bake_args_into_env" => {
            let mappings: Env = serde_json::from_value(
                invocation
                    .properties
                    .get("mappings")
                    .cloned()
                    .unwrap_or(json!({})),
            )?;
            let args = invocation
                .args
                .as_object()
                .context("Signal arguments must be a record")?;
            let mut destinations = BTreeSet::new();
            for (arg, name) in mappings {
                flow_core::environment::validate_name(&name)?;
                ensure!(
                    destinations.insert(name.clone()),
                    "Arguments map to the same variable: {name}"
                );
                let value = args
                    .get(&arg)
                    .with_context(|| format!("Signal argument is missing: {arg}"))?;
                env.insert(
                    name,
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
            }
        }
        _ => anyhow::bail!("Unsupported environment node"),
    }
    flow_core::environment::validate(&env)?;
    Ok(if invocation.type_id == "local.bake_args_into_env" {
        json!({"env":env,"out":invocation.args})
    } else {
        json!({"env":env})
    })
}

pub fn validate_properties(type_id: &str, properties: &Record) -> Result<()> {
    match type_id {
        "local.set_env" => {
            let variables: Env =
                serde_json::from_value(properties.get("variables").cloned().unwrap_or(json!({})))?;
            flow_core::environment::validate(&variables)?;
        }
        "local.unset_env" => {
            let names: Vec<String> =
                serde_json::from_value(properties.get("names").cloned().unwrap_or(json!([])))?;
            for name in names {
                flow_core::environment::validate_name(&name)?;
            }
        }
        "local.bake_args_into_env" => {
            let mappings: Env =
                serde_json::from_value(properties.get("mappings").cloned().unwrap_or(json!({})))?;
            let mut names = BTreeSet::new();
            for (arg, name) in mappings {
                flow_core::graph::validate_name(&arg)?;
                flow_core::environment::validate_name(&name)?;
                ensure!(
                    names.insert(name.clone()),
                    "Arguments map to the same variable: {name}"
                );
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn default_argument_variable(name: &str) -> String {
    format!("FLOWWM_ARG_{}", name.to_ascii_uppercase())
}

pub fn apply(command: &mut std::process::Command, env: &Env) -> Result<()> {
    flow_core::environment::validate(env)?;
    // Preserve adapter metadata that was deliberately added by the process factory.
    let overrides: Vec<_> = command
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(std::ffi::OsStr::to_os_string)))
        .collect();
    command.env_clear().envs(env);
    for (name, value) in overrides {
        if let Some(value) = value {
            command.env(name, value);
        } else {
            command.env_remove(name);
        }
    }
    Ok(())
}
