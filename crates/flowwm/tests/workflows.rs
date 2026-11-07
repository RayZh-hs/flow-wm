use flowwm::{
    engine::{Control, Engine},
    model::*,
    store::Store,
};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    engine: Engine,
    dir: tempfile::TempDir,
}

#[test]
fn row_view_changes_are_local_and_persist_across_restart() {
    let f = Fixture::new();
    let first = f.engine.snapshot().state.selected_workspace.clone();
    let node = f.node(NodeKind::Human, "Notes");
    f.add(&node);
    f.engine
        .request(Control::RowView {
            workspace: first.clone(),
            view: RowView::Nodes,
        })
        .unwrap();
    f.engine
        .request(Control::CreateWorkspace {
            name: "Another row".into(),
            cwd: f.dir.path().into(),
        })
        .unwrap();
    let second = f.engine.snapshot().state.selected_workspace.clone();
    assert_eq!(f.engine.snapshot().state.workspace().view, RowView::Windows);
    f.engine
        .request(Control::SelectWorkspace { id: first.clone() })
        .unwrap();
    assert_eq!(f.engine.snapshot().state.workspace().view, RowView::Nodes);
    f.engine
        .request(Control::RowView {
            workspace: second.clone(),
            view: RowView::Windows,
        })
        .unwrap();
    assert_eq!(
        f.engine
            .snapshot()
            .state
            .node(&node.id)
            .unwrap()
            .column_mode,
        ColumnMode::Node
    );
    f.engine.shutdown();
    let state: State =
        serde_json::from_slice(&fs::read(f.dir.path().join("data/state.json")).unwrap()).unwrap();
    assert_eq!(state.workspace().view, RowView::Nodes);
    assert_eq!(
        state
            .workspaces
            .iter()
            .find(|w| w.id == second)
            .unwrap()
            .view,
        RowView::Windows
    );
    // Old state files migrate consistently, including the first row.
    let mut legacy = serde_json::to_value(&state).unwrap();
    for workspace in legacy["workspaces"].as_array_mut().unwrap() {
        workspace.as_object_mut().unwrap().remove("view");
    }
    let migrated: State = serde_json::from_value(legacy).unwrap();
    assert!(
        migrated
            .workspaces
            .iter()
            .all(|w| w.view == RowView::Windows)
    );
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("data")).unwrap();
        let state = State::empty(dir.path().to_path_buf());
        let (engine, _) = Engine::new(state, store);
        Self { engine, dir }
    }
    fn node(&self, kind: NodeKind, command: &str) -> Node {
        let state = self.engine.snapshot().state;
        let mut node = Node::new(
            &state.selected_workspace,
            kind,
            self.dir.path().to_path_buf(),
            Position::default(),
        );
        node.command = command.into();
        node
    }
    fn add(&self, node: &Node) {
        self.engine
            .request(Control::Create {
                node: Box::new(node.clone()),
            })
            .unwrap();
    }
    fn run(&self, id: &str, downstream: bool) {
        self.engine
            .request(Control::Run {
                id: id.into(),
                downstream,
            })
            .unwrap();
    }
    fn wait(&self, id: &str, status: Status) -> Node {
        let start = Instant::now();
        loop {
            let node = self.engine.snapshot().state.node(id).unwrap().clone();
            if node.status == status {
                return node;
            }
            assert!(
                start.elapsed() < Duration::from_secs(8),
                "Expected {status:?}, got {:?}: {:?}",
                node.status,
                node.logs
            );
            thread::sleep(Duration::from_millis(30));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.engine.shutdown();
    }
}

#[test]
fn pipeline_passes_structured_inputs_after_parent_finishes() {
    let f = Fixture::new();
    let a = f.node(NodeKind::Shell, "printf 'alpha'");
    f.add(&a);
    let mut b = f.node(NodeKind::Shell, "printf '%s' \"$FLOWWM_INPUTS\"");
    b.dependencies.push(a.id.clone());
    f.add(&b);
    f.run(&a.id, true);
    let output = f.wait(&b.id, Status::Done).output;
    let inputs: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(inputs[0]["output"], "alpha\n");
    assert_eq!(inputs[0]["status"], "done");
}

