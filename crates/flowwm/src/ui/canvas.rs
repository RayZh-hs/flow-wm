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
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

pub const WIDTH: f64 = 292.0;
pub const HEIGHT: f64 = 154.0;
pub fn node_height(node: &Node) -> f64 {
    if node.closed_window() {
        super::layout::CLOSED_HEIGHT
    } else if node.kind == NodeKind::Window || node.window_id.is_some() {
        HEIGHT + 120.0
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
    Chat,
    ConnectSearch,
    Arrange,
    Copy(bool),
    Paste,
    Duplicate,
    EditValue(String),
    Search,
    Help,
    ClearHistory,
    Camera,
    Escape,
}
#[derive(Clone)]
enum Drag {
    Node,
    Pan {
        x: f64,
        y: f64,
    },
    Wire {
        source: String,
    },
    Pin {
        source: crate::graph::Endpoint,
    },
    Box {
        start: (f64, f64),
        end: (f64, f64),
        additive: bool,
    },
    Cut {
        points: Vec<(f64, f64)>,
    },
}
#[derive(Clone)]
pub struct View {
    pub gpu: bool,
    pub native_nodes: bool,
    pub theme: Theme,
    pub selected: Option<String>,
    pub selection: HashSet<String>,
    pub query: String,
    pub numbers: bool,
    pub images: HashMap<String, cairo::ImageSurface>,
    pub wallpaper: Option<cairo::ImageSurface>,
    pub image_textures: HashMap<String, gdk::Texture>,
    pub wallpaper_texture: Option<gdk::Texture>,
    pub desktop_rects: HashMap<String, super::collapse::Rect>,
    pub presentations: HashMap<String, f64>,
    pub live_fade: f64,
    pub chrome: f64,
    pub insets: [i32; 4],
    pub output_size: (f64, f64),
    pub transparent: bool,
    pub morphing: bool,
    pub previews: HashMap<String, Position>,
    pub presented_positions: HashMap<String, Position>,
    pub arrangement: super::layout::Arrangement,
    origins: HashMap<String, Position>,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    drag: Option<Drag>,
    pub preview: Option<(String, Position)>,
    pointer: (f64, f64),
    pub workspace: String,
    pub fitted: bool,
    motion_serial: u64,
    camera_target: Option<(f64, f64, f64)>,
    space_down: bool,
    empty_hover: Option<super::empty::Action>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            gpu: false,
            native_nodes: false,
            theme: Theme::load(),
            selected: None,
            selection: HashSet::new(),
            query: String::new(),
            numbers: false,
            images: HashMap::new(),
            wallpaper: None,
            image_textures: HashMap::new(),
            wallpaper_texture: None,
            desktop_rects: HashMap::new(),
            presentations: HashMap::new(),
            live_fade: 0.0,
            chrome: 1.0,
            insets: [0; 4],
            output_size: (1920.0, 1080.0),
            transparent: false,
            morphing: false,
            previews: HashMap::new(),
            presented_positions: HashMap::new(),
            arrangement: super::layout::Arrangement::default(),
            origins: HashMap::new(),
            zoom: 0.9,
            pan_x: 60.0,
            pan_y: 60.0,
            drag: None,
            preview: None,
            pointer: (0.0, 0.0),
            workspace: String::new(),
            fitted: false,
            motion_serial: 0,
            camera_target: None,
            space_down: false,
            empty_hover: None,
        }
    }
}

