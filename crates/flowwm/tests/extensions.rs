mod support;

use flowwm::{
    catalog::Catalog,
    client::{Action, Presentation},
    extensions,
    graph::{Connection, Endpoint, pins},
    model::{Node, NodeKind, State, Status},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

/// Keep the real host for execution, but reproduce the Info response of hosts
/// that predate schema discovery and can survive a shell-only update.
struct LegacyInfo {
    socket: PathBuf,
    closed: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    listener: Option<thread::JoinHandle<()>>,
}
impl LegacyInfo {
    fn start(socket: PathBuf, upstream: PathBuf) -> Self {
        let listener = UnixListener::bind(&socket).unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let (stop, count) = (closed.clone(), requests.clone());
        let listener = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let (upstream, count) = (upstream.clone(), count.clone());
                thread::spawn(move || {
                    let _ = Self::forward(stream.unwrap(), &upstream, &count);
                });
            }
        });
        Self {
            socket,
            closed,
            requests,
            listener: Some(listener),
        }
    }
    fn forward(mut stream: UnixStream, path: &Path, count: &AtomicUsize) -> anyhow::Result<()> {
        let mut line = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut line)?;
        let mut upstream = UnixStream::connect(path)?;
        upstream.write_all(line.as_bytes())?;
        let cancel = upstream.try_clone()?;
        let _connection = flow_host::ipc::disconnect_signal(&stream, move || {
            let _ = cancel.shutdown(std::net::Shutdown::Both);
        })?;
        for line in BufReader::new(upstream).lines() {
            let mut reply: Value = serde_json::from_str(&line?)?;
            if let Some(info) = reply["ok"]["result"].as_object_mut()
                && info.remove("node_types").is_some()
            {
                count.fetch_add(1, Ordering::AcqRel);
            }
            writeln!(stream, "{reply}")?;
        }
        Ok(())
    }
}
impl Drop for LegacyInfo {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.socket);
        let _ = self.listener.take().unwrap().join();
    }
}

fn manifest(invocation: &Path) -> Value {
    let mut manifest = json!({
        "version": 3,
        "providers": [{
            "id": "fixture.send", "owner": "fixture", "behavior": "invoked",
            "name": "Send message", "capabilities": ["shell"],
            "inputs": [
                {"name": "in", "ty": {"kind": "signal", "args": {}, "open": true}},
                {"name": "text", "ty": {"kind": "string"}, "property": "text", "default": ""}
            ],
            "outputs": [
                {"name": "out", "ty": {"kind": "signal", "args": {}, "open": true}},
                {"name": "output", "ty": {"kind": "string"}}
            ],
            "properties": {"text": {"ty": {"kind": "string"}, "default": "Hello"}},
            "command": ["sh", "-c", "cat >\"$1\"; printf '%s\\n' '{\"output\":\"sent\"}'", "fixture", invocation],
            "window": {"title": "Send message", "output": "output"}
        }]
    });
    let mut other = manifest["providers"][0].clone();
    other["id"] = json!("fixture.search");
    manifest["providers"].as_array_mut().unwrap().push(other);
    manifest
}

