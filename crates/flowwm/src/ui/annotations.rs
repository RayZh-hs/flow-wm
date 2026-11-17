use super::{
    canvas::{self, View},
    collapse::Rect,
};
use crate::{
    annotations::{Annotation, Content},
    model::State,
};
use gtk::{cairo::Context, pango};

pub fn rect(item: &Annotation, state: &State, view: &View) -> Option<Rect> {
    if let Some(preview) = view
        .annotation_preview
        .as_ref()
        .filter(|(id, _)| *id == item.id)
    {
        return Some(preview.1);
    }
    match &item.content {
        Content::Text { .. } => Some(Rect {
            x: item.position.x,
            y: item.position.y,
            width: item.width,
            height: item.height,
        }),
        Content::Group { nodes } => {
            let nodes: Vec<_> = nodes
                .iter()
                .filter_map(|id| state.node(id))
                .filter(|node| node.workspace == state.selected_workspace)
                .collect();
            if nodes.is_empty() {
                return None;
            }
            let left = nodes
                .iter()
                .map(|node| view.position(node).x)
                .fold(f64::INFINITY, f64::min)
                - 24.;
            let top = nodes
                .iter()
                .map(|node| view.position(node).y)
                .fold(f64::INFINITY, f64::min)
                - 52.;
            let right = nodes
                .iter()
                .map(|node| view.position(node).x + canvas::node_width(node))
                .fold(f64::NEG_INFINITY, f64::max)
                + 24.;
            let bottom = nodes
                .iter()
                .map(|node| view.position(node).y + canvas::node_height(node))
                .fold(f64::NEG_INFINITY, f64::max)
                + 24.;
            Some(Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            })
        }
    }
}
pub fn hit<'a>(state: &'a State, view: &View, x: f64, y: f64) -> Option<&'a Annotation> {
    state.workspace().canvas.iter().rev().find(|item| {
        rect(item, state, view)
            .is_some_and(|r| x >= r.x && y >= r.y && x <= r.x + r.width && y <= r.y + r.height)
    })
}
pub fn draw(c: &Context, state: &State, view: &View) {
    let theme = &view.theme;
    for item in &state.workspace().canvas {
        let Some(r) = rect(item, state, view) else {
            continue;
        };
        canvas::rounded(c, r.x, r.y, r.width, r.height, theme.radius);
        let text = matches!(item.content, Content::Text { .. });
        if text {
            theme.surface_container.alpha(c, 0.96);
        } else {
            theme.primary.alpha(c, 0.055);
        }
        let _ = c.fill_preserve();
        let selected = view.annotation.as_ref() == Some(&item.id);
        theme.primary.alpha(c, if selected { 0.95 } else { 0.35 });
        c.set_line_width(if selected { 2. } else { 1. });
        let _ = c.stroke();
        let _ = c.save();
        c.rectangle(r.x + 12., r.y + 4., r.width - 24., r.height - 12.);
        c.clip();
        canvas::text(
            c,
            r.x + 14.,
            r.y + 23.,
            &item.title,
            11.,
            theme.primary.rgb(),
            false,
        );
        if let Content::Text { text } = &item.content {
            let layout = canvas::text_layout(
                c,
                if text.is_empty() {
                    "Double-click to write…"
                } else {
                    text
                },
                13.,
                false,
            );
            layout.set_single_paragraph_mode(false);
            layout.set_width(((r.width - 28.) * pango::SCALE as f64) as i32);
            layout.set_wrap(pango::WrapMode::WordChar);
            theme.text.source(c);
            c.move_to(r.x + 14., r.y + 40.);
            pangocairo::functions::show_layout(c, &layout);
        }
        let _ = c.restore();
        if text {
            theme.primary.alpha(c, 0.6);
            c.set_line_width(1.5);
            for d in [6., 11.] {
                c.move_to(r.x + r.width - d - 4., r.y + r.height - 5.);
                c.line_to(r.x + r.width - 5., r.y + r.height - d - 4.);
            }
            let _ = c.stroke();
        }
    }
}
