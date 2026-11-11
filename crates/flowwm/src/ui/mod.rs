mod canvas;
mod collapse;
mod empty;
mod gpu;
mod keybindings;
mod layout;
mod motion;
mod native;
mod parameters;
mod shell;
mod surface;
pub mod theme;
mod wires;
pub use shell::run;
#[derive(Clone, Copy, Default)]
pub struct Options {
    pub windowed: bool,
    pub integrated: bool,
    pub background: bool,
}

use crate::model::*;
/// An explicit example, only created with `start --example`. Nothing auto-runs.
pub fn example(state: &mut State) {
    state.workspaces[0].name = "Checkout reliability".into();
    state.workspaces[0].notes = "Investigate the checkout latency regression.\n\n• Keep the query semantics unchanged.\n• Review the patch before enabling the rollout.\n• Work on the rollout plan while the agents investigate.".into();
    let ws = state.selected_workspace.clone();
    let cwd = state.workspace().cwd.clone();
    let mut metric = Node::new(
        &ws,
        NodeKind::Metric,
        cwd.clone(),
        Position { x: 100.0, y: 80.0 },
    );
    metric.title = "Checkout latency".into();
    metric.command = "printf '{\"value\": 214}\\n'".into();
    metric.metric.unit = "ms".into();
    metric.metric.samples = [
        112., 118., 110., 126., 120., 133., 128., 154., 149., 160., 188., 184., 214.,
    ]
    .into_iter()
    .map(|value| Sample { time: now(), value })
    .collect();
    let mut analyze = Node::new(
        &ws,
        NodeKind::Agent,
        cwd.clone(),
        Position { x: 500.0, y: 80.0 },
    );
    analyze.title = "Analyze issue".into();
    analyze.command = "Investigate checkout latency. Inspect database queries and report evidence. Raise an issue if the cause is unclear.".into();
    let mut draft = Node::new(
        &ws,
        NodeKind::Agent,
        cwd.clone(),
        Position { x: 500.0, y: 320.0 },
    );
    draft.title = "Draft fix".into();
    draft.command = "Use the upstream investigation to propose a minimal patch. Explain the behavior and validation. Do not change files.".into();
    draft.dependencies = vec![analyze.id.clone()];
    let mut gate = Node::new(
        &ws,
        NodeKind::Gate,
        cwd.clone(),
        Position { x: 500.0, y: 560.0 },
    );
    gate.title = "Review the approach".into();
    gate.command =
        "Does the proposed fix address the cause without changing query semantics?".into();
    gate.dependencies = vec![draft.id.clone()];
    let mut verify = Node::new(
        &ws,
        NodeKind::Shell,
        cwd.clone(),
        Position { x: 500.0, y: 800.0 },
    );
    verify.title = "Verify changes".into();
    verify.command = "git diff --check && git status --short".into();
    verify.dependencies = vec![gate.id.clone()];
    let mut human = Node::new(&ws, NodeKind::Human, cwd, Position { x: 920.0, y: 320.0 });
    human.title = "Write the rollout plan".into();
    human.command = "Work on the rollout plan while the investigation runs.\n\n1. Define the canary cohort\n2. Set the rollback threshold\n3. Share the verification checklist".into();
    human.output = human.command.clone();
    state
        .nodes
        .extend([metric, analyze, draft, gate, verify, human]);
}
mod regions;
