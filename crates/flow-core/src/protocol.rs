//! Flow Orchestrator Protocol v3 and Flow Signal Protocol v1.
use crate::domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const ORCHESTRATOR_VERSION: u32 = 3;
pub const SIGNAL_VERSION: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub protocol: u32,
    pub id: String,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub caller: Option<WorkflowId>,
    pub command: Command,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    RowEnvironment {
        workflow: WorkflowId,
    },
    RowSettings {
        workflow: WorkflowId,
    },
    SetRowSettings {
        workflow: WorkflowId,
        settings: crate::environment::Env,
    },
    Info,
    UnregisterApplication {
        node: NodeId,
        token: String,
        incarnation: String,
    },
    RegisterApplication {
        registration: crate::providers::ApplicationRegistration,
    },
    ApplicationEvent {
        node: NodeId,
        token: String,
        incarnation: String,
        pin: String,
        delivery: DeliveryId,
        args: Value,
    },
    ActivateSession {
        workspace: WorkspaceId,
        provider: ProviderId,
        incarnation: String,
    },
    EndSession {
        workspace: WorkspaceId,
        incarnation: String,
    },
    Publish {
        workflow: WorkflowId,
        node: NodeId,
        values: crate::graph::Record,
    },
    Emit {
        workflow: WorkflowId,
        node: NodeId,
        pin: String,
        delivery: DeliveryId,
        args: Value,
    },

    OriginInterrupt {
        invocation: InvocationId,
        generation: u64,
    },
    StageTransfer {
        bundle: crate::transfer::TransferBundle,
    },
    CommitTransfer {
        bundle: crate::transfer::TransferBundle,
    },
    ForwardCompletion {
        execution: Execution,
    },
    Negotiate {
        versions: Vec<u32>,
    },
    Snapshot,
    Workspaces,
    Subscribe {
        after: Option<u64>,
    },
    Create {
        workflow: Workflow,
    },
    Destroy {
        workflow: WorkflowId,
    },
    Move {
        workflow: WorkflowId,
        workspace: WorkspaceId,
    },
    MakePublic {
        workflow: WorkflowId,
    },
    Reference {
        workflow: WorkflowId,
        target: WorkflowId,
    },
    Unreference {
        workflow: WorkflowId,
        target: WorkflowId,
    },
    Read {
        workflow: WorkflowId,
        node: NodeId,
        evaluation: InvocationId,
    },
    Run {
        workflow: WorkflowId,
        node: NodeId,
        invocation: InvocationId,
        args: Value,
    },
    Interrupt {
        invocation: InvocationId,
    },
    Resolve {
        invocation: InvocationId,
        accepted: bool,
        result: Value,
    },
    RelocateNodes {
        source: WorkflowId,
        target: WorkflowId,
        nodes: Vec<NodeId>,
    },
    ReplaceGraph {
        workflow: WorkflowId,
        graph: FlowNet,
    },
    Broadcast {
        delivery: Delivery,
    },
    Receive {
        workflow: WorkflowId,
        channel: String,
    },
    Acknowledge {
        delivery: DeliveryId,
    },
    PrepareTransfer {
        transfer: TransferId,
        workflow: WorkflowId,
        destination: WorkspaceId,
    },
    AbortTransfer {
        transfer: TransferId,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub protocol: u32,
    pub id: String,
    pub revision: u64,
    pub result: Option<Value>,
    pub error: Option<Error>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub protocol: u32,
    pub id: String,
    pub revision: u64,
    pub snapshot: RuntimeState,
}

/// Flow Signal Protocol has its own envelope and version negotiation boundary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalFrame {
    pub signal_protocol: u32,
    pub id: String,
    pub delivery: Delivery,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalReply {
    pub signal_protocol: u32,
    pub id: String,
    pub revision: u64,
    pub accepted: bool,
    pub error: Option<Error>,
}
