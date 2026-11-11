use super::{
    canvas::{self, View, WIDTH, node_height},
    collapse::Rect,
};
use crate::model::*;
use gtk::{gdk, graphene, gsk, prelude::*};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};
#[derive(Default)]
pub struct Cache {
    graph: Option<(u64, gsk::RenderNode)>,
    metadata: HashMap<(String, String), gsk::RenderNode>,
    signatures: HashMap<String, Vec<u8>>,
    revision: Option<u64>,
    cards: HashMap<String, (Node, Rect)>,
    removed: Vec<RemovedCard>,
    card_revision: Option<u64>,
}
struct RemovedCard {
    image: gsk::RenderNode,
    rect: Rect,
    started: std::time::Instant,
}

impl Cache {
    fn removals(&mut self, state: &State, revision: u64, view: &View) {
        let now = std::time::Instant::now();
        let ids: std::collections::HashSet<_> =
            state.nodes.iter().map(|node| node.id.as_str()).collect();
        if view.theme.removal_ms == 0 || view.chrome < 0.999 {
            self.removed.clear();
        } else {
            for (id, (node, rect)) in &self.cards {
                if ids.contains(id.as_str()) {
                    continue;
                }
                let snapshot = gtk::Snapshot::new();
                let c = snapshot.append_cairo(&graphene::Rect::new(
                    -8.,
                    -8.,
                    WIDTH as f32 + 16.,
                    node_height(node) as f32 + 16.,
                ));
                c.select_font_face(
                    &view.theme.font,
                    gtk::cairo::FontSlant::Normal,
                    gtk::cairo::FontWeight::Normal,
                );
                canvas::draw_node(&c, node, &Position::default(), false, &view.theme);
                drop(c);
                if let Some(image) = snapshot.to_node() {
                    self.removed.push(RemovedCard {
                        image,
                        rect: *rect,
                        started: now,
                    });
                }
            }
        }
        self.cards.retain(|id, _| ids.contains(id.as_str()));
        for node in canvas::ordered_nodes(state) {
            let (_, rect, _) = window_geometry(node, view);
            self.cards
                .entry(node.id.clone())
                .and_modify(|(previous, old_rect)| {
                    if self.card_revision != Some(revision) {
                        *previous = node.clone();
                    }
                    *old_rect = rect;
                })
                .or_insert_with(|| (node.clone(), rect));
        }
        self.card_revision = Some(revision);
    }

    pub fn animate_removals(&mut self, snapshot: &gtk::Snapshot, view: &View) -> bool {
        let duration = view.theme.removal_ms as f64 / 1000.;
        self.removed
            .retain(|card| card.started.elapsed().as_secs_f64() < duration);
        for card in &self.removed {
            let t = (card.started.elapsed().as_secs_f64() / duration).clamp(0., 1.);
            let shrink = 1. - 0.24 * super::motion::ease_out(t);
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (card.rect.x + card.rect.width * (1. - shrink) / 2.) as f32,
                (card.rect.y + card.rect.height * (1. - shrink) / 2. + 14. * t * t) as f32,
            ));
            snapshot.scale(
                (card.rect.width / WIDTH * shrink) as f32,
                (card.rect.width / WIDTH * shrink) as f32,
            );
            snapshot.push_opacity((1. - t).powi(2));
            snapshot.append_node(&card.image);
            snapshot.pop();
            snapshot.restore();
        }
        !self.removed.is_empty()
    }
}
fn bounds(r: Rect) -> graphene::Rect {
    graphene::Rect::new(r.x as f32, r.y as f32, r.width as f32, r.height as f32)
}
fn color(c: super::theme::Color) -> gdk::RGBA {
    gdk::RGBA::new(c.0 as f32, c.1 as f32, c.2 as f32, 1.0)
}
fn progress(view: &View, node: &Node) -> f64 {
    view.presentations.get(&node.id).copied().unwrap_or(
        if node.column_mode == ColumnMode::Node || node.window_id.is_none() {
            1.0
        } else {
            0.0
        },
    )
}

pub(super) fn border_opacity(node: &Node, view: &View) -> f64 {
    view.border_opacities
        .get(&node.id)
        .copied()
        .unwrap_or_else(|| border_target(node, view))
}