#[test]
fn subprocess_nodes_work_before_their_first_instance_with_legacy_host_info() {
    let directory = tempfile::tempdir().unwrap();
    let invocation = directory.path().join("invocation.json");
    let mut initial = State::empty(directory.path().into());
    let trigger = Node::new(
        &initial.selected_workspace,
        NodeKind::Human,
        directory.path().into(),
        Default::default(),
    );
    initial.nodes.push(trigger.clone());
    let mut config = PathBuf::new();
    let (engine, _) = support::fixture_with_config(initial, None, |path| {
        config = path.into();
        std::fs::create_dir(path.join("extensions")).unwrap();
        std::fs::write(
            path.join("extensions/fixture.json"),
            manifest(&invocation).to_string(),
        )
        .unwrap();
    });
    let proxy = LegacyInfo::start(
        directory.path().join("legacy.sock"),
        config.parent().unwrap().join("host.sock"),
    );
    let (engine, _) = engine.reconnect_with_socket(proxy.socket.clone());
    assert_eq!(proxy.requests.load(Ordering::Acquire), 1);
    assert!(
        !invocation.exists(),
        "Loading schemas must not invoke programs"
    );

    let bundle = extensions::load_bundle(&config.join("extensions")).unwrap();
    let schema = bundle
        .nodes
        .iter()
        .find(|d| d.id == "fixture.send")
        .unwrap();
    assert_eq!(schema.window.as_ref().unwrap().title, "Send message");
    let catalog = Catalog::new(
        &engine.snapshot(),
        Presentation {
            lookup_node: Some(trigger.id.clone()),
            ..Default::default()
        },
        &bundle.macros,
    );
    assert!(catalog.lookup.unwrap().outputs.iter().any(|output| {
        output
            .targets
            .iter()
            .any(|target| target.create && target.id == "fixture.send" && target.input == "in")
    }));
    let mut node = Node::new(
        &trigger.workspace,
        NodeKind::Extension,
        directory.path().into(),
        Default::default(),
    );
    node.flow.node_type = "fixture.send".into();
    node.flow
        .properties
        .insert("text".into(), json!("A test message"));
    let edge = Connection {
        source: Endpoint {
            node: trigger.id,
            pin: "accepted".into(),
        },
        target: Endpoint {
            node: node.id.clone(),
            pin: "in".into(),
        },
    };
    // Connected creation validates this candidate in the UI before sending it to the host.
    let mut candidate = (*engine.snapshot().state).clone();
    candidate.nodes.push(node.clone());
    candidate.connections.push(edge.clone());
    candidate.validate().unwrap();
    engine
        .request(Action::Create {
            node: Box::new(node.clone()),
        })
        .unwrap();
    engine
        .request(Action::ConnectPins { connection: edge })
        .unwrap();
    let state = engine.snapshot().state;
    let runtime = state.orchestration.as_ref().unwrap();
    let graph = &runtime.workflows[&node.workspace.clone().into()].graph;
    assert!(graph.node_types.iter().any(|d| d.id == "fixture.send"));
    assert!(!graph.node_types.iter().any(|d| d.id == "fixture.search"));
    engine
        .request(Action::Run {
            id: node.id.clone(),
            downstream: false,
        })
        .unwrap();
    support::wait(|| engine.snapshot().state.node(&node.id).unwrap().status == Status::Done);
    let sent: Value = serde_json::from_slice(&std::fs::read(&invocation).unwrap()).unwrap();
    assert_eq!(sent["operation"], "invoke");
    assert_eq!(sent["properties"]["text"], "A test message");
    let (engine, _) = engine.reconnect_with_socket(proxy.socket.clone());
    let state = engine.snapshot().state;
    assert!(state.connections.iter().any(|e| e.target.node == node.id));
    assert!(
        pins(&state, state.node(&node.id).unwrap(), true)
            .iter()
            .any(|p| p.name == "output")
    );
    engine.shutdown();
}

#[test]
fn mounted_host_schemas_take_precedence_over_changed_manifests() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = PathBuf::new();
    let (engine, _) =
        support::fixture_with_config(State::empty(directory.path().into()), None, |path| {
            config = path.into();
            std::fs::create_dir(path.join("extensions")).unwrap();
            std::fs::write(
                path.join("extensions/fixture.json"),
                manifest(&directory.path().join("invocation.json")).to_string(),
            )
            .unwrap();
        });
    let mut changed = manifest(&directory.path().join("invocation.json"));
    changed["providers"][0]["properties"]["text"]["default"] = json!("Changed on disk");
    std::fs::write(config.join("extensions/fixture.json"), changed.to_string()).unwrap();
    let (engine, _) = engine.reconnect();
    let state = engine.snapshot().state;
    let schemas: Vec<_> = state
        .node_types
        .iter()
        .filter(|d| d.id == "fixture.send")
        .collect();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].properties["text"].default, Some(json!("Hello")));
    engine.shutdown();
}
