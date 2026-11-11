//! Flow Orchestrator Protocol v2 and Flow Signal Protocol v1.
use crate::domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const ORCHESTRATOR_VERSION: u32 = 2;
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
    Run {
        workflow: WorkflowId,
        node: NodeId,
        invocation: InvocationId,
        args: Value,
    },
    Interrupt {
        invocation: InvocationId,
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