pub(super) fn border_target(node: &Node, view: &View) -> f64 {
    let window =
        node.window_id.is_some() && (view.window_borders || node.column_mode == ColumnMode::Window);
    let active = if window {
        node.window_id == view.focused_window
    } else {
        view.selected.as_deref() == Some(&node.id) || view.selection.contains(&node.id)
    };
    f64::from(
        active || (!window && !view.query.is_empty() && canvas::matches_query(node, &view.query)),
    )
}
fn metadata<'a>(
    cache: &'a mut Cache,
    state: &State,
    node: &Node,
    index: usize,
    view: &View,
) -> &'a gsk::RenderNode {
    let key = format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        node.id,
        view.numbers,
        index,
        view.theme.primary.hex(),
        view.theme.font,
        view.theme.text.hex(),
        view.theme.muted.hex(),
        view.theme.surface_high.hex(),
        view.theme.outline.hex(),
        view.theme.running.hex(),
        view.theme.running_opacity.to_bits(),
        view.theme.radius.to_bits(),
        node_height(node).to_bits(),
    );
    cache
        .metadata
        .entry((node.id.clone(), key))
        .or_insert_with(|| {
            let meta = gtk::Snapshot::new();
            let c = meta.append_cairo(&graphene::Rect::new(
                -8.0,
                -16.0,
                WIDTH as f32 + 16.0,
                node_height(node) as f32 + 32.0,
            ));
            c.select_font_face(
                &view.theme.font,
                gtk::cairo::FontSlant::Normal,
                gtk::cairo::FontWeight::Normal,
            );
            if !node.closed_window() {
                c.rectangle(-8.0, -8.0, WIDTH + 16.0, 77.0);
                c.rectangle(-8.0, node_height(node) - 39.0, WIDTH + 16.0, 48.0);
                c.clip();
            }
            canvas::draw_node(&c, node, &Position::default(), false, &view.theme);
            drop(c);
            if !node.closed_window() {
                let c = meta.append_cairo(&graphene::Rect::new(
                    -8.,
                    -8.,
                    WIDTH as f32 + 16.,
                    node_height(node) as f32 + 16.,
                ));
                canvas::draw_pin_markers(&c, state, node, &Position::default(), &view.theme);
            }
            if view.numbers && index < 10 {
                let c =
                    meta.append_cairo(&graphene::Rect::new(WIDTH as f32 - 34.0, -14.0, 32.0, 28.0));
                canvas::rounded(&c, WIDTH - 32.0, -13.0, 28.0, 26.0, 8.0);
                view.theme.primary.source(&c);
                let _ = c.fill();
                canvas::text(
                    &c,
                    WIDTH - 22.0,
                    5.0,
                    &((index + 1) % 10).to_string(),
                    14.0,
                    view.theme.on_primary.rgb(),
                    true,
                );
            }
            meta.to_node().unwrap()
        })
}
fn graph_key(width: i32, height: i32, state: &State, revision: u64, view: &View) -> u64 {
    let mut h = DefaultHasher::new();
    (width, height, revision).hash(&mut h);
    view.selected.hash(&mut h);
    view.query.hash(&mut h);
    view.numbers.hash(&mut h);
    view.theme.font.hash(&mut h);
    for c in [
        view.theme.primary,
        view.theme.on_primary,
        view.theme.primary_container,
        view.theme.surface,
        view.theme.surface_low,
        view.theme.surface_high,
        view.theme.surface_highest,
        view.theme.text,
        view.theme.muted,
        view.theme.outline,
        view.theme.running,
    ] {
        c.hex().hash(&mut h);
    }
    view.theme.radius.to_bits().hash(&mut h);
    view.theme.running_opacity.to_bits().hash(&mut h);
    (
        view.zoom.to_bits(),
        view.pan_x.to_bits(),
        view.pan_y.to_bits(),
    )
        .hash(&mut h);
    let mut linked = std::collections::HashSet::new();
    for node in &state.nodes {
        for dependency in &node.dependencies {
            linked.insert(dependency.as_str());
            linked.insert(node.id.as_str());
        }
    }
    for edge in &state.connections {
        linked.insert(edge.source.node.as_str());
        linked.insert(edge.target.node.as_str());
    }
    for watcher in &state.watchers {
        linked.insert(watcher.source.as_str());
        linked.insert(watcher.target.as_str());
    }
    for node in state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
    {
        // Live card transforms are GSK nodes, and do not invalidate the Cairo
        // grid unless an edge actually depends on their moving position.
        if (node.window_id.is_some() || node.desktop_window) && !linked.contains(node.id.as_str()) {
            continue;
        }
        let p = view.position(node);
        (
            p.x.to_bits(),
            p.y.to_bits(),
            view.selection.contains(&node.id),
            border_opacity(node, view).to_bits(),
        )
            .hash(&mut h);
    }
    view.interaction_key().hash(&mut h);
    h.finish()
}
pub(super) fn window_geometry(node: &Node, view: &View) -> (Rect, Rect, f64) {
    let t = progress(view, node);
    let (graph, rect) = window_geometry_at(node, view, t);
    (graph, rect, t)
}

