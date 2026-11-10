//! Linux 6.15+ process observation through the engine's real pidfd monitor.
use flowwm::{
    engine::{Control, Engine},
    graph::{self, Connection, Endpoint},
    groups,
    model::*,
    niri::Desktop,
    store::Store,
};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    engine: Engine,
    desktop: Desktop,
    sources: Vec<String>,
    events: PathBuf,
    _directory: tempfile::TempDir,
}

fn window(id: u64, pid: Option<i32>) -> niri_ipc::Window {
    serde_json::from_value(json!({
        "id":id,"workspace_id":1,"title":"Application","app_id":"test.application",
        "pid":pid,"is_focused":false,"is_floating":false,"is_urgent":false,
        "layout":{"pos_in_scrolling_layout":[id,1],"tile_size":[800,600],
            "window_size":[800,600],"tile_pos_in_workspace_view":[0,0],"window_offset_in_tile":[0,0]},
        "focus_timestamp":null
    })).unwrap()
}

impl Fixture {
    fn new(pids: &[Option<i32>]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let desktop = Desktop {
            connected: true,
            session: "process-exit-test".into(),
            windows_ready: true,
            workspaces_ready: true,
            windows: pids
                .iter()
                .enumerate()
                .map(|(i, pid)| window(i as u64 + 1, *pid))
                .collect(),
            workspaces: vec![
                serde_json::from_value(json!({
                    "id":1,"idx":1,"name":null,"output":"TEST","is_urgent":false,
                    "is_active":true,"is_focused":true,"active_window_id":null
                }))
                .unwrap(),
            ],
            error: None,
        };
        let mut state = State::empty(directory.path().into());
        groups::synchronize(&mut state, &desktop);
        let sources = desktop
            .windows
            .iter()
            .map(|w| {
                state
                    .nodes
                    .iter()
                    .find(|n| n.window_id == Some(w.id))
                    .unwrap()
                    .id
                    .clone()
            })
            .collect::<Vec<_>>();
        let events = directory.path().join("events");
        let mut sink = Node::new(
            &state.selected_workspace,
            NodeKind::Shell,
            directory.path().into(),
            Position::default(),
        );
        sink.command = format!(
            "printf '%s\\n' \"$FLOWWM_ARG_EXIT_CODE\" >> '{}'",
            events.display()
        );
        let sink_id = sink.id.clone();
        state.nodes.push(sink);
        for source in &sources {
            graph::connect(
                &mut state,
                Connection {
                    source: Endpoint {
                        node: source.clone(),
                        pin: "on_exit".into(),
                    },
                    target: Endpoint {
                        node: sink_id.clone(),
                        pin: "in".into(),
                    },
                },
            )
            .unwrap();
        }
        let (engine, _) = Engine::new(state, Store::new(directory.path().join("state")).unwrap());
        let fixture = Self {
            engine,
            desktop,
            sources,
            events,
            _directory: directory,
        };
        fixture.sync();
        fixture
    }

    fn sync(&self) {
        self.engine
            .request(Control::SyncDesktop {
                desktop: self.desktop.clone(),
            })
            .unwrap();
    }

    fn codes(&self) -> Vec<u8> {
        std::fs::read_to_string(&self.events)
            .unwrap_or_default()
            .lines()
            .map(|s| s.parse().unwrap())
            .collect()
    }

