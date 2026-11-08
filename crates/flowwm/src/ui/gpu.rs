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
    metadata: HashMap<String, gsk::RenderNode>,
    revision: u64,
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
fn metadata<'a>(
    cache: &'a mut Cache,
    node: &Node,
    index: usize,
    view: &View,
) -> &'a gsk::RenderNode {
    let key = format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        node.id,
        view.numbers,
        index,
        view.theme.primary.hex(),
        view.theme.font,
        view.theme.text.hex(),
        view.theme.muted.hex(),
        view.theme.surface_high.hex(),
        view.theme.outline.hex(),
        view.theme.radius.to_bits(),
        node_height(node).to_bits(),
    );
    cache.metadata.entry(key).or_insert_with(|| {
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
        if view.numbers && index < 10 {
            let c = meta.append_cairo(&graphene::Rect::new(WIDTH as f32 - 34.0, -14.0, 32.0, 28.0));
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
    ] {
        c.hex().hash(&mut h);
    }
    view.theme.radius.to_bits().hash(&mut h);
    (
        view.zoom.to_bits(),
        view.pan_x.to_bits(),
        view.pan_y.to_bits(),
    )
        .hash(&mut h);
    for node in state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
    {
        let p = view.position(node);
        (
            p.x.to_bits(),
            p.y.to_bits(),
            view.selection.contains(&node.id),
        )
            .hash(&mut h);
    }
    view.interaction_key().hash(&mut h);
    h.finish()
}
fn window_geometry(node: &Node, view: &View) -> (Rect, Rect, f64) {
    let p = view.position(node);
    let t = progress(view, node);
    let graph = Rect {
        x: view.pan_x + p.x * view.zoom,
        y: view.pan_y + p.y * view.zoom,
        width: WIDTH * view.zoom,
        height: node_height(node) * view.zoom,
    };
    let mut rect = view
        .desktop_rects
        .get(&node.id)
        .copied()
        .unwrap_or(graph)
        .interpolate(graph, t);
    if node.closed_window() {
        rect = graph;
        rect.y += 48. * view.zoom * (1. - view.chrome);
    }
    (graph, rect, t)
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
        let (_, rect, t) = window_geometry(node, view);
        // A later card's labels and ports must cover previews belonging to earlier cards.
        let cover = Rect {
            x: rect.x - 8. * view.zoom,
            y: rect.y - 16. * view.zoom,
            width: rect.width + 16. * view.zoom,
            height: rect.height + 32. * view.zoom,
        };
        let _ = holes.subtract_rectangle(&cover.integer());
        if node.window_id.is_some() {
            let _ = holes.union_rectangle(&window_content(rect, t, view.zoom).integer());
        }
    }
    let region =
        gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(0, 0, width, height));
    let _ = region.subtract(&holes);
    region
}

pub fn native_windows(state: &State, view: &View) -> Vec<niri_ipc::node_row::NodeWindow> {
    canvas::ordered_nodes(state)
        .iter()
        .filter_map(|node| {
            let id = node.window_id?;
            let (_, rect, t) = window_geometry(node, view);
            let content = window_content(rect, t, view.zoom);
            Some(niri_ipc::node_row::NodeWindow {
                id,
                bounds: (content.x, content.y, content.width, content.height),
                expanded: node.column_mode == ColumnMode::Window && t < 0.001,
                opacity: if !view.query.is_empty() && !canvas::matches_query(node, &view.query) {
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
    if view.native_nodes {
        let scene = gtk::Snapshot::new();
        render_inner(&scene, width, height, state, revision, view, cache);
        if let Some(scene) = scene.to_node() {
            let region = native_canvas_region(state, view, width, height);
            for i in 0..region.num_rectangles() {
                let r = region.rectangle(i);
                snapshot.push_clip(&graphene::Rect::new(
                    r.x() as f32,
                    r.y() as f32,
                    r.width() as f32,
                    r.height() as f32,
                ));
                snapshot.append_node(&scene);
                snapshot.pop();
            }
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
    for node in &nodes {
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
    if cache.revision != revision {
        cache.metadata.clear();
        cache.revision = revision;
    }
    for (index, node) in nodes.iter().enumerate() {
        if node.window_id.is_none() && !node.desktop_window {
            continue;
        }
        let (graph, rect, t) = window_geometry(node, view);
        if rect.x > width as f64 + 12.0
            || rect.x + rect.width < -12.0
            || rect.y > height as f64 + 12.0
            || rect.y + rect.height < -12.0
        {
            continue;
        }
        let selected =
            view.selected.as_deref() == Some(&node.id) || view.selection.contains(&node.id);
        let matching = canvas::matches_query(node, &view.query);
        let alpha = if node.closed_window() {
            view.chrome
                * if !view.query.is_empty() && !matching {
                    0.25
                } else {
                    1.0
                }
        } else if t < 0.001 {
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
            snapshot.append_node(metadata(cache, node, index, view));
            snapshot.restore();
            if selected || (!view.query.is_empty() && matching) {
                snapshot.append_border(
                    &outline,
                    &[2.0 * view.zoom as f32; 4],
                    &[color(view.theme.primary); 4],
                );
            }
            snapshot.pop();
            continue;
        }
        snapshot.push_rounded_clip(&outline);
        if t < 0.001 {
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
        let fill = view.theme.surface_high;
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
        let accent = if selected || (!view.query.is_empty() && matching) {
            view.theme.primary
        } else {
            view.theme.outline
        };
        let start = view.theme.primary;
        let border = super::theme::Color(
            start.0 + (accent.0 - start.0) * t,
            start.1 + (accent.1 - start.1) * t,
            start.2 + (accent.2 - start.2) * t,
        );
        let line = (2.0 * (1.0 - t) + view.zoom * (if selected { 2.0 } else { 1.0 }) * t) as f32;
        if t > 0.6 {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(graph.x as f32, graph.y as f32));
            snapshot.scale(view.zoom as f32, view.zoom as f32);
            snapshot.push_opacity(((t - 0.6) / 0.4).clamp(0.0, 1.0));
            snapshot.append_node(metadata(cache, node, index, view));
            snapshot.pop();
            snapshot.restore();
        }
        // Metadata contains its own card background and inactive border. Paint
        // the shared outline last so headers and footers cannot cover selection.
        snapshot.append_border(&outline, &[line; 4], &[color(border); 4]);
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