pub(super) fn window_geometry_at(node: &Node, view: &View, t: f64) -> (Rect, Rect) {
    let p = view.position(node);
    let graph = Rect {
        x: view.pan_x + p.x * view.zoom,
        y: view.pan_y + p.y * view.zoom,
        width: WIDTH * view.zoom,
        height: node_height(node) * view.zoom,
    };
    let mut rect = view
        .presented_rects
        .get(&node.id)
        .copied()
        .unwrap_or_else(|| {
            view.desktop_rects
                .get(&node.id)
                .copied()
                .unwrap_or(graph)
                .interpolate(graph, t)
        });
    if node.closed_window() {
        rect = graph;
        rect.y += 48. * view.zoom * (1. - view.chrome);
    }
    (graph, rect)
}
fn window_content(rect: Rect, t: f64, zoom: f64) -> Rect {
    Rect {
        x: rect.x + 10. * zoom * t,
        y: rect.y + 70. * zoom * t,
        width: rect.width - 20. * zoom * t,
        height: rect.height - 110. * zoom * t,
    }
}
fn native_canvas_region(state: &State, view: &View, width: i32, height: i32) -> gtk::cairo::Region {
    let holes = gtk::cairo::Region::create();
    for node in canvas::ordered_nodes(state) {
        if node.window_id.is_none() && !node.desktop_window {
            continue;
        }
        // Off-screen windows fade at their stationary graph positions. Once
        // invisible, their chrome must stop masking the live window beneath it.
        if view.window_opacities.get(&node.id).copied().unwrap_or(1.) <= 0. {
            continue;
        }
        let (_, rect, t) = window_geometry(node, view);
        // A later card's labels and ports must cover previews belonging to earlier cards.
        let cover = Rect {
            x: rect.x - 8. * view.zoom,
            y: rect.y - 16. * view.zoom,
            width: rect.width + 16. * view.zoom,
            height: rect.height + 32. * view.zoom,
        };
        let _ = holes.subtract_rectangle(&cover.integer());
        if node.window_id.is_some_and(|id| {
            view.local_windows
                .as_ref()
                .is_none_or(|ids| ids.contains(&id))
        }) {
            // Keep the shared outline above the native buffer even when fully expanded.
            let content = window_content(rect, t, view.zoom).inset(2. * (1. - t));
            let _ = holes.union_rectangle(&content.integer());
        }
    }
    let region =
        gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(0, 0, width, height));
    let _ = region.subtract(&holes);
    region
}

pub fn native_windows(
    state: &State,
    view: &View,
    width: i32,
    height: i32,
) -> Vec<niri_ipc::node_row::NodeWindow> {
    let viewport = Rect {
        x: 0.,
        y: 0.,
        width: width as f64,
        height: height as f64,
    };
    canvas::ordered_nodes(state)
        .iter()
        .filter_map(|node| {
            let id = node.window_id?;
            if view
                .local_windows
                .as_ref()
                .is_some_and(|ids| !ids.contains(&id))
            {
                return None;
            }
            let (_, rect, t) = window_geometry(node, view);
            let content = window_content(rect, t, view.zoom);
            if content.width <= 0. || content.height <= 0. || !content.overlaps(viewport) {
                return None;
            }
            Some(niri_ipc::node_row::NodeWindow {
                id,
                bounds: (content.x, content.y, content.width, content.height),
                expanded: node.column_mode == ColumnMode::Window && t < 0.001,
                presentation: Some(t as f32),
                opacity: view.window_opacities.get(&node.id).copied().unwrap_or(1.) as f32
                    * if !view.query.is_empty() && !canvas::matches_query(node, &view.query) {
                        0.25
                    } else {
                        1.0
                    },
            })
        })
        .collect()
}