#[test]
fn failed_step_blocks_success_branch_and_runs_recovery_branch() {
    let f = Fixture::new();
    let a = f.node(NodeKind::Shell, "exit 7");
    f.add(&a);
    let mut success = f.node(NodeKind::Shell, "printf 'should not run'");
    success.dependencies.push(a.id.clone());
    f.add(&success);
    let mut recovery = f.node(NodeKind::Shell, "printf 'recovering'");
    recovery.dependencies.push(a.id.clone());
    recovery.run_when = RunWhen::Failure;
    f.add(&recovery);
    f.run(&a.id, true);
    assert!(
        f.wait(&recovery.id, Status::Done)
            .output
            .contains("recovering")
    );
    assert_eq!(
        f.engine.snapshot().state.node(&success.id).unwrap().status,
        Status::Queued
    );
}

#[test]
fn human_gate_pauses_and_supplies_the_handoff() {
    let f = Fixture::new();
    let gate = f.node(NodeKind::Gate, "Review this");
    f.add(&gate);
    let mut shell = f.node(NodeKind::Shell, "printf '%s' \"$FLOWWM_INPUTS\"");
    shell.dependencies.push(gate.id.clone());
    f.add(&shell);
    f.run(&gate.id, true);
    f.wait(&gate.id, Status::Review);
    assert_eq!(
        f.engine.snapshot().state.node(&shell.id).unwrap().status,
        Status::Queued
    );
    f.engine
        .request(Control::Approve {
            id: gate.id,
            output: Some("approved approach".into()),
        })
        .unwrap();
    let output = f.wait(&shell.id, Status::Done).output;
    assert!(output.contains("approved approach"));
}

#[test]
fn human_work_and_a_shell_job_run_in_parallel() {
    let f = Fixture::new();
    let human = f.node(NodeKind::Human, "Plan the rollout");
    f.add(&human);
    let shell = f.node(NodeKind::Shell, "printf 'agent work'");
    f.add(&shell);
    f.engine.request(Control::RunWorkspace).unwrap();
    f.wait(&shell.id, Status::Done);
    assert_eq!(
        f.engine.snapshot().state.node(&human.id).unwrap().status,
        Status::Running
    );
    f.engine
        .request(Control::Approve {
            id: human.id.clone(),
            output: None,
        })
        .unwrap();
    f.wait(&human.id, Status::Done);
}

#[test]
fn stopping_a_process_cancels_its_children_and_releases_the_directory() {
    let f = Fixture::new();
    let a = f.node(
        NodeKind::Shell,
        "sleep 30; printf 'escaped' > should-not-exist",
    );
    f.add(&a);
    f.run(&a.id, false);
    f.wait(&a.id, Status::Running);
    f.engine
        .request(Control::Stop { id: a.id.clone() })
        .unwrap();
    let b = f.node(NodeKind::Shell, "printf 'next'");
    f.add(&b);
    f.run(&b.id, false);
    f.wait(&b.id, Status::Done);
    assert!(!f.dir.path().join("should-not-exist").exists());
    assert_eq!(
        f.engine.snapshot().state.node(&a.id).unwrap().status,
        Status::Stopped
    );
}

#[test]
fn writers_in_one_directory_are_serialized() {
    let f = Fixture::new();
    let a = f.node(
        NodeKind::Shell,
        "printf a >> order; sleep 0.2; printf b >> order",
    );
    f.add(&a);
    let b = f.node(NodeKind::Shell, "printf c >> order");
    f.add(&b);
    f.engine.request(Control::RunWorkspace).unwrap();
    f.wait(&b.id, Status::Done);
    assert_eq!(
        fs::read_to_string(f.dir.path().join("order")).unwrap(),
        "abc"
    );
}

