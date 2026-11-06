use crate::{model::State, niri::Desktop};
use gtk::{graphene, gsk, prelude::*};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
pub struct Ghost {
    pub node: String,
    pub source: Rect,
    pub widget: gtk::Box,
}

/// Native GTK snapshots retain their render nodes while the GPU transforms them.
pub fn create(
    layer: &gtk::Fixed,
    desktop: &Desktop,
    state: &State,
    insets: [i32; 4],
    width: f64,
    height: f64,
) -> Vec<Ghost> {
    clear(layer);
    let Some(row) = desktop.workspaces.iter().find(|w| w.is_focused) else {
        return vec![];
    };
    let windows: Vec<_> = desktop
        .windows
        .iter()
        .filter(|w| w.workspace_id == Some(row.id))
        .collect();
    let focused = windows.iter().find(|w| w.is_focused).copied().or_else(|| {
        windows
            .iter()
            .find(|w| Some(w.id) == row.active_window_id)
            .copied()
    });
    let gap = 16.0;
    let mut columns = BTreeMap::new();
    for window in &windows {
        if let Some((column, _)) = window.layout.pos_in_scrolling_layout {
            columns
                .entry(column)
                .and_modify(|w: &mut f64| *w = w.max(window.layout.tile_size.0))
                .or_insert(window.layout.tile_size.0);
        }
    }
    let focused_column = focused
        .and_then(|w| w.layout.pos_in_scrolling_layout)
        .map(|p| p.0)
        .unwrap_or(1);
    let offset: f64 = columns
        .iter()
        .filter(|(i, _)| **i < focused_column)
        .map(|(_, w)| w + gap)
        .sum();
    let mut starts = HashMap::new();
    let mut x = gap - offset;
    for (column, column_width) in columns {
        starts.insert(column, x);
        x += column_width + gap;
    }
    let mut result = vec![];
    for window in windows {
        let Some(node) = state
            .nodes
            .iter()
            .find(|n| n.workspace == state.selected_workspace && n.window_id == Some(window.id))
        else {
            continue;
        };
        let (x, y) = window
            .layout
            .tile_pos_in_workspace_view
            .map(|(x, y)| (x - insets[3] as f64, y - insets[0] as f64))
            .unwrap_or_else(|| {
                let (column, tile) = window
                    .layout
                    .pos_in_scrolling_layout
                    .unwrap_or((focused_column, 1));
                let y = desktop
                    .windows
                    .iter()
                    .filter(|w| {
                        w.workspace_id == Some(row.id)
                            && w.layout
                                .pos_in_scrolling_layout
                                .is_some_and(|(c, t)| c == column && t < tile)
                    })
                    .map(|w| w.layout.tile_size.1 + gap)
                    .sum::<f64>();
                (*starts.get(&column).unwrap_or(&gap), gap + y)
            });
        let left = (x + window.layout.window_offset_in_tile.0).max(0.0);
        let top = (y + window.layout.window_offset_in_tile.1).max(0.0);
        let right = (x + window.layout.tile_size.0).min(width);
        let bottom = (y + window.layout.tile_size.1).min(height);
        if right <= left || bottom <= top {
            continue;
        }
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        widget.add_css_class("collapse-window");
        widget.set_can_target(false);
        widget.set_size_request(
            super::canvas::WIDTH as i32,
            super::canvas::node_height(node) as i32,
        );
        let title = gtk::Label::new(Some(&node.title));
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_xalign(0.0);
        title.add_css_class("node-title-small");
        widget.append(&title);
        let app = gtk::Label::new(window.app_id.as_deref());
        app.set_xalign(0.0);
        app.add_css_class("path");
        widget.append(&app);
        widget.set_opacity(0.0);
        layer.put(&widget, 0.0, 0.0);
        result.push(Ghost {
            node: node.id.clone(),
            source: Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            },
            widget,
        });
    }
    result
}

pub fn update(
    layer: &gtk::Fixed,
    ghosts: &[Ghost],
    state: &State,
    view: &super::canvas::View,
    progress: f64,
) {
    for ghost in ghosts {
        let Some(node) = state
            .node(&ghost.node)
            .filter(|n| n.workspace == state.selected_workspace)
        else {
            ghost.widget.set_visible(false);
            continue;
        };
        let position = view.position(node);
        let target = Rect {
            x: view.pan_x + position.x * view.zoom,
            y: view.pan_y + position.y * view.zoom,
            width: super::canvas::WIDTH * view.zoom,
            height: super::canvas::node_height(node) * view.zoom,
        };
        let x = ghost.source.x + (target.x - ghost.source.x) * progress;
        let y = ghost.source.y + (target.y - ghost.source.y) * progress;
        let width = ghost.source.width + (target.width - ghost.source.width) * progress;
        let height = ghost.source.height + (target.height - ghost.source.height) * progress;
        let transform = gsk::Transform::new()
            .translate(&graphene::Point::new(x as f32, y as f32))
            .scale(
                (width / super::canvas::WIDTH) as f32,
                (height / super::canvas::node_height(node)) as f32,
            );
        layer.set_child_transform(&ghost.widget, Some(&transform));
        ghost
            .widget
            .set_opacity(0.7 * 4.0 * progress * (1.0 - progress));
    }
}
pub fn clear(layer: &gtk::Fixed) {
    while let Some(child) = layer.first_child() {
        layer.remove(&child);
    }
}