pub fn render(
    snapshot: &gtk::Snapshot,
    width: i32,
    height: i32,
    state: &State,
    revision: u64,
    view: &View,
    cache: &mut Cache,
) {
    cache.removals(state, revision, view);
    if view.native_nodes {
        let scene = gtk::Snapshot::new();
        render_inner(&scene, width, height, state, revision, view, cache);
        if let Some(scene) = scene.to_node() {
            let region = native_canvas_region(state, view, width, height);
            // One mask and one scene traversal. Replaying the whole scene once per
            // hole fragment made rendering quadratic in the number of windows.
            snapshot.push_mask(gsk::MaskMode::Alpha);
            for i in 0..region.num_rectangles() {
                let r = region.rectangle(i);
                snapshot.append_color(
                    &gdk::RGBA::WHITE,
                    &graphene::Rect::new(
                        r.x() as f32,
                        r.y() as f32,
                        r.width() as f32,
                        r.height() as f32,
                    ),
                );
            }
            snapshot.pop();
            snapshot.append_node(&scene);
            snapshot.pop();
        }
        if canvas::has_foreground(view) {
            let c =
                snapshot.append_cairo(&graphene::Rect::new(0., 0., width as f32, height as f32));
            canvas::draw_foreground(&c, view);
        }
    } else {
        render_inner(snapshot, width, height, state, revision, view, cache);
    }
}

