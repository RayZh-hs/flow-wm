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
        view.theme.surface,
        view.theme.surface_high,
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
pub fn render(
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
    } else {
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
            // History has no native rectangle to morph from. Reveal the whole
            // compact card together, including its outline, from slightly below.
            rect = graph;
            rect.y += 48.0 * view.zoom * (1.0 - view.chrome);
        }
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
                    &[1.4 * view.zoom as f32; 4],
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
                snapshot.append_texture(
                    texture,
                    &bounds(Rect {
                        x: (ow - texture.width() as f64 * scale) / 2.0 - view.insets[3] as f64,
                        y: (oh - texture.height() as f64 * scale) / 2.0 - view.insets[0] as f64,
                        width: texture.width() as f64 * scale,
                        height: texture.height() as f64 * scale,
                    }),
                );
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
        let content = Rect {
            x: rect.x + 10.0 * view.zoom * t,
            y: rect.y + 70.0 * view.zoom * t,
            width: rect.width - 20.0 * view.zoom * t,
            height: rect.height - 110.0 * view.zoom * t,
        };
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
        let line = (2.0 * (1.0 - t) + view.zoom * (if selected { 1.4 } else { 1.0 }) * t) as f32;
        snapshot.append_border(&outline, &[line; 4], &[color(border); 4]);
        if t > 0.6 {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(graph.x as f32, graph.y as f32));
            snapshot.scale(view.zoom as f32, view.zoom as f32);
            snapshot.push_opacity(((t - 0.6) / 0.4).clamp(0.0, 1.0));
            snapshot.append_node(metadata(cache, node, index, view));
            snapshot.pop();
            snapshot.restore();
        }
        snapshot.pop();
    }
    // Selection and cut gestures must be above GPU window cards and previews.
    if canvas::has_foreground(view) {
        let c = snapshot.append_cairo(&graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
        canvas::draw_foreground(&c, view);
    }
}
