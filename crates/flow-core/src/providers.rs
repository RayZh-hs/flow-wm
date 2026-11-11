//! Public provider interfaces. Registration is transactional and has an explicit owner.
use crate::{
    domain::*,
    graph::{Pin, Record},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Behavior {
    Lazy,
    Invoked,
    Application,
    Source,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Definition {
    pub id: String,
    pub owner: ProviderId,
    pub behavior: Behavior,
    pub inputs: Vec<Pin>,
    pub outputs: Vec<Pin>,
    pub capabilities: BTreeSet<String>,
    #[serde(default)]
    pub window: Option<WindowInterface>,
    #[serde(default)]
    pub renderer: Option<RendererContract>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowInterface {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub fields: Vec<InterfaceField>,
    #[serde(default)]
    pub actions: Vec<InterfaceAction>,
    #[serde(default)]
    pub output: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterfaceField {
    pub label: String,
    pub property: String,
    #[serde(default)]
    pub editable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterfaceAction {
    pub label: String,
    pub operation: InterfaceOperation,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum InterfaceOperation {
    Activate,
    Interrupt,
    Emit { pin: String, args: Value },
}
impl WindowInterface {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.title.trim().is_empty() && self.title.len() <= 200,
            "Interface needs a short title"
        );
        ensure!(
            self.fields.len() <= 16 && self.actions.len() <= 6,
            "Interface has too many controls"
        );
        ensure!(
            !self.fields.is_empty() || !self.actions.is_empty() || self.output.is_some(),
            "Interface must provide useful content"
        );
        for field in &self.fields {
            ensure!(
                !field.label.trim().is_empty() && !field.property.is_empty(),
                "Interface field needs a label and property"
            );
        }
        for action in &self.actions {
            ensure!(
                !action.label.trim().is_empty(),
                "Interface action needs a label"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct Invocation {
    pub id: InvocationId,
    pub workflow: WorkflowId,
    pub node: NodeId,
    pub type_id: String,
    pub properties: Record,
    pub args: Value,
    pub generation: u64,
}
#[derive(Clone, Debug)]
pub struct Completion {
    pub invocation: InvocationId,
    pub generation: u64,
    pub status: ExecutionStatus,
    pub result: Value,
}
pub type EventSink = Arc<dyn Fn(Completion) + Send + Sync>;
#[derive(Clone, Debug)]
pub struct Output {
    pub invocation: InvocationId,
    pub generation: u64,
    pub stream: String,
    pub line: String,
}
pub type OutputSink = Arc<dyn Fn(Output) + Send + Sync>;
pub trait ExecutionProvider: Send + Sync {
    fn invoke(&self, invocation: Invocation, events: EventSink) -> Result<String>;
    fn invoke_observed(
        &self,
        invocation: Invocation,
        events: EventSink,
        _output: Option<OutputSink>,
    ) -> Result<String> {
        self.invoke(invocation, events)
    }
    fn interrupt(&self, handle: &str) -> Result<()>;
    fn resolve(&self, _handle: &str, _accepted: bool, _result: Value) -> Result<()> {
        anyhow::bail!("This provider does not accept a human resolution")
    }
    fn release(&self, _handle: &str) -> Result<()> {
        Ok(())
    }
}
pub trait SessionProvider: Send + Sync {
    fn definitions(&self) -> Vec<WorkspaceDefinition>;
    fn availability(&self, workspace: &WorkspaceId) -> Result<Availability>;
}
pub trait Clock: Send + Sync {
    fn now_millis(&self) -> u64;
}
pub trait Storage: Send + Sync {
    fn save(&self, state: &RuntimeState) -> Result<()>;
}
#[derive(Default)]
pub struct Registry {
    definitions: BTreeMap<String, Definition>,
    providers: BTreeMap<ProviderId, Arc<dyn ExecutionProvider>>,
}
impl Registry {
    pub fn mount(
        &mut self,
        owner: ProviderId,
        definitions: Vec<Definition>,
        provider: Arc<dyn ExecutionProvider>,
    ) -> Result<()> {
        ensure!(
            !self.providers.contains_key(&owner),
            "Provider is already mounted"
        );
        let mut ids = BTreeSet::new();
        for definition in &definitions {
            ensure!(
                definition.owner == owner && definition.id.starts_with(&format!("{}.", owner.0)),
                "Definition must use its owner's namespace"
            );
            ensure!(
                ids.insert(&definition.id) && !self.definitions.contains_key(&definition.id),
                "Duplicate node definition"
            );
            if let Some(window) = &definition.window {
                window.validate()?;
            }
        }
        for definition in definitions {
            self.definitions.insert(definition.id.clone(), definition);
        }
        self.providers.insert(owner, provider);
        Ok(())
    }
    pub fn definition(&self, id: &str) -> Result<&Definition> {
        self.definitions
            .get(id)
            .context("Node provider is unavailable")
    }
    pub fn provider(&self, owner: &ProviderId) -> Result<Arc<dyn ExecutionProvider>> {
        self.providers
            .get(owner)
            .cloned()
            .context("Execution provider is unavailable")
    }
    pub fn unmount(
        &mut self,
        owner: &ProviderId,
        executions: &BTreeMap<InvocationId, Execution>,
    ) -> Result<Vec<Definition>> {
        ensure!(
            !executions
                .values()
                .any(|e| &e.provider == owner && e.active()),
            "Resolve active invocations before unmounting their provider"
        );
        let schemas = self
            .definitions
            .values()
            .filter(|d| &d.owner == owner)
            .cloned()
            .collect();
        self.definitions.retain(|_, d| &d.owner != owner);
        self.providers.remove(owner);
        Ok(schemas)
    }
}

/// Platform attachments expose only operations the provider can perform precisely.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationOperations {
    pub open: bool,
    pub close_window: bool,
    pub interrupt: bool,
    pub disown: bool,
}
pub trait ApplicationProvider: Send + Sync {
    fn operations(&self, handle: &str) -> Result<ApplicationOperations>;
    fn open(&self, handle: &str) -> Result<()>;
    fn close_window(&self, handle: &str) -> Result<()>;
    fn interrupt(&self, handle: &str) -> Result<()>;
    fn disown(&self, handle: &str) -> Result<()>;
}