#[test]
fn watcher_triggers_once_until_the_threshold_rearms() {
    let f = Fixture::new();
    fs::write(f.dir.path().join("value"), "100").unwrap();
    let mut metric = f.node(NodeKind::Metric, "cat value");
    metric.metric.interval_secs = 1;
    f.add(&metric);
    let target = f.node(NodeKind::Shell, "printf x >> triggers");
    f.add(&target);
    let mut child = f.node(NodeKind::Shell, "printf y >> child-triggers");
    child.dependencies.push(target.id.clone());
    f.add(&child);
    f.engine
        .request(Control::Watch {
            watcher: Watcher {
                id: id(),
                source: metric.id.clone(),
                target: target.id.clone(),
                threshold: 50.0,
                above: true,
                enabled: true,
                armed: true,
                last_triggered: None,
            },
        })
        .unwrap();
    f.run(&metric.id, false);
    f.wait(&target.id, Status::Done);
    f.wait(&child.id, Status::Done);
    thread::sleep(Duration::from_millis(1250));
    assert_eq!(
        fs::read_to_string(f.dir.path().join("triggers")).unwrap(),
        "x"
    );
    fs::write(f.dir.path().join("value"), "0").unwrap();
    let start = Instant::now();
    while !f.engine.snapshot().state.watchers[0].armed {
        assert!(start.elapsed() < Duration::from_secs(4));
        thread::sleep(Duration::from_millis(40));
    }
    fs::write(f.dir.path().join("value"), "100").unwrap();
    let start = Instant::now();
    loop {
        if fs::read_to_string(f.dir.path().join("triggers")).unwrap() == "xx" {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(4));
        thread::sleep(Duration::from_millis(40));
    }
    f.engine.request(Control::Stop { id: metric.id }).unwrap();
    f.wait(&child.id, Status::Done);
    assert_eq!(
        fs::read_to_string(f.dir.path().join("child-triggers")).unwrap(),
        "yy"
    );
}

#[test]
fn cycle_is_rejected_without_mutating_the_graph() {
    let mut state = State::empty(PathBuf::from("/tmp"));
    let ws = state.selected_workspace.clone();
    let a = Node::new(
        &ws,
        NodeKind::Shell,
        PathBuf::from("/tmp"),
        Position::default(),
    );
    let mut b = a.clone();
    b.id = id();
    b.dependencies.push(a.id.clone());
    state.nodes = vec![a.clone(), b.clone()];
    assert!(state.connect(&b.id, &a.id).is_err());
    assert!(state.node(&a.id).unwrap().dependencies.is_empty());
    state.validate().unwrap();
}

#[test]
fn recipe_is_portable_idle_and_uses_new_ids_and_directory() {
    let mut state = State::empty(PathBuf::from("/tmp"));
    let ws = state.selected_workspace.clone();
    let mut a = Node::new(
        &ws,
        NodeKind::Agent,
        PathBuf::from("/old/path"),
        Position::default(),
    );
    a.status = Status::Done;
    a.output = "private output".into();
    a.window_match = Some("old-window".into());
    let mut b = a.clone();
    b.id = id();
    b.dependencies = vec![a.id.clone()];
    state.nodes = vec![a.clone(), b.clone()];
    let recipe = state
        .recipe("Reusable work".into(), &[a.id.clone(), b.id.clone()])
        .unwrap();
    assert!(
        recipe
            .nodes
            .iter()
            .all(|n| n.output.is_empty() && n.window_match.is_none())
    );
    let fresh = state
        .instantiate(&recipe, &ws, PathBuf::from("/new/path"))
        .unwrap();
    assert_eq!(
        state.node(&fresh[1]).unwrap().dependencies,
        vec![fresh[0].clone()]
    );
    assert!(fresh.iter().all(|id| {
        let n = state.node(id).unwrap();
        n.status == Status::Idle && n.cwd == std::path::Path::new("/new/path")
    }));
}

#[test]
fn corrupt_state_is_preserved_and_restart_stops_running_work() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut state = State::empty(dir.path().into());
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Agent,
        dir.path().into(),
        Position::default(),
    );
    node.status = Status::Running;
    state.nodes.push(node);
    store.save(&state).unwrap();
    assert_eq!(
        store.load(dir.path().into()).unwrap().nodes[0].status,
        Status::Stopped
    );
    let invalid = b"{ corrupt state";
    fs::write(dir.path().join("state.json"), invalid).unwrap();
    assert!(store.load(dir.path().into()).is_err());
    assert_eq!(fs::read(dir.path().join("state.json")).unwrap(), invalid);
}

