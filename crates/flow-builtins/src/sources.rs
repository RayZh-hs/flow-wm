//! Registered polling sources. Polling jobs and threshold state are owned here.
use crate::{
    local::{Environment, Limits},
    process::Supervisor,
};
use anyhow::{Result, ensure};
use flow_core::{graph::Record, providers::*};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
#[derive(Default)]
pub struct Sources {
    running: Mutex<BTreeMap<String, Control>>,
}
struct Control {
    cancel: Arc<AtomicBool>,
    processes: Arc<Supervisor>,
    current: Arc<Mutex<Option<String>>>,
    wake: Arc<(Mutex<()>, Condvar)>,
}
impl Sources {
    pub fn subscribe(
        &self,
        source: SourceRegistration,
        events: SourceSink,
        processes: Arc<Supervisor>,
        limits: Limits,
        environment: Environment,
    ) -> Result<String> {
        let interval = source
            .properties
            .get("interval_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(limits.source_interval_secs);
        ensure!(interval > 0, "Source interval must be positive");
        let handle = source.handle.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut running = self.running.lock().unwrap();
        ensure!(!running.contains_key(&handle), "Source already installed");
        let current = Arc::new(Mutex::new(None));
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        running.insert(
            handle.clone(),
            Control {
                cancel: cancel.clone(),
                processes: processes.clone(),
                current: current.clone(),
                wake: wake.clone(),
            },
        );
        drop(running);
        std::thread::spawn(move || {
            let mut sequence = source.sequence;
            let same_configuration = source.state["configuration"] == json!(source.properties);
            let mut armed = if same_configuration {
                source.state["armed"].as_bool().unwrap_or(true)
            } else {
                true
            };
            let mut samples = if same_configuration {
                source.state["samples"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let history = source
                .properties
                .get("history_capacity")
                .and_then(|v| v.as_u64())
                .unwrap_or(120)
                .clamp(1, 4096) as usize;
            loop {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                sequence += 1;
                let mut values = Record::new();
                let mut pin = "out";
                let mut args = json!({});
                if source.type_id == "local.receive" {
                    let Some(messaging) = &environment.messaging else {
                        break;
                    };
                    let channel = source
                        .properties
                        .get("channel")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    match messaging.receive(source.workflow.clone(), channel.into()) {
                        Ok(Some(delivery)) => {
                            args = delivery.args.clone();
                            values.insert("args".into(), args.clone());
                            let accepted = events(SourceEvent {
                                delivery: Some(delivery.id.clone()),
                                node: source.node.clone(),
                                generation: source.generation,
                                sequence,
                                pin: "out".into(),
                                args,
                                values,
                                state: json!({}),
                            });
                            if accepted.is_ok() {
                                let _ = messaging.acknowledge(source.workflow.clone(), delivery.id);
                            }
                        }
                        Ok(None) => (),
                        Err(_) => (),
                    }
                    let guard = wake.0.lock().unwrap();
                    let _wait = wake
                        .1
                        .wait_timeout_while(guard, Duration::from_secs(interval), |_| {
                            !cancel.load(Ordering::Acquire)
                        })
                        .unwrap();
                    continue;
                }
                if source.type_id == "local.timer" {
                    args = json!({"tick":sequence});
                } else {
                    let invocation = Invocation {
                        environment: source.environment.clone(),
                        input: String::new(),
                        id: format!("{}:sample:{sequence}", source.handle).into(),
                        workflow: source.workflow.clone(),
                        node: source.node.clone(),
                        type_id: "local.metric_sample".into(),
                        properties: source.properties.clone(),
                        args: json!({}),
                        generation: source.generation,
                    };
                    let (tx, rx) = mpsc::channel();
                    let id = invocation.id.0.clone();
                    let mut active = current.lock().unwrap();
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    *active = Some(id.clone());
                    let sample = crate::local::metric(&invocation, &environment, &limits).and_then(
                        |process| {
                            processes.start(
                                process,
                                invocation,
                                &limits,
                                Arc::new(move |c| {
                                    let _ = tx.send(c);
                                }),
                                None,
                            )
                        },
                    );
                    drop(active);
                    if sample.is_err() {
                        *current.lock().unwrap() = None;
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    let completion = loop {
                        if cancel.load(Ordering::Acquire) {
                            let _ = processes.interrupt(&id);
                            break None;
                        }
                        match rx.recv_timeout(Duration::from_millis(20)) {
                            Ok(c) => break Some(c),
                            Err(mpsc::RecvTimeoutError::Timeout) => (),
                            Err(_) => break None,
                        }
                    };
                    let Some(completion) = completion else {
                        break;
                    };
                    *current.lock().unwrap() = None;
                    if let Some(value) = completion.result.get("value").and_then(|v| v.as_f64()) {
                        values.insert("value".into(), json!(value));
                        let time = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        samples.push(json!({"time":time,"value":value}));
                        if samples.len() > history {
                            samples.drain(..samples.len() - history);
                        }
                        args = json!({"value":value});
                        pin = if source.type_id == "local.metric" {
                            "on_sample"
                        } else {
                            "out"
                        };
                        if source.type_id == "local.watcher" {
                            let threshold = source
                                .properties
                                .get("threshold")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.);
                            let above = source
                                .properties
                                .get("above")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            let crossed = if above {
                                value > threshold
                            } else {
                                value < threshold
                            };
                            if !crossed {
                                armed = true;
                                pin = "";
                            } else if armed {
                                armed = false;
                            } else {
                                pin = "";
                            }
                        }
                    } else {
                        values.insert("error".into(), completion.result);
                        pin = "";
                    }
                }
                let event = SourceEvent {
                    delivery: None,
                    node: source.node.clone(),
                    generation: source.generation,
                    sequence,
                    pin: pin.into(),
                    args,
                    values,
                    state: json!({"armed":armed,"samples":samples,"configuration":source.properties}),
                };
                while !cancel.load(Ordering::Acquire) && events(event.clone()).is_err() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let guard = wake.0.lock().unwrap();
                let _wait = wake
                    .1
                    .wait_timeout_while(guard, Duration::from_secs(interval), |_| {
                        !cancel.load(Ordering::Acquire)
                    })
                    .unwrap();
            }
        });
        Ok(handle)
    }
    pub fn unsubscribe(&self, handle: &str) -> Result<()> {
        if let Some(control) = self.running.lock().unwrap().remove(handle) {
            control.cancel.store(true, Ordering::Release);
            control.wake.1.notify_all();
            if let Some(invocation) = control.current.lock().unwrap().clone() {
                control.processes.interrupt(&invocation)?;
                while control.processes.running(&invocation) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        Ok(())
    }
}
