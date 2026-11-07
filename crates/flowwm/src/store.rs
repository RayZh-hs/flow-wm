use crate::model::{State, Status};
use anyhow::{Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct Store {
    directory: PathBuf,
    _lock: Arc<File>,
}
impl Store {
    pub fn new(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory).context("Create FlowWM data directory")?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("state.lock"))?;
        // Keep one writer even when a second instance uses a different control socket.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            anyhow::bail!(
                "This FlowWM data directory is already in use: {}",
                directory.display()
            );
        }
        Ok(Self {
            directory,
            _lock: Arc::new(lock),
        })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn load(&self, cwd: PathBuf) -> Result<State> {
        let path = self.directory.join("state.json");
        let mut state = match fs::read(&path) {
            Ok(data) => serde_json::from_slice::<State>(&data)
                .context("Read FlowWM state (the original file has been preserved)")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::empty(cwd),
            Err(e) => return Err(e.into()),
        };
        state.validate()?;
        restore_window_bindings(&mut state, std::env::var("NIRI_SOCKET").ok().as_deref());
        for node in &mut state.nodes {
            if !node.desktop_window && matches!(node.status, Status::Running | Status::Queued) {
                node.status = Status::Stopped;
                node.log(
                    "system",
                    "FlowWM restarted. Run this node again when ready.",
                );
            }
        }
        Ok(state)
    }
    pub fn save(&self, state: &State) -> Result<()> {
        state.validate()?;
        let bytes = serde_json::to_vec_pretty(state)?;
        let path = self.directory.join("state.json");
        let temporary = self.directory.join("state.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

fn restore_window_bindings(state: &mut State, session: Option<&str>) {
    let groups: std::collections::HashSet<_> = state
        .workspaces
        .iter()
        .filter(|w| {
            w.row
                .as_ref()
                .is_some_and(|r| Some(r.session.as_str()) == session)
        })
        .map(|w| w.id.clone())
        .collect();
    for node in &mut state.nodes {
        if !groups.contains(&node.workspace) {
            node.window_id = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeKind, Position, Row};
    #[test]
    fn live_ids_survive_shell_restart_but_are_discarded_in_a_new_compositor_session() {
        let mut state = State::empty("/tmp".into());
        state.workspaces[0].row = Some(Row {
            session: "niri-A".into(),
            id: 1,
            index: 1,
            output: None,
            name: None,
        });
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            "/tmp".into(),
            Position::default(),
        );
        node.window_id = Some(123);
        state.nodes.push(node);
        restore_window_bindings(&mut state, Some("niri-A"));
        assert_eq!(state.nodes[0].window_id, Some(123));
        restore_window_bindings(&mut state, Some("niri-B"));
        assert_eq!(state.nodes[0].window_id, None);
    }
}