    fn wait_codes(&self, expected: &[u8]) {
        let start = Instant::now();
        while self.codes() != expected {
            assert!(
                start.elapsed() < Duration::from_secs(8),
                "Timed out: {:?}; {:?}",
                self.engine.snapshot().error,
                self.engine.snapshot().state
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            self.engine.snapshot().error.is_none(),
            "{:?}",
            self.engine.snapshot().error
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.engine.shutdown();
    }
}

fn wait(predicate: impl Fn() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "Timed out waiting for process exit delivery"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn application(body: &str) -> Child {
    Command::new("python3")
        .args([
            "-c",
            &format!("import os, signal, sys\nsys.stdin.buffer.read(1)\n{body}"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn release(child: &mut Child) {
    child.stdin.take().unwrap().write_all(b"x").unwrap();
}

#[test]
fn observes_a_non_child_process_and_delivers_its_actual_exit_code_once() {
    let mut owner = Command::new("python3").args(["-c", r#"
import subprocess, sys
child = subprocess.Popen([sys.executable, '-c', 'import os,sys; sys.stdin.buffer.read(1); os._exit(37)'], stdin=subprocess.PIPE)
print(child.pid, flush=True)
sys.stdin.buffer.read(1)
child.communicate(b'x')
"#]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(owner.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let pid: i32 = line.trim().parse().unwrap();
    let fixture = Fixture::new(&[Some(pid)]);
    assert_eq!(
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    release(&mut owner);
    assert!(owner.wait().unwrap().success());
    fixture.wait_codes(&[37]);
    for _ in 0..3 {
        fixture.sync();
    }
    assert_eq!(fixture.codes(), [37]);
    assert_eq!(
        fixture
            .engine
            .snapshot()
            .state
            .node(&fixture.sources[0])
            .unwrap()
            .status,
        Status::Failed
    );
}

#[test]
fn closing_shared_process_windows_keeps_connections_until_the_process_exits() {
    let mut child = application("os._exit(0)");
    let pid = child.id() as i32;
    let mut fixture = Fixture::new(&[Some(pid), Some(pid)]);
    fixture.desktop.windows.remove(0);
    fixture.sync();
    assert!(fixture.codes().is_empty());
    fixture.desktop.windows.clear();
    fixture.sync();
    let snapshot = fixture.engine.snapshot();
    assert_eq!(snapshot.state.connections.len(), 2);
    for id in &fixture.sources {
        let node = snapshot.state.node(id).unwrap();
        assert!(node.window_id.is_none());
        assert_eq!(node.status, Status::Running);
    }
    assert!(fixture.codes().is_empty());
    release(&mut child);
    assert!(child.wait().unwrap().success());
    fixture.wait_codes(&[0, 0]);
}

#[test]
fn closing_a_window_without_exit_consumers_removes_it_while_its_process_lives() {
    let mut child = application("os._exit(0)");
    let mut fixture = Fixture::new(&[Some(child.id() as i32), Some(child.id() as i32)]);
    let source = fixture.sources[0].clone();
    let target = fixture.engine.snapshot().state.connections[0]
        .target
        .node
        .clone();
    fixture
        .engine
        .request(Control::Disconnect {
            source: source.clone(),
            target,
        })
        .unwrap();
    fixture.desktop.windows.remove(0);
    fixture.sync();
    let state = fixture.engine.snapshot().state;
    assert!(state.node(&source).is_none());
    assert_eq!(state.node(&fixture.sources[1]).unwrap().window_id, Some(2));
    assert!(child.try_wait().unwrap().is_none());
    release(&mut child);
    child.wait().unwrap();
    fixture.wait_codes(&[0]);
}

#[test]
fn a_reopened_window_does_not_replace_the_closed_window_still_awaiting_exit() {
    let mut first = application("os._exit(37)");
    let mut second = application("os._exit(0)");
    let mut fixture = Fixture::new(&[Some(first.id() as i32)]);
    fixture.desktop.windows.clear();
    fixture.sync();
    fixture
        .desktop
        .windows
        .push(window(2, Some(second.id() as i32)));
    fixture.sync();
    let snapshot = fixture.engine.snapshot();
    assert!(
        snapshot
            .state
            .node(&fixture.sources[0])
            .unwrap()
            .window_id
            .is_none()
    );
    let reopened = snapshot
        .state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(2))
        .unwrap();
    assert_ne!(reopened.id, fixture.sources[0]);
    release(&mut first);
    first.wait().unwrap();
    fixture.wait_codes(&[37]);
    release(&mut second);
    second.wait().unwrap();
}

#[test]
fn waits_for_reaping_and_preserves_uint8_boundaries() {
    for code in [0, 255] {
        let mut child = application(&format!("os._exit({code})"));
        let pid = child.id();
        let fixture = Fixture::new(&[Some(pid as i32)]);
        release(&mut child);
        wait(|| {
            std::fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|s| {
                s.lines()
                    .any(|l| l.starts_with("State:") && l.contains("Z (zombie)"))
            })
        });
        assert!(fixture.codes().is_empty());
        assert_eq!(child.wait().unwrap().code(), Some(code));
        fixture.wait_codes(&[code as u8]);
    }
}

#[test]
fn signal_termination_is_encoded_as_128_plus_signal() {
    let mut child = application("os.kill(os.getpid(), signal.SIGTERM)");
    let fixture = Fixture::new(&[Some(child.id() as i32)]);
    release(&mut child);
    assert!(!child.wait().unwrap().success());
    fixture.wait_codes(&[143]);
}

#[test]
fn unknown_processes_do_not_emit_success_and_shutdown_does_not_kill_applications() {
    let mut child = application("os._exit(0)");
    let mut fixture = Fixture::new(&[None, Some(-1), Some(child.id() as i32)]);
    fixture.desktop.windows.clear();
    fixture.sync();
    assert!(fixture.codes().is_empty());
    let start = Instant::now();
    fixture.engine.shutdown();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(child.try_wait().unwrap().is_none());
    release(&mut child);
    child.wait().unwrap();
}

#[test]
fn explicitly_deleting_a_closed_window_cancels_its_exit_delivery() {
    let mut child = application("os._exit(37)");
    let mut fixture = Fixture::new(&[Some(child.id() as i32)]);
    fixture.desktop.windows.clear();
    fixture.sync();
    fixture
        .engine
        .request(Control::Delete {
            id: fixture.sources[0].clone(),
        })
        .unwrap();
    release(&mut child);
    child.wait().unwrap();
    fixture.sync();
    assert!(fixture.codes().is_empty());
}

#[test]
fn manually_stopping_a_closed_window_removes_it_and_cancels_exit_delivery() {
    let mut child = application("os._exit(37)");
    let mut fixture = Fixture::new(&[Some(child.id() as i32)]);
    fixture.desktop.windows.clear();
    fixture.sync();
    fixture
        .engine
        .request(Control::Stop {
            id: fixture.sources[0].clone(),
        })
        .unwrap();
    assert!(
        fixture
            .engine
            .snapshot()
            .state
            .node(&fixture.sources[0])
            .is_none()
    );
    release(&mut child);
    child.wait().unwrap();
    fixture.sync();
    assert!(fixture.codes().is_empty());
}