fn render_inner(
    snapshot: &gtk::Snapshot,
    width: i32,
    height: i32,
    state: &State,
    revision: u64,
    view: &View,
    cache: &mut Cache,
) {
    let nodes = canvas::ordered_nodes(state);
    let region =
        gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(0, 0, width, height));
    for node in nodes.iter().filter(|_| !view.native_nodes) {
        if progress(view, node) < 0.001
            && let Some(rect) = view.desktop_rects.get(&node.id)
        {
            let _ = region.subtract_rectangle(&rect.integer());
        }
    }
    let backdrop = gtk::Snapshot::new();
    if let Some(texture) = &view.wallpaper_texture {
        let (ow, oh) = view.output_size;
        let scale = (ow / texture.width() as f64).max(oh / texture.height() as f64);
        backdrop.append_texture(
            texture,
            &bounds(Rect {
                x: (ow - texture.width() as f64 * scale) / 2.0 - view.insets[3] as f64,
                y: (oh - texture.height() as f64 * scale) / 2.0 - view.insets[0] as f64,
                width: texture.width() as f64 * scale,
                height: texture.height() as f64 * scale,
            }),
        );
    } else if !view.native_nodes {
        backdrop.append_color(
            &color(view.theme.surface),
            &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
        );
    }
    let key = graph_key(width, height, state, revision, view);
    if cache.graph.as_ref().is_none_or(|(old, _)| *old != key) {
        let layer = gtk::Snapshot::new();
        let c = layer.append_cairo(&graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
        let mut graph = view.clone();
        graph.gpu = true;
        graph.chrome = 1.0;
        canvas::draw(&c, width as f64, height as f64, state, &graph);
        drop(c);
        cache.graph = layer.to_node().map(|node| (key, node));
    }
    if let Some((_, node)) = &cache.graph {
        backdrop.push_opacity(view.chrome);
        backdrop.append_node(node);
        backdrop.pop();
    }
    if let Some(backdrop) = backdrop.to_node() {
        if view.native_nodes {
            snapshot.append_node(&backdrop);
        } else {
            for i in 0..region.num_rectangles() {
                let r = region.rectangle(i);
                snapshot.push_clip(&graphene::Rect::new(
                    r.x() as f32,
                    r.y() as f32,
                    r.width() as f32,
                    r.height() as f32,
                ));
                snapshot.append_node(&backdrop);
                snapshot.pop();
            }
        }
    }
    if cache.revision != Some(revision) {
        // Focus and presentation changes do not change a card's labels or ports.
        // Retain their rasterizations across view changes and unrelated updates.
        let signatures: HashMap<_, _> = nodes
            .iter()
            .map(|n| {
                let signature = serde_json::to_vec(&(
                    &n.title,
                    n.kind,
                    n.status,
                    &n.metric,
                    &n.command,
                    n.window_id,
                    n.desktop_window,
                    &n.flow,
                    crate::graph::pins(state, n, true),
                    crate::graph::pins(state, n, false),
                ))
                .unwrap_or_default();
                (n.id.clone(), signature)
            })
            .collect();
        cache.metadata.retain(|(id, _), _| {
            signatures
                .get(id)
                .is_some_and(|s| cache.signatures.get(id) == Some(s))
        });
        cache.signatures = signatures;
        cache.revision = Some(revision);
    }
    for (index, node) in nodes.iter().enumerate() {
        if node.window_id.is_none() && !node.desktop_window {
            continue;
        }
        let (_, rect, t) = window_geometry(node, view);
        if rect.x > width as f64 + 12.0
            || rect.x + rect.width < -12.0
            || rect.y > height as f64 + 12.0
            || rect.y + rect.height < -12.0
        {
            continue;
        }
        let matching = canvas::matches_query(node, &view.query);
        let alpha = view.window_opacities.get(&node.id).copied().unwrap_or(1.)
            * if node.closed_window() {
                view.chrome
                    * if !view.query.is_empty() && !matching {
                        0.25
                    } else {
                        1.0
                    }
            } else if t < 0.001 && !view.native_nodes {
                view.live_fade
            } else if !view.query.is_empty() && !matching {
                0.25
            } else {
                1.0
            };
        if alpha <= 0.0 {
            continue;
        }
        snapshot.push_opacity(alpha);
        let radius = (view.theme.radius * (1.0 - t) + view.theme.radius * view.zoom * t) as f32;
        let outline = gsk::RoundedRect::from_rect(bounds(rect), radius);
        if node.closed_window() {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(rect.x as f32, rect.y as f32));
            snapshot.scale(view.zoom as f32, view.zoom as f32);
            snapshot.append_node(metadata(cache, state, node, index, view));
            snapshot.restore();
            let border_alpha = border_opacity(node, view);
            if border_alpha > 0. {
                let mut accent = color(view.theme.primary);
                accent.set_alpha(border_alpha as f32);
                snapshot.append_border(&outline, &[2.0 * view.zoom as f32; 4], &[accent; 4]);
            }
            snapshot.pop();
            continue;
        }
        snapshot.push_rounded_clip(&outline);
        // Native clients and their backdrops are rendered by the compositor.
        // An opaque fallback here leaks through the mask around fading peers.
        if t < 0.001 && !view.native_nodes {
            if let Some(texture) = &view.wallpaper_texture {
                let (ow, oh) = view.output_size;
                let scale = (ow / texture.width() as f64).max(oh / texture.height() as f64);
                snapshot.push_blur(18.0);
                snapshot.append_texture(
                    texture,
                    &bounds(Rect {
                        x: (ow - texture.width() as f64 * scale) / 2.0 - view.insets[3] as f64,
                        y: (oh - texture.height() as f64 * scale) / 2.0 - view.insets[0] as f64,
                        width: texture.width() as f64 * scale,
                        height: texture.height() as f64 * scale,
                    }),
                );
                snapshot.pop();
            } else {
                snapshot.append_color(&color(view.theme.surface), &bounds(rect));
            }
        }
        let fill = view.theme.node_background(node.status);
        snapshot.append_color(
            &gdk::RGBA::new(fill.0 as f32, fill.1 as f32, fill.2 as f32, t as f32),
            &bounds(rect),
        );
        snapshot.pop();
        let content = window_content(rect, t, view.zoom);
        if let Some(texture) = view.image_textures.get(&node.id) {
            let scale = (content.width / texture.width() as f64)
                .min(content.height / texture.height() as f64);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(
                bounds(content),
                (radius - 2.0 * t as f32).max(0.0),
            ));
            snapshot.append_texture(
                texture,
                &bounds(Rect {
                    x: content.x + (content.width - texture.width() as f64 * scale) / 2.0,
                    y: content.y + (content.height - texture.height() as f64 * scale) / 2.0,
                    width: texture.width() as f64 * scale,
                    height: texture.height() as f64 * scale,
                }),
            );
            snapshot.pop();
        }
        let border_alpha = border_opacity(node, view);
        let line = (2.0 * (1.0 - t) + 2.0 * view.zoom * t) as f32;
        if t > 0.6 {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(rect.x as f32, rect.y as f32));
            snapshot.scale(
                (rect.width / WIDTH) as f32,
                (rect.height / node_height(node)) as f32,
            );
            snapshot.push_opacity(((t - 0.6) / 0.4).clamp(0.0, 1.0));
            snapshot.append_node(metadata(cache, state, node, index, view));
            snapshot.pop();
            snapshot.restore();
        }
        // Metadata contains its own card background and inactive border. Paint
        // the shared outline last so headers and footers cannot cover selection.
        let mut inactive = color(view.theme.outline);
        inactive.set_alpha((0.5 * t) as f32);
        snapshot.append_border(&outline, &[view.zoom as f32; 4], &[inactive; 4]);
        let mut accent = color(view.theme.primary);
        accent.set_alpha(border_alpha as f32);
        snapshot.append_border(&outline, &[line; 4], &[accent; 4]);
        snapshot.pop();
    }
    // Selection and cut gestures must be above GPU window cards and previews.
    if !view.native_nodes && canvas::has_foreground(view) {
        let c = snapshot.append_cairo(&graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
        canvas::draw_foreground(&c, view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::field_reassign_with_default)]
    fn expanded_window_with_fading_peers(opacity: f64) -> (State, View) {
        let mut state = State::empty("/tmp".into());
        let mut view = View::default();
        view.native_nodes = true;
        view.zoom = 1.;
        view.pan_x = 0.;
        view.pan_y = 0.;
        view.chrome = 0.;
        for index in 0..3 {
            let mut node = Node::new(
                &state.selected_workspace,
                NodeKind::Window,
                "/tmp".into(),
                Position {
                    x: index as f64 * 320.,
                    y: 150.,
                },
            );
            node.window_id = Some(index + 1);
            node.column_mode = if index == 0 {
                ColumnMode::Window
            } else {
                ColumnMode::Node
            };
            view.presentations
                .insert(node.id.clone(), f64::from(index != 0));
            view.presented_rects.insert(
                node.id.clone(),
                if index == 0 {
                    Rect {
                        x: 0.,
                        y: 0.,
                        width: 1000.,
                        height: 600.,
                    }
                } else {
                    Rect {
                        x: index as f64 * 320.,
                        y: 150.,
                        width: WIDTH,
                        height: node_height(&node),
                    }
                },
            );
            view.window_opacities
                .insert(node.id.clone(), if index == 0 { 1. } else { opacity });
            state.nodes.push(node);
        }
        (state, view)
    }

    #[test]
    fn native_scenes_exclude_attachments_from_another_row_without_cutting_a_hole() {
        let (state, mut view) = expanded_window_with_fading_peers(1.);
        view.local_windows = Some(std::collections::HashSet::new());
        assert!(native_windows(&state, &view, 1200, 800).is_empty());
        assert!(native_canvas_region(&state, &view, 1200, 800).contains_point(50, 50));
    }

    #[test]
    fn invisible_stationary_previews_do_not_mask_the_expanded_window() {
        let (state, view) = expanded_window_with_fading_peers(0.);
        let region = native_canvas_region(&state, &view, 1000, 600);
        for x in [340, 660] {
            assert!(
                !region.contains_point(x, 180),
                "a faded off-screen window must not cover the live client below it"
            );
        }
    }

    #[test]
    #[ignore = "requires a GTK display; run explicitly inside Niri or Xvfb"]
    fn native_morphs_do_not_paint_opaque_boxes_behind_stationary_previews() {
        gtk::init().unwrap();
        for opacity in [0., 0.001, 0.1, 0.5, 1.] {
            let (state, mut view) = expanded_window_with_fading_peers(opacity);
            // The shared outline has its own opacity; inspect the interior only.
            view.border_opacities
                .extend(state.nodes.iter().map(|n| (n.id.clone(), 0.)));
            let snapshot = gtk::Snapshot::new();
            render(
                &snapshot,
                1000,
                600,
                &state,
                1,
                &view,
                &mut Cache::default(),
            );
            let mut image =
                gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 1000, 600).unwrap();
            let context = gtk::cairo::Context::new(&image).unwrap();
            snapshot.to_node().unwrap().draw(&context);
            drop(context);
            let stride = image.stride() as usize;
            let pixels = image.data().unwrap();
            for x in [315, 635] {
                let offset = 140 * stride + x * 4;
                assert_eq!(
                    &pixels[offset..offset + 4],
                    &[0; 4],
                    "the mask padding around a stationary preview must stay transparent at opacity {opacity}"
                );
            }
        }
    }

    #[test]
    fn interrupted_camera_morphs_never_publish_degenerate_preview_bounds() {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            "/tmp".into(),
            Position::default(),
        );
        node.window_id = Some(7);
        let mut view = View::default();
        view.zoom = 1.3;
        view.presented_rects.insert(
            node.id.clone(),
            Rect {
                x: 90.,
                y: 330.,
                width: 145.,
                height: 134.,
            },
        );
        state.nodes.push(node);
        assert!(native_windows(&state, &view, 1200, 800).is_empty());
        view.presented_rects.clear();
        let scene = niri_ipc::node_row::NodeRowScene {
            windows: native_windows(&state, &view, 1200, 800),
            insets: [0; 4],
        };
        assert_eq!(scene.windows.len(), 1);
        assert!(scene.validate());
    }

    #[test]
    fn offscreen_native_previews_are_culled_and_fades_reach_the_compositor() {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            "/tmp".into(),
            Position::default(),
        );
        node.window_id = Some(7);
        node.column_mode = ColumnMode::Node;
        let id = node.id.clone();
        state.nodes.push(node);
        let mut view = View::default();
        view.zoom = 1.;
        view.pan_x = 0.;
        view.pan_y = 0.;
        view.window_opacities.insert(id.clone(), 0.4);
        let scene = native_windows(&state, &view, 800, 600);
        assert_eq!(scene.len(), 1);
        assert!((scene[0].opacity - 0.4).abs() < 1e-6);
        view.presented_rects.insert(
            id,
            Rect {
                x: -500.,
                y: 20.,
                width: 292.,
                height: 274.,
            },
        );
        assert!(native_windows(&state, &view, 800, 600).is_empty());
    }

    #[test]
    fn interrupted_morphs_and_expanded_frames_share_the_live_rectangle() {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            "/tmp".into(),
            Position::default(),
        );
        node.window_id = Some(42);
        node.column_mode = ColumnMode::Window;
        let id = node.id.clone();
        state.nodes.push(node);
        let mut view = View::default();
        let actual = Rect {
            x: 321.25,
            y: 28.5,
            width: 800.,
            height: 700.,
        };
        view.desktop_rects.insert(
            id.clone(),
            Rect {
                x: -900.,
                y: 0.,
                width: 800.,
                height: 700.,
            },
        );
        view.presented_rects.insert(id.clone(), actual);
        assert_eq!(window_geometry(&state.nodes[0], &view).1, actual);
        let native = native_windows(&state, &view, 1920, 1080);
        assert!(native[0].expanded);
        assert_eq!(native[0].presentation, Some(0.));
        assert_eq!(
            native[0].bounds,
            (actual.x, actual.y, actual.width, actual.height)
        );
        let region = native_canvas_region(&state, &view, 1500, 1000);
        assert!(
            region.contains_point(322, 29),
            "the shared border must survive at rest"
        );
        assert!(
            !region.contains_point(400, 100),
            "the client interior remains live"
        );
        view.presentations.insert(id, 0.45);
        assert_eq!(
            window_geometry(&state.nodes[0], &view).1,
            actual,
            "reversal starts at the last displayed frame"
        );
        let native = native_windows(&state, &view, 1920, 1080);
        assert_eq!(native[0].presentation, Some(0.45));
        assert!(
            !native[0].expanded,
            "input stays with the graph during a morph"
        );
        assert_eq!(
            native[0].bounds.0,
            window_content(actual, 0.45, view.zoom).x
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn overlapping_cards_keep_their_headers_and_ports_above_live_previews() {
        let cwd = std::path::PathBuf::from("/tmp");
        let mut state = State::empty(cwd.clone());
        let mut first = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            cwd.clone(),
            Position::default(),
        );
        first.window_id = Some(1);
        let mut second = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            cwd,
            Position { x: 0., y: 120. },
        );
        second.window_id = Some(2);
        state.nodes = vec![first, second];
        let mut view = View::default();
        view.zoom = 1.;
        view.pan_x = 0.;
        view.pan_y = 0.;
        view.native_nodes = true;
        let region = native_canvas_region(&state, &view, 500, 500);
        assert!(
            !region.contains_point(30, 80),
            "the first preview is transparent"
        );
        assert!(
            region.contains_point(30, 150),
            "the later header covers the earlier preview"
        );
        assert!(
            region.contains_point(146, 115),
            "the later top port remains visible"
        );
        assert!(
            !region.contains_point(30, 200),
            "the later preview remains transparent"
        );
        state.nodes[1].window_id = None;
        state.nodes[1].desktop_window = true;
        let region = native_canvas_region(&state, &view, 500, 500);
        assert!(
            region.contains_point(30, 150),
            "closed history cards also cover previews"
        );
    }
}