pub fn animate_camera(area: &super::surface::Surface, view: &Rc<RefCell<View>>, target: &View) {
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
pub fn fit_animated(area: &super::surface::Surface, view: &Rc<RefCell<View>>, state: &State) {
    view.borrow_mut()
        .arrangement
        .update(state, area.width(), area.height());
    if area.width() < 100 || area.height() < 100 {
        return;
    }
    let mut target = view.borrow().clone();
    target.fit(state, area.width() as f64, area.height() as f64);
    animate_camera(area, view, &target);
}
pub fn fit_immediate(area: &super::surface::Surface, view: &Rc<RefCell<View>>, state: &State) {
    let mut view = view.borrow_mut();
    view.motion_serial += 1;
    view.camera_target = None;
    view.arrangement.update(state, area.width(), area.height());
    view.fit(state, area.width() as f64, area.height() as f64);
    area.queue_draw();
}
pub fn suspend(view: &Rc<RefCell<View>>) {
    let mut view = view.borrow_mut();
    view.motion_serial += 1;
    view.camera_target = None;
    view.space_down = false;
    view.numbers = false;
    view.previews.clear();
    view.drag = None;
    view.preview = None;
    view.empty_hover = None;
}

pub fn restore(view: &Rc<RefCell<View>>, mut saved: View) {
    // Animation callbacks outlive a row change; never reuse their serials.
    saved.motion_serial = view.borrow().motion_serial + 1;
    *view.borrow_mut() = saved;
    suspend(view);
}
pub fn navigate(
    area: &super::surface::Surface,
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
pub fn pan_keyboard(
    area: &super::surface::Surface,
    view: &Rc<RefCell<View>>,
    direction: &str,
    fast: bool,
) {
    let mut target = view.borrow().clone();
    if let Some((z, x, y)) = target.camera_target {
        target.zoom = z;
        target.pan_x = x;
        target.pan_y = y;
    }
    let step = if fast { 240. } else { 70. };
    match direction {
        "left" => target.pan_x += step,
        "right" => target.pan_x -= step,
        "up" => target.pan_y += step,
        _ => target.pan_y -= step,
    }
    animate_camera(area, view, &target);
}
pub fn move_selection(engine: &Engine, view: &Rc<RefCell<View>>, direction: &str) {
    let state = engine.snapshot().state;
    let view = view.borrow();
    for node in state.nodes.iter().filter(|n| {
        n.workspace == state.selected_workspace
            && (view.selection.contains(&n.id) || view.selected.as_ref() == Some(&n.id))
    }) {
        let mut position = node.position.clone();
        match direction {
            "up" => position.y -= GRID,
            "down" => position.y += GRID,
            "left" => position.x -= GRID,
            _ => position.x += GRID,
        }
        engine.dispatch(Control::Move {
            id: node.id.clone(),
            position,
        });
    }
}

pub fn focus_node(
    area: &super::surface::Surface,
    view: &Rc<RefCell<View>>,
    state: &State,
    id: &str,
) {
    let Some(node) = state.node(id) else {
        return;
    };
    let mut target = view.borrow().clone();
    target.zoom = target.zoom.clamp(0.85, 1.2);
    let position = target.position(node);
    target.pan_x = area.width() as f64 / 2.0 - (position.x + WIDTH / 2.0) * target.zoom;
    target.pan_y =
        area.height() as f64 / 2.0 - (position.y + node_height(node) / 2.0) * target.zoom;
    animate_camera(area, view, &target);
}
impl View {
    pub fn for_row(&self, workspace: String) -> Self {
        Self {
            workspace,
            native_nodes: self.native_nodes,
            theme: self.theme.clone(),
            images: self.images.clone(),
            image_textures: self.image_textures.clone(),
            wallpaper: self.wallpaper.clone(),
            wallpaper_texture: self.wallpaper_texture.clone(),
            ..Self::default()
        }
    }
    pub fn interaction_key(&self) -> String {
        format!(
            "{}:{}:{:?}:{:?}",
            self.pointer.0,
            self.pointer.1,
            self.empty_hover,
            self.drag.as_ref().map(|d| match d {
                Drag::Box { start, end, .. } => format!("box:{start:?}:{end:?}"),
                Drag::Cut { points } => format!("cut:{points:?}"),
                Drag::Wire { source } => format!("wire:{source}"),
                Drag::Pin { source } => format!("pin:{}:{}", source.node, source.pin),
                _ => String::new(),
            })
        )
    }
    pub fn world(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.pan_x) / self.zoom, (y - self.pan_y) / self.zoom)
    }
    pub fn position(&self, node: &Node) -> Position {
        if let Some(position) = self.previews.get(&node.id) {
            return position.clone();
        }
        self.preview
            .as_ref()
            .filter(|(id, _)| id == &node.id)
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| {
                let base = self.arrangement.position(node);
                let offset = self
                    .presented_positions
                    .get(&node.id)
                    .cloned()
                    .unwrap_or_default();
                Position {
                    x: base.x + offset.x,
                    y: base.y + offset.y,
                }
            })
    }
    pub fn stored_position(&self, node: &Node, position: &Position) -> Position {
        let shown = self.position(node);
        Position {
            x: node.position.x + position.x - shown.x,
            y: node.position.y + position.y - shown.y,
        }
    }
    pub fn fit(&mut self, state: &State, width: f64, height: f64) {
        self.fitted = true;
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
            .map(|n| self.arrangement.position(n).x)
            .fold(f64::INFINITY, f64::min);
        let min_y = nodes
            .iter()
            .map(|n| self.arrangement.position(n).y)
            .fold(f64::INFINITY, f64::min);
        let max_x = nodes
            .iter()
            .map(|n| self.arrangement.position(n).x + WIDTH)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = nodes
            .iter()
            .map(|n| self.arrangement.position(n).y + node_height(n))
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
            .filter(|n| n.workspace == state.selected_workspace && matches_query(n, &self.query))
            .collect();
        let current = self.selected.as_deref().and_then(|id| state.node(id));
        let target = if let Some(current) = current {
            nodes
                .iter()
                .filter(|n| n.id != current.id)
                .filter_map(|n| {
                    let p = self.position(n);
                    let origin = self.position(current);
                    let dx = p.x - origin.x;
                    let dy = p.y - origin.y;
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
        let position = self.position(target);
        let left = position.x * self.zoom + self.pan_x;
        let top = position.y * self.zoom + self.pan_y;
        if left < 30.0 || left + WIDTH * self.zoom > width - 30.0 {
            self.pan_x = width / 2.0 - (position.x + WIDTH / 2.0) * self.zoom;
        }
        if top < 30.0 || top + node_height(target) * self.zoom > height - 30.0 {
            self.pan_y = height / 2.0 - (position.y + node_height(target) / 2.0) * self.zoom;
        }
        Some(target.id.clone())
    }
}

pub fn make(
    engine: Engine,
    view: Rc<RefCell<View>>,
    callback: Callback,
) -> super::surface::Surface {
    make_scoped(engine, view, callback, None)
}

pub type RowViews = Rc<RefCell<HashMap<String, View>>>;

fn canvas_node(node: &Node) -> Node {
    Node {
        flow: node.flow.clone(),
        id: node.id.clone(),
        workspace: node.workspace.clone(),
        title: node.title.clone(),
        kind: node.kind,
        command: node.command.clone(),
        cwd: node.cwd.clone(),
        position: node.position.clone(),
        dependencies: node.dependencies.clone(),
        run_when: node.run_when,
        review: node.review,
        writable: node.writable,
        status: node.status,
        metric: node.metric.clone(),
        window_match: node.window_match.clone(),
        window_id: node.window_id,
        desktop_window: node.desktop_window,
        column: node.column.clone(),
        column_mode: node.column_mode,
        started_at: node.started_at,
        finished_at: node.finished_at,
        output: String::new(),
        logs: vec![],
        chat: vec![],
        agent_session: None,
    }
}

pub(super) fn row_state(state: &State, workspace: &str) -> State {
    State {
        connections: state
            .connections
            .iter()
            .filter(|e| {
                state
                    .node(&e.source.node)
                    .is_some_and(|n| n.workspace == workspace)
            })
            .cloned()
            .collect(),
        types: state.types.clone(),
        casts: state.casts.clone(),
        node_types: state.node_types.clone(),
        version: state.version,
        selected_workspace: workspace.into(),
        workspaces: state
            .workspaces
            .iter()
            .filter(|w| w.id == workspace)
            .cloned()
            .collect(),
        nodes: state
            .nodes
            .iter()
            .filter(|n| n.workspace == workspace)
            .map(canvas_node)
            .collect(),
        watchers: state.watchers.clone(),
        recipes: vec![],
    }
}

pub fn make_row(
    engine: Engine,
    view: Rc<RefCell<View>>,
    callback: Callback,
    workspace: String,
    rows: RowViews,
) -> super::surface::Surface {
    make_scoped(engine, view, callback, Some((workspace, rows)))
}

fn make_scoped(
    engine: Engine,
    view: Rc<RefCell<View>>,
    callback: Callback,
    row: Option<(String, RowViews)>,
) -> super::surface::Surface {
    let e = engine.clone();
    let v = view.clone();
    let cache = RefCell::new(super::gpu::Cache::default());
    let publisher = super::native::Publisher::default();
    let on_map = publisher.clone();
    let scoped = RefCell::new(None::<(u64, State)>);
    let area = super::surface::Surface::new(move |snapshot, width, height| {
        let started = std::time::Instant::now();
        let state = e.snapshot();
        let revision = state.revision;
        let render = |state: &State, view: &View| {
            if view.native_nodes
                && let Some(row) = &state.workspace().row
            {
                publisher.publish(
                    row.id,
                    niri_ipc::node_row::NodeRowScene {
                        windows: super::gpu::native_windows(state, view),
                        insets: view.insets,
                    },
                );
            }
            super::gpu::render(
                snapshot,
                width,
                height,
                state,
                revision,
                view,
                &mut cache.borrow_mut(),
            );
        };
        if let Some((id, rows)) = &row {
            let mut scoped = scoped.borrow_mut();
            if scoped
                .as_ref()
                .is_none_or(|(revision, _)| *revision != state.revision)
            {
                let selected = row_state(&state.state, id);
                *scoped = Some((state.revision, selected));
            }
            let state = &scoped.as_ref().unwrap().1;
            let mut current = v.borrow_mut();
            if current.workspace == *id {
                if !current.fitted && width > 100 && height > 100 {
                    current.arrangement.update(state, width, height);
                    current.fit(state, width as f64, height as f64);
                }
                render(state, &current);
            } else {
                let mut rows = rows.borrow_mut();
                let saved = rows
                    .entry(id.clone())
                    .or_insert_with(|| current.for_row(id.clone()));
                saved.theme = current.theme.clone();
                saved.wallpaper = current.wallpaper.clone();
                saved.wallpaper_texture = current.wallpaper_texture.clone();
                saved.images.extend(current.images.clone());
                saved.image_textures.extend(current.image_textures.clone());
                saved.arrangement.update(state, width, height);
                if !saved.fitted && width > 100 && height > 100 {
                    saved.fit(state, width as f64, height as f64);
                    saved.fitted = true;
                }
                render(state, saved);
            }
        } else {
            render(&state.state, &v.borrow());
        }
        e.performance.record("canvas_draw", started.elapsed());
    });
    area.connect_map(move |_| on_map.invalidate());
    area.set_cursor_from_name(Some("default"));
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let empty_pressed = Rc::new(Cell::new(None));
    let pressed = empty_pressed.clone();
    let e = engine.clone();
    let v = view.clone();
    let cb = callback.clone();
    let a = area.clone();
    click.connect_pressed(move |gesture, count, x, y| {
        a.grab_focus();
        let state = e.snapshot().state;
        let mut view = v.borrow_mut();
        let action = (!view.space_down && workspace_empty(&state))
            .then(|| super::empty::hit(a.width() as f64, a.height() as f64, x, y))
            .flatten();
        pressed.set(action);
        if action.is_some() {
            drop(view);
            gesture.set_state(gtk::EventSequenceState::Claimed);
            return;
        }
        let (x, y) = view.world(x, y);
        let hit = state.nodes.iter().rev().find(|n| {
            n.workspace == state.selected_workspace && contains(n, &view.position(n), x, y)
        });
        let additive = gesture
            .current_event_state()
            .intersects(gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::CONTROL_MASK);
        if !additive && hit.is_none_or(|n| !view.selection.contains(&n.id)) {
            view.selection.clear();
        }
        if let Some(node) = hit {
            if additive && view.selection.contains(&node.id) {
                view.selection.remove(&node.id);
            } else {
                view.selection.insert(node.id.clone());
            }
        }
        view.selected = hit
            .filter(|n| view.selection.contains(&n.id))
            .map(|n| n.id.clone())
            .or_else(|| view.selection.iter().next().cloned());
        let selected = view.selected.clone();
        let inline = hit
            .filter(|n| {
                n.kind == NodeKind::Constant
                    && n.flow.editable_in_graph
                    && y > view.position(n).y + 70.
                    && y < view.position(n).y + 112.
            })
            .map(|n| n.id.clone());
        let background = hit.is_none();
        drop(view);
        if background {
            cb(CanvasAction::Camera);
        }
        cb(CanvasAction::Select(selected));
        if let Some(id) = inline {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            cb(CanvasAction::EditValue(id));
            return;
        }
        if count == 2
            && let Some(node) = hit
        {
            cb(CanvasAction::Open(node.id.clone()));
        }
        a.queue_draw();
    });
    let e = engine.clone();
    let cb = callback.clone();
    let a = area.clone();
    click.connect_released(move |_, _, x, y| {
        if let Some(action) = empty_pressed.take()
            && workspace_empty(&e.snapshot().state)
            && super::empty::hit(a.width() as f64, a.height() as f64, x, y) == Some(action)
        {
            cb(action.canvas_action());
        }
    });
    area.add_controller(click);
    let motion = gtk::EventControllerMotion::new();
    let e = engine.clone();
    let v = view.clone();
    let a = area.clone();
    motion.connect_motion(move |_, x, y| {
        let state = e.snapshot().state;
        let view = v.borrow();
        let (wx, wy) = view.world(x, y);
        let hovered_pin = hit_pin(&state, &view, wx, wy, true)
            .map(|p| (p, true))
            .or_else(|| hit_pin(&state, &view, wx, wy, false).map(|p| (p, false)));
        let tooltip = hovered_pin.and_then(|(endpoint, output)| {
            state.node(&endpoint.node).and_then(|n| {
                crate::graph::pins(&state, n, output)
                    .into_iter()
                    .find(|p| p.name == endpoint.pin)
                    .map(|p| format!("{}.{}: {}", n.title, p.name, p.ty))
            })
        });
        drop(view);
        a.set_tooltip_text(tooltip.as_deref());
        let hit = super::empty::hit(a.width() as f64, a.height() as f64, x, y);
        let hover = hit.filter(|_| !v.borrow().space_down && workspace_empty(&e.snapshot().state));
        if v.borrow().empty_hover != hover {
            v.borrow_mut().empty_hover = hover;
            a.set_cursor_from_name(Some(if hover.is_some() {
                "pointer"
            } else {
                "default"
            }));
            a.queue_draw();
        }
    });
    let v = view.clone();
    let a = area.clone();
    motion.connect_leave(move |_| {
        if v.borrow_mut().empty_hover.take().is_some() {
            a.set_cursor_from_name(Some("default"));
            a.queue_draw();
        }
    });
    area.add_controller(motion);
    let gesture = gtk::GestureDrag::new();
    gesture.set_button(1);
    let e = engine.clone();
    let v = view.clone();
    let a = area.clone();
    let cb = callback.clone();
    gesture.connect_drag_begin(move |g, x, y| {
        a.grab_focus();
        let panning = v.borrow().space_down;
        if panning {
            cb(CanvasAction::Camera);
        }
        let state = e.snapshot().state;
        if !panning
            && workspace_empty(&state)
            && super::empty::hit(a.width() as f64, a.height() as f64, x, y).is_some()
        {
            g.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        let mut view = v.borrow_mut();
        view.motion_serial += 1;
        view.camera_target = None;
        let (wx, wy) = view.world(x, y);
        view.pointer = (wx, wy);
        if panning {
            view.drag = Some(Drag::Pan {
                x: view.pan_x,
                y: view.pan_y,
            });
        } else if let Some(source) = hit_pin(&state, &view, wx, wy, true) {
            view.drag = Some(Drag::Pin { source });
        } else if let Some(node) = state.nodes.iter().find(|n| {
            n.workspace == state.selected_workspace
                && (wx - view.position(n).x - WIDTH / 2.0)
                    .hypot(wy - view.position(n).y - node_height(n))
                    < 12.0
        }) {
            view.drag = Some(Drag::Wire {
                source: node.id.clone(),
            });
        } else if let Some(node) = state.nodes.iter().rev().find(|n| {
            n.workspace == state.selected_workspace && contains(n, &view.position(n), wx, wy)
        }) {
            if g.current_event_state()
                .intersects(gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::CONTROL_MASK)
            {
                g.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            if !view.selection.contains(&node.id) {
                view.selection.clear();
                view.selection.insert(node.id.clone());
            }
            view.selected = Some(node.id.clone());
            view.origins = state
                .nodes
                .iter()
                .filter(|n| view.selection.contains(&n.id))
                .map(|n| (n.id.clone(), view.position(n)))
                .collect();
            view.drag = Some(Drag::Node);
        } else {
            let additive = g
                .current_event_state()
                .intersects(gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::CONTROL_MASK);
            if !additive {
                view.selection.clear();
                view.selected = None;
            }
            view.drag = Some(Drag::Box {
                start: (wx, wy),
                end: (wx, wy),
                additive,
            });
        }
    });
    let v = view.clone();
    let a = area.clone();
    gesture.connect_drag_update(move |g, dx, dy| {
        let mut view = v.borrow_mut();
        match view.drag.clone() {
            Some(Drag::Node) => {
                let dx = (dx / view.zoom / GRID).round() * GRID;
                let dy = (dy / view.zoom / GRID).round() * GRID;
                view.previews = view
                    .origins
                    .iter()
                    .map(|(id, p)| {
                        (
                            id.clone(),
                            Position {
                                x: p.x + dx,
                                y: p.y + dy,
                            },
                        )
                    })
                    .collect();
            }
            Some(Drag::Pan { x, y }) => {
                view.pan_x = x + dx;
                view.pan_y = y + dy;
            }
            Some(Drag::Wire { .. } | Drag::Pin { .. }) => {
                if let Some((x, y)) = g.start_point() {
                    view.pointer = view.world(x + dx, y + dy);
                }
            }
            Some(Drag::Box {
                start, additive, ..
            }) => {
                if let Some((x, y)) = g.start_point() {
                    let end = view.world(x + dx, y + dy);
                    view.drag = Some(Drag::Box {
                        start,
                        end,
                        additive,
                    });
                }
            }
            _ => {}
        }
        a.queue_draw();
    });
    let e = engine.clone();
    let v = view.clone();
    let a = area.clone();
    let cb = callback.clone();
    gesture.connect_drag_end(move |_, _, _| {
        let state = e.snapshot().state;
        let mut view = v.borrow_mut();
        let moves = std::mem::take(&mut view.previews);
        for (id, position) in moves {
            if let Some(node) = state.node(&id) {
                let position = view.stored_position(node, &position);
                e.dispatch(Control::Move { id, position });
            }
        }
        match view.drag.take() {
            Some(Drag::Pin { source }) => {
                if let Some(target) = hit_pin(&state, &view, view.pointer.0, view.pointer.1, false)
                {
                    e.dispatch(Control::ConnectPins {
                        connection: crate::graph::Connection { source, target },
                    });
                }
            }
            Some(Drag::Wire { source }) => {
                let (x, y) = view.pointer;
                if let Some(target) = state.nodes.iter().find(|n| {
                    n.workspace == state.selected_workspace
                        && n.id != source
                        && contains(n, &view.position(n), x, y)
                }) {
                    e.dispatch(Control::Connect {
                        source,
                        target: target.id.clone(),
                    });
                }
            }
            Some(Drag::Box { start, end, .. }) => {
                for n in state
                    .nodes
                    .iter()
                    .filter(|n| n.workspace == state.selected_workspace)
                {
                    if intersects(
                        (
                            start.0.min(end.0),
                            start.1.min(end.1),
                            start.0.max(end.0),
                            start.1.max(end.1),
                        ),
                        n,
                        &view.position(n),
                    ) {
                        view.selection.insert(n.id.clone());
                    }
                }
                view.selected = ordered_nodes(&state)
                    .into_iter()
                    .find(|n| view.selection.contains(&n.id))
                    .map(|n| n.id.clone());
            }
            _ => {}
        }
        view.origins.clear();
        let selected = view.selected.clone();
        drop(view);
        cb(CanvasAction::Select(selected));
        a.queue_draw();
    });
    area.add_controller(gesture);
    let pan = gtk::GestureDrag::new();
    pan.set_button(2);
    let v = view.clone();
    let cb = callback.clone();
    pan.connect_drag_begin(move |_, _, _| {
        cb(CanvasAction::Camera);
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
    let cut = gtk::GestureDrag::new();
    cut.set_button(3);
    let v = view.clone();
    let a = area.clone();
    cut.connect_drag_begin(move |_, x, y| {
        a.grab_focus();
        let mut v = v.borrow_mut();
        let p = v.world(x, y);
        v.drag = Some(Drag::Cut { points: vec![p] });
    });
    let v = view.clone();
    let a = area.clone();
    cut.connect_drag_update(move |g, dx, dy| {
        let mut v = v.borrow_mut();
        if let Some((x, y)) = g.start_point() {
            let p = v.world(x + dx, y + dy);
            if let Some(Drag::Cut { points }) = &mut v.drag {
                points.push(p);
            }
        }
        a.queue_draw();
    });
    let v = view.clone();
    let e = engine.clone();
    let a = area.clone();
    cut.connect_drag_end(move |_, _, _| {
        let points = match v.borrow_mut().drag.take() {
            Some(Drag::Cut { points }) => points,
            _ => return,
        };
        let state = e.snapshot().state;
        let view = v.borrow();
        for edge in &state.connections {
            if let (Some(source), Some(target)) =
                (state.node(&edge.source.node), state.node(&edge.target.node))
                && source.workspace == state.selected_workspace
                && let (Some(from), Some(to)) = (
                    pin_position(
                        &state,
                        source,
                        &view.position(source),
                        &edge.source.pin,
                        true,
                    ),
                    pin_position(
                        &state,
                        target,
                        &view.position(target),
                        &edge.target.pin,
                        false,
                    ),
                )
                && pin_wire_cut(from, to, &points)
            {
                e.dispatch(Control::DisconnectPins {
                    connection: edge.clone(),
                });
            }
        }
        for node in state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
        {
            for source in &node.dependencies {
                if let Some(parent) = state.node(source)
                    && wire_cut(parent, node, &points, &view)
                {
                    e.dispatch(Control::Disconnect {
                        source: source.clone(),
                        target: node.id.clone(),
                    });
                }
            }
        }
        for watcher in state.watchers.iter().filter(|w| w.enabled) {
            if let (Some(a), Some(b)) = (state.node(&watcher.source), state.node(&watcher.target))
                && a.workspace == state.selected_workspace
                && wire_cut(a, b, &points, &view)
            {
                e.dispatch(Control::DeleteWatcher {
                    id: watcher.id.clone(),
                });
            }
        }
        a.queue_draw();
    });
    area.add_controller(cut);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    let v = view.clone();
    let a = area.clone();
    let cb = callback.clone();
    scroll.connect_scroll(move |controller, dx, dy| {
        cb(CanvasAction::Camera);
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
    let shortcuts: Vec<_> = crate::config::shortcuts()
        .into_iter()
        .filter(|s| s.scope == "graph")
        .flat_map(|s| {
            s.keys.into_iter().filter_map(move |key| {
                gtk::accelerator_parse(&key).map(|(key, mods)| (s.action.clone(), key, mods))
            })
        })
        .collect();
    key.connect_key_pressed(move |_, key, _, modifiers| {
        let selected = v.borrow().selected.clone();
        let state = e.snapshot().state;
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        if matches!(key, gdk::Key::Alt_L | gdk::Key::Alt_R) {
            v.borrow_mut().numbers = true;
            a.queue_draw();
            return glib::Propagation::Stop;
        }
        if let Some(number) = key.to_unicode().and_then(|c| c.to_digit(10))
            && (alt || v.borrow().space_down)
        {
            let index = if number == 0 { 9 } else { number as usize - 1 };
            if let Some(target) = ordered_nodes(&state).get(index) {
                if v.borrow().space_down {
                    if let Some(source) = selected.filter(|id| id != &target.id) {
                        if target.dependencies.contains(&source)
                            || state
                                .connections
                                .iter()
                                .any(|e| e.source.node == source && e.target.node == target.id)
                        {
                            e.dispatch(Control::Disconnect {
                                source,
                                target: target.id.clone(),
                            });
                        } else if state
                            .node(&source)
                            .is_some_and(|n| n.dependencies.contains(&target.id))
                            || state
                                .connections
                                .iter()
                                .any(|e| e.source.node == target.id && e.target.node == source)
                        {
                            e.dispatch(Control::Disconnect {
                                source: target.id.clone(),
                                target: source,
                            });
                        } else {
                            e.dispatch(Control::Connect {
                                source,
                                target: target.id.clone(),
                            });
                        }
                    }
                } else {
                    cb(CanvasAction::Camera);
                    v.borrow_mut().selection = HashSet::from([target.id.clone()]);
                    cb(CanvasAction::Select(Some(target.id.clone())));
                    focus_node(&a, &v, &state, &target.id);
                }
            }
            a.queue_draw();
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::space
            && !modifiers.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::SUPER_MASK
                    | gdk::ModifierType::META_MASK
                    | gdk::ModifierType::ALT_MASK,
            )
        {
            v.borrow_mut().space_down = true;
            v.borrow_mut().numbers = true;
            a.queue_draw();
            return glib::Propagation::Stop;
        }
        let mask = gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK;
        let action = shortcuts
            .iter()
            .find(|(_, k, mods)| {
                k.to_lower() == key.to_lower()
                    && (*mods == (modifiers & mask)
                        || (*k == gdk::Key::question
                            && (modifiers & mask) == gdk::ModifierType::SHIFT_MASK))
            })
            .map(|(action, _, _)| action.as_str());
        match action {
            Some("connect") => cb(CanvasAction::ConnectSearch),
            Some("search") => cb(CanvasAction::Search),
            Some("help") => cb(CanvasAction::Help),
            Some("arrange") => cb(CanvasAction::Arrange),
            Some("copy") => cb(CanvasAction::Copy(false)),
            Some("cut") => cb(CanvasAction::Copy(true)),
            Some("paste") => cb(CanvasAction::Paste),
            Some("duplicate") => cb(CanvasAction::Duplicate),
            Some("select_all") => {
                v.borrow_mut().selection = state
                    .nodes
                    .iter()
                    .filter(|n| n.workspace == state.selected_workspace)
                    .map(|n| n.id.clone())
                    .collect();
                let id = ordered_nodes(&state).first().map(|n| n.id.clone());
                cb(CanvasAction::Select(id));
                a.queue_draw();
            }
            Some("open") => {
                if let Some(id) = selected {
                    cb(CanvasAction::Open(id));
                }
            }
            Some("edit") => {
                if let Some(id) = selected {
                    cb(CanvasAction::Edit(id));
                }
            }
            Some("run") => {
                if let Some(id) = selected {
                    cb(CanvasAction::Run(id));
                }
            }
            Some("clear_history") => cb(CanvasAction::ClearHistory),
            Some("delete") => {
                let mut ids = v.borrow().selection.clone();
                ids.extend(selected);
                for id in ids {
                    cb(CanvasAction::Delete(id));
                }
            }
            Some("escape") => cb(CanvasAction::Escape),
            Some("focus") => {
                cb(CanvasAction::Camera);
                if let Some(id) = selected {
                    focus_node(&a, &v, &state, &id);
                } else {
                    fit_animated(&a, &v, &state);
                }
            }
            Some("fit") => {
                cb(CanvasAction::Camera);
                fit_animated(&a, &v, &state);
            }
            Some(action) if action.starts_with("pan_") || action.starts_with("fast_pan_") => {
                cb(CanvasAction::Camera);
                pan_keyboard(
                    &a,
                    &v,
                    action
                        .trim_start_matches("fast_")
                        .trim_start_matches("pan_"),
                    action.starts_with("fast_"),
                );
            }
            Some(action) if action.starts_with("navigate_") => {
                cb(CanvasAction::Camera);
                let id = navigate(&a, &v, &state, action.trim_start_matches("navigate_"));
                v.borrow_mut().selection = id.clone().into_iter().collect();
                cb(CanvasAction::Select(id));
                a.queue_draw();
            }
            Some(action) if action.starts_with("move_") => {
                move_selection(&e, &v, action.trim_start_matches("move_"))
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    let v = view.clone();
    let a = area.clone();
    key.connect_key_released(move |_, key, _, _| {
        if key == gdk::Key::space {
            v.borrow_mut().space_down = false;
            v.borrow_mut().numbers = false;
            a.queue_draw();
        }
        if matches!(key, gdk::Key::Alt_L | gdk::Key::Alt_R) {
            v.borrow_mut().numbers = false;
            a.queue_draw();
        }
    });
    let v = view.clone();
    let a = area.clone();
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave(move |_| {
        let mut v = v.borrow_mut();
        v.space_down = false;
        v.numbers = false;
        v.drag = None;
        v.previews.clear();
        a.queue_draw();
    });
    area.add_controller(focus);
    area.add_controller(key);
    area
}
fn contains(node: &Node, p: &Position, x: f64, y: f64) -> bool {
    x >= p.x && x <= p.x + WIDTH && y >= p.y - 8.0 && y <= p.y + node_height(node) + 8.0
}
fn workspace_empty(state: &State) -> bool {
    !state
        .nodes
        .iter()
        .any(|node| node.workspace == state.selected_workspace)
}
pub fn draw(c: &Context, width: f64, height: f64, state: &State, view: &View) {
    let theme = &view.theme;
    c.select_font_face(
        &theme.font,
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    // Every scene uses the same wallpaper crop, including the portions beneath the DMS bar.
    let _ = c.save();
    c.set_operator(cairo::Operator::Source);
    c.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    let _ = c.paint();
    c.set_operator(cairo::Operator::Over);
    if !view.gpu
        && let Some(wallpaper) = &view.wallpaper
    {
        let (ow, oh) = view.output_size;
        let scale = (ow / wallpaper.width() as f64).max(oh / wallpaper.height() as f64);
        c.translate(
            (ow - wallpaper.width() as f64 * scale) / 2.0 - view.insets[3] as f64,
            (oh - wallpaper.height() as f64 * scale) / 2.0 - view.insets[0] as f64,
        );
        c.scale(scale, scale);
        let _ = c.set_source_surface(wallpaper, 0.0, 0.0);
        let _ = c.paint();
    } else if !view.gpu {
        theme.surface.source(c);
        let _ = c.paint();
    }
    let _ = c.restore();
    let _ = c.save();
    c.push_group();
    draw_grid(c, view);
    let _ = c.pop_group_to_source();
    let _ = c.paint_with_alpha(0.32 * view.chrome);
    let _ = c.restore();
    let _ = c.save();
    c.translate(view.pan_x, view.pan_y);
    c.scale(view.zoom, view.zoom);
    let nodes: Vec<_> = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
        .collect();
    let by_id: HashMap<_, _> = state.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    c.push_group();
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
    for edge in &state.connections {
        if let (Some(source), Some(target)) =
            (state.node(&edge.source.node), state.node(&edge.target.node))
            && source.workspace == state.selected_workspace
            && let (Some(from), Some(to)) = (
                pin_position(
                    state,
                    source,
                    &view.position(source),
                    &edge.source.pin,
                    true,
                ),
                pin_position(
                    state,
                    target,
                    &view.position(target),
                    &edge.target.pin,
                    false,
                ),
            )
        {
            let signal = crate::graph::pins(state, source, true)
                .iter()
                .any(|p| p.name == edge.source.pin && p.ty.is_signal());
            let ty = crate::graph::pins(state, source, true)
                .into_iter()
                .find(|p| p.name == edge.source.pin)
                .map(|p| p.ty)
                .unwrap_or(crate::graph::Type::Any);
            let color = type_color(&ty, theme);
            c.set_source_rgb(color.0, color.1, color.2);
            c.set_line_width(1.6);
            if !signal {
                c.set_dash(&[4.0, 3.0], 0.0);
            }
            pin_wire(c, from, to);
            let _ = c.stroke();
            c.set_dash(&[], 0.0);
        }
    }
    if let Some(Drag::Pin { source }) = &view.drag
        && let Some(node) = state.node(&source.node)
        && let Some(from) = pin_position(state, node, &view.position(node), &source.pin, true)
    {
        theme.primary.source(c);
        c.set_line_width(2.0);
        pin_wire(c, from, view.pointer);
        let _ = c.stroke();
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
    let _ = c.pop_group_to_source();
    let _ = c.paint_with_alpha(view.chrome);
    let ordered = ordered_nodes(state);
    for node in &nodes {
        if view.gpu && (node.window_id.is_some() || node.desktop_window) {
            continue;
        }
        let p = view.position(node);
        let progress = view.presentations.get(&node.id).copied().unwrap_or(
            if node.column_mode == ColumnMode::Node {
                1.0
            } else {
                0.0
            },
        );
        let native = view.desktop_rects.get(&node.id);
        let visible_rect = native
            .map(|native| {
                let (x, y) = view.world(native.x, native.y);
                super::collapse::Rect {
                    x,
                    y,
                    width: native.width / view.zoom,
                    height: native.height / view.zoom,
                }
                .interpolate(
                    super::collapse::Rect {
                        x: p.x,
                        y: p.y,
                        width: WIDTH,
                        height: node_height(node),
                    },
                    progress,
                )
            })
            .unwrap_or(super::collapse::Rect {
                x: p.x,
                y: p.y,
                width: WIDTH,
                height: node_height(node),
            });
        if visible_rect.x * view.zoom + view.pan_x > width + 12.0
            || (visible_rect.x + visible_rect.width) * view.zoom + view.pan_x < -12.0
            || visible_rect.y * view.zoom + view.pan_y > height + 12.0
            || (visible_rect.y + visible_rect.height) * view.zoom + view.pan_y < -12.0
        {
            continue;
        }
        let selected =
            view.selected.as_deref() == Some(&node.id) || view.selection.contains(&node.id);
        let matches = matches_query(node, &view.query);
        let _ = c.save();
        c.rectangle(
            visible_rect.x - 10.0 / view.zoom,
            visible_rect.y - 16.0 / view.zoom,
            visible_rect.width + 20.0 / view.zoom,
            visible_rect.height + 32.0 / view.zoom,
        );
        c.clip();
        c.push_group();
        if node.closed_window() {
            c.translate(0.0, 48.0 * (1.0 - view.chrome));
            draw_node(c, node, &p, selected, theme);
        } else if node.window_id.is_some() || node.desktop_window {
            draw_window_node(c, node, &p, visible_rect, progress, selected, view);
        } else {
            draw_node(c, node, &p, selected, theme);
        }
        if !node.closed_window() {
            for output in [false, true] {
                for pin in crate::graph::pins(state, node, output) {
                    if let Some((x, y)) = pin_position(state, node, &p, &pin.name, output) {
                        let color = type_color(&pin.ty, theme);
                        c.set_source_rgb(color.0, color.1, color.2);
                        if pin.ty.is_signal() {
                            c.arc(x, y, 4.0, 0.0, std::f64::consts::TAU);
                            let _ = c.stroke();
                        } else {
                            c.rectangle(x - 3.5, y - 3.5, 7.0, 7.0);
                            let _ = c.fill();
                        }
                    }
                }
            }
        }
        if !view.query.is_empty() && matches {
            rounded(
                c,
                p.x - 5.0,
                p.y - 5.0,
                WIDTH + 10.0,
                node_height(node) + 10.0,
                theme.radius + 5.0,
            );
            theme.primary.source(c);
            c.set_line_width(2.0 / view.zoom);
            let _ = c.stroke();
        }
        if view.numbers
            && let Some(index) = ordered
                .iter()
                .position(|n| n.id == node.id)
                .filter(|i| *i < 10)
        {
            rounded(c, p.x + WIDTH - 32.0, p.y - 13.0, 28.0, 26.0, 8.0);
            theme.primary.source(c);
            let _ = c.fill();
            text(
                c,
                p.x + WIDTH - 22.0,
                p.y + 5.0,
                &((index + 1) % 10).to_string(),
                14.0,
                theme.on_primary.rgb(),
                true,
            );
        }
        let _ = c.pop_group_to_source();
        let alpha = if node.closed_window() {
            view.chrome
        } else {
            1.0
        };
        let _ = c.paint_with_alpha(
            alpha
                * if !view.query.is_empty() && !matches {
                    0.25
                } else {
                    1.0
                },
        );
        let _ = c.restore();
    }
    let _ = c.restore();
    if !view.gpu {
        draw_foreground(c, view);
    }
    c.push_group();
    if nodes.is_empty() {
        super::empty::draw(c, width, height, theme, view.empty_hover);
    } else {
        text(c, 24.0, 30.0, "WORKFLOW", 10.0, theme.muted.rgb(), false);
        text(
            c,
            24.0,
            height - 22.0,
            "Tab  ask agent   /  search   F  focus   HJKL  select   WASD  move   ?  help",
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
    let _ = c.pop_group_to_source();
    let _ = c.paint_with_alpha(view.chrome);
    // Native columns remain real Niri clients. The layer has matching input holes.
    if !view.gpu && !view.morphing {
        for node in &nodes {
            if node.column_mode == ColumnMode::Window
                && let Some(rect) = view.desktop_rects.get(&node.id)
            {
                let _ = c.save();
                c.rectangle(rect.x, rect.y, rect.width, rect.height);
                c.set_operator(cairo::Operator::Clear);
                let _ = c.fill();
                let _ = c.restore();
            }
        }
    }
}

pub(super) fn has_foreground(view: &View) -> bool {
    matches!(view.drag, Some(Drag::Box { .. } | Drag::Cut { .. }))
}

pub(super) fn draw_foreground(c: &Context, view: &View) {
    let theme = &view.theme;
    let _ = c.save();
    c.translate(view.pan_x, view.pan_y);
    c.scale(view.zoom, view.zoom);
    if let Some(Drag::Box { start, end, .. }) = &view.drag {
        c.rectangle(
            start.0.min(end.0),
            start.1.min(end.1),
            (end.0 - start.0).abs(),
            (end.1 - start.1).abs(),
        );
        theme.primary.alpha(c, 0.13);
        let _ = c.fill_preserve();
        theme.primary.alpha(c, 0.8);
        c.set_line_width(1.0 / view.zoom);
        let _ = c.stroke();
    }
    if let Some(Drag::Cut { points }) = &view.drag {
        if let Some(start) = points.first() {
            c.move_to(start.0, start.1);
            for p in &points[1..] {
                c.line_to(p.0, p.1);
            }
        }
        theme.error.source(c);
        c.set_line_width(2.0 / view.zoom);
        let _ = c.stroke();
    }
    let _ = c.restore();
}
fn draw_window_node(
    c: &Context,
    n: &Node,
    p: &Position,
    rect: super::collapse::Rect,
    progress: f64,
    selected: bool,
    view: &View,
) {
    let theme = &view.theme;
    let t = progress.clamp(0.0, 1.0);
    let radius = theme.radius * (1.0 - t) / view.zoom + theme.radius * t;
    let _ = c.save();
    if t < 0.001 {
        // Fade the old capture to the live client after geometry reaches its endpoint.
        let _ = c.save();
        c.rectangle(rect.x, rect.y, rect.width, rect.height);
        c.set_operator(cairo::Operator::Clear);
        let _ = c.fill();
        let _ = c.restore();
        c.push_group();
    }
    rounded(c, rect.x, rect.y, rect.width, rect.height, radius);
    theme.surface_high.alpha(c, t);
    let _ = c.fill_preserve();
    let accent = if selected {
        theme.primary
    } else {
        theme.outline
    };
    c.new_path();
    let content = super::collapse::Rect {
        x: rect.x + 10.0 * t,
        y: rect.y + 70.0 * t,
        width: rect.width - 20.0 * t,
        height: rect.height - 110.0 * t,
    };
    if let Some(image) = view.images.get(&n.id) {
        let _ = c.save();
        rounded(
            c,
            content.x,
            content.y,
            content.width,
            content.height,
            (radius - 2.0 * t).max(0.0),
        );
        c.clip();
        let scale =
            (content.width / image.width() as f64).min(content.height / image.height() as f64);
        c.translate(
            content.x + (content.width - image.width() as f64 * scale) / 2.0,
            content.y + (content.height - image.height() as f64 * scale) / 2.0,
        );
        c.scale(scale, scale);
        let _ = c.set_source_surface(image, 0.0, 0.0);
        c.source().set_filter(cairo::Filter::Bilinear);
        let _ = c.paint();
        let _ = c.restore();
    }
    if t > 0.0 {
        let _ = c.save();
        c.push_group();
        // Clip the existing card renderer to metadata and ports; the image and outline are shared.
        c.rectangle(p.x - 8.0, p.y - 8.0, WIDTH + 16.0, 77.0);
        c.rectangle(p.x - 8.0, p.y + node_height(n) - 39.0, WIDTH + 16.0, 48.0);
        c.clip();
        draw_node(c, n, p, selected, theme);
        let _ = c.pop_group_to_source();
        let _ = c.paint_with_alpha(((t - 0.6) / 0.4).clamp(0.0, 1.0));
        let _ = c.restore();
    }
    rounded(c, rect.x, rect.y, rect.width, rect.height, radius);
    c.set_source_rgb(
        theme.primary.0 + (accent.0 - theme.primary.0) * t,
        theme.primary.1 + (accent.1 - theme.primary.1) * t,
        theme.primary.2 + (accent.2 - theme.primary.2) * t,
    );
    c.set_line_width(
        (2.0 * (1.0 - t) + (if selected { 2.0 } else { 1.0 }) * view.zoom * t) / view.zoom,
    );
    let _ = c.stroke();
    if t < 0.001 {
        let _ = c.pop_group_to_source();
        let _ = c.paint_with_alpha(view.live_fade);
    }
    let _ = c.restore();
}

pub fn matches_query(node: &Node, query: &str) -> bool {
    let text = format!("{} {} {}", node.title, node.kind.label(), node.command).to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| text.contains(word))
}
pub fn ordered_nodes(state: &State) -> Vec<&Node> {
    let mut nodes: Vec<_> = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
        .collect();
    nodes.sort_by(|a, b| {
        a.position
            .x
            .total_cmp(&b.position.x)
            .then(a.position.y.total_cmp(&b.position.y))
            .then(a.id.cmp(&b.id))
    });
    nodes
}

/// Use the same indices as Alt+number and the canvas, regardless of selection order.
pub fn first_selected<'a>(state: &'a State, view: &View) -> Option<&'a Node> {
    if !view.workspace.is_empty() && view.workspace != state.selected_workspace {
        return None;
    }
    ordered_nodes(state).into_iter().find(|node| {
        view.selected.as_deref() == Some(&node.id) || view.selection.contains(&node.id)
    })
}
fn intersects((left, top, right, bottom): (f64, f64, f64, f64), node: &Node, p: &Position) -> bool {
    p.x <= right && p.x + WIDTH >= left && p.y <= bottom && p.y + node_height(node) >= top
}
fn crossing(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    fn cross(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    }
    let (ab_c, ab_d, cd_a, cd_b) = (
        cross(a, b, c),
        cross(a, b, d),
        cross(c, d, a),
        cross(c, d, b),
    );
    ab_c * ab_d <= 0.0
        && cd_a * cd_b <= 0.0
        && a.0.min(b.0) <= c.0.max(d.0)
        && c.0.min(d.0) <= a.0.max(b.0)
        && a.1.min(b.1) <= c.1.max(d.1)
        && c.1.min(d.1) <= a.1.max(b.1)
}
fn wire_cut(parent: &Node, node: &Node, points: &[(f64, f64)], view: &View) -> bool {
    let from = view.position(parent);
    let to = view.position(node);
    let a = (from.x + WIDTH / 2.0, from.y + node_height(parent));
    let b = (to.x + WIDTH / 2.0, to.y);
    route_cut(a, b, points, false)
}

pub(super) fn draw_node(c: &Context, n: &Node, p: &Position, selected: bool, theme: &Theme) {
    let (x, y) = (p.x, p.y);
    let height = node_height(n);
    let accent = if n.kind == NodeKind::Constant {
        type_color(&crate::graph::constant_type(n), theme)
    } else {
        theme.primary.rgb()
    };
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
    c.set_line_width(if selected { 2.0 } else { 1.0 });
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
        _ => n.kind.symbol(),
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
        if n.closed_window() {
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
    if n.closed_window() {
        draw_ports(c, x, y, height, theme);
        return;
    }
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
        let display = if n.kind == NodeKind::Display {
            n.flow
                .values
                .get("value")
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_else(|| "Waiting for input".into())
        } else if n.kind == NodeKind::Constant {
            n.flow
                .properties
                .get("value")
                .unwrap_or(&serde_json::Value::Null)
                .to_string()
        } else {
            n.command.clone()
        };
        let snippet = display
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
    let footer = if n.kind == NodeKind::Constant {
        format!(
            "{} · {}",
            crate::graph::constant_type(n),
            if n.flow.editable_in_graph {
                "Click value to edit"
            } else {
                "Read only in graph"
            }
        )
    } else if n.desktop_window {
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
    draw_ports(c, x, y, height, theme);
}
fn type_color(ty: &crate::graph::Type, theme: &Theme) -> (f64, f64, f64) {
    use crate::graph::Type;
    match ty {
        Type::Bool => (0.92, 0.66, 0.42),
        Type::Int => (0.49, 0.78, 0.64),
        Type::Float => (0.40, 0.76, 0.84),
        Type::String => (0.76, 0.64, 0.91),
        Type::List { .. } => (0.83, 0.68, 0.81),
        Type::Record { .. } => (0.65, 0.72, 0.90),
        Type::Named { name } if name == "Error" => theme.error.rgb(),
        _ => theme.primary.rgb(),
    }
}

fn draw_ports(c: &Context, x: f64, y: f64, height: f64, theme: &Theme) {
    let accent = theme.primary.rgb();
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
    draw_route(c, (x1, y1), (x2, y2), false);
}
fn pin_position(
    state: &State,
    node: &Node,
    p: &Position,
    name: &str,
    output: bool,
) -> Option<(f64, f64)> {
    let pins = crate::graph::pins(state, node, output);
    let index = pins.iter().position(|pin| pin.name == name)?;
    Some((
        p.x + if output { WIDTH } else { 0.0 },
        p.y + node_height(node) * (index + 1) as f64 / (pins.len() + 1) as f64,
    ))
}
fn hit_pin(
    state: &State,
    view: &View,
    x: f64,
    y: f64,
    output: bool,
) -> Option<crate::graph::Endpoint> {
    for node in state
        .nodes
        .iter()
        .rev()
        .filter(|n| n.workspace == state.selected_workspace && !n.closed_window())
    {
        for pin in crate::graph::pins(state, node, output) {
            if let Some((px, py)) =
                pin_position(state, node, &view.position(node), &pin.name, output)
                && (px - x).hypot(py - y) < 9.0
            {
                return Some(crate::graph::Endpoint {
                    node: node.id.clone(),
                    pin: pin.name,
                });
            }
        }
    }
    None
}
fn pin_wire(c: &Context, from: (f64, f64), to: (f64, f64)) {
    draw_route(c, from, to, true);
}
fn draw_route(c: &Context, from: (f64, f64), to: (f64, f64), horizontal: bool) {
    let points = super::wires::route(from, to, horizontal);
    c.move_to(from.0, from.1);
    for p in points.iter().skip(1) {
        c.line_to(p.0, p.1);
    }
}
fn route_cut(from: (f64, f64), to: (f64, f64), points: &[(f64, f64)], horizontal: bool) -> bool {
    super::wires::route(from, to, horizontal)
        .windows(2)
        .any(|segment| {
            points
                .windows(2)
                .any(|cut| crossing(segment[0], segment[1], cut[0], cut[1]))
        })
}
fn pin_wire_cut(from: (f64, f64), to: (f64, f64), points: &[(f64, f64)]) -> bool {
    route_cut(from, to, points, true)
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
pub(super) fn rounded(c: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
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
pub(super) fn text(
    c: &Context,
    x: f64,
    y: f64,
    text: &str,
    size: f64,
    rgb: (f64, f64, f64),
    mono: bool,
) {
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
    fn cut_stroke_crosses_the_rendered_bezier_and_ignores_distant_strokes() {
        let state = State::empty("/tmp".into());
        let a = Node::new(
            &state.selected_workspace,
            NodeKind::Shell,
            "/tmp".into(),
            Position { x: 100.0, y: 700.0 },
        );
        let b = Node::new(
            &state.selected_workspace,
            NodeKind::Shell,
            "/tmp".into(),
            Position { x: 460.0, y: 700.0 },
        );
        assert!(wire_cut(
            &a,
            &b,
            &[(426.0, 680.0), (426.0, 900.0)],
            &View::default()
        ));
        assert!(!wire_cut(
            &a,
            &b,
            &[(0.0, 0.0), (20.0, 20.0)],
            &View::default()
        ));
        assert!(!crossing((0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)));
    }
    #[test]
    fn search_keeps_spatial_navigation_on_matching_nodes() {
        let mut state = State::empty("/tmp".into());
        for (title, x) in [("Editor", 0.0), ("Browser", 360.0), ("Editor tests", 720.0)] {
            let mut node = Node::new(
                &state.selected_workspace,
                NodeKind::Shell,
                "/tmp".into(),
                Position { x, y: 0.0 },
            );
            node.title = title.into();
            state.nodes.push(node);
        }
        let mut view = View {
            query: "editor".into(),
            selected: Some(state.nodes[0].id.clone()),
            ..View::default()
        };
        assert_eq!(
            view.navigate(&state, "right", 1200.0, 800.0),
            Some(state.nodes[2].id.clone())
        );
        assert!(matches_query(&state.nodes[2], "EDITOR tests"));
        assert!(!matches_query(&state.nodes[1], "editor"));
    }

    #[test]
    fn row_render_snapshots_skip_outputs_and_other_rows() {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Agent,
            "/tmp".into(),
            Position::default(),
        );
        node.output = "large agent output".repeat(8192);
        node.log(
            "agent",
            "activity that does not belong in a canvas snapshot",
        );
        node.chat.push(ChatMessage {
            role: "assistant".into(),
            text: "private conversation".into(),
        });
        state.nodes.push(node);
        state.nodes.push(Node::new(
            "other row",
            NodeKind::Shell,
            "/tmp".into(),
            Position::default(),
        ));
        let snapshot = row_state(&state, &state.selected_workspace);
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].output.capacity(), 0);
        assert!(snapshot.nodes[0].logs.is_empty() && snapshot.nodes[0].chat.is_empty());
        assert_eq!(snapshot.nodes[0].command, state.nodes[0].command);
        assert!(!state.nodes[0].output.is_empty());
    }

    #[test]
    fn windows_focus_uses_canvas_indices_instead_of_selection_order() {
        let mut state = State::empty("/tmp".into());
        for x in [720.0, 360.0, 0.0] {
            state.nodes.push(Node::new(
                &state.selected_workspace,
                NodeKind::Window,
                "/tmp".into(),
                Position { x, y: 80.0 },
            ));
        }
        let mut view = View {
            selected: Some(state.nodes[0].id.clone()),
            ..View::default()
        };
        view.selection
            .extend(state.nodes.iter().map(|n| n.id.clone()));
        assert_eq!(first_selected(&state, &view).unwrap().id, state.nodes[2].id);
        view.selection.clear();
        assert_eq!(first_selected(&state, &view).unwrap().id, state.nodes[0].id);
        view.selected = None;
        assert!(first_selected(&state, &view).is_none());
        view.selected = Some(state.nodes[0].id.clone());
        view.workspace = "other row".into();
        assert!(first_selected(&state, &view).is_none());
    }

    #[test]
    fn selected_window_outline_survives_headers_and_footers() {
        let mut node = Node::new(
            "row",
            NodeKind::Window,
            "/tmp".into(),
            Position { x: 80.0, y: 100.0 },
        );
        node.window_id = Some(1);
        let mut view = View {
            zoom: 1.0,
            ..View::default()
        };
        view.theme.primary = super::super::theme::Color(1.0, 0.0, 0.0);
        view.theme.outline = super::super::theme::Color(0.0, 1.0, 0.0);
        let bytes = raster(500, 450, |c| {
            draw_window_node(
                c,
                &node,
                &node.position,
                super::super::collapse::Rect {
                    x: 80.0,
                    y: 100.0,
                    width: WIDTH,
                    height: node_height(&node),
                },
                1.0,
                true,
                &view,
            )
        });
        for (x, y) in [
            (100, 100),
            (80, 130),
            (80, 230),
            (80, 360),
            (100, 374),
            (371, 130),
            (371, 360),
        ] {
            let rgb = pixel(&bytes, 500, x, y);
            assert!(
                rgb & 0xff0000 > 0xaa0000 && rgb & 0x00ff00 < 0x002800,
                "selection at {x},{y}: {rgb:x}"
            );
        }
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

    #[test]
    fn dragging_an_arranged_card_commits_the_displayed_delta_once() {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            "/tmp".into(),
            Position { x: 2200.0, y: 80.0 },
        );
        node.desktop_window = true;
        state.nodes.push(node);
        let mut view = View::default();
        view.arrangement.update(&state, 1200, 800);
        view.presented_positions
            .insert(state.nodes[0].id.clone(), Position { x: 40.0, y: 60.0 });
        let shown = view.position(&state.nodes[0]);
        assert!(contains(
            &state.nodes[0],
            &shown,
            shown.x + 20.0,
            shown.y + 20.0
        ));
        let stored = view.stored_position(
            &state.nodes[0],
            &Position {
                x: shown.x + 20.0,
                y: shown.y - 40.0,
            },
        );
        assert_eq!(stored.x, state.nodes[0].position.x + 20.0);
        assert_eq!(stored.y, state.nodes[0].position.y - 40.0);
    }

    #[test]
    #[ignore = "requires a GTK display; run explicitly inside Niri or Xvfb"]
    fn gpu_window_cards_keep_full_selection_outlines_and_foreground_gestures() {
        gtk::init().unwrap();
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            "/tmp".into(),
            Position { x: 80.0, y: 100.0 },
        );
        node.desktop_window = true;
        node.title = "Closed window".into();
        state.nodes.push(node);
        let mut view = View {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            theme: Theme::default(),
            ..View::default()
        };
        let mut cache = super::super::gpu::Cache::default();
        let mut render = |state: &State, view: &View| {
            raster(500, 400, |c| {
                let snapshot = gtk::Snapshot::new();
                super::super::gpu::render(&snapshot, 500, 400, state, 1, view, &mut cache);
                snapshot.to_node().unwrap().draw(c);
            })
        };
        let visible = render(&state, &view);
        assert_eq!(node_height(&state.nodes[0]), 78.0);
        assert_ne!(
            pixel(&visible, 500, 200, 140),
            pixel(&visible, 500, 200, 220)
        );
        view.chrome = 0.0;
        let hidden = render(&state, &view);
        assert_eq!(pixel(&hidden, 500, 200, 140), pixel(&hidden, 500, 200, 220));
        view.chrome = 0.5;
        let entering = render(&state, &view);
        assert_eq!(
            pixel(&entering, 500, 200, 110),
            pixel(&hidden, 500, 200, 110)
        );
        assert_ne!(
            pixel(&entering, 500, 200, 140),
            pixel(&hidden, 500, 200, 140)
        );
        view.chrome = 1.0;
        view.drag = Some(Drag::Box {
            start: (50.0, 130.0),
            end: (400.0, 160.0),
            additive: false,
        });
        let selected = render(&state, &view);
        // Both the tint and the rectangle edge must cross the GPU card's body.
        assert_ne!(
            pixel(&visible, 500, 200, 145),
            pixel(&selected, 500, 200, 145)
        );
        assert_ne!(
            pixel(&visible, 500, 200, 130),
            pixel(&selected, 500, 200, 130)
        );
        assert_eq!(
            pixel(&visible, 500, 200, 120),
            pixel(&selected, 500, 200, 120)
        );
        // Live window metadata used to overwrite the top and bottom selection.
        view.drag = None;
        view.selected = Some(state.nodes[0].id.clone());
        view.theme.primary = super::super::theme::Color(1.0, 0.0, 0.0);
        view.theme.outline = super::super::theme::Color(0.0, 1.0, 0.0);
        state.nodes[0].window_id = Some(1);
        let selected = render(&state, &view);
        for (x, y) in [
            (100, 100),
            (80, 130),
            (80, 230),
            (80, 360),
            (100, 373),
            (371, 130),
            (371, 360),
        ] {
            let rgb = pixel(&selected, 500, x, y);
            assert!(
                rgb & 0xff0000 > 0xaa0000 && rgb & 0x00ff00 < 0x002800,
                "GPU selection at {x},{y}: {rgb:x}"
            );
        }
    }
}
