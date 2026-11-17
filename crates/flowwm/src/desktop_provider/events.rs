//! Superseded snapshots must not delay window closes or application operations.
use super::Event;
use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex},
};

#[derive(Default)]
pub(super) struct Queue {
    pending: Mutex<VecDeque<Event>>,
    ready: Condvar,
}

impl Queue {
    pub(super) fn send(&self, event: Event) {
        let mut pending = self.pending.lock().unwrap();
        // Replace adjacent metadata snapshots from the same source. Preserve
        // window lifetime/row transitions, mutations and process exits: they
        // can retire an attachment before its replacement is reconciled.
        match (pending.back_mut(), event) {
            (Some(Event::Desktop(previous)), Event::Desktop(latest))
                if previous.session == latest.session
                    && previous.connected == latest.connected
                    && previous.session_ended == latest.session_ended
                    && previous
                        .windows
                        .iter()
                        .map(|w| (w.id, w.pid, w.workspace_id))
                        .eq(latest.windows.iter().map(|w| (w.id, w.pid, w.workspace_id))) =>
            {
                *previous = latest;
            }
            (Some(Event::Runtime(previous)), Event::Runtime(latest)) => *previous = latest,
            (_, event) => pending.push_back(event),
        }
        self.ready.notify_one();
    }

    pub(super) fn recv(&self) -> Event {
        let pending = self.pending.lock().unwrap();
        let mut pending = self
            .ready
            .wait_while(pending, |pending| pending.is_empty())
            .unwrap();
        pending.pop_front().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::niri::Desktop;
    use flow_core::domain::RuntimeState;
    use std::sync::mpsc;

    #[test]
    fn a_snapshot_burst_keeps_only_the_latest_desktop_and_runtime() {
        let queue = Queue::default();
        for i in 0..10_000 {
            queue.send(Event::Desktop(Desktop {
                session: "desktop".into(),
                error: Some(format!("title-{i}")),
                ..Default::default()
            }));
        }
        for revision in 0..10_000 {
            let mut runtime = RuntimeState::local();
            runtime.revision = revision;
            queue.send(Event::Runtime(Box::new(runtime)));
        }
        assert_eq!(queue.pending.lock().unwrap().len(), 2);
        let Event::Desktop(desktop) = queue.recv() else {
            panic!("expected desktop snapshot");
        };
        assert_eq!(desktop.error.as_deref(), Some("title-9999"));
        let Event::Runtime(runtime) = queue.recv() else {
            panic!("expected runtime snapshot");
        };
        assert_eq!(runtime.revision, 9999);
    }

    #[test]
    fn mutations_exits_and_other_sources_preserve_snapshot_order() {
        let queue = Queue::default();
        let snapshot = |label: &str| {
            Event::Desktop(Desktop {
                session: "desktop".into(),
                error: Some(label.into()),
                ..Default::default()
            })
        };
        let (reply, _) = mpsc::channel();
        queue.send(snapshot("before-delete"));
        queue.send(Event::Detach {
            node: "window".into(),
            close_window: true,
            reply,
        });
        queue.send(snapshot("after-delete"));
        queue.send(Event::Exit(1, Ok(0)));
        queue.send(snapshot("after-exit"));
        queue.send(Event::Runtime(Box::new(RuntimeState::local())));
        queue.send(snapshot("after-runtime"));

        assert_eq!(queue.pending.lock().unwrap().len(), 7);
        for expected in [
            "before-delete",
            "after-delete",
            "after-exit",
            "after-runtime",
        ] {
            let Event::Desktop(desktop) = queue.recv() else {
                panic!("expected desktop snapshot before {expected}");
            };
            assert_eq!(desktop.error.as_deref(), Some(expected));
            match expected {
                "before-delete" => assert!(matches!(queue.recv(), Event::Detach { .. })),
                "after-delete" => assert!(matches!(queue.recv(), Event::Exit(1, Ok(0)))),
                "after-exit" => assert!(matches!(queue.recv(), Event::Runtime(_))),
                _ => (),
            }
        }
    }

    #[test]
    fn compositor_disconnects_and_session_changes_are_not_superseded() {
        let queue = Queue::default();
        let mut desktop = Desktop {
            session: "first".into(),
            connected: true,
            ..Default::default()
        };
        queue.send(Event::Desktop(desktop.clone()));
        desktop.connected = false;
        queue.send(Event::Desktop(desktop.clone()));
        desktop.session_ended = true;
        queue.send(Event::Desktop(desktop.clone()));
        desktop.connected = true;
        desktop.session_ended = false;
        queue.send(Event::Desktop(desktop.clone()));
        desktop.session = "second".into();
        queue.send(Event::Desktop(desktop));

        assert_eq!(queue.pending.lock().unwrap().len(), 5);
        for expected in [
            ("first", true, false),
            ("first", false, false),
            ("first", false, true),
            ("first", true, false),
            ("second", true, false),
        ] {
            let Event::Desktop(desktop) = queue.recv() else {
                panic!("expected compositor transition");
            };
            assert_eq!(
                (
                    desktop.session.as_str(),
                    desktop.connected,
                    desktop.session_ended
                ),
                expected
            );
        }
    }

    #[test]
    fn closing_and_reopening_a_window_preserves_the_intermediate_detachment() {
        let queue = Queue::default();
        let window = |id| {
            serde_json::from_value(serde_json::json!({
                "id": id, "title": "Tray app", "app_id": "test.tray", "pid": 100,
                "workspace_id": 1, "is_focused": false, "is_floating": false,
                "is_urgent": false,
                "layout": {
                    "pos_in_scrolling_layout": [1, 1], "tile_size": [800, 600],
                    "window_size": [800, 600], "tile_pos_in_workspace_view": [0, 0],
                    "window_offset_in_tile": [0, 0]
                },
                "focus_timestamp": null
            }))
            .unwrap()
        };
        let mut desktop = Desktop {
            session: "desktop".into(),
            connected: true,
            windows: vec![window(1)],
            ..Default::default()
        };
        queue.send(Event::Desktop(desktop.clone()));
        desktop.windows.clear();
        queue.send(Event::Desktop(desktop.clone()));
        desktop.windows.push(window(2));
        queue.send(Event::Desktop(desktop));

        assert_eq!(queue.pending.lock().unwrap().len(), 3);
        for expected in [vec![1], vec![], vec![2]] {
            let Event::Desktop(desktop) = queue.recv() else {
                panic!("expected window lifetime transition");
            };
            assert_eq!(
                desktop.windows.iter().map(|w| w.id).collect::<Vec<_>>(),
                expected
            );
        }
    }
}
