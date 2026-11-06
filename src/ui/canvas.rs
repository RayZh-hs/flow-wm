use super::theme::Theme;
use crate::{
    engine::{Control, Engine},
    model::*,
};
use gtk::{
    cairo::{self, Context},
    gdk, glib,
    prelude::*,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

pub const WIDTH: f64 = 292.0;
pub const HEIGHT: f64 = 154.0;
pub fn node_height(node: &Node) -> f64 {
    if node.kind == NodeKind::Window || node.window_id.is_some() {
        HEIGHT + 60.0
    } else {
        HEIGHT
    }
}
pub const GRID: f64 = 20.0;
type GridTile = ([u64; 5], cairo::ImageSurface);
thread_local! { static GRID_TILE: RefCell<Option<GridTile>> = const { RefCell::new(None) }; }

fn draw_grid(c: &Context, view: &View) {
    let spacing = GRID * view.zoom;
    let scale = c.matrix().xx().hypot(c.matrix().yx()).max(1.0);
    let color = view.theme.outline;
    let key = [
        spacing.to_bits(),
        scale.to_bits(),
        color.0.to_bits(),
        color.1.to_bits(),
        color.2.to_bits(),
    ];
    GRID_TILE.with(|cache| {
        let mut cached = cache.borrow_mut();
        if cached.as_ref().is_none_or(|(old, _)| *old != key) {
            let size = (spacing * scale).ceil().max(3.0) as i32;
            let Ok(tile) = cairo::ImageSurface::create(cairo::Format::ARgb32, size, size) else {
                return;
            };
            let Ok(context) = Context::new(&tile) else {
                return;
            };
            color.alpha(&context, 0.3);
            context.arc(
                size as f64 / 2.0,
                size as f64 / 2.0,
                0.75 * size as f64 / spacing,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = context.fill();
            *cached = Some((key, tile));
        }
        if let Some((_, tile)) = cached.as_ref() {
            let ratio = tile.width() as f64 / spacing;
            let pattern = cairo::SurfacePattern::create(tile);
            pattern.set_extend(cairo::Extend::Repeat);
            pattern.set_filter(cairo::Filter::Nearest);
            pattern.set_matrix(cairo::Matrix::new(
                ratio,
                0.0,
                0.0,
                ratio,
                tile.width() as f64 / 2.0 - view.pan_x.rem_euclid(spacing) * ratio,
                tile.height() as f64 / 2.0 - view.pan_y.rem_euclid(spacing) * ratio,
            ));
            let _ = c.save();
            let _ = c.set_source(&pattern);
            let _ = c.paint();
            let _ = c.restore();
        }
    });
}
type Callback = Rc<dyn Fn(CanvasAction)>;
#[derive(Clone)]
pub enum CanvasAction {
    Select(Option<String>),
    Open(String),
    Add,
    Edit(String),
    Delete(String),
    Run(String),
    Windows,
}
#[derive(Clone)]
enum Drag {
    Node { id: String, origin: Position },
    Pan { x: f64, y: f64 },
    Wire { source: String },
}
#[derive(Clone)]
pub struct View {
    pub theme: Theme,
    pub selected: Option<String>,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    drag: Option<Drag>,
    pub preview: Option<(String, Position)>,
    pointer: (f64, f64),
    pub workspace: String,
    motion_serial: u64,
    camera_target: Option<(f64, f64, f64)>,
    space_down: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            theme: Theme::load(),
            selected: None,
            zoom: 0.9,
            pan_x: 60.0,
            pan_y: 60.0,
            drag: None,
            preview: None,
            pointer: (0.0, 0.0),
            workspace: String::new(),
            motion_serial: 0,
            camera_target: None,
            space_down: false,
        }
    }
}

