//! Node-only rows follow runtime graph edits without depending on GTK frames or desktop events.
mod support;
use flowwm::{
    client::Action,
    model::{Node, NodeKind, RowView, State},
};
use niri_ipc::Request;
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    sync::{Arc, Mutex},
};

#[test]
fn desktop_provider_publishes_row_content_when_the_last_node_is_deleted_and_restored() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("niri.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let contents = Arc::new(Mutex::new(None));
    let received = contents.clone();
    std::thread::spawn(move || {
        let mut peers = Vec::new();
        for mut stream in listener.incoming().flatten() {
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            match serde_json::from_str::<Request>(&line).unwrap() {
                Request::SetNodeRowContents { workspace_ids } => {
                    *received.lock().unwrap() = Some(workspace_ids);
                    writeln!(stream, "{}", json!({"Ok":"Handled"})).unwrap();
                }
                Request::EventStream => {
                    writeln!(stream, "{}", json!({"Ok":"Handled"})).unwrap();
                    let workspace = json!({"id":1,"idx":1,"name":null,"output":"TEST","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":null});
                    writeln!(
                        stream,
                        "{}",
                        json!({"WorkspacesChanged":{"workspaces":[workspace]}})
                    )
                    .unwrap();
                    writeln!(stream, "{}", json!({"WindowsChanged":{"windows":[]}})).unwrap();
                    peers.push(stream);
                }
                _ => writeln!(stream, "{}", json!({"Ok":"Handled"})).unwrap(),
            }
        }
    });

    let mut state = State::empty("/tmp".into());
    state.workspaces[0].view = RowView::Windows;
    let workflow = state.selected_workspace.clone();
    let node = Node::new(
        &workflow,
        NodeKind::Shell,
        "/tmp".into(),
        Default::default(),
    );
    let id = node.id.clone();
    state.nodes.push(node);
    let (desktop, _) = support::fixture_with_niri(state, Some(&socket));
    support::wait(|| *contents.lock().unwrap() == Some(vec![1]));
    desktop.request(Action::Delete { id }).unwrap();
    support::wait(|| *contents.lock().unwrap() == Some(vec![]));
    desktop.request(Action::Undo).unwrap();
    support::wait(|| *contents.lock().unwrap() == Some(vec![1]));
    desktop.request(Action::Redo).unwrap();
    support::wait(|| *contents.lock().unwrap() == Some(vec![]));
    desktop.shutdown();
}
