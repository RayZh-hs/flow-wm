//! Configurable Linux session actions. Commands are literal argv vectors.
use crate::{
    local::{Environment, Limits},
    process::Process,
};
use anyhow::{Context, Result, ensure};
use flow_core::{graph::*, providers::*};
use serde_json::json;
use std::{collections::BTreeMap, process::Command, time::Duration};

pub const ACTIONS: &[(&str, &str, &[&str])] = &[
    ("power_off", "Power off", &["systemctl", "poweroff"]),
    ("reboot", "Restart computer", &["systemctl", "reboot"]),
    ("suspend", "Suspend", &["systemctl", "suspend"]),
    ("hibernate", "Hibernate", &["systemctl", "hibernate"]),
    (
        "log_out",
        "Log out",
        &["niri", "msg", "action", "quit", "--skip-confirmation"],
    ),
    (
        "lock_session",
        "Lock session",
        &["loginctl", "lock-session"],
    ),
];

pub fn name(type_id: &str) -> Option<&'static str> {
    let id = type_id.strip_prefix("local.")?;
    ACTIONS
        .iter()
        .find(|(action, _, _)| *action == id)
        .map(|(_, name, _)| *name)
}

#[derive(knuffel::Decode)]
struct Config {
    #[knuffel(children(name = "action"))]
    actions: Vec<Action>,
}
#[derive(knuffel::Decode)]
struct Action {
    #[knuffel(argument)]
    name: String,
    #[knuffel(arguments)]
    command: Vec<String>,
}

pub fn definitions(environment: &Environment) -> Result<Vec<Definition>> {
    let mut commands: BTreeMap<String, Vec<String>> = ACTIONS
        .iter()
        .map(|(id, _, args)| ((*id).into(), args.iter().map(|s| (*s).into()).collect()))
        .collect();
    if let Some(directory) = &environment.config_directory {
        let path = directory.join("session.kdl");
        if path.exists() {
            let config: Config =
                knuffel::parse(&path.to_string_lossy(), &std::fs::read_to_string(&path)?)
                    .map_err(|e| anyhow::anyhow!("Invalid session.kdl: {e:?}"))?;
            let mut seen = std::collections::BTreeSet::new();
            for action in config.actions {
                ensure!(
                    seen.insert(action.name.clone()),
                    "Duplicate session action: {}",
                    action.name
                );
                ensure!(
                    commands.contains_key(&action.name),
                    "Unknown session action: {}",
                    action.name
                );
                commands.insert(action.name, action.command);
            }
        }
    }
    Ok(ACTIONS
        .iter()
        .map(|(id, _, _)| {
            let event = Type::Signal {
                args: Default::default(),
                open: true,
            };
            Definition {
                id: format!("local.{id}"),
                owner: "local".into(),
                behavior: Behavior::Invoked,
                inputs: vec![Pin::new("in", event.clone())],
                outputs: vec![Pin::new("out", event.clone()), Pin::new("failed", event)],
                properties: BTreeMap::from([(
                    "argv".into(),
                    Property {
                        ty: Type::List {
                            item: Box::new(Type::String),
                        },
                        default: Some(json!(commands[*id])),
                        output: false,
                    },
                )]),
                capabilities: ["application".into()].into(),
                window: None,
                renderer: None,
            }
        })
        .collect())
}

pub fn process(invocation: &Invocation, _: &Environment, limits: &Limits) -> Result<Process> {
    let args: Vec<String> = serde_json::from_value(
        invocation
            .properties
            .get("argv")
            .cloned()
            .context("Session action needs argv")?,
    )?;
    let executable = args
        .first()
        .filter(|s| !s.is_empty())
        .context("This session action is disabled (empty argv)")?;
    let mut command = Command::new(executable);
    command.args(&args[1..]);
    Ok(Process {
        command,
        input: None,
        stdout_stream: "stdout",
        timeout: Duration::from_secs(limits.execution_timeout_secs),
        directory: crate::row_settings::directory(
            &invocation.properties,
            invocation
                .environment
                .as_ref()
                .unwrap_or(&Default::default()),
        ),
        writer: false,
        decode: |stdout| Ok(json!({"stdout":stdout})),
    })
}