pub fn animate_camera(area: &gtk::DrawingArea, view: &Rc<RefCell<View>>, target: &View) {
    let mut current = view.borrow_mut();
    current.motion_serial += 1;
    let serial = current.motion_serial;
    let from = (current.zoom, current.pan_x, current.pan_y);
    let to = (target.zoom, target.pan_x, target.pan_y);
    current.camera_target = Some(to);
    let duration = current.theme.motion_ms as f64 * 1000.0;
    if duration == 0.0 || !area.is_mapped() {
        current.zoom = target.zoom;
        current.pan_x = target.pan_x;
        current.pan_y = target.pan_y;
        current.camera_target = None;
        drop(current);
        area.queue_draw();
        return;
    }
    drop(current);
    let view = view.clone();
    let started = std::time::Instant::now();
    area.add_tick_callback(move |area, _| {
        let mut view = view.borrow_mut();
        if view.motion_serial != serial {
            return glib::ControlFlow::Break;
        }
        let progress = if duration == 0.0 {
            1.0
        } else {
            (started.elapsed().as_micros() as f64 / duration).clamp(0.0, 1.0)
        };
        let t = 1.0 - (1.0 - progress).powi(3);
        view.zoom = from.0 + (to.0 - from.0) * t;
        view.pan_x = from.1 + (to.1 - from.1) * t;
        view.pan_y = from.2 + (to.2 - from.2) * t;
        area.queue_draw();
        if progress >= 1.0 {
            view.camera_target = None;
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
pub fn fit_animated(area: &gtk::DrawingArea, view: &Rc<RefCell<View>>, state: &State) {
    if area.width() < 100 || area.height() < 100 {
        return;
    }
    let mut target = view.borrow().clone();
    target.fit(state, area.width() as f64, area.height() as f64);
    animate_camera(area, view, &target);
}
pub fn fit_immediate(area: &gtk::DrawingArea, view: &Rc<RefCell<View>>, state: &State) {
    let mut view = view.borrow_mut();
    view.motion_serial += 1;
    view.camera_target = None;
    view.fit(state, area.width() as f64, area.height() as f64);
    area.queue_draw();
}
pub fn suspend(view: &Rc<RefCell<View>>) {
    let mut view = view.borrow_mut();
    view.motion_serial += 1;
    view.camera_target = None;
    view.space_down = false;
    view.drag = None;
    view.preview = None;
}
pub fn navigate(
    area: &gtk::DrawingArea,
    view: &Rc<RefCell<View>>,
    state: &State,
    direction: &str,
) -> Option<String> {
    let mut target = view.borrow().clone();
    let id = target.navigate(state, direction, area.width() as f64, area.height() as f64);
    view.borrow_mut().selected = target.selected.clone();
    animate_camera(area, view, &target);
    id
}
pub fn focus_node(area: &gtk::DrawingArea, view: &Rc<RefCell<View>>, state: &State, id: &str) {
    let Some(node) = state.node(id) else {
        return;
    };
    let mut target = view.borrow().clone();
    target.zoom = target.zoom.clamp(0.85, 1.2);
    target.pan_x = area.width() as f64 / 2.0 - (node.position.x + WIDTH / 2.0) * target.zoom;
    target.pan_y =
        area.height() as f64 / 2.0 - (node.position.y + node_height(node) / 2.0) * target.zoom;
    animate_camera(area, view, &target);
}
impl View {
    pub fn world(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.pan_x) / self.zoom, (y - self.pan_y) / self.zoom)
    }
    pub fn position(&self, node: &Node) -> Position {
        self.preview
            .as_ref()
            .filter(|(id, _)| id == &node.id)
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| node.position.clone())
    }
    pub fn fit(&mut self, state: &State, width: f64, height: f64) {
        let nodes: Vec<_> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .collect();
        if nodes.is_empty() {
            self.zoom = 1.0;
            self.pan_x = 60.0;
            self.pan_y = 60.0;
            return;
        }
        let min_x = nodes
            .iter()
            .map(|n| n.position.x)
            .fold(f64::INFINITY, f64::min);
        let min_y = nodes
            .iter()
            .map(|n| n.position.y)
            .fold(f64::INFINITY, f64::min);
        let max_x = nodes
            .iter()
            .map(|n| n.position.x + WIDTH)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = nodes
            .iter()
            .map(|n| n.position.y + node_height(n))
            .fold(f64::NEG_INFINITY, f64::max);
        self.zoom = ((width - 160.0) / (max_x - min_x))
            .min((height - 160.0) / (max_y - min_y))
            .clamp(0.35, 1.2);
        self.pan_x = (width - (max_x - min_x) * self.zoom) / 2.0 - min_x * self.zoom;
        self.pan_y = (height - (max_y - min_y) * self.zoom) / 2.0 - min_y * self.zoom;
    }
    pub fn navigate(
        &mut self,
        state: &State,
        direction: &str,
        width: f64,
        height: f64,
    ) -> Option<String> {
        let nodes: Vec<_> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .collect();
        let current = self.selected.as_deref().and_then(|id| state.node(id));
        let target = if let Some(current) = current {
            nodes
                .iter()
                .filter(|n| n.id != current.id)
                .filter_map(|n| {
                    let dx = n.position.x - current.position.x;
                    let dy = n.position.y - current.position.y;
                    let (forward, sideways) = match direction {
                        "up" => (-dy, dx),
                        "down" => (dy, dx),
                        "left" => (-dx, dy),
                        _ => (dx, dy),
                    };
                    (forward > 1.0).then_some((*n, forward + sideways.abs() * 2.0))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(n, _)| n)
        } else {
            nodes.first().copied()
        };
        let target = target?;
        self.selected = Some(target.id.clone());
        let left = target.position.x * self.zoom + self.pan_x;
        let top = target.position.y * self.zoom + self.pan_y;
        if left < 30.0 || left + WIDTH * self.zoom > width - 30.0 {
            self.pan_x = width / 2.0 - (target.position.x + WIDTH / 2.0) * self.zoom;
        }
        if top < 30.0 || top + node_height(target) * self.zoom > height - 30.0 {
            self.pan_y = height / 2.0 - (target.position.y + node_height(target) / 2.0) * self.zoom;
        }
        Some(target.id.clone())
    }
}

pub fn make(engine: Engine, view: Rc<RefCell<View>>, callback: Callback) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .focusable(true)
        .build();
    area.set_cursor_from_name(Some("default"));
    let e = engine.clone();
    let v = view.clone();
    area.set_draw_func(move |_, context, width, height| {
        let started = std::time::Instant::now();
        draw(
            context,
            width as f64,
            height as f64,
            &e.snapshot().state,
            &v.borrow(),
        );
        e.performance.record("canvas_draw", started.elapsed());
    });
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let e = engine.clone();
    let v = view.clone();
    let cb = callback.clone();
    let a = area.clone();
    click.connect_pressed(move |_, count, x, y| {
        a.grab_focus();
        let state = e.snapshot().state;
        let mut view = v.borrow_mut();
        let (x, y) = view.world(x, y);
        let hit = state.nodes.iter().rev().find(|n| {
            n.workspace == state.selected_workspace && contains(n, &view.position(n), x, y)
        });
        view.selected = hit.map(|n| n.id.clone());
        let selected = view.selected.clone();
        drop(view);
        cb(CanvasAction::Select(selected));
        if count == 2
            && let Some(node) = hit
        {
            cb(CanvasAction::Open(node.id.clone()));
        }
        a.queue_draw();
    });
    area.add_controller(click);
    let gesture = gtk::GestureDrag::new();
    gesture.set_button(1);
    let e = engine.clone();
    let v = view.clone();
    let a = area.clone();
    gesture.connect_drag_begin(move |_, x, y| {
        a.grab_focus();
        let state = e.snapshot().state;
        let mut view = v.borrow_mut();
        view.motion_serial += 1;
        view.camera_target = None;
        let (wx, wy) = view.world(x, y);
        view.pointer = (wx, wy);
        if view.space_down {
            view.drag = Some(Drag::Pan {
                x: view.pan_x,
                y: view.pan_y,
            });
        } else if let Some(node) = state.nodes.iter().find(|n| {
            n.workspace == state.selected_workspace
                && ((wx - n.position.x - WIDTH / 2.0).hypot(wy - n.position.y - node_height(n))
                    < 16.0)
        }) {
            view.drag = Some(Drag::Wire {
                source: node.id.clone(),
            });
        } else if let Some(node) =
            state.nodes.iter().rev().find(|n| {
                n.workspace == state.selected_workspace && contains(n, &n.position, wx, wy)
            })
        {
            view.selected = Some(node.id.clone());
            view.drag = Some(Drag::Node {
                id: node.id.clone(),
                origin: node.position.clone(),
            });
        } else {
            view.drag = Some(Drag::Pan {
                x: view.pan_x,
                y: view.pan_y,
            });
        }
    });
    let v = view.clone();
    let a = area.clone();
    gesture.connect_drag_update(move |g, dx, dy| {
        let mut view = v.borrow_mut();
        match view.drag.clone() {
            Some(Drag::Node { id, origin }) => {
                let zoom = view.zoom;
                view.preview = Some((
                    id,
                    Position {
                        x: ((origin.x + dx / zoom) / GRID).round() * GRID,
                        y: ((origin.y + dy / zoom) / GRID).round() * GRID,
                    },
                ));
            }
            Some(Drag::Pan { x, y }) => {
                view.pan_x = x + dx;
                view.pan_y = y + dy;
            }
            Some(Drag::Wire { .. }) => {
                if let Some((x, y)) = g.start_point() {
                    view.pointer = view.world(x + dx, y + dy);
                }
            }
            None => {}
        }
        a.queue_draw();
    });
    let e = engine.clone();
    let v = view.clone();
    let a = area.clone();
    let cb = callback.clone();
    gesture.connect_drag_end(move |_, _, _| {
        let mut view = v.borrow_mut();
        if let Some((id, position)) = view.preview.take() {
            e.dispatch(Control::Move { id, position });
        }
        if let Some(Drag::Wire { source }) = view.drag.take() {
            let state = e.snapshot().state;
            let (x, y) = view.pointer;
            if let Some(target) = state.nodes.iter().find(|n| {
                n.workspace == state.selected_workspace
                    && n.id != source
                    && (contains(n, &n.position, x, y)
                        || (x - n.position.x - WIDTH / 2.0).hypot(y - n.position.y) < 20.0)
            }) {
                e.dispatch(Control::Connect {
                    source,
                    target: target.id.clone(),
                });
            }
        }
        view.drag = None;
        let selected = view.selected.clone();
        drop(view);
        cb(CanvasAction::Select(selected));
        a.queue_draw();
    });
    area.add_controller(gesture);
    let pan = gtk::GestureDrag::new();
    pan.set_button(2);
    let v = view.clone();
    pan.connect_drag_begin(move |_, _, _| {
        let mut v = v.borrow_mut();
        v.motion_serial += 1;
        v.camera_target = None;
        v.drag = Some(Drag::Pan {
            x: v.pan_x,
            y: v.pan_y,
        });
    });
    let v = view.clone();
    let a = area.clone();
    pan.connect_drag_update(move |_, dx, dy| {
        let mut v = v.borrow_mut();
        if let Some(Drag::Pan { x, y }) = v.drag.clone() {
            v.pan_x = x + dx;
            v.pan_y = y + dy;
        }
        a.queue_draw();
    });
    let v = view.clone();
    pan.connect_drag_end(move |_, _, _| v.borrow_mut().drag = None);
    area.add_controller(pan);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    let v = view.clone();
    let a = area.clone();
    scroll.connect_scroll(move |controller, dx, dy| {
        let mut target = v.borrow().clone();
        if let Some((zoom, x, y)) = target.camera_target {
            target.zoom = zoom;
            target.pan_x = x;
            target.pan_y = y;
        }
        let old = target.zoom;
        target.zoom = (old * (-dy * 0.12).exp()).clamp(0.25, 2.0);
        let (cx, cy) = controller
            .current_event()
            .and_then(|event| event.position())
            .unwrap_or((a.width() as f64 / 2.0, a.height() as f64 / 2.0));
        target.pan_x = cx - (cx - target.pan_x) * target.zoom / old - dx * 30.0;
        target.pan_y = cy - (cy - target.pan_y) * target.zoom / old;
        animate_camera(&a, &v, &target);
        glib::Propagation::Stop
    });
    area.add_controller(scroll);
    let key = gtk::EventControllerKey::new();
    let e = engine.clone();
    let v = view.clone();
    let cb = callback;
    let a = area.clone();
    key.connect_key_pressed(move |_, key, _, modifiers| {
        let selected = v.borrow().selected.clone();
        match key {
            gdk::Key::space => v.borrow_mut().space_down = true,
            gdk::Key::Tab => cb(CanvasAction::Add),
            gdk::Key::Return => {
                if let Some(id) = selected {
                    cb(CanvasAction::Open(id));
                }
            }
            gdk::Key::Delete | gdk::Key::BackSpace => {
                if let Some(id) = selected {
                    cb(CanvasAction::Delete(id));
                }
            }
            gdk::Key::e => {
                if let Some(id) = selected {
                    cb(CanvasAction::Edit(id));
                }
            }
            gdk::Key::r => {
                if let Some(id) = selected {
                    cb(CanvasAction::Run(id));
                }
            }
            gdk::Key::Escape => cb(CanvasAction::Windows),
            gdk::Key::h | gdk::Key::Home => {
                fit_animated(&a, &v, &e.snapshot().state);
            }
            gdk::Key::Up | gdk::Key::Down | gdk::Key::Left | gdk::Key::Right => {
                let direction = match key {
                    gdk::Key::Up => "up",
                    gdk::Key::Down => "down",
                    gdk::Key::Left => "left",
                    _ => "right",
                };
                if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    if let Some(id) = selected
                        && let Some(n) = e.snapshot().state.node(&id)
                    {
                        let mut p = n.position.clone();
                        match direction {
                            "up" => p.y -= GRID,
                            "down" => p.y += GRID,
                            "left" => p.x -= GRID,
                            _ => p.x += GRID,
                        };
                        e.dispatch(Control::Move { id, position: p });
                    }
                } else {
                    let id = navigate(&a, &v, &e.snapshot().state, direction);
                    cb(CanvasAction::Select(id));
                }
                a.queue_draw();
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    let v = view.clone();
    key.connect_key_released(move |_, key, _, _| {
        if key == gdk::Key::space {
            v.borrow_mut().space_down = false;
        }
    });
    area.add_controller(key);
    area
}
fn contains(node: &Node, p: &Position, x: f64, y: f64) -> bool {
    x >= p.x && x <= p.x + WIDTH && y >= p.y - 8.0 && y <= p.y + node_height(node) + 8.0
}
pub fn draw(c: &Context, width: f64, height: f64, state: &State, view: &View) {
    let theme = &view.theme;
    c.select_font_face(
        &theme.font,
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    theme.surface.source(c);
    let _ = c.paint();
    draw_grid(c, view);
    let _ = c.save();
    c.translate(view.pan_x, view.pan_y);
    c.scale(view.zoom, view.zoom);
    let nodes: Vec<_> = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
        .collect();
    let by_id: HashMap<_, _> = state.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    for node in &nodes {
        let to = view.position(node);
        for dependency in &node.dependencies {
            if let Some(parent) = by_id.get(dependency.as_str()) {
                let from = view.position(parent);
                let x1 = (from.x + WIDTH / 2.0) * view.zoom + view.pan_x;
                let x2 = (to.x + WIDTH / 2.0) * view.zoom + view.pan_x;
                let y1 = (from.y + node_height(parent)) * view.zoom + view.pan_y;
                let y2 = to.y * view.zoom + view.pan_y;
                let bend = ((y2 - y1).abs() * 0.5).max(50.0 * view.zoom);
                if x1.max(x2) < -4.0
                    || x1.min(x2) > width + 4.0
                    || y1.max(y2) + bend < -4.0
                    || y1.min(y2) - bend > height + 4.0
                {
                    continue;
                }
                let active = parent.status == Status::Done;
                let rgb = if node.run_when == RunWhen::Failure {
                    theme.error.rgb()
                } else if active {
                    theme.primary.rgb()
                } else {
                    theme.outline.rgb()
                };
                c.set_source_rgb(rgb.0, rgb.1, rgb.2);
                c.set_line_width(1.6);
                wire(
                    c,
                    from.x + WIDTH / 2.0,
                    from.y + node_height(parent),
                    to.x + WIDTH / 2.0,
                    to.y,
                );
                let _ = c.stroke();
            }
        }
    }
    for w in state.watchers.iter().filter(|w| w.enabled) {
        if let (Some(a), Some(b)) = (by_id.get(w.source.as_str()), by_id.get(w.target.as_str()))
            && a.workspace == state.selected_workspace
        {
            let source_height = node_height(a);
            let a = view.position(a);
            let b = view.position(b);
            theme.primary.alpha(c, 0.65);
            c.set_line_width(1.3);
            c.set_dash(&[5.0, 5.0], 0.0);
            wire(
                c,
                a.x + WIDTH / 2.0,
                a.y + source_height,
                b.x + WIDTH / 2.0,
                b.y,
            );
            let _ = c.stroke();
            c.set_dash(&[], 0.0);
        }
    }
    if let Some(Drag::Wire { source }) = &view.drag
        && let Some(n) = state.node(source)
    {
        let p = view.position(n);
        theme.primary.source(c);
        c.set_line_width(2.0);
        wire(
            c,
            p.x + WIDTH / 2.0,
            p.y + node_height(n),
            view.pointer.0,
            view.pointer.1,
        );
        let _ = c.stroke();
    }
    for node in &nodes {
        let p = view.position(node);
        if p.x * view.zoom + view.pan_x > width + 12.0
            || (p.x + WIDTH) * view.zoom + view.pan_x < -12.0
            || p.y * view.zoom + view.pan_y > height + 12.0
            || (p.y + node_height(node)) * view.zoom + view.pan_y < -12.0
        {
            continue;
        }
        draw_node(
            c,
            node,
            &view.position(node),
            view.selected.as_deref() == Some(&node.id),
            theme,
        );
    }
    let _ = c.restore();
    if nodes.is_empty() {
        text(
            c,
            width / 2.0 - 155.0,
            height / 2.0 - 36.0,
            "A workspace for how you work.",
            21.0,
            theme.text.rgb(),
            false,
        );
        text(
            c,
            width / 2.0 - 155.0,
            height / 2.0,
            "Press Tab to create your first node.",
            13.0,
            theme.muted.rgb(),
            false,
        );
        text(
            c,
            width / 2.0 - 155.0,
            height / 2.0 + 29.0,
            "Agents, shell commands, metrics, and you.",
            12.0,
            theme.muted.rgb(),
            false,
        );
    }
    text(c, 24.0, 30.0, "WORKFLOW", 10.0, theme.muted.rgb(), false);
    text(
        c,
        24.0,
        height - 22.0,
        "Tab  add     Enter  expand     E  edit     R  run     H  frame all",
        11.0,
        theme.muted.rgb(),
        false,
    );
    text(
        c,
        width - 72.0,
        height - 22.0,
        &format!("{}%", (view.zoom * 100.0) as i32),
        11.0,
        theme.muted.rgb(),
        true,
    );
}
fn draw_node(c: &Context, n: &Node, p: &Position, selected: bool, theme: &Theme) {
    let (x, y) = (p.x, p.y);
    let height = node_height(n);
    let accent = theme.primary.rgb();
    if selected {
        rounded(c, x - 4.0, y - 4.0, WIDTH + 8.0, height + 8.0, 13.0);
        c.set_source_rgba(accent.0, accent.1, accent.2, 0.09);
        let _ = c.fill();
    }
    rounded(c, x, y + 4.0, WIDTH, height, theme.radius);
    c.set_source_rgba(0.0, 0.0, 0.0, 0.18);
    let _ = c.fill();
    rounded(c, x, y, WIDTH, height, theme.radius);
    theme.surface_high.source(c);
    let _ = c.fill_preserve();
    if selected {
        theme.primary.source(c);
    } else {
        theme.outline.alpha(c, 0.5);
    }
    c.set_line_width(if selected { 1.4 } else { 1.0 });
    let _ = c.stroke();
    rounded(c, x + 14.0, y + 16.0, 25.0, 23.0, 5.0);
    c.set_source_rgba(accent.0, accent.1, accent.2, 0.13);
    let _ = c.fill();
    let glyph = match n.kind {
        NodeKind::Agent => "*",
        NodeKind::Shell => ">_",
        NodeKind::Metric => "~",
        NodeKind::Gate => "?",
        NodeKind::Human => "@",
        NodeKind::Window => "[]",
    };
    text(c, x + 20.0, y + 32.0, glyph, 12.0, accent, true);
    text(
        c,
        x + 49.0,
        y + 31.0,
        &n.kind.label().to_uppercase(),
        9.0,
        theme.muted.rgb(),
        false,
    );
    let status_color = match n.status {
        Status::Running => accent,
        Status::Done => accent,
        Status::Failed => theme.error.rgb(),
        Status::Review => accent,
        _ => theme.muted.rgb(),
    };
    c.arc(x + WIDTH - 82.0, y + 27.0, 3.0, 0.0, std::f64::consts::TAU);
    c.set_source_rgb(status_color.0, status_color.1, status_color.2);
    let _ = c.fill();
    text(
        c,
        x + WIDTH - 72.0,
        y + 31.0,
        if n.desktop_window && n.window_id.is_none() {
            "Closed"
        } else if n.window_id.is_some() && n.kind == NodeKind::Window {
            "Open"
        } else {
            n.status.label()
        },
        9.0,
        status_color,
        false,
    );
    text(
        c,
        x + 17.0,
        y + 62.0,
        &ellipsize(&n.title, 31),
        15.0,
        theme.text.rgb(),
        false,
    );
    if n.kind == NodeKind::Metric && !n.metric.samples.is_empty() {
        let value = n.metric.samples.last().unwrap().value;
        text(
            c,
            x + 17.0,
            y + 99.0,
            &format!("{value:.0} {}", n.metric.unit),
            24.0,
            theme.text.rgb(),
            false,
        );
        sparkline(
            c,
            &n.metric.samples,
            (x + 130.0, y + 76.0, 140.0, 33.0),
            accent,
        );
    } else {
        let snippet = n
            .command
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("Select a window in Parameters");
        let prefix = match n.kind {
            NodeKind::Agent => "codex · ",
            NodeKind::Shell => "$ ",
            _ => "",
        };
        text(
            c,
            x + 17.0,
            y + 90.0,
            &ellipsize(&format!("{prefix}{snippet}"), 35),
            10.5,
            theme.muted.rgb(),
            true,
        );
        if n.desktop_window {
            text(
                c,
                x + 17.0,
                y + 119.0,
                if n.window_id.is_some() {
                    "Enter to return to this window"
                } else {
                    "Window closed · graph retained"
                },
                10.0,
                theme.muted.rgb(),
                false,
            );
        }
    }
    c.move_to(x + 1.0, y + height - 36.0);
    c.line_to(x + WIDTH - 1.0, y + height - 36.0);
    theme.outline.alpha(c, 0.3);
    c.set_line_width(1.0);
    let _ = c.stroke();
    let footer = if n.desktop_window {
        if n.window_id.is_some() {
            "Window open on this row"
        } else {
            "Window closed"
        }
        .to_string()
    } else if n.status == Status::Review {
        "Waiting for your decision".to_string()
    } else if n.status == Status::Failed {
        "Issue raised · inspect output".into()
    } else if n.kind == NodeKind::Human {
        "Working alongside your agents".into()
    } else if n.kind == NodeKind::Metric {
        format!(
            "every {}s  ·  {} samples",
            n.metric.interval_secs,
            n.metric.samples.len()
        )
    } else {
        let name = n.cwd.file_name().unwrap_or_default().to_string_lossy();
        format!(
            "~/{}{}",
            name,
            if n.writable { "  ·  can edit" } else { "" }
        )
    };
    text(
        c,
        x + 17.0,
        y + height - 14.0,
        &ellipsize(&footer, 36),
        9.5,
        theme.muted.rgb(),
        true,
    );
    for py in [y, y + height] {
        c.new_sub_path();
        c.arc(x + WIDTH / 2.0, py, 5.0, 0.0, std::f64::consts::TAU);
        theme.surface_high.source(c);
        let _ = c.fill_preserve();
        c.set_source_rgb(accent.0 * 0.75, accent.1 * 0.75, accent.2 * 0.75);
        c.set_line_width(1.3);
        let _ = c.stroke();
    }
}
fn wire(c: &Context, x1: f64, y1: f64, x2: f64, y2: f64) {
    let bend = ((y2 - y1).abs() * 0.5).max(50.0);
    c.move_to(x1, y1);
    c.curve_to(x1, y1 + bend, x2, y2 - bend, x2, y2);
}
fn sparkline(c: &Context, samples: &[Sample], rect: (f64, f64, f64, f64), rgb: (f64, f64, f64)) {
    let (x, y, w, h) = rect;
    let min = samples
        .iter()
        .map(|s| s.value)
        .fold(f64::INFINITY, f64::min);
    let max = samples
        .iter()
        .map(|s| s.value)
        .fold(f64::NEG_INFINITY, f64::max);
    for (i, sample) in samples.iter().enumerate() {
        let px = x + w * i as f64 / (samples.len() - 1).max(1) as f64;
        let py = y + h - (sample.value - min) / (max - min).max(1.0) * h;
        if i == 0 {
            c.move_to(px, py);
        } else {
            c.line_to(px, py);
        }
    }
    c.set_source_rgb(rgb.0, rgb.1, rgb.2);
    c.set_line_width(1.8);
    let _ = c.stroke();
}
fn rounded(c: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    c.new_sub_path();
    c.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    c.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    c.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    c.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    c.close_path();
}
fn text(c: &Context, x: f64, y: f64, text: &str, size: f64, rgb: (f64, f64, f64), mono: bool) {
    let _ = c.save();
    if mono {
        c.select_font_face(
            "monospace",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Normal,
        );
    }
    c.set_font_size(size);
    c.set_source_rgb(rgb.0, rgb.1, rgb.2);
    c.move_to(x, y);
    let _ = c.show_text(text);
    let _ = c.restore();
}
fn ellipsize(s: &str, length: usize) -> String {
    if s.chars().count() > length {
        format!("{}…", s.chars().take(length - 1).collect::<String>())
    } else {
        s.into()
    }
}

#[cfg(test)]
mod rendering_tests {
    use super::*;
    fn raster(width: i32, height: i32, render: impl FnOnce(&Context)) -> Vec<u8> {
        let mut image = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).unwrap();
        {
            let c = Context::new(&image).unwrap();
            render(&c);
        }
        image.flush();
        image.data().unwrap().to_vec()
    }
    fn pixel(bytes: &[u8], width: usize, x: usize, y: usize) -> u32 {
        let offset = (y * width + x) * 4;
        u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
    #[test]
    fn grid_cache_tracks_pan_zoom_and_palette() {
        let mut view = View {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            theme: Theme::default(),
            ..View::default()
        };
        view.theme.outline = super::super::theme::Color(1.0, 0.0, 0.0);
        let first = raster(80, 80, |c| draw_grid(c, &view));
        assert!(pixel(&first, 80, 20, 20) & 0x00ff0000 > 0);
        view.pan_x = 7.0;
        view.pan_y = 3.0;
        let moved = raster(80, 80, |c| draw_grid(c, &view));
        assert_eq!(pixel(&moved, 80, 20, 20), 0);
        assert!(pixel(&moved, 80, 27, 23) & 0x00ff0000 > 0);
        view.theme.outline = super::super::theme::Color(0.0, 1.0, 0.0);
        view.zoom = 0.5;
        let themed = raster(80, 80, |c| draw_grid(c, &view));
        assert_eq!(pixel(&themed, 80, 17, 13) & 0x00ff0000, 0);
        assert!(pixel(&themed, 80, 17, 13) & 0x0000ff00 > 0);
    }
    #[test]
    fn culling_keeps_partial_cards_and_skips_offscreen_cards_without_changing_pixels() {
        let mut state = State::empty("/tmp".into());
        let edge = Node::new(
            &state.selected_workspace,
            NodeKind::Shell,
            "/tmp".into(),
            Position {
                x: -WIDTH + 8.0,
                y: 50.0,
            },
        );
        state.nodes.push(edge);
        let view = View {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            theme: Theme::default(),
            ..View::default()
        };
        let first = raster(100, 300, |c| draw(c, 100.0, 300.0, &state, &view));
        assert_ne!(pixel(&first, 100, 1, 127), pixel(&first, 100, 90, 127));
        for i in 0..1000 {
            state.nodes.push(Node::new(
                &state.selected_workspace,
                NodeKind::Agent,
                "/tmp".into(),
                Position {
                    x: 10000.0 + (i % 20) as f64 * 360.0,
                    y: 10000.0 + (i / 20) as f64 * 240.0,
                },
            ));
        }
        let large = raster(100, 300, |c| draw(c, 100.0, 300.0, &state, &view));
        assert_eq!(first, large);
    }
}
