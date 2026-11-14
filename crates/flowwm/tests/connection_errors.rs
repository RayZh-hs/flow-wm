mod support;

use flowwm::{
    catalog::Catalog,
    client::{Action, Presentation, ViewSnapshot},
    extensions,
    graph::*,
    model::*,
};
use std::sync::Arc;

fn node(state: &mut State, kind: NodeKind) -> String {
    let node = Node::new(
        &state.selected_workspace,
        kind,
        "/tmp".into(),
        Position::default(),
    );
    let id = node.id.clone();
    state.nodes.push(node);
    id
}

fn edge(source: &str, output: &str, target: &str, input: &str) -> Connection {
    Connection {
        source: Endpoint {
            node: source.into(),
            pin: output.into(),
        },
        target: Endpoint {
            node: target.into(),
            pin: input.into(),
        },
    }
}

#[test]
fn connection_search_checks_any_specialization_in_both_directions() {
    let mut state = State::empty("/tmp".into());
    let split = node(&mut state, NodeKind::BreakSignal);
    let display = node(&mut state, NodeKind::Display);
    let alert = node(&mut state, NodeKind::Alert);
    connect(&mut state, edge(&display, "value", &alert, "message")).unwrap();
    let snapshot = ViewSnapshot {
        state: Arc::new(state),
        revision: 1,
        error: None,
    };
    let lookup = |node: &str, pin: &str, reverse| {
        Catalog::new(
            &snapshot,
            Presentation {
                lookup_node: Some(node.into()),
                lookup_pin: Some(pin.into()),
                lookup_input: reverse,
                ..Default::default()
            },
            &extensions::builtins(),
        )
        .lookup
        .unwrap()
        .outputs
        .remove(0)
        .targets
    };
    let targets = lookup(&split, "args", false);
    assert!(targets.iter().any(|t| t.create && t.id == "display"));
    assert!(!targets.iter().any(|t| t.id == display));
    assert!(
        !targets
            .iter()
            .any(|t| t.id == alert && t.input == "message")
    );
    let sources = lookup(&display, "value", true);
    assert!(!sources.iter().any(|t| t.id == split && t.input == "args"));
    assert!(
        !sources
            .iter()
            .any(|t| t.create && t.id == "break_signal" && t.input == "args")
    );
    assert!(sources.iter().any(|t| t.create && t.id == "constant"));
}

#[test]
fn records_pass_through_any_pins_but_cannot_be_alert_messages() {
    let mut state = State::empty("/tmp".into());
    let split = node(&mut state, NodeKind::BreakSignal);
    let display = node(&mut state, NodeKind::Display);
    let tail = node(&mut state, NodeKind::Display);
    let alert = node(&mut state, NodeKind::Alert);
    connect(&mut state, edge(&split, "args", &display, "value")).unwrap();
    connect(&mut state, edge(&display, "value", &tail, "value")).unwrap();
    for id in [&display, &tail] {
        assert_eq!(
            pins(&state, state.node(id).unwrap(), true)[0].ty,
            Type::record()
        );
    }
    let before = state.connections.clone();
    let error = connect(&mut state, edge(&split, "args", &alert, "message"))
        .unwrap_err()
        .to_string();
    assert_eq!(state.connections, before);
    assert!(
        error.contains("args") && error.contains("message"),
        "{error}"
    );
    assert!(
        error.contains("String") && !error.contains("fields:"),
        "{error}"
    );
}

#[test]
fn rejected_async_connections_are_reported_once_and_can_be_retried() {
    let mut state = State::empty("/tmp".into());
    let split = node(&mut state, NodeKind::BreakSignal);
    let alert = node(&mut state, NodeKind::Alert);
    let display = node(&mut state, NodeKind::Display);
    let (engine, _) = support::fixture(state);
    let invalid = edge(&split, "args", &alert, "message");
    let mut previous = None;
    for _ in 0..2 {
        engine.dispatch(Action::ConnectPins {
            connection: invalid.clone(),
        });
        support::wait(|| engine.snapshot().error.is_some());
        let error = engine.take_error().expect("The failure must be presented");
        assert!(
            error.contains("args") && error.contains("message"),
            "{error}"
        );
        if let Some(previous) = &previous {
            assert_eq!(&error, previous);
        }
        previous = Some(error);
        engine.wake_ui();
        assert!(engine.take_error().is_none());
        assert!(engine.snapshot().error.is_none());
        assert!(engine.snapshot().state.connections.is_empty());
    }
    engine
        .request(Action::ConnectPins {
            connection: edge(&split, "args", &display, "value"),
        })
        .unwrap();
    assert_eq!(engine.snapshot().state.connections.len(), 1);
    assert!(engine.take_error().is_none());
    engine.shutdown();
}