#[test]
fn editing_an_active_node_is_rejected() {
    let f = Fixture::new();
    let human = f.node(NodeKind::Human, "Work");
    f.add(&human);
    f.run(&human.id, false);
    f.wait(&human.id, Status::Running);
    assert!(
        f.engine
            .request(Control::Update {
                node: Box::new(human)
            })
            .is_err()
    );
}

#[test]
fn agent_handoff_requires_an_explicit_success_or_issue() {
    use flowwm::engine::{AgentOutcome, agent_result};
    let issue = agent_result(
        r#"{"outcome":"issue","summary":"Regression needs a decision","artifacts":[]}"#,
    )
    .unwrap();
    assert_eq!(issue.outcome, AgentOutcome::Issue);
    assert!(agent_result("I think this is probably fine").is_err());
    assert!(agent_result(r#"{"outcome":"unknown","summary":"?","artifacts":[]}"#).is_err());
}

#[test]
fn material_palette_uses_the_dms_primary_and_validates_colors() {
    use flowwm::ui::theme::{Color, Theme};
    let mut theme = Theme::default();
    theme.apply_material(
        &serde_json::json!({"primary":"#a2c9fe", "on_primary":"#00325b", "surface":"invalid"}),
    );
    assert_eq!(theme.primary.hex(), "#a2c9fe");
    assert_eq!(theme.surface.hex(), "#141218");
    assert!(theme.css().contains("@define-color flow_primary #a2c9fe"));
    assert!(Color::parse("#aabbzz").is_none());
}

#[test]
fn a_failed_branch_can_be_retried_while_descendants_are_waiting() {
    let f = Fixture::new();
    let a = f.node(NodeKind::Shell, "test -f fixed");
    f.add(&a);
    let mut b = f.node(NodeKind::Shell, "printf 'continued'");
    b.dependencies.push(a.id.clone());
    f.add(&b);
    f.run(&a.id, true);
    f.wait(&a.id, Status::Failed);
    fs::write(f.dir.path().join("fixed"), "").unwrap();
    f.run(&a.id, true);
    f.wait(&b.id, Status::Done);
}

#[test]
fn human_work_survives_start_and_a_handoff_note() {
    let f = Fixture::new();
    let human = f.node(NodeKind::Human, "Instructions");
    f.add(&human);
    f.engine
        .request(Control::HumanProgress {
            id: human.id.clone(),
            text: "My working document".into(),
        })
        .unwrap();
    f.run(&human.id, false);
    f.wait(&human.id, Status::Running);
    assert_eq!(
        f.engine.snapshot().state.node(&human.id).unwrap().output,
        "My working document"
    );
    f.engine
        .request(Control::Approve {
            id: human.id.clone(),
            output: Some("Ready".into()),
        })
        .unwrap();
    let output = f.wait(&human.id, Status::Done).output;
    assert!(output.contains("My working document") && output.contains("Ready"));
}

#[test]
fn recipe_watchers_remap_endpoints_and_require_explicit_enablement() {
    let mut state = State::empty(PathBuf::from("/tmp"));
    let ws = state.selected_workspace.clone();
    let metric = Node::new(
        &ws,
        NodeKind::Metric,
        PathBuf::from("/tmp"),
        Position::default(),
    );
    let target = Node::new(
        &ws,
        NodeKind::Agent,
        PathBuf::from("/tmp"),
        Position::default(),
    );
    state.nodes = vec![metric.clone(), target.clone()];
    state.watchers.push(Watcher {
        id: id(),
        source: metric.id.clone(),
        target: target.id.clone(),
        threshold: 42.0,
        above: true,
        enabled: true,
        armed: false,
        last_triggered: Some(now()),
    });
    let recipe = state
        .recipe("Monitor and repair".into(), &[metric.id, target.id])
        .unwrap();
    let ids = state
        .instantiate(&recipe, &ws, PathBuf::from("/tmp"))
        .unwrap();
    let watcher = state.watchers.last().unwrap();
    assert_eq!((&watcher.source, &watcher.target), (&ids[0], &ids[1]));
    assert!(!watcher.enabled && watcher.armed && watcher.last_triggered.is_none());
}

#[test]
fn human_work_is_not_blocked_by_a_writer_in_the_same_directory() {
    let f = Fixture::new();
    let shell = f.node(NodeKind::Shell, "sleep 2");
    f.add(&shell);
    f.run(&shell.id, false);
    f.wait(&shell.id, Status::Running);
    let human = f.node(NodeKind::Human, "Think in parallel");
    f.add(&human);
    f.run(&human.id, false);
    f.wait(&human.id, Status::Running);
    assert_eq!(
        f.engine.snapshot().state.node(&shell.id).unwrap().status,
        Status::Running
    );
}

#[test]
fn a_data_directory_has_one_writer_even_with_different_sockets() {
    let dir = tempfile::tempdir().unwrap();
    let first = Store::new(dir.path().into()).unwrap();
    let retained = first.clone();
    assert!(Store::new(dir.path().into()).is_err());
    drop(first);
    assert!(Store::new(dir.path().into()).is_err());
    drop(retained);
    assert!(Store::new(dir.path().into()).is_ok());
}

#[test]
fn column_presentation_is_shared_and_history_clear_preserves_open_windows() {
    let f = Fixture::new();
    let mut first = f.node(NodeKind::Window, "");
    first.desktop_window = true;
    first.column = Some("column-a".into());
    let mut second = f.node(NodeKind::Window, "");
    second.desktop_window = true;
    second.column = first.column.clone();
    let mut other = f.node(NodeKind::Window, "");
    other.desktop_window = true;
    other.column = Some("column-b".into());
    other.window_id = Some(123);
    f.add(&first);
    f.add(&second);
    f.add(&other);
    f.engine
        .request(Control::ColumnMode {
            id: first.id.clone(),
            mode: ColumnMode::Window,
        })
        .unwrap();
    let state = f.engine.snapshot().state;
    assert_eq!(
        state.node(&first.id).unwrap().column_mode,
        ColumnMode::Window
    );
    assert_eq!(
        state.node(&second.id).unwrap().column_mode,
        ColumnMode::Window
    );
    assert_eq!(state.node(&other.id).unwrap().column_mode, ColumnMode::Node);
    f.engine.request(Control::ClearHistory).unwrap();
    let state = f.engine.snapshot().state;
    assert!(state.node(&first.id).is_none() && state.node(&second.id).is_none());
    assert!(state.node(&other.id).is_some());
}

#[test]
fn live_desktop_windows_accept_connections_without_changing_workflow_edit_guards() {
    let mut state = State::empty("/tmp".into());
    let source = Node::new(
        &state.selected_workspace,
        NodeKind::Shell,
        "/tmp".into(),
        Position::default(),
    );
    let mut window = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        "/tmp".into(),
        Position::default(),
    );
    window.desktop_window = true;
    window.status = Status::Running;
    let mut job = Node::new(
        &state.selected_workspace,
        NodeKind::Shell,
        "/tmp".into(),
        Position::default(),
    );
    job.status = Status::Running;
    state
        .nodes
        .extend([source.clone(), window.clone(), job.clone()]);
    state.connect(&source.id, &window.id).unwrap();
    assert!(state.connect(&source.id, &job.id).is_err());
    state.remove(&window.id).unwrap();
    state.validate().unwrap();
}

#[test]
fn deleting_a_running_node_cancels_its_process_before_removing_the_node() {
    let f = Fixture::new();
    let node = f.node(NodeKind::Shell, "sleep 30");
    f.add(&node);
    f.run(&node.id, false);
    f.wait(&node.id, Status::Running);
    f.engine
        .request(Control::Delete {
            id: node.id.clone(),
        })
        .unwrap();
    let start = Instant::now();
    while f.engine.snapshot().state.node(&node.id).is_some() {
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "Deletion did not finish after cancellation"
        );
        thread::sleep(Duration::from_millis(20));
    }
    f.engine.snapshot().state.validate().unwrap();
    // A later writer in the same directory can start after cancellation released its lock.
    let next = f.node(NodeKind::Shell, "printf done");
    f.add(&next);
    f.run(&next.id, false);
    f.wait(&next.id, Status::Done);
}
