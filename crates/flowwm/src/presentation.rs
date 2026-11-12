//! WM-owned layout, preferences and recipes. Native bindings belong to the desktop provider.
use crate::model::State;
use anyhow::{Result, ensure};
use flow_core::domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PresentationDocument {
    pub selected_workflow: WorkflowId,
    #[serde(default)]
    pub recipes: Vec<crate::model::Recipe>,
    pub workflows: BTreeMap<WorkflowId, Value>,
    pub nodes: BTreeMap<NodeId, Value>,
}
impl PresentationDocument {
    pub fn capture(state: &State) -> Self {
        Self {
            selected_workflow: state.selected_workspace.clone().into(),
            recipes:state.recipes.clone(),
            workflows: state.workspaces.iter().map(|row| (row.id.clone().into(),
                serde_json::json!({"cwd":row.cwd,"notes":row.notes,"view":row.view,"window_array":row.window_array,"graph_arranged":row.graph_arranged}))).collect(),
            nodes: state.nodes.iter().map(|node| (node.id.clone().into(),
                serde_json::json!({"position":node.position,"column":node.column,"column_mode":node.column_mode}))).collect(),
        }
    }
    pub fn apply(&self, state: &mut State) -> Result<()> {
        state.recipes = self.recipes.clone();
        for row in &mut state.workspaces {
            if let Some(value) = self.workflows.get(&row.id.clone().into()) {
                if let Some(cwd) = value.get("cwd") {
                    row.cwd = serde_json::from_value(cwd.clone())?;
                }
                row.notes = value["notes"].as_str().unwrap_or_default().into();
                row.view = serde_json::from_value(value["view"].clone())?;
                row.window_array = serde_json::from_value(value["window_array"].clone())?;
                row.graph_arranged = serde_json::from_value(value["graph_arranged"].clone())?;
            }
        }
        for node in &mut state.nodes {
            if let Some(value) = self.nodes.get(&node.id.clone().into()) {
                node.position = serde_json::from_value(value["position"].clone())?;
                node.column = serde_json::from_value(value["column"].clone())?;
                node.column_mode = serde_json::from_value(value["column_mode"].clone())?;
            }
        }
        if state
            .workspaces
            .iter()
            .any(|w| w.id == self.selected_workflow.0)
        {
            state.selected_workspace = self.selected_workflow.0.clone();
        }
        Ok(())
    }
}

pub struct Writer {
    directory: PathBuf,
    _lock: File,
    last_saved: Mutex<Option<Vec<u8>>>,
}
impl Writer {
    pub fn new(directory: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("presentation.lock"))?;
        ensure!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Another WM owns presentation in {}",
            directory.display()
        );
        Ok(Self {
            directory,
            _lock: lock,
            last_saved: Mutex::new(None),
        })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn load(&self, state: &mut State) -> Result<()> {
        let path = self.directory.join("presentation.json");
        if path.exists() {
            let bytes = std::fs::read(path)?;
            let document: PresentationDocument = serde_json::from_slice(&bytes)?;
            document.apply(state)?;
            *self.last_saved.lock().unwrap() = Some(serde_json::to_vec_pretty(&document)?);
        }
        Ok(())
    }
    pub fn save(&self, state: &State) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&PresentationDocument::capture(state))?;
        let mut last_saved = self.last_saved.lock().unwrap();
        // Native titles and runtime activity do not change presentation. In
        // particular, terminal title animations must never trigger an fsync.
        if last_saved.as_ref() == Some(&bytes) {
            return Ok(());
        }
        let temporary = self.directory.join("presentation.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(temporary, self.directory.join("presentation.json"))?;
        File::open(&self.directory)?.sync_all()?;
        *last_saved = Some(bytes);
        Ok(())
    }
}
