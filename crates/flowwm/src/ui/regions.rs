//! Owned and referenced workflows use the existing canvas theme and camera.
use super::{
    canvas::{self, View},
    collapse::Rect,
};
use crate::model::State;
use flow_core::domain::WorkflowId;
use gtk::cairo::Context;
pub struct Region {
    pub target: String,
    pub name: String,
    pub workspace: String,
    pub owned: bool,
    pub nodes: usize,
    pub rect: Rect,
}
pub fn layout(state: &State, view: &View) -> Vec<Region> {
    let Some(runtime) = &state.orchestration else {
        return vec![];
    };
    let current = WorkflowId(state.selected_workspace.clone());
    let Some(parent) = runtime.workflows.get(&current) else {
        return vec![];
    };
    let bottom = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
        .map(|n| view.position(n).y + canvas::node_height(n))
        .fold(20., f64::max)
        + 60.;
    runtime
        .workflows
        .values()
        .filter(|w| w.owner.as_ref() == Some(&current) || parent.references.contains(&w.id))
        .enumerate()
        .map(|(index, w)| Region {
            target: w.id.0.clone(),
            name: w.name.clone(),
            workspace: w
                .workspace
                .as_ref()
                .map(|w| w.0.clone())
                .unwrap_or_else(|| "Unassigned".into()),
            owned: w.owner.as_ref() == Some(&current),
            nodes: w.graph.nodes.len(),
            rect: Rect {
                x: 100. + (index % 2) as f64 * 360.,
                y: bottom + (index / 2) as f64 * 164.,
                width: 320.,
                height: 124.,
            },
        })
        .collect()
}
pub fn hit(state: &State, view: &View, x: f64, y: f64) -> Option<String> {
    layout(state, view)
        .into_iter()
        .find(|r| {
            x >= r.rect.x
                && x <= r.rect.x + r.rect.width
                && y >= r.rect.y
                && y <= r.rect.y + r.rect.height
        })
        .map(|r| r.target)
}
pub fn draw(c: &Context, state: &State, view: &View) {
    for region in layout(state, view) {
        let r = region.rect;
        let theme = &view.theme;
        canvas::rounded(c, r.x, r.y, r.width, r.height, theme.radius);
        theme
            .primary
            .alpha(c, if region.owned { 0.09 } else { 0.035 });
        let _ = c.fill_preserve();
        if region.owned {
            theme.primary.alpha(c, 0.5);
        } else {
            theme.outline.source(c);
            c.set_dash(&[5., 4.], 0.);
        }
        c.set_line_width(1.);
        let _ = c.stroke();
        c.set_dash(&[], 0.);
        canvas::text(
            c,
            r.x + 18.,
            r.y + 25.,
            &format!(
                "{}{}",
                if region.owned { "&" } else { "" },
                region.workspace
            ),
            10.,
            theme.primary.rgb(),
            true,
        );
        let title: String = region.name.chars().take(35).collect();
        canvas::text(
            c,
            r.x + 18.,
            r.y + 53.,
            &title,
            15.,
            theme.text.rgb(),
            false,
        );
        canvas::text(
            c,
            r.x + 18.,
            r.y + 82.,
            &format!(
                "{} nodes · {}",
                region.nodes,
                if region.owned {
                    "Owned workflow"
                } else {
                    "Reference"
                }
            ),
            11.,
            theme.muted.rgb(),
            false,
        );
        canvas::text(
            c,
            r.x + 18.,
            r.y + 105.,
            "Double-click to open",
            10.,
            theme.muted.rgb(),
            false,
        );
    }
}
