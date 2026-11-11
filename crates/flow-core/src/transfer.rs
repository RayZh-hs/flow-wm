//! Recoverable orchestration handoff. Execution handles always retain their origin.
use crate::domain::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferBundle {
    pub transfer: Transfer,
    pub workflow: Workflow,
    pub executions: Vec<Execution>,
    pub deliveries: Vec<Delivery>,
    pub ancestors: Vec<Workflow>,
    pub workspaces: Vec<WorkspaceDefinition>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForwardedCompletion {
    pub destination: HostId,
    pub execution: Execution,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteCommand {
    pub host: HostId,
    #[serde(default)]
    pub caller: Option<WorkflowId>,
    pub command: crate::protocol::Command,
}
impl RuntimeState {
    pub fn transfer_bundle(&self, id: &TransferId) -> Result<TransferBundle> {
        let transfer = self.transfers.get(id).context("Transfer not found")?;
        let workflow = self
            .workflows
            .get(&transfer.workflow)
            .context("Workflow was destroyed during handoff")?;
        let mut ancestors = vec![];
        let mut parent = workflow.owner.as_ref();
        while let Some(id) = parent {
            let ancestor = self.workflows.get(id).context("Missing owning workflow")?;
            let mut shadow = ancestor.clone();
            shadow.graph = Default::default();
            shadow.references.clear();
            ancestors.push(shadow);
            parent = ancestor.owner.as_ref();
        }
        Ok(TransferBundle {
            ancestors,
            workspaces: self.workspaces.definitions().cloned().collect(),
            transfer: transfer.clone(),
            workflow: workflow.clone(),
            executions: self
                .executions
                .values()
                .filter(|e| e.workflow == workflow.id)
                .cloned()
                .collect(),
            deliveries: self
                .deliveries
                .iter()
                .filter(|d| d.target == workflow.id)
                .cloned()
                .collect(),
        })
    }
    pub fn stage_transfer(&mut self, bundle: TransferBundle) -> Result<()> {
        let transfer = &bundle.transfer;
        ensure!(
            transfer.destination.host() == self.host_id.0,
            "Transfer is addressed to another host"
        );
        self.workspaces.require_active(&transfer.destination)?;
        ensure!(
            bundle.workflow.id == transfer.workflow && bundle.workflow.authority == transfer.source,
            "Invalid transfer source"
        );
        ensure!(
            !self.destroyed.contains(&transfer.workflow),
            "Workflow has a destruction tombstone"
        );
        if let Some(existing) = self.transfers.get(&transfer.id) {
            ensure!(
                existing.workflow == transfer.workflow
                    && existing.destination == transfer.destination,
                "Transfer ID reused"
            );
            return Ok(());
        }
        ensure!(
            !self.workflows.contains_key(&transfer.workflow),
            "Destination already has this workflow identity"
        );
        bundle.workflow.graph.validate()?;
        let mut accepted = transfer.clone();
        accepted.phase = TransferPhase::Accepted;
        self.transfers.insert(transfer.id.clone(), accepted);
        self.incoming.insert(transfer.id.clone(), bundle);
        self.revision += 1;
        Ok(())
    }
    pub fn commit_outgoing(&mut self, id: &TransferId) -> Result<TransferBundle> {
        let transfer = self.transfers.get_mut(id).context("Transfer not found")?;
        ensure!(
            matches!(
                transfer.phase,
                TransferPhase::Prepared | TransferPhase::Committed
            ),
            "Transfer cannot be committed"
        );
        let workflow = self
            .workflows
            .get_mut(&transfer.workflow)
            .context("Workflow was destroyed during handoff")?;
        transfer.phase = TransferPhase::Committed;
        workflow.authority = transfer.destination.host().into();
        workflow.workspace = Some(transfer.destination.clone());
        workflow.transfer = None;
        for execution in self
            .executions
            .values()
            .filter(|e| e.workflow == workflow.id && e.host == self.host_id)
        {
            self.forwarding_targets
                .insert(execution.id.clone(), workflow.authority.clone());
        }
        self.revision += 1;
        self.transfer_bundle(id)
    }
    pub fn commit_incoming(&mut self, bundle: TransferBundle) -> Result<()> {
        let transfer = &bundle.transfer;
        ensure!(
            transfer.phase == TransferPhase::Committed,
            "Source has not committed authority"
        );
        ensure!(
            transfer.destination.host() == self.host_id.0,
            "Wrong destination host"
        );
        ensure!(
            !self.destroyed.contains(&transfer.workflow),
            "Workflow was destroyed during handoff"
        );
        let record = self
            .transfers
            .get(&transfer.id)
            .context("Transfer was not staged")?;
        if record.phase == TransferPhase::Committed {
            return Ok(());
        }
        ensure!(
            record.phase == TransferPhase::Accepted,
            "Transfer was aborted"
        );
        for definition in bundle.workspaces {
            self.workspaces.register(definition)?;
        }
        for ancestor in bundle.ancestors {
            self.workflows
                .entry(ancestor.id.clone())
                .or_insert(ancestor);
        }
        let mut workflow = bundle.workflow;
        workflow.authority = self.host_id.clone();
        workflow.workspace = Some(transfer.destination.clone());
        workflow.transfer = None;
        self.workflows.insert(workflow.id.clone(), workflow);
        for execution in bundle.executions {
            self.executions
                .entry(execution.id.clone())
                .or_insert(execution);
        }
        for delivery in bundle.deliveries {
            if !self.acknowledged.contains(&delivery.id)
                && !self.deliveries.iter().any(|d| d.id == delivery.id)
            {
                self.deliveries.push_back(delivery);
            }
        }
        self.transfers.insert(transfer.id.clone(), transfer.clone());
        self.incoming.remove(&transfer.id);
        self.revision += 1;
        Ok(())
    }
    pub fn abort_transfer(&mut self, id: &TransferId) -> Result<()> {
        let transfer = self.transfers.get_mut(id).context("Transfer not found")?;
        ensure!(
            transfer.phase != TransferPhase::Committed,
            "Committed handoff cannot be rolled back; reconcile with its destination"
        );
        transfer.phase = TransferPhase::Aborted;
        if let Some(workflow) = self.workflows.get_mut(&transfer.workflow) {
            workflow.transfer = None;
        }
        self.incoming.remove(id);
        self.revision += 1;
        Ok(())
    }
}
