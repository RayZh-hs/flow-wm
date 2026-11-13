use super::canvas::{self, CanvasAction, View};
use super::{Options, theme::Theme};
use crate::{
    client::{Action, DesktopClient, Presentation, UiCommand},
    extensions::Macro,
    model::*,
    niri::{self, Desktop},
};
use anyhow::{Context, Result, bail};
use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub fn run(
    engine: DesktopClient,
    events: mpsc::Receiver<UiCommand>,
    macros: Vec<Macro>,
    options: Options,
) -> Result<()> {
    let application = gtk::Application::builder()
        .application_id("dev.flowwm.Shell")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let events = Rc::new(RefCell::new(Some(events)));
    let fatal = Rc::new(RefCell::new(None));
    let error = fatal.clone();
    application.connect_activate(move |app| {
        if !options.windowed && !gtk4_layer_shell::is_supported() { *error.borrow_mut() = Some("A Wayland compositor with layer-shell is required. Run inside Niri, or use `flowwm start --windowed` for development.".to_string()); app.quit(); return; }
        if options.integrated && !options.windowed && !niri::node_rows_supported() { *error.borrow_mut() = Some("Native Nodes rows require FlowWM's bundled niri. Build it with `python3 scripts/dev.py build niri --release` and use that compositor at the next graphical login.".to_string()); app.quit(); return; }
        let provider = gtk::CssProvider::new(); provider.load_from_string(&Theme::load().css());
        if let Some(display) = gdk::Display::default() { gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION); }
        if let Some(settings) = gtk::Settings::default() {
            if let Some(property) = settings.find_property("gtk-interface-color-scheme")
                && let Some(class) = glib::EnumClass::with_type(property.value_type())
                && let Some(value) = class.to_value_by_nick("dark") {
                settings.set_property_from_value("gtk-interface-color-scheme", &value);
            } else { settings.set_gtk_application_prefer_dark_theme(true); }
        }
        let stop = Arc::new(AtomicBool::new(false)); let desktop = niri::listen(stop.clone());
        let shell = Shell::build(app, engine.clone(), macros.clone(), options, desktop);
        shell.view.borrow_mut().native_nodes = shell.native_nodes;
        gtk::prelude::WidgetExt::realize(&shell.window());
        if let Some(renderer) = shell.window().renderer() { eprintln!("FlowWM renderer: {}", renderer.type_().name()); }
        for path in Theme::paths() {
            if let Some(parent) = path.parent() {
                let directory = gio::File::for_path(parent);
                if let Ok(monitor) = directory.monitor_directory(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>) {
                    let w = shell.weak.clone(); let provider = provider.clone();
                    monitor.connect_changed(move |_, file, _, _| {
                        if file.basename().is_some_and(|name| name.as_os_str() == "dms-colors.json" || name.as_os_str() == "dms-typography.json" || name.as_os_str() == "settings.json" || name.as_os_str() == "session.json") && let Some(shell) = w.upgrade() {
                            let theme = Theme::load(); provider.load_from_string(&theme.css()); shell.view.borrow_mut().theme = theme; shell.ensure_row_surfaces(&shell.engine.snapshot().state); shell.apply_insets(); shell.request_captures(false); shell.canvas().queue_draw();
                        }
                    });
                    shell.theme_monitors.borrow_mut().push(monitor);
                }
            }
        }
        let commands = events.borrow_mut().take().unwrap();
        let pulse = gio::SimpleAction::new("flowwm-pulse", None);
        let active_shell = shell.clone();
        pulse.connect_activate(move |_, _| {
            while let Ok(command) = commands.try_recv() { active_shell.control(command); }
            active_shell.refresh();
        });
        app.add_action(&pulse);
        app.connect_shutdown(|app| app.remove_action("flowwm-pulse"));
        let weak_app: glib::SendWeakRef<gtk::Application> = app.downgrade().into();
        let changed = engine.clone();
        let pending = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || {
            let mut generation = 0;
            while let Some(next) = changed.wait_for_change(generation) {
                generation = next;
                if pending.swap(true, Ordering::AcqRel) { continue; }
                let weak_app = weak_app.clone(); let pending = pending.clone();
                glib::idle_add_once(move || {
                    pending.store(false, Ordering::Release);
                    if let Some(app) = weak_app.upgrade() { app.activate_action("flowwm-pulse", None); }
                });
            }
        });
        let mut signals = signal_hook::iterator::Signals::new([libc::SIGTERM, libc::SIGINT]).expect("signal handler");
        let signal_handle = signals.handle(); let signal_engine = engine.clone();
        std::thread::spawn(move || { if signals.forever().next().is_some() { signal_engine.dispatch(Action::Ui { command: UiCommand::Quit }); } });
        app.connect_shutdown(move |_| signal_handle.close());
        let stop_signal = stop.clone(); app.connect_shutdown(move |_| stop_signal.store(true, Ordering::Relaxed));
        if !options.background { shell.show_nodes(); }
    });
    // GTK must not parse FlowWM's CLI arguments.
    application.run_with_args::<&str>(&[]);
    if let Some(error) = fatal.borrow_mut().take() {
        bail!("{error}");
    }
    Ok(())
}

struct NodeWindow {
    window: gtk::ApplicationWindow,
    panel: ExtensionPanel,
}

#[derive(Clone)]
struct RowSurface {
    window: gtk::ApplicationWindow,
    canvas: super::surface::Surface,
    row_id: u64,
}

fn row_direction(previous: Option<&Row>, next: Option<&Row>) -> Option<f32> {
    let (previous, next) = (previous?, next?);
    if previous.session != next.session || previous.output != next.output {
        return None;
    }
    match next.index.cmp(&previous.index) {
        std::cmp::Ordering::Greater => Some(1.0),
        std::cmp::Ordering::Less => Some(-1.0),
        std::cmp::Ordering::Equal => None,
    }
}

#[test]
fn row_slides_follow_vertical_order_only_within_the_same_output_and_session() {
    let first = Row {
        session: "session".into(),
        id: 100,
        index: 2,
        output: Some("DP-1".into()),
        name: None,
    };
    let mut next = first.clone();
    next.id = 1;
    next.index = 3;
    assert_eq!(row_direction(Some(&first), Some(&next)), Some(1.0));
    next.index = 1;
    assert_eq!(row_direction(Some(&first), Some(&next)), Some(-1.0));
    next.output = Some("DP-2".into());
    assert_eq!(row_direction(Some(&first), Some(&next)), None);
    next.output = first.output.clone();
    next.session = "new session".into();
    assert_eq!(row_direction(Some(&first), Some(&next)), None);
    assert_eq!(row_direction(None, Some(&next)), None);
}
struct Shell {
    app: gtk::Application,
    fallback_window: gtk::ApplicationWindow,
    bar: gtk::ApplicationWindow,
    engine: DesktopClient,
    macros: Vec<Macro>,
    desktop: Arc<Mutex<Desktop>>,
    fallback_canvas: super::surface::Surface,
    view: Rc<RefCell<View>>,
    inspector: gtk::Box,
    workspace_label: gtk::Label,
    workspace_path: gtk::Label,
    footer: gtk::Label,
    connection: gtk::Label,
    bar_status: gtk::Label,
    error: gtk::Label,
    error_dialog: RefCell<Option<(gtk::Window, gtk::Label)>>,
    windows: RefCell<HashMap<String, NodeWindow>>,
    revision: Cell<u64>,
    weak: Weak<Shell>,
    last_workspace: RefCell<String>,
    row_views: canvas::RowViews,
    row_surfaces: RefCell<HashMap<String, RowSurface>>,
    native_nodes: bool,
    displayed_row: RefCell<Option<Row>>,
    windowed: bool,
    theme_monitors: RefCell<Vec<gio::FileMonitor>>,
    integrated: bool,
    graph_visible: Cell<bool>,
    transition_serial: Cell<u64>,
    first_frame: Cell<bool>,
    pending_focus: RefCell<Option<String>>,
    return_window: Cell<Option<u64>>,
    bar_geometry: RefCell<HashMap<String, [i32; 4]>>,
    fallback_content: gtk::Box,
    captures: Option<super::collapse::Captures>,
    chat: RefCell<Option<ChatUi>>,
    search_popup: RefCell<Option<gtk::Window>>,
    reveal: Cell<f64>,
    paint_started: Cell<Option<std::time::Instant>>,
    frame_monitors: RefCell<HashSet<usize>>,
    pinned_rows: RefCell<HashSet<u64>>,
}
impl Shell {
    fn window(&self) -> gtk::ApplicationWindow {
        let workspace = &self.engine.snapshot().state.selected_workspace;
        self.row_surfaces
            .borrow()
            .get(workspace)
            .map(|row| row.window.clone())
            .unwrap_or_else(|| self.fallback_window.clone())
    }

    fn canvas(&self) -> super::surface::Surface {
        let workspace = &self.engine.snapshot().state.selected_workspace;
        self.row_surfaces
            .borrow()
            .get(workspace)
            .map(|row| row.canvas.clone())
            .unwrap_or_else(|| self.fallback_canvas.clone())
    }

    fn content(&self) -> gtk::Widget {
        if self.native_nodes {
            self.canvas().upcast()
        } else {
            self.fallback_content.clone().upcast()
        }
    }

    fn ensure_row_surfaces(&self, state: &State) -> bool {
        if !self.native_nodes {
            return false;
        }
        let desktop = self.desktop.lock().unwrap().clone();
        let display = gtk::prelude::WidgetExt::display(&self.fallback_window);
        let mut surfaces = self.row_surfaces.borrow_mut();
        let mut keep = HashSet::new();
        let mut claimed_rows = HashSet::new();
        let mut created_current = false;
        for group in &state.workspaces {
            let Some(row) = group.row.as_ref().filter(|row| {
                row.session == desktop.session && desktop.workspaces.iter().any(|w| w.id == row.id)
            }) else {
                continue;
            };
            let monitor = (0..display.monitors().n_items())
                .filter_map(|i| display.monitors().item(i)?.downcast::<gdk::Monitor>().ok())
                .find(|m| m.connector().as_deref() == row.output.as_deref());
            let Some(monitor) = monitor else { continue };
            if !claimed_rows.insert(row.id) {
                continue;
            }
            keep.insert(group.id.clone());
            if surfaces.get(&group.id).is_some_and(|s| s.row_id != row.id) {
                let surface = surfaces.remove(&group.id).unwrap();
                self.frame_monitors
                    .borrow_mut()
                    .remove(&(surface.window.as_ptr() as usize));
                surface.window.close();
            }
            if !surfaces.contains_key(&group.id) && group.view == RowView::Nodes {
                let window = gtk::ApplicationWindow::builder()
                    .application(&self.app)
                    .title(format!("FlowWM — Row {}", row.index))
                    .build();
                window.add_css_class("flow-shell");
                window.add_css_class("integrated");
                window.set_decorated(false);
                super::native::bind(&window, row.id);
                let weak = self.weak.clone();
                let workspace = group.id.clone();
                let canvas = canvas::make_row(
                    self.engine.clone(),
                    self.view.clone(),
                    Rc::new(move |action| {
                        if let Some(shell) = weak.upgrade() {
                            if shell.engine.snapshot().state.selected_workspace != workspace {
                                let _ = shell.engine.request(Action::SelectWorkspace {
                                    id: workspace.clone(),
                                });
                                shell.refresh();
                            }
                            shell.canvas_action(action);
                        }
                    }),
                    group.id.clone(),
                    self.row_views.clone(),
                );
                window.set_child(Some(&canvas));
                let weak = self.weak.clone();
                let workspace = group.id.clone();
                let input = gtk::EventControllerLegacy::new();
                input.set_propagation_phase(gtk::PropagationPhase::Capture);
                input.connect_event(move |_, event| {
                    if matches!(
                        event.event_type(),
                        gdk::EventType::ButtonPress
                            | gdk::EventType::KeyPress
                            | gdk::EventType::Scroll
                            | gdk::EventType::TouchBegin
                    ) && let Some(shell) = weak.upgrade()
                        && shell.engine.snapshot().state.selected_workspace != workspace
                    {
                        let _ = shell.engine.request(Action::SelectWorkspace {
                            id: workspace.clone(),
                        });
                        shell.refresh();
                    }
                    glib::Propagation::Proceed
                });
                window.add_controller(input);
                if group.id == state.selected_workspace {
                    created_current = true;
                }
                surfaces.insert(
                    group.id.clone(),
                    RowSurface {
                        window,
                        canvas,
                        row_id: row.id,
                    },
                );
            }
            let Some(surface) = surfaces.get(&group.id) else {
                continue;
            };
            let insets = row
                .output
                .as_ref()
                .and_then(|o| self.bar_geometry.borrow().get(o).copied())
                .unwrap_or_else(Theme::desktop_insets);
            if group.id != state.selected_workspace {
                let mixed = state.nodes.iter().any(|n| {
                    n.workspace == group.id
                        && n.window_id.is_some()
                        && n.column_mode == ColumnMode::Window
                });
                surface.window.set_visible(group.view == RowView::Nodes);
                let mut views = self.row_views.borrow_mut();
                let saved = views
                    .entry(group.id.clone())
                    .or_insert_with(|| self.view.borrow().for_row(group.id.clone()));
                saved.insets = insets;
                let geometry = monitor.geometry();
                saved.output_size = (geometry.width() as f64, geometry.height() as f64);
                saved.transparent = true;
                saved.native_nodes = true;
                saved.local_windows = Some(
                    desktop
                        .windows
                        .iter()
                        .filter(|w| w.workspace_id == Some(row.id))
                        .map(|w| w.id)
                        .collect(),
                );
                saved.chrome = 1.0;
                saved.morphing = false;
                saved.presentations.clear();
                saved.border_opacities.clear();
                saved.border_fades.clear();
                saved.window_borders = false;
                saved.live_fade = 0.0;
                if mixed {
                    let scoped = canvas::row_state(state, &group.id);
                    saved.desktop_rects = super::collapse::rectangles(
                        &desktop,
                        &scoped,
                        insets,
                        (geometry.width() - insets[1] - insets[3]) as f64,
                        false,
                    );
                    if let Some(native) = surface.window.surface() {
                        let region =
                            gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                                0,
                                0,
                                geometry.width() - insets[1] - insets[3],
                                geometry.height() - insets[0] - insets[2],
                            ));
                        for node in scoped.nodes.iter().filter(|n| {
                            n.column_mode == ColumnMode::Window && n.window_id.is_some()
                        }) {
                            if let Some(rect) = saved
                                .presented_rects
                                .get(&node.id)
                                .or_else(|| saved.desktop_rects.get(&node.id))
                            {
                                let _ = region.subtract_rectangle(&rect.integer());
                            }
                        }
                        native.set_input_region(Some(&region));
                    }
                } else if let Some(native) = surface.window.surface() {
                    native.set_input_region(None);
                }
                surface.canvas.queue_draw();
            }
        }
        surfaces.retain(|id, surface| {
            if keep.contains(id) {
                return true;
            }
            self.frame_monitors
                .borrow_mut()
                .remove(&(surface.window.as_ptr() as usize));
            surface.window.close();
            false
        });
        self.row_views
            .borrow_mut()
            .retain(|id, _| state.workspaces.iter().any(|w| w.id == *id));
        created_current
    }

    fn build(
        app: &gtk::Application,
        engine: DesktopClient,
        macros: Vec<Macro>,
        options: Options,
        desktop: Arc<Mutex<Desktop>>,
    ) -> Rc<Self> {
        let windowed = options.windowed;
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("FlowWM — Workflow")
            .default_width(1440)
            .default_height(940)
            .build();
        window.add_css_class("flow-shell");
        let bar = gtk::ApplicationWindow::builder()
            .application(app)
            .title("FlowWM bar")
            .build();
        bar.add_css_class("flow-bar");
        if !windowed && !options.integrated {
            window.init_layer_shell();
            window.set_namespace(Some("flowwm-graph"));
            window.set_layer(Layer::Overlay);
            window.set_keyboard_mode(KeyboardMode::Exclusive);
            window.set_exclusive_zone(-1);
            for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                window.set_anchor(edge, true);
            }
            bar.init_layer_shell();
            bar.set_namespace(Some("flowwm-bar"));
            bar.set_layer(Layer::Top);
            bar.set_keyboard_mode(KeyboardMode::None);
            for edge in [Edge::Top, Edge::Left, Edge::Right] {
                bar.set_anchor(edge, true);
            }
            bar.set_exclusive_zone(44);
        }
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("graph-surface");
        root.set_overflow(gtk::Overflow::Hidden);
        let surface = gtk::Overlay::new();
        surface.set_child(Some(&root));
        window.set_child(Some(&surface));
        let header = gtk::CenterBox::new();
        header.add_css_class("topbar");
        let left = row(12);
        left.append(&label("∿", "logo"));
        left.append(&label("FlowWM", "wordmark"));
        left.append(&separator());
        let workspace_label = gtk::Label::new(Some(&engine.snapshot().state.workspace().name));
        let workspace_button = gtk::Button::new();
        let ws_row = row(10);
        ws_row.append(&gtk::Image::from_icon_name("view-grid-symbolic"));
        ws_row.append(&workspace_label);
        ws_row.append(&label("⌄", "muted"));
        workspace_button.set_child(Some(&ws_row));
        workspace_button.add_css_class("flat");
        left.append(&workspace_button);
        header.set_start_widget(Some(&left));
        let views = row(3);
        views.add_css_class("view-switch");
        let nodes_button = gtk::Button::with_label("⌘  Nodes");
        nodes_button.add_css_class("selected");
        let windows_button = gtk::Button::with_label("▤  Windows");
        views.append(&nodes_button);
        views.append(&windows_button);
        header.set_center_widget(Some(&views));
        let right = row(16);
        let connection = label("○  Connecting", "muted");
        right.append(&connection);
        let add_button = gtk::Button::with_label("+  New node");
        add_button.add_css_class("accent");
        right.append(&add_button);
        header.set_end_widget(Some(&right));
        if !options.integrated {
            root.append(&header);
        }
        let subbar = row(12);
        subbar.add_css_class("subbar");
        subbar.append(&label("WORKSPACE", "eyebrow"));
        let workspace_path = label(
            &engine.snapshot().state.workspace().cwd.to_string_lossy(),
            "path",
        );
        workspace_path.set_hexpand(true);
        workspace_path.set_xalign(0.0);
        workspace_path.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        subbar.append(&workspace_path);
        let recipes_button = gtk::Button::with_label("Recipes");
        recipes_button.add_css_class("flat");
        let notes_button = gtk::Button::with_label("Workspace notes");
        notes_button.add_css_class("flat");
        let fit_button = gtk::Button::from_icon_name("zoom-fit-best-symbolic");
        fit_button.set_tooltip_text(Some("Frame all nodes · Home"));
        fit_button.add_css_class("flat");
        let run_button = gtk::Button::with_label("▷  Run workflow");
        run_button.add_css_class("quiet");
        subbar.append(&recipes_button);
        subbar.append(&notes_button);
        subbar.append(&fit_button);
        subbar.append(&run_button);
        if !options.integrated {
            root.append(&subbar);
        }
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        root.append(&body);
        let inspector = gtk::Box::new(gtk::Orientation::Vertical, 0);
        inspector.add_css_class("inspector");
        inspector.set_width_request(344);
        inspector.set_hexpand(false);
        inspector.set_visible(false);
        let view = Rc::new(RefCell::new(View::default()));
        let footer_box = gtk::CenterBox::new();
        footer_box.add_css_class("footer");
        let footer = label("Persistent workspace", "muted");
        footer_box.set_start_widget(Some(&footer));
        let ws_navigation = row(8);
        let previous = gtk::Button::with_label("‹");
        previous.add_css_class("flat");
        let workspace_title = label("Workspaces", "muted");
        let next = gtk::Button::with_label("›");
        next.add_css_class("flat");
        ws_navigation.append(&previous);
        ws_navigation.append(&workspace_title);
        ws_navigation.append(&next);
        footer_box.set_center_widget(Some(&ws_navigation));
        let help_button = gtk::Button::with_label("?  Key bindings");
        help_button.add_css_class("flat");
        footer_box.set_end_widget(Some(&help_button));
        let error = label("", "error-banner");
        error.set_wrap(true);
        error.set_visible(false);
        root.append(&error);
        if !options.integrated {
            root.append(&footer_box);
        }
        let bar_box = gtk::CenterBox::new();
        bar_box.add_css_class("bar-content");
        let bar_brand = row(10);
        bar_brand.append(&label("∿", "logo-small"));
        bar_brand.append(&label("FlowWM", "wordmark"));
        let show_nodes = gtk::Button::with_label("⌘  Nodes");
        show_nodes.add_css_class("flat");
        bar_brand.append(&show_nodes);
        bar_box.set_start_widget(Some(&bar_brand));
        let bar_status = label("", "muted");
        bar_box.set_center_widget(Some(&bar_status));
        let bar_right = row(8);
        let bar_notes = gtk::Button::with_label("My work");
        bar_notes.add_css_class("flat");
        let quit = gtk::Button::from_icon_name("application-exit-symbolic");
        quit.add_css_class("flat");
        quit.set_tooltip_text(Some("Quit FlowWM"));
        bar_right.append(&bar_notes);
        bar_right.append(&quit);
        bar_box.set_end_widget(Some(&bar_right));
        bar.set_child(Some(&bar_box));
        let shell: Rc<Shell> = Rc::new_cyclic(|weak: &Weak<Shell>| {
            let w = weak.clone();
            let canvas = canvas::make(
                engine.clone(),
                view.clone(),
                Rc::new(move |action| {
                    if let Some(s) = w.upgrade() {
                        s.canvas_action(action);
                    }
                }),
            );
            body.append(&canvas);
            body.append(&inspector);
            Self {
                app: app.clone(),
                fallback_window: window.clone(),
                bar: bar.clone(),
                engine: engine.clone(),
                macros,
                desktop,
                fallback_canvas: canvas,
                view,
                inspector,
                workspace_label,
                workspace_path,
                footer,
                connection,
                bar_status,
                error,
                error_dialog: RefCell::new(None),
                windows: RefCell::new(HashMap::new()),
                revision: Cell::new(u64::MAX),
                weak: weak.clone(),
                last_workspace: RefCell::new(String::new()),
                row_views: Rc::new(RefCell::new(HashMap::new())),
                row_surfaces: RefCell::new(HashMap::new()),
                native_nodes: options.integrated && !windowed,
                displayed_row: RefCell::new(None),
                windowed,
                theme_monitors: RefCell::new(vec![]),
                integrated: options.integrated,
                graph_visible: Cell::new(false),
                transition_serial: Cell::new(0),
                first_frame: Cell::new(true),
                pending_focus: RefCell::new(None),
                return_window: Cell::new(None),
                bar_geometry: RefCell::new(HashMap::new()),
                fallback_content: root.clone(),
                captures: (!options.integrated || windowed)
                    .then(|| super::collapse::Captures::new(engine.clone())),
                chat: RefCell::new(None),
                search_popup: RefCell::new(None),
                reveal: Cell::new(1.0),
                paint_started: Cell::new(None),
                frame_monitors: RefCell::new(HashSet::new()),
                pinned_rows: RefCell::new(HashSet::new()),
            }
        });
        on_click(&workspace_button, &shell, |s| s.row_settings());
        on_click(&add_button, &shell, |s| s.palette());
        on_click(&windows_button, &shell, |s| s.show_windows());
        on_click(&show_nodes, &shell, |s| s.show_nodes());
        on_click(&nodes_button, &shell, |s| s.fit());
        on_click(&fit_button, &shell, |s| s.fit());
        on_click(&run_button, &shell, |s| {
            s.engine.dispatch(Action::RunWorkspace)
        });
        on_click(&notes_button, &shell, |s| s.notes());
        on_click(&bar_notes, &shell, |s| {
            s.show_nodes();
            s.notes();
        });
        on_click(&recipes_button, &shell, |s| s.recipes());
        on_click(&help_button, &shell, |s| s.help());
        on_click(&previous, &shell, |s| s.switch_workspace(-1));
        on_click(&next, &shell, |s| s.switch_workspace(1));
        on_click(&quit, &shell, |s| s.app.quit());
        let w = shell.weak.clone();
        window.connect_close_request(move |_| {
            if let Some(s) = w.upgrade() {
                s.app.quit();
            }
            glib::Propagation::Proceed
        });
        shell.apply_insets();
        shell.refresh();
        shell
    }
    fn canvas_action(&self, action: CanvasAction) {
        match action {
            CanvasAction::Workflow(id) => self.control(UiCommand::Workspace { id }),
            CanvasAction::Select(id) => self.select(id),
            CanvasAction::Open(id) => self.open(&id),
            CanvasAction::Add => self.palette(),
            CanvasAction::Edit(id) => self.edit(&id),
            CanvasAction::Delete(ids) => self.engine.dispatch(Action::DeleteMany { ids }),
            CanvasAction::Run(id) => self.engine.dispatch(Action::Run {
                id,
                downstream: false,
            }),
            CanvasAction::Stop(id) => self.engine.dispatch(Action::Stop { id }),
            CanvasAction::Chat => self.agent_chat(),
            CanvasAction::ConnectSearch => self.connect_search(),
            CanvasAction::ConnectPin(endpoint, output) => self.connect_pin_search(endpoint, output),
            CanvasAction::Arrange => self.arrange(),
            CanvasAction::Macros => self.search(Some("macros")),
            CanvasAction::Undo => self.engine.dispatch(Action::Undo),
            CanvasAction::Redo => self.engine.dispatch(Action::Redo),
            CanvasAction::Copy(cut) => self.copy_selection(cut),
            CanvasAction::Paste => self.paste_selection(),
            CanvasAction::Duplicate => {
                self.copy_selection(false);
                self.paste_selection();
            }
            CanvasAction::EditValue(id) => self.edit_value(&id),
            CanvasAction::Search => self.search_nodes(),
            CanvasAction::Help => self.help(),
            CanvasAction::ClearHistory => self.engine.dispatch(Action::ClearHistory),
            CanvasAction::Camera => self.prepare_camera(),
            CanvasAction::Escape => self.escape(),
        }
    }
    fn arrange(&self) {
        self.prepare_camera();
        let state = self.engine.snapshot().state;
        let (width, height) = self.scene_size();
        let positions = super::layout::arrange(
            &state,
            width,
            height,
            crate::config::flow()
                .arrange_wrap_zoom
                .unwrap_or(super::layout::DEFAULT_WRAP_ZOOM),
        );
        let previous: HashMap<_, _> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .map(|n| (n.id.clone(), self.view.borrow().position(n)))
            .collect();
        match self.engine.request(Action::Arrange { positions }) {
            Ok(_) => {
                let state = self.engine.snapshot().state;
                let mut view = self.view.borrow_mut();
                view.arrangement
                    .update(&state, self.canvas().width(), self.canvas().height());
                for node in &state.nodes {
                    if let Some(old) = previous.get(&node.id) {
                        view.presented_positions.insert(
                            node.id.clone(),
                            Position {
                                x: old.x - node.position.x,
                                y: old.y - node.position.y,
                            },
                        );
                    }
                }
                let mut framed = view.clone();
                framed.fit(&state, width as f64, height as f64);
                view.fitted = framed.fitted;
                let camera = (framed.zoom, framed.pan_x, framed.pan_y);
                drop(view);
                if framed.fitted {
                    self.first_frame.set(false);
                    self.pending_focus.borrow_mut().take();
                }
                self.transition_with_camera(true, None, Some(camera));
            }
            Err(error) => self.show_error(&error.to_string()),
        }
    }
    fn copy_selection(&self, cut: bool) {
        let state = self.engine.snapshot().state;
        let view = self.view.borrow();
        let selected: Vec<_> = state
            .nodes
            .iter()
            .filter(|n| {
                n.workspace == state.selected_workspace
                    && (view.selection.contains(&n.id) || view.selected.as_ref() == Some(&n.id))
            })
            .map(|n| n.id.clone())
            .collect();
        let Ok(mut recipe) = state.recipe("Clipboard".into(), &selected) else {
            return;
        };
        for node in &mut recipe.nodes {
            if let Some(original) = state.node(&node.id) {
                node.position = view.position(original);
            }
        }
        drop(view);
        let text = serde_json::json!({"flowwm_clipboard":1,"recipe":recipe}).to_string();
        self.canvas().clipboard().set_text(&text);
        if cut {
            self.engine.dispatch(Action::DeleteMany { ids: selected });
        }
    }
    fn paste_selection(&self) {
        let weak = self.weak.clone();
        let clipboard = self.canvas().clipboard();
        glib::MainContext::default().spawn_local(async move {
            let Ok(Some(text)) = clipboard.read_text_future().await else {
                return;
            };
            let Some(shell) = weak.upgrade() else { return };
            if text.len() > 8 * 1024 * 1024 {
                shell.show_error("The copied graph is too large");
                return;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                return;
            };
            if value["flowwm_clipboard"] != 1 {
                return;
            }
            let Ok(recipe) = serde_json::from_value::<Recipe>(value["recipe"].clone()) else {
                return;
            };
            let state = shell.engine.snapshot().state;
            let mut offset = Position { x: 40., y: 40. };
            while recipe.nodes.iter().any(|copy| {
                state.nodes.iter().any(|existing| {
                    let p = shell.view.borrow().position(existing);
                    existing.workspace == state.selected_workspace
                        && copy.position.x + offset.x < p.x + canvas::WIDTH + 20.
                        && copy.position.x + offset.x + canvas::WIDTH + 20. > p.x
                        && copy.position.y + offset.y < p.y + canvas::node_height(existing) + 20.
                        && copy.position.y + offset.y + canvas::node_height(copy) + 20. > p.y
                })
            }) {
                offset.x += 40.;
                offset.y += 40.;
            }
            match shell.engine.request(Action::Paste { recipe, offset }) {
                Ok(result) => {
                    let ids: Vec<String> =
                        serde_json::from_value(result["ids"].clone()).unwrap_or_default();
                    shell.view.borrow_mut().selection = ids.iter().cloned().collect();
                    shell.select(ids.first().cloned());
                    shell.canvas().grab_focus();
                }
                Err(error) => shell.show_error(&error.to_string()),
            }
        });
    }
    fn select(&self, id: Option<String>) {
        if let Some(id) = id.as_ref().filter(|id| {
            self.engine
                .snapshot()
                .state
                .node(id)
                .is_some_and(|n| n.flow.properties.contains_key("_alert"))
        }) {
            self.engine.dispatch(Action::ClearAlert { id: id.clone() });
        }
        let mut view = self.view.borrow_mut();
        view.selected = id.clone();
        if let Some(id) = id {
            if !view.selection.contains(&id) {
                view.selection.clear();
            }
            view.selection.insert(id);
        } else {
            view.selection.clear();
        }
        drop(view);
        // Selection never resizes the canvas. Settings are explicitly opened with Shift+Enter.
        clear(&self.inspector);
        self.inspector.set_visible(false);
        if !self.windowed && !self.native_nodes && self.graph_visible.get() {
            let state = self.engine.snapshot().state;
            let mixed = state.nodes.iter().any(|n| {
                n.workspace == state.selected_workspace
                    && n.column_mode == ColumnMode::Window
                    && n.window_id.is_some()
            });
            self.window().set_keyboard_mode(if mixed {
                KeyboardMode::OnDemand
            } else {
                KeyboardMode::Exclusive
            });
        }
        self.canvas().queue_draw();
    }
    fn peeking(&self) -> bool {
        let state = self.engine.snapshot().state;
        state.workspace().view == RowView::Nodes
            && state.nodes.iter().any(|n| {
                n.workspace == state.selected_workspace
                    && n.column_mode == ColumnMode::Window
                    && n.window_id.is_some()
            })
    }
    fn escape(&self) {
        if self.canvas().dismiss_popovers() {
            self.canvas().grab_focus();
            return;
        }
        if self.peeking() {
            self.prepare_camera();
            self.canvas().grab_focus();
            return;
        }
        let chat = self.chat.borrow_mut().take();
        if let Some(chat) = chat {
            chat.window.close();
            self.canvas().grab_focus();
            return;
        }
        let search = self.search_popup.borrow_mut().take();
        if let Some(search) = search {
            search.close();
            self.canvas().grab_focus();
            return;
        }
        if !self.view.borrow().query.is_empty() {
            self.view.borrow_mut().query.clear();
            self.canvas().queue_draw();
            return;
        }
        self.select(None);
    }
    fn fit_immediate(&self) -> bool {
        canvas::fit_immediate(&self.canvas(), &self.view, &self.engine.snapshot().state)
    }
    fn fit(&self) {
        self.prepare_camera();
        canvas::fit_animated(&self.canvas(), &self.view, &self.engine.snapshot().state);
    }
    fn prepare_camera(&self) {
        let state = self.engine.snapshot().state;
        let mut changed = false;
        for node in state.nodes.iter().filter(|n| {
            n.workspace == state.selected_workspace && n.column_mode == ColumnMode::Window
        }) {
            if let Err(error) = self.engine.request(Action::ColumnMode {
                id: node.id.clone(),
                mode: ColumnMode::Node,
            }) {
                self.show_error(&error.to_string());
                return;
            }
            changed = true;
        }
        if changed {
            // Native clients have fixed screen coordinates. Finish in graph space
            // when the camera moves so cards cannot drift over live input holes.
            self.transition(true, None);
        }
    }
    fn scene_size(&self) -> (i32, i32) {
        let view = self.view.borrow();
        (
            if self.canvas().width() > 100 {
                self.canvas().width()
            } else {
                view.output_size.0 as i32 - view.insets[1] - view.insets[3]
            },
            if self.canvas().height() > 100 {
                self.canvas().height()
            } else {
                view.output_size.1 as i32 - view.insets[0] - view.insets[2]
            },
        )
    }
    fn apply_insets(&self) {
        if self.windowed || !self.integrated || self.native_nodes {
            return;
        }
        let margins = self.current_insets();
        for (edge, margin) in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left]
            .into_iter()
            .zip(margins)
        {
            self.window().set_margin(edge, margin);
        }
        self.window().add_css_class("integrated");
    }
    fn current_insets(&self) -> [i32; 4] {
        let output = self
            .desktop
            .lock()
            .unwrap()
            .workspaces
            .iter()
            .find(|w| w.is_focused)
            .and_then(|w| w.output.clone());
        output
            .as_ref()
            .and_then(|o| self.bar_geometry.borrow().get(o).copied())
            .unwrap_or_else(Theme::desktop_insets)
    }
    fn update_scene_geometry(&self) {
        let desktop = self.desktop.lock().unwrap().clone();
        let display = gtk::prelude::WidgetExt::display(&self.window());
        let output = desktop
            .workspaces
            .iter()
            .find(|w| w.is_focused)
            .and_then(|w| w.output.as_deref());
        let monitor = (0..display.monitors().n_items())
            .filter_map(|i| display.monitors().item(i)?.downcast::<gdk::Monitor>().ok())
            .find(|m| m.connector().as_deref() == output);
        let geometry = monitor
            .as_ref()
            .map(|m| m.geometry())
            .unwrap_or_else(|| gdk::Rectangle::new(0, 0, 1920, 1080));
        if !self.windowed
            && !self.native_nodes
            && let Some(monitor) = monitor
        {
            self.window().set_monitor(Some(&monitor));
        }
        let insets = self.current_insets();
        let mut view = self.view.borrow_mut();
        view.insets = insets;
        view.output_size = (geometry.width() as f64, geometry.height() as f64);
        view.transparent = !self.windowed;
        view.native_nodes = self.native_nodes;
        let binding = self.engine.snapshot().state.workspace().row.clone();
        view.local_windows = Some(
            desktop
                .windows
                .iter()
                .filter(|w| {
                    binding.as_ref().is_some_and(|r| {
                        r.session == desktop.session && Some(r.id) == w.workspace_id
                    })
                })
                .map(|w| w.id)
                .collect(),
        );
        view.focused_window = desktop
            .workspaces
            .iter()
            .find(|w| w.is_focused)
            .and_then(|w| w.active_window_id);
        view.desktop_rects = super::collapse::rectangles(
            &desktop,
            &self.engine.snapshot().state,
            insets,
            (geometry.width() - insets[1] - insets[3]) as f64,
            false,
        );
    }
    fn input_region(&self) {
        let Some(surface) = self.window().surface() else {
            return;
        };
        let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
            0,
            0,
            self.window().width(),
            self.window().height(),
        ));
        let state = self.engine.snapshot().state;
        let view = self.view.borrow();
        if !view.morphing {
            for node in state.nodes.iter().filter(|n| {
                n.workspace == state.selected_workspace && n.column_mode == ColumnMode::Window
            }) {
                if let Some(rect) = view
                    .presented_rects
                    .get(&node.id)
                    .or_else(|| view.desktop_rects.get(&node.id))
                {
                    let _ = region.subtract_rectangle(&rect.integer());
                }
            }
        }
        surface.set_input_region(Some(&region));
    }
    fn request_captures(&self, refresh: bool) {
        if let Some(captures) = &self.captures {
            captures.request(self.desktop.lock().unwrap().clone(), refresh);
        }
    }
    fn collect_images(&self) {
        if self.view.borrow().morphing {
            return;
        }
        let Some(captures) = &self.captures else {
            return;
        };
        let images = std::mem::take(&mut *captures.ready.lock().unwrap());
        let state = self.engine.snapshot().state;
        let mut view = self.view.borrow_mut();
        for image in images {
            let texture = image.texture();
            if let Some((key, surface)) = image.surface() {
                if key == "wallpaper" {
                    view.wallpaper = Some(surface);
                    view.wallpaper_texture = Some(texture);
                } else if let Ok(id) = key.parse::<u64>()
                    && let Some(node) = state.nodes.iter().find(|n| n.window_id == Some(id))
                {
                    view.images.insert(node.id.clone(), surface);
                    view.image_textures.insert(node.id.clone(), texture);
                }
            }
        }
        let live: HashSet<_> = state.nodes.iter().map(|n| n.id.clone()).collect();
        view.images.retain(|id, _| live.contains(id));
        view.image_textures.retain(|id, _| live.contains(id));
        drop(view);
        self.canvas().queue_draw();
    }
    fn transition(&self, visible: bool, after: Option<Box<dyn FnOnce()>>) {
        self.transition_with_camera(visible, after, None);
    }
    fn transition_with_camera(
        &self,
        visible: bool,
        after: Option<Box<dyn FnOnce()>>,
        camera: Option<(f64, f64, f64)>,
    ) {
        self.canvas().slide(None);
        let started = std::time::Instant::now();
        self.transition_serial.set(self.transition_serial.get() + 1);
        let serial = self.transition_serial.get();
        let was_visible = self.graph_visible.replace(visible);
        let was_mapped =
            self.window().is_visible() && (!self.native_nodes || !visible || was_visible);
        let chrome_from = if was_mapped {
            self.view.borrow().chrome
        } else {
            0.0
        };
        canvas::suspend(&self.view);
        let presentation = self.engine.presentation();
        self.engine.set_presentation(Presentation {
            graph_visible: visible,
            lookup_return: presentation.lookup_return,
            lookup_node: None,
            ..Default::default()
        });
        if !visible && !was_mapped {
            if let Some(after) = after {
                after();
            }
            return;
        }
        self.collect_images();
        let state = self.engine.snapshot().state;
        // ColumnMode already describes the destination. Start from the last
        // presentation, or from the settled graph and its screen-space peeks.
        let (mut from, from_rects): (HashMap<_, _>, HashMap<_, _>) = {
            let view = self.view.borrow();
            state
                .nodes
                .iter()
                .filter(|n| n.workspace == state.selected_workspace)
                .map(|n| {
                    let t = if was_mapped {
                        view.presentations
                            .get(&n.id)
                            .copied()
                            .unwrap_or_else(|| f64::from(!view.presented_rects.contains_key(&n.id)))
                    } else {
                        0.0
                    };
                    let (_, rect) = super::gpu::window_geometry_at(n, &view, t);
                    ((n.id.clone(), t), (n.id.clone(), rect))
                })
                .unzip()
        };
        let border_from: HashMap<_, _> = {
            let view = self.view.borrow();
            state
                .nodes
                .iter()
                .filter(|n| n.workspace == state.selected_workspace)
                .map(|n| {
                    (
                        n.id.clone(),
                        if was_mapped {
                            super::gpu::border_opacity(n, &view)
                        } else {
                            f64::from(
                                n.window_id.is_some() && n.window_id == self.return_window.get(),
                            )
                        },
                    )
                })
                .collect()
        };
        self.update_scene_geometry();
        if !visible
            && self.native_nodes
            && let Some(row) = &state.workspace().row
        {
            match niri::request(niri_ipc::Request::PrepareNodeRowExit {
                workspace_id: row.id,
            }) {
                Ok(niri_ipc::Response::NodeRowExit(scene)) => {
                    let destinations: HashMap<_, _> =
                        scene.windows.iter().map(|w| (w.id, w.bounds)).collect();
                    let mut view = self.view.borrow_mut();
                    for node in state
                        .nodes
                        .iter()
                        .filter(|n| n.workspace == state.selected_workspace)
                    {
                        if let Some((x, y, width, height)) =
                            node.window_id.and_then(|id| destinations.get(&id)).copied()
                        {
                            view.desktop_rects.insert(
                                node.id.clone(),
                                super::collapse::Rect {
                                    x,
                                    y,
                                    width,
                                    height,
                                },
                            );
                        }
                    }
                }
                Err(error) => self.show_error(&format!("Prepare window layout: {error:#}")),
                _ => {}
            }
        }
        let mut target: HashMap<_, _> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .map(|n| {
                (
                    n.id.clone(),
                    if visible && (n.column_mode == ColumnMode::Node || n.window_id.is_none()) {
                        1.0
                    } else {
                        0.0
                    },
                )
            })
            .collect();
        let mut from_positions: HashMap<_, _> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .map(|n| (n.id.clone(), self.view.borrow().position(n)))
            .collect();
        let (width, height) = self.scene_size();
        self.view
            .borrow_mut()
            .arrangement
            .update(&state, width, height);
        let view = self.view.borrow();
        let camera_from = (view.zoom, view.pan_x, view.pan_y);
        let (target_zoom, target_pan_x, target_pan_y) = camera.unwrap_or(camera_from);
        let bases: HashMap<_, _> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .map(|n| (n.id.clone(), view.arrangement.position(n)))
            .collect();
        let mut target_rects = HashMap::new();
        let mut expanded = Vec::new();
        let mut cards = Vec::new();
        for node in state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
        {
            let p = &bases[&node.id];
            let graph = super::collapse::Rect {
                x: target_pan_x + p.x * target_zoom,
                y: target_pan_y + p.y * target_zoom,
                width: canvas::WIDTH * target_zoom,
                height: canvas::node_height(node) * target_zoom,
            };
            let mut rect = view.desktop_rects.get(&node.id).copied().unwrap_or(graph);
            if target[&node.id] == 1.0 {
                cards.push((node.id.clone(), graph));
            } else {
                if visible && self.native_nodes {
                    // Peeking is a scene placement, independent of niri's scroll position.
                    rect.x = (width as f64 - rect.width) / 2.;
                    rect.y = (height as f64 - rect.height) / 2.;
                }
                expanded.push(rect);
                target_rects.insert(node.id.clone(), rect);
            }
        }
        if visible && self.native_nodes && expanded.len() > 1 {
            let mut peeks: Vec<_> = target_rects.keys().cloned().collect();
            peeks.sort_by(|a, b| bases[a].x.total_cmp(&bases[b].x).then_with(|| a.cmp(b)));
            let total = peeks.iter().map(|id| target_rects[id].width).sum::<f64>()
                + (peeks.len() - 1) as f64 * canvas::GRID;
            let mut x = (width as f64 - total) / 2.;
            expanded.clear();
            for id in peeks {
                let rect = target_rects.get_mut(&id).unwrap();
                rect.x = x;
                x += rect.width + canvas::GRID;
                expanded.push(*rect);
            }
        }
        if visible {
            super::layout::make_room(&mut cards, &expanded);
        }
        let mut target_positions = bases.clone();
        for (id, rect) in cards {
            target_positions.insert(
                id.clone(),
                Position {
                    x: (rect.x - target_pan_x) / target_zoom,
                    y: (rect.y - target_pan_y) / target_zoom,
                },
            );
            target_rects.insert(id, rect);
        }
        let from_rects: HashMap<_, _> = target_rects
            .iter()
            .map(|(id, target)| {
                let rect = if was_mapped {
                    from_rects.get(id).copied().unwrap_or(*target)
                } else {
                    view.desktop_rects.get(id).copied().unwrap_or(*target)
                };
                (id.clone(), rect)
            })
            .collect();
        let viewport = super::collapse::Rect {
            x: 0.,
            y: 0.,
            width: width as f64,
            height: height as f64,
        };
        let fade_only: HashSet<_> = target_rects
            .iter()
            .filter_map(|(id, destination)| {
                // Cards outside a panned graph still move with the graph camera.
                // The fade path is only for entering/leaving desktop windows.
                if from[id] == 1.0 && target[id] == 1.0 {
                    return None;
                }
                let desktop_end = if visible {
                    from_rects[id]
                } else {
                    *destination
                };
                (!desktop_end.overlaps(viewport)).then(|| id.clone())
            })
            .collect();
        let opacity_from: HashMap<_, _> = target_rects
            .keys()
            .map(|id| {
                let opacity = view.window_opacities.get(id).copied().unwrap_or(
                    if visible && fade_only.contains(id) {
                        0.
                    } else {
                        1.
                    },
                );
                (id.clone(), opacity)
            })
            .collect();
        for id in &fade_only {
            if visible {
                from.insert(id.clone(), target[id]);
                from_positions.insert(id.clone(), target_positions[id].clone());
            } else {
                target.insert(id.clone(), from[id]);
                target_positions.insert(id.clone(), from_positions[id].clone());
            }
        }
        drop(view);
        {
            let mut view = self.view.borrow_mut();
            view.window_opacities = opacity_from.clone();
            view.border_opacities = border_from;
            view.border_fades.clear();
            view.window_borders = !visible;
            if !visible {
                view.focused_window = self.return_window.get();
            }
            view.presented_rects = from_rects.clone();
            for id in &fade_only {
                if visible {
                    view.presented_rects.insert(id.clone(), target_rects[id]);
                }
            }
            for (id, from) in &from_positions {
                let base = &bases[id];
                view.presented_positions.insert(
                    id.clone(),
                    Position {
                        x: from.x - base.x,
                        y: from.y - base.y,
                    },
                );
            }
        }
        self.view.borrow_mut().presentations = from.clone();
        self.view.borrow_mut().camera_target = camera;
        self.view.borrow_mut().morphing = true;
        self.view.borrow_mut().live_fade = 1.0;
        self.view.borrow_mut().chrome = chrome_from;
        self.content().set_opacity(1.0);
        self.window().set_opacity(1.0);
        self.bar.set_visible(false);
        if !self.windowed && !self.native_nodes {
            self.window().set_keyboard_mode(if visible {
                KeyboardMode::Exclusive
            } else {
                KeyboardMode::None
            });
        }
        self.window().present();
        self.input_region();
        if visible {
            self.canvas().grab_focus();
        }
        if !self
            .frame_monitors
            .borrow()
            .contains(&(self.window().as_ptr() as usize))
            && let Some(clock) = self.window().frame_clock()
        {
            let weak = self.weak.clone();
            clock.connect_after_paint(move |_| {
                if let Some(shell) = weak.upgrade()
                    && let Some(start) = shell.paint_started.take()
                {
                    shell
                        .engine
                        .performance
                        .record("first_graph_frame", start.elapsed());
                }
            });
            self.frame_monitors
                .borrow_mut()
                .insert(self.window().as_ptr() as usize);
        }
        let weak = self.weak.clone();
        let after = RefCell::new(after);
        let previous = Cell::new(started);
        let animation_started = Cell::new(None::<i64>);
        self.window().add_tick_callback(move |_, clock| {
            let Some(shell) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if shell.transition_serial.get() != serial {
                return glib::ControlFlow::Break;
            }
            shell.engine.performance.record(
                "transition_frame_gap",
                previous.replace(std::time::Instant::now()).elapsed(),
            );
            if visible && shell.first_frame.get() && shell.fit_immediate() {
                shell.first_frame.set(false);
                gtk::prelude::GtkWindowExt::set_focus(&shell.window(), Some(&shell.canvas()));
                shell.canvas().grab_focus();
                if let Some(id) = shell.pending_focus.borrow_mut().take() {
                    let state = shell.engine.snapshot().state;
                    if let Some(node) = state.node(&id) {
                        let mut view = shell.view.borrow_mut();
                        view.zoom = 0.9;
                        let position = view.arrangement.position(node);
                        view.pan_x = shell.canvas().width() as f64 / 2.0
                            - (position.x + canvas::WIDTH / 2.0) * view.zoom;
                        view.pan_y = shell.canvas().height() as f64 / 2.0
                            - (position.y + canvas::node_height(node) / 2.0) * view.zoom;
                    }
                }
            }
            let duration = shell.view.borrow().theme.motion_ms as f64;
            let animation_started = animation_started.get().unwrap_or_else(|| {
                let now = clock.frame_time();
                animation_started.set(Some(now));
                now
            });
            let elapsed = (clock.frame_time() - animation_started) as f64 / 1000.;
            let progress = if duration == 0.0 {
                1.0
            } else {
                (elapsed / duration).clamp(0.0, 1.0)
            };
            let eased = super::motion::ease_out(progress);
            let fade = if duration == 0.0 || shell.native_nodes {
                0.0
            } else {
                1.0 - ((elapsed - duration) / 65.0).clamp(0.0, 1.0)
            };
            let has_windows = target.values().any(|t| *t == 0.0);
            {
                let mut view = shell.view.borrow_mut();
                let following_camera = camera.is_some() && view.camera_target == camera;
                if following_camera {
                    if progress >= 1.0 {
                        (view.zoom, view.pan_x, view.pan_y) =
                            (target_zoom, target_pan_x, target_pan_y);
                        view.camera_target = None;
                    } else {
                        view.zoom = camera_from.0 + (target_zoom - camera_from.0) * eased;
                        view.pan_x = camera_from.1 + (target_pan_x - camera_from.1) * eased;
                        view.pan_y = camera_from.2 + (target_pan_y - camera_from.2) * eased;
                    }
                }
                for (id, target) in &target {
                    let from = from.get(id).copied().unwrap_or(1.0);
                    view.presentations
                        .insert(id.clone(), from + (*target - from) * eased);
                }
                for (id, position) in &target_positions {
                    let from = from_positions.get(id).unwrap_or(position);
                    let base = &bases[id];
                    let shown = if progress >= 1.0 {
                        position.clone()
                    } else {
                        Position {
                            x: from.x + (position.x - from.x) * eased,
                            y: from.y + (position.y - from.y) * eased,
                        }
                    };
                    view.presented_positions.insert(
                        id.clone(),
                        Position {
                            x: shown.x - base.x,
                            y: shown.y - base.y,
                        },
                    );
                }
                for (id, rect) in &target_rects {
                    // The camera and card rectangle share one interpolation.
                    // Applying the camera again here would ease the preview
                    // twice and could shrink it below its header's dimensions.
                    let destination = if following_camera {
                        *rect
                    } else if target.get(id) == Some(&1.0) {
                        let p = &target_positions[id];
                        super::collapse::Rect {
                            x: view.pan_x + p.x * view.zoom,
                            y: view.pan_y + p.y * view.zoom,
                            width: canvas::WIDTH * view.zoom,
                            height: rect.height * view.zoom / target_zoom,
                        }
                    } else {
                        *rect
                    };
                    let fade = fade_only.contains(id);
                    let rect = if fade {
                        if visible { destination } else { from_rects[id] }
                    } else {
                        super::motion::transform(from_rects[id], destination, eased)
                    };
                    view.presented_rects.insert(id.clone(), rect);
                    let opacity_to = if fade && !visible { 0. } else { 1. };
                    let opacity = opacity_from[id] + (opacity_to - opacity_from[id]) * eased;
                    view.window_opacities.insert(id.clone(), opacity);
                }
                view.chrome =
                    chrome_from + ((if visible { 1.0 } else { 0.0 }) - chrome_from) * eased;
                view.live_fade = fade;
            }
            shell.reveal.set(if visible { eased } else { 1.0 - eased });
            // Geometry reaches the real client before the capture fades away.
            if !visible && progress >= 1.0 && !shell.native_nodes {
                shell.content().set_opacity(fade);
            }
            shell.canvas().queue_draw();
            if progress >= 1.0 && (!has_windows || fade <= 0.0) {
                let weak = shell.weak.clone();
                let after = after.borrow_mut().take();
                let target = target.clone();
                // Submit the endpoint scene and chrome together before unmapping.
                // Native previews keep rendering until the canvas's null commit.
                super::surface::after_paint(clock, move || {
                    let Some(shell) = weak.upgrade() else { return };
                    if shell.transition_serial.get() != serial {
                        return;
                    }
                    {
                        let mut view = shell.view.borrow_mut();
                        view.morphing = false;
                        view.window_opacities.clear();
                        // Graph cards follow the camera again; peeks keep screen coordinates.
                        view.presented_rects
                            .retain(|id, _| visible && target.get(id) == Some(&0.0));
                    }
                    shell.collect_images();
                    if visible {
                        shell.request_captures(!was_visible);
                    }
                    shell.input_region();
                    shell.canvas().queue_draw();
                    if !visible {
                        shell.window().set_visible(false);
                    } else if has_windows && !shell.windowed && !shell.native_nodes {
                        shell.window().set_keyboard_mode(KeyboardMode::OnDemand);
                        if let Some(id) = shell.return_window.get().filter(|id| {
                            shell.engine.snapshot().state.nodes.iter().any(|n| {
                                n.window_id == Some(*id) && n.column_mode == ColumnMode::Window
                            })
                        }) {
                            let _ = niri::focus(id);
                        }
                    }
                    shell.engine.performance.record(
                        if visible {
                            "transition_open"
                        } else {
                            "transition_close"
                        },
                        started.elapsed(),
                    );
                    if let Some(after) = after {
                        after();
                    }
                    // Desktop events can arrive while a morph owns the scene.
                    // Reconcile once more even if no later engine event wakes us.
                    shell.refresh_scene();
                });
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
    fn show_nodes(&self) {
        let started = std::time::Instant::now();
        let was_visible = self.graph_visible.get();
        if !was_visible {
            self.paint_started.set(Some(started));
        }
        self.synchronize_row();
        self.apply_insets();
        if !was_visible {
            let desktop = self.desktop.lock().unwrap().clone();
            if let Some(window) = desktop.windows.iter().find(|w| w.is_focused) {
                self.return_window.set(Some(window.id));
            }
        }
        let state = self.engine.snapshot().state;
        if let Err(error) = self.engine.request(Action::RowView {
            workspace: state.selected_workspace.clone(),
            view: RowView::Nodes,
        }) {
            self.show_error(&error.to_string());
            return;
        }
        self.engine.set_presentation(Presentation {
            graph_visible: true,
            lookup_return: false,
            lookup_node: None,
            ..Default::default()
        });
        self.refresh();
        if self.native_nodes
            && !self
                .row_surfaces
                .borrow()
                .contains_key(&state.selected_workspace)
        {
            return;
        }
        if self.native_nodes && !was_visible {
            self.graph_visible.set(false);
            self.view.borrow_mut().chrome = 0.0;
        }
        if !was_visible {
            self.fit_immediate();
        }
        self.transition(true, None);
        self.engine
            .performance
            .record("show_nodes_prepare", started.elapsed());
    }
    fn synchronize_row(&self) {
        if !self.integrated {
            return;
        }
        let desktop = self.desktop.lock().unwrap().clone();
        if let Err(error) = self.engine.request(Action::SyncDesktop {
            desktop: desktop.clone(),
        }) {
            self.show_error(&error.to_string());
        }
    }
    fn activate_group(&self, id: &str) -> Result<()> {
        let state = self.engine.snapshot().state;
        let group = state
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .context("Group not found")?;
        let desktop = self.desktop.lock().unwrap().clone();
        let live = group.row.as_ref().and_then(|row| {
            desktop
                .workspaces
                .iter()
                .find(|w| w.id == row.id && row.session == desktop.session)
        });
        let row = if let Some(live) = live {
            live
        } else {
            let output = desktop
                .workspaces
                .iter()
                .find(|w| w.is_focused)
                .and_then(|w| w.output.as_ref());
            let empty = desktop
                .workspaces
                .iter()
                .find(|row| {
                    row.output.as_ref() == output
                        && row.active_window_id.is_none()
                        && row.name.is_none()
                        && !state.workspaces.iter().any(|g| {
                            g.row
                                .as_ref()
                                .is_some_and(|r| r.session == desktop.session && r.id == row.id)
                                && state.nodes.iter().any(|n| n.workspace == g.id)
                        })
                })
                .context("No empty row is available for this group")?;
            self.engine.request(Action::BindRow {
                workspace: id.into(),
                row: Row {
                    session: desktop.session.clone(),
                    id: empty.id,
                    index: empty.idx,
                    output: empty.output.clone(),
                    name: empty.name.clone(),
                },
            })?;
            empty
        };
        niri::request(niri_ipc::Request::Action(
            niri_ipc::Action::FocusWorkspace {
                reference: niri_ipc::WorkspaceReferenceArg::Id(row.id),
            },
        ))?;
        self.desktop.lock().unwrap().activate_row(row.id, true);
        Ok(())
    }
    fn pin_groups(&self, state: &State) {
        if !self.integrated {
            return;
        }
        let desktop = self.desktop.lock().unwrap().clone();
        for group in &state.workspaces {
            let Some(row) = group
                .row
                .as_ref()
                .filter(|r| r.session == desktop.session && r.name.is_none())
            else {
                continue;
            };
            if !desktop.workspaces.iter().any(|w| w.id == row.id) {
                continue;
            }
            if !state
                .nodes
                .iter()
                .any(|n| n.workspace == group.id && !n.desktop_window)
                || !self.pinned_rows.borrow_mut().insert(row.id)
            {
                continue;
            }
            let name = if desktop
                .workspaces
                .iter()
                .any(|w| w.name.as_deref() == Some(group.name.as_str()))
            {
                format!("{} {}", group.name, row.index)
            } else {
                group.name.clone()
            };
            let id = row.id;
            std::thread::spawn(move || {
                if let Err(error) = niri::request(niri_ipc::Request::Action(
                    niri_ipc::Action::SetWorkspaceName {
                        name,
                        workspace: Some(niri_ipc::WorkspaceReferenceArg::Id(id)),
                    },
                )) {
                    eprintln!("Keep node-group row: {error:#}");
                }
            });
        }
    }
    fn show_windows(&self) {
        self.synchronize_row();
        self.refresh();
        let state = self.engine.snapshot().state;
        let selected = canvas::first_selected(&state, &self.view.borrow()).cloned();
        if let Some(window) = selected.as_ref().and_then(|n| n.window_id) {
            self.return_window.set(Some(window));
        }
        if let Err(error) = self.engine.request(Action::RowView {
            workspace: state.selected_workspace.clone(),
            view: RowView::Windows,
        }) {
            self.show_error(&error.to_string());
            return;
        }
        self.hide_graph(true);
        if let Some(node) = selected.filter(|n| {
            n.window_id.is_none()
                && n.kind != NodeKind::Window
                && window_interface(&state, n).is_some()
        }) {
            self.open(&node.id);
        }
    }
    fn hide_graph(&self, restore_focus: bool) {
        let was_visible = self.graph_visible.get();
        let state = self.engine.snapshot().state;
        self.engine.set_presentation(Presentation {
            graph_visible: false,
            lookup_return: false,
            lookup_node: None,
            ..Default::default()
        });
        if restore_focus
            && was_visible
            && let Some(id) = self.return_window.get()
            && self.desktop.lock().unwrap().windows.iter().any(|w| {
                w.id == id
                    && state
                        .nodes
                        .iter()
                        .any(|n| n.window_id == Some(id) && n.workspace == state.selected_workspace)
            })
            && let Err(error) = niri::focus(id)
        {
            self.show_error(&error.to_string());
        }
        let weak = self.weak.clone();
        self.transition(
            false,
            Some(Box::new(move || {
                if let Some(shell) = weak.upgrade()
                    && !shell.windowed
                    && !shell.integrated
                {
                    shell.bar.present();
                }
            })),
        );
    }
    fn connect_search(&self) {
        let Some(id) = self.view.borrow().selected.clone() else {
            self.palette();
            return;
        };
        if !self.integrated {
            self.show_error("Connection search requires the FlowWM Dux desktop. Drag between pins to connect in standalone mode.");
            return;
        }
        self.dms_launcher(Some("connect"));
        let mut presentation = self.engine.presentation();
        presentation.lookup_node = Some(id);
        self.engine.set_presentation(presentation);
    }
    fn connect_pin_search(&self, endpoint: crate::graph::Endpoint, output: bool) {
        self.dms_launcher(Some("nodes"));
        let mut presentation = self.engine.presentation();
        presentation.lookup_node = Some(endpoint.node);
        presentation.lookup_pin = Some(endpoint.pin);
        presentation.lookup_input = !output;
        self.engine.set_presentation(presentation);
    }
    fn create_connected(
        &self,
        macro_id: &str,
        source: crate::graph::Endpoint,
        input: &str,
        reverse: bool,
        application: Option<crate::client::ApplicationSelection>,
    ) {
        let Some(m) = self.macros.iter().find(|m| m.id == macro_id) else {
            return;
        };
        let state = self.engine.snapshot().state;
        let Some(parent) = state.node(&source.node) else {
            return;
        };
        let mut position = self.view.borrow().position(parent);
        position.y += if reverse {
            -canvas::HEIGHT - 110.
        } else {
            canvas::node_height(parent) + 110.
        };
        while state.nodes.iter().any(|n| {
            n.workspace == parent.workspace
                && (self.view.borrow().position(n).x - position.x).abs() < canvas::WIDTH + 20.
                && (self.view.borrow().position(n).y - position.y).abs()
                    < canvas::node_height(n) + 20.
        }) {
            position.x += canvas::WIDTH + 100.;
        }
        let mut node = Node::new(&parent.workspace, m.kind, parent.cwd.clone(), position);
        node.title = m.name.clone();
        node.command = m.command.clone();
        node.flow = m.flow.clone();
        if let Some(application) = application {
            node.title = application.name;
            node.flow
                .properties
                .insert("application".into(), serde_json::json!(application.id));
            if !application.app_id.is_empty() {
                node.flow.properties.insert(
                    "application_id".into(),
                    serde_json::json!(application.app_id),
                );
            }
        }

        let id = node.id.clone();
        let mut edge = crate::graph::Connection {
            source,
            target: crate::graph::Endpoint {
                node: id.clone(),
                pin: input.into(),
            },
        };
        if reverse {
            std::mem::swap(&mut edge.source, &mut edge.target);
        }
        let mut candidate = (*state).clone();
        candidate.nodes.push(node.clone());
        candidate.connections.push(edge.clone());
        if let Err(error) = candidate.validate() {
            self.show_error(&error.to_string());
            return;
        }
        match self
            .engine
            .request(Action::Create {
                node: Box::new(node),
            })
            .and_then(|_| {
                self.engine
                    .request(Action::ConnectPins { connection: edge })
            }) {
            Ok(_) => {
                self.show_nodes();
                self.select(Some(id.clone()));
                canvas::focus_node(
                    &self.canvas(),
                    &self.view,
                    &self.engine.snapshot().state,
                    &id,
                );
                self.edit(&id);
            }
            Err(error) => {
                self.engine.dispatch(Action::Delete { id });
                self.show_error(&error.to_string());
            }
        }
    }
    fn search(&self, mode: Option<&str>) {
        if self.integrated {
            self.synchronize_row();
            self.refresh();
            // Opening search must preserve the current row presentation.
            // Win+Space launches apps in Windows and creates nodes in Nodes.
            let mode = mode.or_else(|| {
                (self.engine.snapshot().state.workspace().view == RowView::Nodes)
                    .then_some("create")
            });
            self.dms_launcher(mode);
        } else if mode == Some("macros") {
            self.workflow_macros();
        } else if mode.is_none_or(|mode| mode == "create") {
            self.palette();
        } else {
            self.search_nodes();
        }
    }
    fn workflow_macros(&self) {
        let popup = self.dialog("Macros", 480, 300);
        let root = column(8);
        root.add_css_class("dialog-body");
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search workflow operations…"));
        search.set_search_delay(0);
        root.append(&search);
        let list = gtk::ListBox::new();
        let operations = [
            ("Rearrange", UiCommand::Arrange),
            ("Frame all nodes", UiCommand::Fit),
            ("Undo", UiCommand::Undo),
            ("Redo", UiCommand::Redo),
        ];
        let titles = operations
            .iter()
            .map(|(title, _)| title.to_lowercase())
            .collect::<Vec<_>>();
        for (title, _) in &operations {
            list.append(&label(title, ""));
        }
        let query = Rc::new(RefCell::new(String::new()));
        let filter = query.clone();
        list.set_filter_func(move |row| {
            titles[row.index() as usize].contains(filter.borrow().as_str())
        });
        let filtered = list.clone();
        search.connect_search_changed(move |entry| {
            *query.borrow_mut() = entry.text().to_lowercase();
            filtered.invalidate_filter();
        });
        let weak = self.weak.clone();
        let p = popup.clone();
        list.connect_row_activated(move |_, row| {
            p.close();
            if let Some(shell) = weak.upgrade() {
                shell.control(operations[row.index() as usize].1.clone());
            }
        });
        let chosen = list.clone();
        search.connect_activate(move |_| {
            if let Some(row) = (0..4)
                .filter_map(|i| chosen.row_at_index(i))
                .find(|r| r.is_child_visible())
            {
                row.activate();
            }
        });
        root.append(&list);
        popup.set_child(Some(&root));
        popup.present();
        search.grab_focus();
    }
    fn dms_launcher(&self, mode: Option<&str>) {
        let query = mode.map(|mode| {
            if mode == "connect" {
                let state = self.engine.snapshot().state;
                if let Some(node) = self
                    .view
                    .borrow()
                    .selected
                    .as_deref()
                    .and_then(|id| state.node(id))
                    && crate::graph::pins(&state, node, true).len() > 1
                {
                    return format!("@ {}: ", node.id);
                }
            }
            match mode {
                "create" => "@ + ",
                "recipes" => "@ # ",
                "macros" => "@ > ",
                _ => "@ ",
            }
            .to_string()
        });
        let restore = self.graph_visible.get() || self.engine.presentation().lookup_return;
        self.engine.set_presentation(Presentation {
            graph_visible: self.native_nodes && self.graph_visible.get(),
            lookup_return: restore,
            lookup_node: None,
            ..Default::default()
        });
        if self.graph_visible.get() {
            self.fit();
        }
        if !self.native_nodes {
            self.transition(false, None);
        }
        self.bar.set_visible(false);
        if self.engine.open_launcher(query.clone()) {
            return;
        }
        std::thread::spawn(move || {
            let mut command = std::process::Command::new("dms");
            command.args(["ipc", "call", "spotlight"]);
            if let Some(query) = query {
                command.args(["openQuery", &query]);
            } else {
                command.arg("toggle");
            }
            if let Err(error) = command.output() {
                eprintln!("DMS launcher: {error}");
            }
        });
    }
    fn focus_node(&self, id: &str, expand: bool) {
        let state = self.engine.snapshot().state;
        let Some(node) = state.node(id) else {
            self.show_error("This node no longer exists.");
            return;
        };
        if state.selected_workspace != node.workspace {
            if self.integrated
                && let Err(error) = self.activate_group(&node.workspace)
            {
                self.show_error(&error.to_string());
                return;
            }
            if let Err(error) = self.engine.request(Action::SelectWorkspace {
                id: node.workspace.clone(),
            }) {
                self.show_error(&error.to_string());
                return;
            }
            self.refresh();
        }
        self.engine.set_presentation(Presentation {
            graph_visible: !expand,
            lookup_return: false,
            lookup_node: None,
            ..Default::default()
        });
        self.select(Some(id.into()));
        if self.first_frame.get() {
            *self.pending_focus.borrow_mut() = Some(id.into());
        }
        if expand {
            self.open(id);
        } else {
            self.show_nodes();
            let weak = self.weak.clone();
            let id = id.to_string();
            if !self.first_frame.get() {
                glib::idle_add_local_once(move || {
                    if let Some(shell) = weak.upgrade() {
                        canvas::focus_node(
                            &shell.canvas(),
                            &shell.view,
                            &shell.engine.snapshot().state,
                            &id,
                        );
                    }
                });
            }
        }
    }
    fn stop_focused_window(&self) -> bool {
        let focused = self
            .desktop
            .lock()
            .unwrap()
            .windows
            .iter()
            .find(|w| w.is_focused)
            .map(|w| w.id);
        let state = self.engine.snapshot().state;
        let Some(node) = state.nodes.iter().find(|n| {
            n.kind == NodeKind::Window && n.window_id.is_some() && n.window_id == focused
        }) else {
            return false;
        };
        self.engine.dispatch(Action::Stop {
            id: node.id.clone(),
        });
        true
    }
    fn control(&self, command: UiCommand) {
        match command {
            UiCommand::RowSettings => self.row_settings(),
            UiCommand::Workflows => self.workspace_dialog(),
            UiCommand::RunSelected | UiCommand::StopSelected => {
                let run = matches!(command, UiCommand::RunSelected);
                if !run
                    && (!self.graph_visible.get() || self.peeking())
                    && self.stop_focused_window()
                {
                    return;
                }
                if self.graph_visible.get() {
                    let view = self.view.borrow();
                    let mut ids = view.selection.clone();
                    ids.extend(view.selected.clone());
                    drop(view);
                    let state = self.engine.snapshot().state;
                    for id in ids {
                        let Some(node) = state.node(&id) else {
                            continue;
                        };
                        if state.behavior(node) == Some(flow_core::providers::Behavior::Source) {
                            if run {
                                self.edit(&id);
                            }
                            continue;
                        }
                        if run && !node.status.active() {
                            self.engine.dispatch(Action::Run {
                                id,
                                downstream: false,
                            });
                        } else if !run && (node.status.active() || node.kind == NodeKind::Window) {
                            self.engine.dispatch(Action::Stop { id });
                        }
                    }
                } else {
                    let result = if run {
                        let state = self.engine.snapshot().state;
                        let host = flow_host::client::Client::new(flow_host::ipc::host_socket());
                        crate::launch::row_environment(
                            &host,
                            &state.selected_workspace.clone().into(),
                        )
                        .and_then(|env| {
                            crate::launch::command(
                                &crate::config::flow()
                                    .terminal
                                    .clone()
                                    .unwrap_or_else(|| vec!["alacritty".into()]),
                                &env,
                            )
                        })
                    } else {
                        niri::request(niri_ipc::Request::Action(niri_ipc::Action::CloseWindow {
                            id: None,
                        }))
                        .map(|_| ())
                    };
                    if let Err(error) = result {
                        self.show_error(&error.to_string());
                    }
                }
            }
            UiCommand::RefreshImages => self.collect_images(),
            UiCommand::AgentChat => self.agent_chat(),
            UiCommand::Arrange => self.arrange(),
            UiCommand::Undo => self.engine.dispatch(Action::Undo),
            UiCommand::Redo => self.engine.dispatch(Action::Redo),
            UiCommand::ConnectSearch => self.connect_search(),
            UiCommand::CreateConnected {
                id,
                source,
                input,
                reverse,
                application,
            } => self.create_connected(&id, source, &input, reverse, application),
            UiCommand::MoveSelection { direction } => {
                if self.graph_visible.get() {
                    canvas::move_selection(&self.engine, &self.view, &direction);
                } else if let Err(error) = niri::move_window(&direction) {
                    self.show_error(&error.to_string());
                }
            }
            UiCommand::Toggle => {
                self.synchronize_row();
                self.refresh();
                if self.peeking() {
                    self.prepare_camera();
                    self.canvas().grab_focus();
                } else if self.engine.snapshot().state.workspace().view == RowView::Nodes {
                    self.show_windows();
                } else {
                    self.show_nodes();
                }
            }
            UiCommand::Nodes => self.show_nodes(),
            UiCommand::Windows => self.show_windows(),
            UiCommand::ZoomIn | UiCommand::ZoomOut => {
                if self.graph_visible.get() {
                    self.prepare_camera();
                    canvas::zoom_keyboard(
                        &self.canvas(),
                        &self.view,
                        matches!(command, UiCommand::ZoomIn),
                    );
                } else if let Err(error) = niri::request(niri_ipc::Request::Action(
                    niri_ipc::Action::SetColumnWidth {
                        change: niri_ipc::SizeChange::AdjustProportion(
                            if matches!(command, UiCommand::ZoomIn) {
                                10.
                            } else {
                                -10.
                            },
                        ),
                    },
                )) {
                    self.show_error(&error.to_string());
                }
            }
            UiCommand::Quit => self.app.quit(),
            UiCommand::Notes => {
                self.show_nodes();
                self.notes();
            }
            UiCommand::Recipes => {
                self.show_nodes();
                self.recipes();
            }
            UiCommand::RunGroup => self.engine.dispatch(Action::RunWorkspace),
            UiCommand::Help => self.help(),
            UiCommand::BarGeometry { output, insets } => {
                self.bar_geometry
                    .borrow_mut()
                    .insert(output, insets.map(|n| n.clamp(0, 512)));
                self.ensure_row_surfaces(&self.engine.snapshot().state);
                self.apply_insets();
            }
            UiCommand::Open { id } => self.focus_node(&id, true),
            UiCommand::Select { id } => self.select(Some(id)),
            UiCommand::Focus { id, expand } => self.focus_node(&id, expand),
            UiCommand::Search { mode } => self.search(Some(&mode)),
            UiCommand::Launcher => {
                if !self.integrated && self.graph_visible.get() {
                    self.palette();
                } else {
                    self.dms_launcher(None);
                }
            }
            UiCommand::ResumeLookup => {
                if self.engine.presentation().lookup_return {
                    self.show_nodes();
                }
            }
            UiCommand::CreateApplication { id, name, app_id } => {
                if let Some(index) = self.macros.iter().position(|m| m.id == "window") {
                    self.show_nodes();
                    self.create_macro_with_application(index, Some((id, name, app_id)));
                }
            }
            UiCommand::CreateMacro { id } => {
                if let Some(index) = self.macros.iter().position(|m| m.id == id) {
                    self.show_nodes();
                    self.create_macro(index);
                }
            }
            UiCommand::Workspace { id } => {
                if self.integrated
                    && let Err(error) = self.activate_group(&id)
                {
                    self.show_error(&error.to_string());
                    return;
                }
                match self.engine.request(Action::SelectWorkspace { id }) {
                    Ok(_) => {
                        self.refresh();
                    }
                    Err(error) => self.show_error(&error.to_string()),
                }
            }
            UiCommand::Recipe { id } => match self.engine.request(Action::Instantiate { id }) {
                Ok(_) => {
                    self.refresh();
                    self.show_nodes();
                    self.fit();
                }
                Err(error) => self.show_error(&error.to_string()),
            },
            UiCommand::Edit { id } => self.edit(&id),
            UiCommand::Palette => {
                if self.integrated {
                    self.search(None);
                } else {
                    if !self.graph_visible.get() {
                        self.show_nodes();
                    }
                    self.palette();
                }
            }
            UiCommand::Fit => self.fit(),
            UiCommand::Dismiss => {
                self.canvas().dismiss_popovers();
                for window in self.app.windows() {
                    if window.has_css_class("flow-dialog") {
                        window.close();
                    }
                }
            }
            UiCommand::Capture { path } => {
                if let Err(error) = self.capture(path) {
                    self.show_error(&format!("Capture: {error:#}"));
                }
            }
            UiCommand::Navigate { direction } => {
                if self.graph_visible.get() {
                    self.prepare_camera();
                    let id = canvas::navigate(
                        &self.canvas(),
                        &self.view,
                        &self.engine.snapshot().state,
                        &direction,
                    );
                    self.select(id);
                    self.canvas().queue_draw();
                } else if let Err(error) = niri::navigate(&direction) {
                    self.show_error(&error.to_string());
                }
            }
        }
    }
    fn refresh(&self) {
        let snapshot = self.engine.snapshot();
        let state = &snapshot.state;
        let connected = self.desktop.lock().unwrap().connected;
        self.connection.set_text(if connected {
            "●  Niri connected"
        } else {
            "○  Niri offline"
        });
        if snapshot.revision == self.revision.get() {
            return;
        }
        self.revision.set(snapshot.revision);
        if !self.graph_visible.get() {
            self.request_captures(false);
        }
        self.refresh_chat(state);
        self.pin_groups(state);
        let visible: Vec<_> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .collect();
        let running = visible
            .iter()
            .filter(|n| n.status == Status::Running)
            .count();
        let attention = visible
            .iter()
            .filter(|n| matches!(n.status, Status::Review | Status::Failed))
            .count();
        let edges: usize = state.connections.len();
        self.footer.set_text(&format!(
            "{} nodes  ·  {} connections  ·  saved locally",
            visible.len(),
            edges
        ));
        self.bar_status.set_text(&format!(
            "{}    ·    {} working    ·    {} need you",
            state.workspace().name,
            running,
            attention
        ));
        self.workspace_label.set_text(&state.workspace().name);
        self.workspace_path
            .set_text(&state.workspace().cwd.to_string_lossy());
        if let Some(error) = &snapshot.error {
            self.show_error(error);
        } else {
            self.error.set_visible(false);
        }
        let new_surface = self.ensure_row_surfaces(state);
        let workspace_changed = *self.last_workspace.borrow() != state.selected_workspace;
        if workspace_changed || new_surface {
            let previous = self.last_workspace.borrow().clone();
            if let Some(surface) = self.row_surfaces.borrow().get(&previous) {
                surface.canvas.dismiss_popovers();
            }
            let outgoing = self
                .window()
                .is_visible()
                .then(|| self.canvas().frame())
                .flatten();
            let direction = row_direction(
                self.displayed_row.borrow().as_ref(),
                state.workspace().row.as_ref(),
            );
            canvas::suspend(&self.view);
            if !previous.is_empty() {
                let mut saved = self.view.borrow().clone();
                if self.native_nodes {
                    saved.presentations.clear();
                    saved.border_opacities.clear();
                    saved.border_fades.clear();
                    saved.window_borders = false;
                    saved.morphing = false;
                    saved.live_fade = 0.0;
                }
                self.row_views.borrow_mut().insert(previous, saved);
            }
            let saved = self
                .row_views
                .borrow()
                .get(&state.selected_workspace)
                .cloned();
            let known = saved.is_some();
            let mut restored = saved
                .unwrap_or_else(|| self.view.borrow().for_row(state.selected_workspace.clone()));
            // Palette and image resources may have changed while this row was away.
            let current = self.view.borrow();
            restored.theme = current.theme.clone();
            restored.wallpaper = current.wallpaper.clone();
            restored.wallpaper_texture = current.wallpaper_texture.clone();
            restored.images.extend(current.images.clone());
            restored
                .image_textures
                .extend(current.image_textures.clone());
            drop(current);
            canvas::restore(&self.view, restored);
            if self.integrated {
                self.return_window.set(
                    self.desktop
                        .lock()
                        .unwrap()
                        .workspaces
                        .iter()
                        .find(|w| w.is_focused)
                        .and_then(|w| w.active_window_id),
                );
                self.apply_insets();
            }
            *self.last_workspace.borrow_mut() = state.selected_workspace.clone();
            let selected = self.view.borrow().selected.clone();
            self.select(selected);
            if !known {
                self.fit_immediate();
            }
            self.restore_row(state.workspace().view, outgoing, direction);
        }
        *self.displayed_row.borrow_mut() = state.workspace().row.clone();
        let selected = self.view.borrow().selected.clone();
        self.view.borrow_mut().selection.retain(|id| {
            state
                .node(id)
                .is_some_and(|n| n.workspace == state.selected_workspace)
        });
        if selected.as_ref().is_some_and(|id| {
            state
                .node(id)
                .is_none_or(|n| n.workspace != state.selected_workspace)
        }) {
            self.select(None);
        }
        let missing: Vec<_> = self
            .windows
            .borrow()
            .keys()
            .filter(|id| state.node(id).is_none())
            .cloned()
            .collect();
        for id in missing {
            let removed = self.windows.borrow_mut().remove(&id);
            if let Some(win) = removed {
                win.window.close();
            }
        }
        for (id, win) in self.windows.borrow().iter() {
            if let Some(node) = state.node(id) {
                win.window.set_title(Some(&node.title));
                win.panel.refresh(node, state);
            }
        }
        self.refresh_scene();
        self.canvas().queue_draw();
    }
    fn restore_row(
        &self,
        mode: RowView,
        outgoing: Option<gtk::gsk::RenderNode>,
        direction: Option<f32>,
    ) {
        let visible = mode == RowView::Nodes;
        let was_visible = self.graph_visible.replace(visible);
        self.transition_serial.set(self.transition_serial.get() + 1);
        let serial = self.transition_serial.get();
        self.engine.set_presentation(Presentation {
            graph_visible: visible,
            lookup_return: false,
            lookup_node: None,
            ..Default::default()
        });
        self.canvas().slide(None);
        self.update_scene_geometry();
        {
            let mut view = self.view.borrow_mut();
            view.presentations.clear();
            view.border_opacities.clear();
            view.border_fades.clear();
            view.window_borders = !visible;
            if !self.native_nodes || !visible {
                view.presented_positions.clear();
            }
            view.morphing = false;
            view.chrome = if visible { 1.0 } else { 0.0 };
            view.live_fade = 0.0;
        }
        self.content().set_opacity(1.0);
        self.window().set_opacity(1.0);
        self.bar.set_visible(false);
        if !self.windowed && !self.native_nodes {
            self.window().set_keyboard_mode(if visible {
                KeyboardMode::Exclusive
            } else {
                KeyboardMode::None
            });
        }
        if self.native_nodes {
            if visible
                && !self
                    .row_surfaces
                    .borrow()
                    .contains_key(&self.engine.snapshot().state.selected_workspace)
            {
                self.graph_visible.set(false);
                self.engine.set_presentation(Presentation {
                    graph_visible: false,
                    lookup_return: false,
                    lookup_node: None,
                    ..Default::default()
                });
                self.fallback_window.set_visible(false);
            } else {
                self.finish_row(visible);
            }
            return;
        }
        let duration = self.view.borrow().theme.motion_ms;
        let animate =
            direction.is_some() && duration > 0 && (visible || was_visible || outgoing.is_some());
        if !animate {
            self.finish_row(visible);
            return;
        }
        self.view.borrow_mut().morphing = true;
        self.view.borrow_mut().live_fade = 1.0;
        self.canvas().slide(Some(super::surface::RowSlide {
            outgoing,
            incoming: visible,
            direction: direction.unwrap(),
            progress: 0.0,
        }));
        self.window().present();
        self.input_region();
        let started = std::time::Instant::now();
        let weak = self.weak.clone();
        self.window().add_tick_callback(move |_, _| {
            let Some(shell) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if shell.transition_serial.get() != serial {
                return glib::ControlFlow::Break;
            }
            let t = (started.elapsed().as_secs_f32() * 1000.0 / duration as f32).min(1.0);
            shell.canvas().slide_progress(t * t * (3.0 - 2.0 * t));
            if t >= 1.0 {
                let weak = shell.weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(shell) = weak.upgrade()
                        && shell.transition_serial.get() == serial
                    {
                        shell.finish_row(visible);
                    }
                });
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn finish_row(&self, visible: bool) {
        self.canvas().slide(None);
        self.view.borrow_mut().morphing = false;
        self.view.borrow_mut().live_fade = 0.0;
        self.window().set_visible(visible);
        self.input_region();
        if visible {
            let selected = self.view.borrow().selected.clone();
            self.select(selected);
            self.canvas().grab_focus();
            if self.first_frame.get() {
                let weak = self.weak.clone();
                self.canvas().add_tick_callback(move |canvas, _| {
                    let Some(shell) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    if !shell.first_frame.get() {
                        return glib::ControlFlow::Break;
                    }
                    if canvas.width() <= 160 || canvas.height() <= 160 || !shell.fit_immediate() {
                        return glib::ControlFlow::Continue;
                    }
                    shell.first_frame.set(false);
                    glib::ControlFlow::Break
                });
            }
            self.collect_images();
            self.request_captures(false);
        } else if !self.integrated && !self.windowed {
            self.bar.present();
        }
    }
    fn refresh_scene(&self) {
        if self.graph_visible.get() && !self.view.borrow().morphing {
            self.update_scene_geometry();
            self.input_region();
            let state = self.engine.snapshot().state;
            let resized = {
                let view = self.view.borrow();
                self.native_nodes
                    && state.nodes.iter().any(|n| {
                        n.workspace == state.selected_workspace
                            && n.column_mode == ColumnMode::Window
                            && n.window_id.is_some()
                            && view
                                .presented_rects
                                .get(&n.id)
                                .zip(view.desktop_rects.get(&n.id))
                                .is_some_and(|(shown, actual)| {
                                    (shown.width - actual.width).abs() > 0.5
                                        || (shown.height - actual.height).abs() > 0.5
                                })
                    })
            };
            let changed = resized
                || state
                    .nodes
                    .iter()
                    .filter(|n| n.workspace == state.selected_workspace)
                    .any(|n| {
                        let expected =
                            if n.column_mode == ColumnMode::Window && n.window_id.is_some() {
                                0.0
                            } else {
                                1.0
                            };
                        self.view
                            .borrow()
                            .presentations
                            .get(&n.id)
                            .is_some_and(|p| (*p - expected).abs() > 0.001)
                    });
            let (width, height) = self.scene_size();
            if changed
                || self
                    .view
                    .borrow()
                    .arrangement
                    .changed(&state, width, height)
            {
                self.transition(true, None);
            }
        }
    }
    fn show_error(&self, text: &str) {
        eprintln!("FlowWM: {text}");
        // Native row canvases do not contain the standalone editor's error banner.
        if self.native_nodes {
            if let Some((_, message)) = self.error_dialog.borrow().as_ref() {
                message.set_text(text);
                return;
            }
            let dialog = self.dialog("FlowWM error", 480, -1);
            dialog.set_namespace(Some("flowwm-error"));
            let root = column(14);
            root.add_css_class("dialog-body");
            root.append(&label("Could not complete the action", "dialog-title"));
            let message = label(text, "");
            message.set_wrap(true);
            message.set_max_width_chars(55);
            root.append(&message);
            let close = gtk::Button::with_label("Close");
            close.add_css_class("accent");
            close.set_halign(gtk::Align::End);
            let d = dialog.clone();
            close.connect_clicked(move |_| d.close());
            root.append(&close);
            dialog.set_child(Some(&root));
            dialog.set_default_widget(Some(&close));
            let weak = self.weak.clone();
            dialog.connect_close_request(move |_| {
                if let Some(shell) = weak.upgrade() {
                    shell.error_dialog.borrow_mut().take();
                }
                glib::Propagation::Proceed
            });
            *self.error_dialog.borrow_mut() = Some((dialog.clone(), message));
            dialog.present();
            return;
        }
        self.error.set_text(text);
        self.error.set_visible(true);
    }
    fn open(&self, id: &str) {
        let state = self.engine.snapshot().state;
        let Some(node) = state.node(id) else {
            return;
        };
        if node.kind == NodeKind::Constant && node.flow.editable_in_graph {
            self.edit_value(id);
            return;
        }
        if flow_builtins::agent::is_session(&node.spec().type_id) && node.window_id.is_none() {
            if let Err(error) = self.engine.open_window(id) {
                self.show_error(&format!("Open agent terminal: {error:#}"));
            }
            return;
        }
        if node.kind == NodeKind::Window || node.window_id.is_some() {
            let desktop = self.desktop.lock().unwrap().clone();
            let row = state.workspace().row.as_ref();
            let window = if let Some(id) = node.window_id {
                desktop.windows.iter().find(|w| w.id == id)
            } else {
                node.window_match.as_ref().and_then(|selector| {
                    desktop.windows.iter().find(|w| {
                        (!self.integrated || row.is_some_and(|r| Some(r.id) == w.workspace_id))
                            && crate::groups::selector(w) == *selector
                    })
                })
            };
            if let Some(window) = window {
                if self.native_nodes
                    && row.is_some_and(|r| Some(r.id) != window.workspace_id)
                    && let Some(destination) = state.workspaces.iter().find(|w| {
                        w.row.as_ref().is_some_and(|r| {
                            Some(r.id) == window.workspace_id && r.session == desktop.session
                        })
                    })
                {
                    if let Err(error) = self.engine.request(Action::RowView {
                        workspace: destination.id.clone(),
                        view: RowView::Windows,
                    }) {
                        self.show_error(&error.to_string());
                        return;
                    }
                    self.control(UiCommand::Workspace {
                        id: destination.id.clone(),
                    });
                    gtk::prelude::WidgetExt::display(&self.window()).sync();
                    if let Err(error) = niri::focus(window.id) {
                        self.show_error(&error.to_string());
                    }
                    return;
                }
                let native_peek = self.native_nodes && state.workspace().view == RowView::Nodes;
                if !native_peek && let Err(error) = niri::focus(window.id) {
                    self.show_error(&error.to_string());
                    return;
                }
                self.return_window.set(Some(window.id));
                {
                    let mut desktop = self.desktop.lock().unwrap();
                    for w in &mut desktop.windows {
                        w.is_focused = w.id == window.id;
                    }
                    if let Some(row) = desktop
                        .workspaces
                        .iter_mut()
                        .find(|row| Some(row.id) == window.workspace_id)
                    {
                        row.active_window_id = Some(window.id);
                    }
                }
                if let Err(error) = self.engine.request(Action::ColumnMode {
                    id: id.into(),
                    mode: ColumnMode::Window,
                }) {
                    self.show_error(&error.to_string());
                    return;
                }
                if state.workspace().view == RowView::Nodes {
                    let window_id = window.id;
                    let row_id = row.map(|r| r.id);
                    let weak = self.weak.clone();
                    self.transition(
                        true,
                        native_peek.then(|| {
                            Box::new(move || {
                                if let Some(shell) = weak.upgrade()
                                    && {
                                        let desktop = shell.desktop.lock().unwrap();
                                        desktop
                                            .workspaces
                                            .iter()
                                            .any(|r| Some(r.id) == row_id && r.is_focused)
                                            && desktop.windows.iter().any(|w| {
                                                w.id == window_id && w.workspace_id == row_id
                                            })
                                    }
                                {
                                    let _ = niri::focus(window_id);
                                }
                            }) as Box<dyn FnOnce()>
                        }),
                    );
                } else {
                    self.hide_graph(false);
                }
                return;
            }
            if node.flow.properties.contains_key("application") {
                self.engine.dispatch(Action::Run {
                    id: id.into(),
                    downstream: false,
                });
            } else {
                self.edit(id);
            }
            return;
        }
        let interface = window_interface(&state, node);
        let Some(interface) = interface else {
            self.edit(id);
            return;
        };
        if !self.windowed
            && let Err(error) = if self.integrated {
                self.activate_group(&node.workspace)
            } else {
                niri::activate_workspace(&node.workspace, true)
            }
        {
            self.show_error(&format!("Open workspace: {error:#}"));
            return;
        }
        if let Err(error) = self.engine.request(Action::RowView {
            workspace: node.workspace.clone(),
            view: RowView::Windows,
        }) {
            self.show_error(&error.to_string());
            return;
        }
        if let Some(window) = self.windows.borrow().get(id) {
            self.hide_graph(false);
            window.window.present();
            return;
        }
        let panel = ExtensionPanel::new(self, node, interface);
        let window = gtk::ApplicationWindow::builder()
            .application(&self.app)
            .title(&node.title)
            .default_width(560)
            .default_height(-1)
            .build();
        super::native::interface(&window, &node.id);
        window.add_css_class("node-window");
        window.set_child(Some(&panel.root));
        let w = self.weak.clone();
        let id_copy = id.to_string();
        window.connect_close_request(move |_| {
            if let Some(s) = w.upgrade() {
                s.windows.borrow_mut().remove(&id_copy);
            }
            glib::Propagation::Proceed
        });
        self.windows.borrow_mut().insert(
            id.into(),
            NodeWindow {
                window: window.clone(),
                panel,
            },
        );
        self.hide_graph(false);
        window.present();
    }
    fn switch_workspace(&self, offset: isize) {
        let state = self.engine.snapshot().state;
        let current = state
            .workspaces
            .iter()
            .position(|w| w.id == state.selected_workspace)
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(state.workspaces.len() as isize) as usize;
        self.control(UiCommand::Workspace {
            id: state.workspaces[next].id.clone(),
        });
    }
    fn search_nodes(&self) {
        self.fit();
        if self.integrated {
            self.dms_launcher(Some("nodes"));
            return;
        }
        if let Some(popup) = self.search_popup.borrow().as_ref() {
            popup.present();
            return;
        }
        let popup = self.dialog("Find a node", 480, 110);
        popup.add_css_class("flow-search");
        let root = column(8);
        root.add_css_class("chat-body");
        let search = gtk::SearchEntry::new();
        search.set_search_delay(0);
        search.set_placeholder_text(Some("Find a node…"));
        search.set_text(&self.view.borrow().query);
        root.append(&search);
        let count = label("Matching nodes stay highlighted on the graph", "hint");
        root.append(&count);
        popup.set_child(Some(&root));
        let weak = self.weak.clone();
        let matches = count.clone();
        search.connect_search_changed(move |entry| {
            if let Some(shell) = weak.upgrade() {
                shell.view.borrow_mut().query = entry.text().to_string();
                let state = shell.engine.snapshot().state;
                let count = state
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.workspace == state.selected_workspace
                            && canvas::matches_query(n, &entry.text())
                    })
                    .count();
                matches.set_text(&format!(
                    "{count} matching nodes · Enter to focus · Esc to dismiss"
                ));
                shell.canvas().queue_draw();
            }
        });
        let weak = self.weak.clone();
        let p = popup.clone();
        search.connect_activate(move |entry| {
            if let Some(shell) = weak.upgrade() {
                let state = shell.engine.snapshot().state;
                if let Some(node) = canvas::ordered_nodes(&state)
                    .into_iter()
                    .find(|n| canvas::matches_query(n, &entry.text()))
                {
                    shell.select(Some(node.id.clone()));
                    canvas::focus_node(&shell.canvas(), &shell.view, &state, &node.id);
                    p.close();
                }
            }
        });
        let weak = self.weak.clone();
        popup.connect_close_request(move |_| {
            if let Some(shell) = weak.upgrade() {
                shell.search_popup.borrow_mut().take();
                shell.view.borrow_mut().query.clear();
                shell.canvas().queue_draw();
                shell.canvas().grab_focus();
            }
            glib::Propagation::Proceed
        });
        *self.search_popup.borrow_mut() = Some(popup.clone());
        popup.present();
        search.grab_focus();
    }
    fn edit_value(&self, id: &str) {
        let state = self.engine.snapshot().state;
        let Some(node) = state
            .node(id)
            .filter(|n| n.kind == NodeKind::Constant && n.flow.editable_in_graph)
            .cloned()
        else {
            return;
        };
        self.canvas().dismiss_popovers();
        let popover = gtk::Popover::new();
        popover.set_parent(&self.canvas());
        popover.set_autohide(true);
        let view = self.view.borrow();
        let p = view.position(&node);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(
            (view.pan_x + p.x * view.zoom) as i32,
            (view.pan_y + (p.y + 75.) * view.zoom) as i32,
            (canvas::WIDTH * view.zoom) as i32,
            30,
        )));
        drop(view);
        let body = column(8);
        body.add_css_class("chat-body");
        let ty = crate::graph::constant_type(&node);
        let value = node
            .flow
            .properties
            .get("value")
            .cloned()
            .unwrap_or_else(|| super::parameters::default_value(&ty));
        let (widget, read) = super::parameters::value_editor(&state, &ty, &value);
        body.append(&widget);
        let save = gtk::Button::with_label("Save value");
        body.append(&save);
        popover.set_child(Some(&body));
        let weak = self.weak.clone();
        let p = popover.downgrade();
        save.connect_clicked(move |_| {
            let Some(p) = p.upgrade() else {
                return;
            };
            let Some(shell) = weak.upgrade() else { return };
            match read() {
                Ok(value) => {
                    let mut n = node.clone();
                    n.flow.properties.insert("value".into(), value);
                    match shell.engine.request(Action::Update { node: Box::new(n) }) {
                        Ok(_) => p.popdown(),
                        Err(e) => shell.show_error(&e.to_string()),
                    }
                }
                Err(e) => shell.show_error(&e.to_string()),
            }
        });
        let canvas = self.canvas().downgrade();
        popover.connect_closed(move |p| {
            p.unparent();
            if let Some(canvas) = canvas.upgrade() {
                canvas.grab_focus();
            }
        });
        popover.popup();
        widget.grab_focus();
    }
    fn agent_chat(&self) {
        if self.chat.borrow().is_some() {
            self.expel_chat();
            return;
        }
        self.show_chat(None);
    }
    fn expel_chat(&self) {
        let chat = self.chat.borrow_mut().take();
        if let Some(chat) = chat {
            let id = chat.id.borrow().clone();
            chat.window.close();
            if let Some(id) = id {
                self.select(Some(id.clone()));
                canvas::focus_node(
                    &self.canvas(),
                    &self.view,
                    &self.engine.snapshot().state,
                    &id,
                );
            }
        }
        self.canvas().grab_focus();
    }
    fn show_chat(&self, id: Option<String>) {
        let popup = self.dialog("Ask FlowGraph", 560, 150);
        popup.add_css_class("flow-chat");
        let root = column(10);
        root.add_css_class("chat-body");
        let heading = row(8);
        heading.append(&label("✳", "logo-small"));
        let status = label("FlowGraph", "chat-status");
        status.set_hexpand(true);
        heading.append(&status);
        let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings.add_css_class("flat");
        settings.set_tooltip_text(Some("Configure agent command"));
        heading.append(&settings);
        root.append(&heading);
        let transcript = output_view();
        transcript.set_monospace(false);
        transcript.add_css_class("chat-transcript");
        let scroll = gtk::ScrolledWindow::builder()
            .child(&transcript)
            .max_content_height(330)
            .propagate_natural_height(true)
            .build();
        scroll.set_visible(false);
        root.append(&scroll);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Ask your graph to do something…"));
        entry.add_css_class("chat-entry");
        root.append(&entry);
        root.append(&label(
            "Enter to send · Tab to put this session on the graph · Esc to dismiss",
            "hint",
        ));
        popup.set_child(Some(&root));
        let session = Rc::new(RefCell::new(id));
        let current = session.clone();
        let weak = self.weak.clone();
        entry.connect_activate(move |entry| {
            if let Some(shell) = weak.upgrade() {
                let text = entry.text().to_string();
                if text.trim().is_empty() {
                    return;
                }
                let state = shell.engine.snapshot().state;
                let existing = current.borrow().clone();
                let id = if let Some(id) = existing {
                    id
                } else {
                    let view = shell.view.borrow();
                    let center = view.world(
                        shell.canvas().width() as f64 / 2.0,
                        shell.canvas().height() as f64 / 2.0,
                    );
                    let selected = view.selected.as_deref().and_then(|id| state.node(id));
                    let mut position = selected
                        .map(|n| Position {
                            x: n.position.x,
                            y: n.position.y + 340.0,
                        })
                        .unwrap_or(Position {
                            x: center.0 - canvas::WIDTH / 2.0,
                            y: center.1,
                        });
                    while state.nodes.iter().any(|n| {
                        n.workspace == state.selected_workspace
                            && (n.position.x - position.x).abs() < 320.0
                            && (n.position.y - position.y).abs() < 220.0
                    }) {
                        position.x += 360.0;
                    }
                    let mut node = Node::new(
                        &state.selected_workspace,
                        NodeKind::Agent,
                        state.workspace().cwd.clone(),
                        position,
                    );
                    node.kind = NodeKind::Extension;
                    node.flow.node_type = flow_builtins::agent::TYPE_ID.into();
                    node.flow
                        .properties
                        .insert("writable".into(), serde_json::json!(true));
                    node.title = text.chars().take(48).collect();
                    node.writable = true;
                    let id = node.id.clone();
                    drop(view);
                    if let Err(error) = shell.engine.request(Action::Create {
                        node: Box::new(node),
                    }) {
                        shell.show_error(&error.to_string());
                        return;
                    }
                    *current.borrow_mut() = Some(id.clone());
                    id
                };
                match shell.engine.request(Action::Chat { id, text }) {
                    Ok(_) => entry.set_text(""),
                    Err(error) => shell.show_error(&error.to_string()),
                }
            }
        });
        let weak = self.weak.clone();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, key, _, _| {
            if matches!(key, gdk::Key::Tab | gdk::Key::ISO_Left_Tab) {
                if let Some(shell) = weak.upgrade() {
                    shell.expel_chat();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        popup.add_controller(key);
        let weak = self.weak.clone();
        popup.connect_close_request(move |_| {
            if let Some(shell) = weak.upgrade() {
                shell.chat.borrow_mut().take();
                shell.canvas().grab_focus();
            }
            glib::Propagation::Proceed
        });
        let weak = self.weak.clone();
        settings.connect_clicked(move |_| {
            if let Some(shell) = weak.upgrade() {
                shell.agent_settings();
            }
        });
        *self.chat.borrow_mut() = Some(ChatUi {
            window: popup.clone(),
            id: session,
            transcript,
            entry: entry.clone(),
            status,
            scroll,
        });
        self.refresh_chat(&self.engine.snapshot().state);
        popup.present();
        entry.grab_focus();
    }
    fn refresh_chat(&self, state: &State) {
        let chat = self.chat.borrow();
        let Some(chat) = chat.as_ref() else { return };
        let Some(id) = chat.id.borrow().clone() else {
            return;
        };
        let Some(node) = state.node(&id) else { return };
        let mut text = node
            .chat
            .iter()
            .map(|m| {
                format!(
                    "{}\n{}",
                    if m.role == "user" { "You" } else { "Agent" },
                    m.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if node.status.active() && !node.output.is_empty() {
            text.push_str(&format!("\n\nAgent\n{}", node.output));
        }
        if node.status == Status::Failed
            && let Some(error) = node.logs.iter().rev().find(|l| l.kind == "error")
        {
            text.push_str(&format!("\n\n{}", error.text));
        }
        set_buffer(&chat.transcript, &text);
        chat.scroll.set_visible(!text.is_empty());
        chat.status.set_text(if node.status.active() {
            "Working…"
        } else {
            "FlowGraph"
        });
        chat.entry.set_sensitive(!node.status.active());
        if !node.status.active() {
            chat.entry.grab_focus();
        }
    }
    fn agent_settings(&self) {
        let popup = self.dialog("FlowGraph agent", 540, 230);
        let root = column(12);
        root.add_css_class("dialog-body");
        root.append(&label("Agent command", "panel-title"));
        root.append(&label(
            "Executable and fixed arguments as a JSON array",
            "hint",
        ));
        let config = crate::agent::config()
            .map(|c| c.command)
            .unwrap_or_else(|_| vec!["codex".into()]);
        let command = entry(&serde_json::to_string(&config).unwrap());
        root.append(&command);
        let error = label("", "hint");
        root.append(&error);
        let save = gtk::Button::with_label("Save");
        save.add_css_class("accent");
        root.append(&save);
        let p = popup.clone();
        save.connect_clicked(move |_| {
            let result = (|| -> Result<()> {
                let values: Vec<String> = serde_json::from_str(&command.text())?;
                anyhow::ensure!(
                    !values.is_empty() && !values[0].is_empty(),
                    "Enter an executable"
                );
                let file = crate::config::directory().join("agent.json");
                std::fs::create_dir_all(file.parent().unwrap())?;
                std::fs::write(
                    file,
                    serde_json::to_vec_pretty(&serde_json::json!({"command":values}))?,
                )?;
                Ok(())
            })();
            match result {
                Ok(()) => p.close(),
                Err(e) => error.set_text(&e.to_string()),
            }
        });
        popup.set_child(Some(&root));
        popup.present();
    }
    fn palette(&self) {
        if self.integrated {
            self.dms_launcher(Some("create"));
            return;
        }
        let dialog = self.dialog("Create a node", 480, 560);
        let root = column(14);
        root.add_css_class("dialog-body");
        root.append(&label("What happens next?", "dialog-title"));
        root.append(&label(
            "A node is a unit of work. Connect it to build a workflow.",
            "muted",
        ));
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search nodes and macros…"));
        root.append(&search);
        let list = gtk::ListBox::new();
        list.add_css_class("macro-list");
        list.set_selection_mode(gtk::SelectionMode::Single);
        for (index, m) in self.macros.iter().enumerate() {
            let line = row(14);
            line.add_css_class("macro-row");
            let symbol = label(m.kind.symbol(), "macro-symbol");
            line.append(&symbol);
            let words = column(5);
            words.append(&label(&m.name, "node-title-small"));
            words.append(&label(&m.description, "muted"));
            line.append(&words);
            list.append(&line);
            if index == 0 {
                list.select_row(list.row_at_index(0).as_ref());
            }
        }
        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&list)
            .build();
        root.append(&scroll);
        root.append(&label("Enter  create     ·     Esc  cancel", "hint"));
        dialog.set_child(Some(&root));
        let l = list.clone();
        let macros = self.macros.clone();
        search.connect_search_changed(move |search| {
            let query = search.text().to_lowercase();
            for (i, m) in macros.iter().enumerate() {
                if let Some(row) = l.row_at_index(i as i32) {
                    row.set_visible(
                        format!("{} {}", m.name, m.description)
                            .to_lowercase()
                            .contains(&query),
                    );
                }
            }
            if let Some(row) = (0..macros.len())
                .filter_map(|i| l.row_at_index(i as i32))
                .find(|r| r.is_visible())
            {
                l.select_row(Some(&row));
            }
        });
        let w = self.weak.clone();
        let d = dialog.clone();
        list.connect_row_activated(move |_, row| {
            if let Some(s) = w.upgrade() {
                s.create_macro(row.index() as usize);
            }
            d.close();
        });
        let w = self.weak.clone();
        let l = list.clone();
        let d = dialog.clone();
        search.connect_activate(move |_| {
            if let Some(row) = l.selected_row()
                && let Some(s) = w.upgrade()
            {
                s.create_macro(row.index() as usize);
                d.close();
            }
        });
        dialog.present();
        search.grab_focus();
    }
    fn create_macro(&self, index: usize) {
        self.create_macro_with_application(index, None);
    }
    fn create_macro_with_application(
        &self,
        index: usize,
        application: Option<(String, String, String)>,
    ) {
        let Some(m) = self.macros.get(index) else {
            return;
        };
        let state = self.engine.snapshot().state;
        let view = self.view.borrow();
        let center = view.world(
            self.canvas().width() as f64 / 2.0,
            self.canvas().height() as f64 / 2.0,
        );
        let mut position =
            if let Some(selected) = view.selected.as_deref().and_then(|id| state.node(id)) {
                Position {
                    x: selected.position.x,
                    y: selected.position.y + 240.0,
                }
            } else {
                Position {
                    x: ((center.0 - canvas::WIDTH / 2.0) / 20.0).round() * 20.0,
                    y: ((center.1 - canvas::HEIGHT / 2.0) / 20.0).round() * 20.0,
                }
            };
        while state.nodes.iter().any(|n| {
            n.workspace == state.selected_workspace
                && (n.position.x - position.x).abs() < canvas::WIDTH + 20.0
                && (n.position.y - position.y).abs() < canvas::node_height(n) + 20.0
        }) {
            position.x += 360.0;
        }
        let mut node = Node::new(
            &state.selected_workspace,
            m.kind,
            state.workspace().cwd.clone(),
            position,
        );
        node.title = m.name.clone();
        node.command = m.command.clone();
        node.flow = m.flow.clone();
        if let Some((id, name, app_id)) = application {
            node.title = name;
            if !app_id.is_empty() {
                node.flow
                    .properties
                    .insert("application_id".into(), serde_json::json!(app_id));
            }
            node.flow
                .properties
                .insert("application".into(), serde_json::json!(id));
        }
        let id = node.id.clone();
        drop(view);
        match self.engine.request(Action::Create {
            node: Box::new(node),
        }) {
            Ok(_) => {
                self.select(Some(id.clone()));
                self.edit(&id);
            }
            Err(error) => self.show_error(&error.to_string()),
        }
    }
    fn edit(&self, id: &str) {
        let state = self.engine.snapshot().state;
        let Some(node) = state.node(id).cloned() else {
            return;
        };
        if node.status.active() && node.kind != NodeKind::Window && !node.desktop_window {
            self.show_error("Stop this node before changing its parameters.");
            return;
        }
        if let Some(command) = self
            .macros
            .iter()
            .find(|m| m.id == node.spec().type_id)
            .and_then(|m| m.settings.clone())
        {
            let dialog = self.dialog("Extension settings", 560, 620);
            super::extension_settings::open(dialog, self.engine.clone(), node, command);
            return;
        }
        let dialog = self.dialog("Node parameters", 560, -1);
        let root = column(14);
        root.add_css_class("dialog-body");
        let environment_node = matches!(
            node.flow.node_type.as_str(),
            "local.set_env" | "local.unset_env" | "local.empty_env" | "local.bake_args_into_env"
        );
        root.append(&label(
            &if environment_node {
                node.title.clone()
            } else {
                format!("{} parameters", node.kind.label())
            },
            "dialog-title",
        ));
        let title = entry(&node.title);
        title.set_activates_default(true);
        field(&root, "Name", &title);
        let directory = entry(&node.cwd.to_string_lossy());
        if (!node.kind.primitive() || matches!(node.kind, NodeKind::Vox | NodeKind::Python))
            && !environment_node
        {
            field(&root, "Working directory", &directory);
        }
        let fields = super::parameters::Fields::new(&state, &node);
        root.append(&fields.widget);
        let editor = gtk::TextView::new();
        editor.set_monospace(true);
        editor.set_wrap_mode(gtk::WrapMode::WordChar);
        editor.buffer().set_text(&node.command);
        editor.add_css_class("code-editor");
        let editor_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(160)
            .vexpand(true)
            .child(&editor)
            .build();
        let has_source = flow_builtins::agent::is_session(&node.spec().type_id)
            || matches!(
                node.kind,
                NodeKind::Agent
                    | NodeKind::Shell
                    | NodeKind::Metric
                    | NodeKind::Gate
                    | NodeKind::Human
                    | NodeKind::Vox
                    | NodeKind::Python
            );
        if has_source {
            field(
                &root,
                match node.kind {
                    NodeKind::Agent => "Prompt",
                    NodeKind::Shell | NodeKind::Metric => "Shell script",
                    NodeKind::Vox => "Vox source",
                    NodeKind::Python => "Python source",
                    _ => "Instructions",
                },
                &editor_scroll,
            );
        }
        if matches!(node.kind, NodeKind::Shell | NodeKind::Metric) {
            let info = label(
                "Typed inputs are in $FLOWWM_DATA_INPUTS. Use Bake Args into Env to expose signal arguments as variables.",
                "hint",
            );
            info.set_wrap(true);
            root.append(&info);
        }
        let writable =
            gtk::CheckButton::with_label("Allow the agent to edit files in this directory");
        writable.set_active(node.writable);
        if flow_builtins::agent::is_session(&node.spec().type_id) {
            root.append(&writable);
        }
        let interval = gtk::SpinButton::with_range(1.0, 86400.0, 1.0);
        interval.set_value(node.metric.interval_secs as f64);
        let unit = entry(&node.metric.unit);
        if node.kind == NodeKind::Metric {
            field(&root, "Sample every (seconds)", &interval);
            field(&root, "Unit", &unit);
            let watchers = gtk::Button::with_label("Create threshold source");
            let weak = self.weak.clone();
            let id = node.id.clone();
            watchers.connect_clicked(move |_| {
                if let Some(shell) = weak.upgrade() {
                    shell.watcher(&id);
                }
            });
            root.append(&watchers);
        }
        if let Some(application) = node
            .flow
            .properties
            .get("application")
            .and_then(serde_json::Value::as_str)
        {
            field(&root, "Application", &label(application, "muted"));
        } else if let Some(window) = self
            .desktop
            .lock()
            .unwrap()
            .windows
            .iter()
            .find(|w| Some(w.id) == node.window_id)
        {
            field(
                &root,
                "Application",
                &label(window.app_id.as_deref().unwrap_or("Application"), "muted"),
            );
        }
        if let Some(alert) = node.flow.properties.get("_alert") {
            let message = alert["message"].as_str().unwrap_or("Needs your attention");
            root.append(&label(message, "hint"));
            let clear = gtk::Button::with_label("Clear alert");
            let engine = self.engine.clone();
            let id = node.id.clone();
            clear.connect_clicked(move |button| {
                engine.dispatch(Action::ClearAlert { id: id.clone() });
                button.set_sensitive(false);
            });
            root.append(&clear);
        }
        let actions = row(10);
        actions.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        let save = gtk::Button::with_label("Save node");
        save.add_css_class("accent");
        dialog.set_default_widget(Some(&save));
        actions.append(&cancel);
        actions.append(&save);
        root.append(&actions);
        let d = dialog.clone();
        cancel.connect_clicked(move |_| d.close());
        let w = self.weak.clone();
        let d = dialog.clone();
        let original = node.clone();
        save.connect_clicked(move |_| {
            let Some(s) = w.upgrade() else {
                return;
            };
            let mut node = original.clone();
            if let Err(error) = fields.apply(&mut node) {
                s.show_error(&error.to_string());
                return;
            }
            node.title = title.text().to_string();
            node.cwd = PathBuf::from(directory.text().as_str());
            let b = editor.buffer();
            node.command = b.text(&b.start_iter(), &b.end_iter(), true).to_string();
            if flow_builtins::agent::is_session(&node.spec().type_id) {
                node.writable = writable.is_active();
                node.flow
                    .properties
                    .insert("writable".into(), serde_json::json!(node.writable));
            }
            if node.kind == NodeKind::Metric {
                node.metric.interval_secs = interval.value() as u64;
                node.metric.unit = unit.text().to_string();
                node.flow.properties.insert(
                    "interval_secs".into(),
                    serde_json::json!(node.metric.interval_secs),
                );
                node.flow
                    .properties
                    .insert("unit".into(), serde_json::json!(node.metric.unit));
            }
            match s.engine.request(Action::Update {
                node: Box::new(node),
            }) {
                Ok(_) => {
                    d.close();
                    s.rebuild_panel();
                }
                Err(error) => s.show_error(&error.to_string()),
            }
        });
        let scroll = gtk::ScrolledWindow::builder()
            .propagate_natural_height(true)
            .max_content_height(680)
            .child(&root)
            .build();
        dialog.set_child(Some(&scroll));
        dialog.present();
    }
    fn rebuild_panel(&self) {
        let selected = self.view.borrow().selected.clone();
        self.select(selected);
    }
    fn notes(&self) {
        let state = self.engine.snapshot().state;
        let dialog = self.dialog("Workspace notes", 700, 600);
        let root = column(18);
        root.add_css_class("dialog-body");
        root.append(&label("Your thinking space.", "dialog-title"));
        root.append(&label("The agents keep working while you do.", "muted"));
        let text = gtk::TextView::new();
        text.set_wrap_mode(gtk::WrapMode::WordChar);
        text.add_css_class("notes-editor");
        text.buffer().set_text(&state.workspace().notes);
        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&text)
            .build();
        root.append(&scroll);
        let save = gtk::Button::with_label("Save notes");
        save.add_css_class("accent");
        save.set_halign(gtk::Align::End);
        root.append(&save);
        let e = self.engine.clone();
        let d = dialog.clone();
        save.connect_clicked(move |_| {
            let b = text.buffer();
            e.dispatch(Action::Notes {
                text: b.text(&b.start_iter(), &b.end_iter(), true).to_string(),
            });
            d.close();
        });
        dialog.set_child(Some(&root));
        dialog.present();
    }
    fn orchestrate(
        &self,
        command: flow_core::protocol::Command,
        caller: Option<String>,
    ) -> Result<serde_json::Value> {
        self.engine.orchestrate(command, caller.map(Into::into))
    }

    fn row_settings(&self) {
        use flow_core::environment::Env;
        use flow_core::{domain::WorkflowId, protocol::Command};
        if let Some(dialog) = self
            .app
            .windows()
            .into_iter()
            .find(|w| w.has_css_class("row-settings-window"))
        {
            if let Some(popup) = dialog.child().and_downcast::<super::popup::Popup>() {
                popup.reopen();
            }
            dialog.present();
            return;
        }
        let state = self.engine.snapshot().state;
        let current = WorkflowId(state.selected_workspace.clone());
        let Some(runtime) = state.orchestration.as_ref() else {
            return;
        };
        let Some(workflow) = runtime.workflows.get(&current) else {
            return;
        };
        let value = match self.orchestrate(
            Command::RowSettings {
                workflow: current.clone(),
            },
            Some(current.0.clone()),
        ) {
            Ok(value) => value,
            Err(error) => {
                self.show_error(&error.to_string());
                return;
            }
        };
        let overrides: Env = serde_json::from_value(value["settings"].clone()).unwrap_or_default();
        let dialog = self.dialog("Row settings", 520, 560);
        dialog.add_css_class("row-settings-window");
        let root = column(12);
        root.add_css_class("row-settings-body");
        let heading = gtk::CenterBox::new();
        let icon = gtk::Image::from_icon_name("emblem-system-symbolic");
        icon.set_pixel_size(20);
        icon.set_margin_start(8);
        heading.set_start_widget(Some(&icon));
        let title = label("Row settings", "dialog-title");
        title.set_xalign(0.5);
        heading.set_center_widget(Some(&title));
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_valign(gtk::Align::Start);
        close.set_tooltip_text(Some("Close · Escape"));
        let d = dialog.clone();
        close.connect_clicked(move |_| d.close());
        heading.set_end_widget(Some(&close));
        root.append(&heading);
        let sections = column(2);
        let form = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&sections)
            .build();
        root.append(&form);
        let mut workspaces: Vec<_> = runtime
            .workspaces
            .definitions()
            .filter(|w| {
                runtime
                    .workspaces
                    .availability(&w.id)
                    .is_some_and(|a| a.active)
                    || workflow.workspace.as_ref() == Some(&w.id)
            })
            .map(|w| w.id.clone())
            .collect();
        workspaces.sort();
        let names: Vec<_> = workspaces
            .iter()
            .map(|w| {
                if runtime.workspaces.availability(w).is_some_and(|a| a.active) {
                    w.0.clone()
                } else {
                    format!("{} · unavailable", w.0)
                }
            })
            .collect();
        let workspace =
            gtk::DropDown::from_strings(&names.iter().map(String::as_str).collect::<Vec<_>>());
        if let Some(index) = workspaces
            .iter()
            .position(|w| Some(w) == workflow.workspace.as_ref())
        {
            workspace.set_selected(index as u32);
        }
        let section = column(8);
        section.add_css_class("row-settings-section");
        section.add_css_class("first");
        field(&section, "Owning workspace", &workspace);
        sections.append(&section);
        let pwd = entry(overrides.get("pwd").map(String::as_str).unwrap_or(""));
        pwd.set_placeholder_text(value["definitions"]["pwd"]["default"].as_str());
        pwd.set_hexpand(true);
        pwd.set_width_chars(1);
        let directory = row(6);
        directory.append(&pwd);
        let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
        reset.add_css_class("flat");
        reset.set_tooltip_text(Some("Use the default working directory"));
        let p = pwd.clone();
        reset.connect_clicked(move |_| p.set_text(""));
        directory.append(&reset);
        let section = column(8);
        section.add_css_class("row-settings-section");
        field(&section, "Working directory", &directory);
        sections.append(&section);
        let custom: Env = overrides
            .iter()
            .filter(|(k, _)| k.as_str() != "pwd")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let variables = super::environment::MapEditor::new(&custom, "Setting name", "Value", false);
        let section = column(8);
        section.add_css_class("row-settings-section");
        section.add_css_class("last");
        field(&section, "Row variables", &variables.widget);
        let hint = label(
            "New nodes receive FLOWWM_ROWSET_<NAME>. Running nodes keep their environment.",
            "row-settings-hint",
        );
        hint.set_wrap(true);
        hint.set_max_width_chars(50);
        section.append(&hint);
        sections.append(&section);
        let error = label("", "row-settings-error");
        error.set_wrap(true);
        error.set_visible(false);
        root.append(&error);
        let actions = row(8);
        actions.add_css_class("row-settings-actions");
        actions.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("flat");
        let d = dialog.clone();
        cancel.connect_clicked(move |_| d.close());
        actions.append(&cancel);
        let save = gtk::Button::with_label("Save");
        save.add_css_class("accent");
        actions.append(&save);
        root.append(&actions);
        let old_workspace = workflow.workspace.clone();
        let weak = self.weak.clone();
        let d = dialog.clone();
        save.connect_clicked(move |_| {
            let Some(shell) = weak.upgrade() else {
                return;
            };
            let result = (|| -> Result<()> {
                let mut settings = variables.read()?;
                anyhow::ensure!(
                    !settings.contains_key("pwd"),
                    "Use Working directory to set pwd"
                );
                if !pwd.text().is_empty() {
                    settings.insert("pwd".into(), pwd.text().to_string());
                }
                flow_core::environment::validate_row_settings(&settings)?;
                let destination = workspaces
                    .get(workspace.selected() as usize)
                    .context("Choose an owning workspace")?;
                if let Some(runtime) = shell.engine.snapshot().state.orchestration.as_ref() {
                    runtime.workspaces.require_active(destination)?;
                }
                shell.orchestrate(
                    Command::SetRowSettings {
                        workflow: current.clone(),
                        settings,
                    },
                    Some(current.0.clone()),
                )?;
                if old_workspace.as_ref() != Some(destination) {
                    shell.orchestrate(
                        Command::Move {
                            workflow: current.clone(),
                            workspace: destination.clone(),
                        },
                        Some(current.0.clone()),
                    )?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => d.close(),
                Err(e) => {
                    error.set_text(&e.to_string());
                    error.set_visible(true);
                }
            }
        });
        dialog.set_default_widget(Some(&save));
        {
            let view = self.view.borrow();
            super::popup::Popup::attach(&dialog, &root, &view.theme);
        }
        dialog.present();
        gtk::prelude::GtkWindowExt::set_focus(&dialog, None::<&gtk::Widget>);
    }

    fn workspace_dialog(&self) {
        use flow_core::{domain::*, protocol::Command};
        let value = match self.orchestrate(
            Command::Snapshot,
            Some(self.engine.snapshot().state.selected_workspace.clone()),
        ) {
            Ok(value) => value,
            Err(error) => {
                self.show_error(&error.to_string());
                return;
            }
        };
        let Ok(runtime) = serde_json::from_value::<RuntimeState>(value) else {
            return;
        };
        let state = self.engine.snapshot().state;
        let current = WorkflowId(state.selected_workspace.clone());
        let dialog = self.dialog("Workflows", 560, 520);
        let root = column(16);
        root.add_css_class("dialog-body");
        root.append(&label("Workflows", "dialog-title"));
        for workflow in runtime
            .workflows
            .values()
            .filter(|w| runtime.visible(&w.id, Some(&current)))
        {
            let item = row(8);
            let button = gtk::Button::with_label(&workflow.name);
            button.set_hexpand(true);
            button.set_halign(gtk::Align::Fill);
            button.add_css_class("workspace-choice");
            let weak = self.weak.clone();
            let id = workflow.id.0.clone();
            let d = dialog.clone();
            button.connect_clicked(move |_| {
                if let Some(shell) = weak.upgrade() {
                    shell.control(UiCommand::Workspace { id: id.clone() });
                    d.close();
                }
            });
            item.append(&button);
            let assignment = workflow
                .workspace
                .as_ref()
                .map(|w| w.0.as_str())
                .unwrap_or("Unassigned");
            item.append(&label(assignment, "muted"));
            if workflow.owner.as_ref() == Some(&current) {
                let public = gtk::Button::with_label("Make public");
                let weak = self.weak.clone();
                let child = workflow.id.clone();
                let parent = current.0.clone();
                let d = dialog.clone();
                public.connect_clicked(move |_| {
                    if let Some(shell) = weak.upgrade() {
                        match shell.orchestrate(
                            Command::MakePublic {
                                workflow: child.clone(),
                            },
                            Some(parent.clone()),
                        ) {
                            Ok(_) => d.close(),
                            Err(e) => shell.show_error(&e.to_string()),
                        }
                    }
                });
                item.append(&public);
            } else if workflow.id != current {
                let reference = gtk::Button::with_label("Reference");
                let weak = self.weak.clone();
                let target = workflow.id.clone();
                let parent = current.clone();
                let d = dialog.clone();
                reference.connect_clicked(move |_| {
                    if let Some(shell) = weak.upgrade() {
                        match shell.orchestrate(
                            Command::Reference {
                                workflow: parent.clone(),
                                target: target.clone(),
                            },
                            Some(parent.0.clone()),
                        ) {
                            Ok(_) => d.close(),
                            Err(e) => shell.show_error(&e.to_string()),
                        }
                    }
                });
                item.append(&reference);
            }
            root.append(&item);
        }
        let workspaces: Vec<_> = runtime
            .workspaces
            .definitions()
            .filter(|w| {
                runtime
                    .workspaces
                    .availability(&w.id)
                    .is_some_and(|a| a.active)
            })
            .map(|w| w.id.clone())
            .collect();
        let labels: Vec<_> = workspaces.iter().map(|w| w.0.as_str()).collect();
        let workspace = gtk::DropDown::from_strings(&labels);
        if let Some(assigned) = runtime
            .workflows
            .get(&current)
            .and_then(|w| w.workspace.as_ref())
            && let Some(index) = workspaces.iter().position(|w| w == assigned)
        {
            workspace.set_selected(index as u32);
        }
        let placement = row(8);
        workspace.set_hexpand(true);
        placement.append(&workspace);
        let move_button = gtk::Button::with_label("Move current");
        move_button.set_sensitive(!workspaces.is_empty());
        placement.append(&move_button);
        field(&root, "Workspace", &placement);
        let weak = self.weak.clone();
        let selected = workspace.clone();
        let destinations = workspaces.clone();
        let workflow = current.clone();
        let d = dialog.clone();
        move_button.connect_clicked(move |_| {
            if let (Some(shell), Some(destination)) = (
                weak.upgrade(),
                destinations.get(selected.selected() as usize),
            ) {
                match shell.orchestrate(
                    Command::Move {
                        workflow: workflow.clone(),
                        workspace: destination.clone(),
                    },
                    Some(workflow.0.clone()),
                ) {
                    Ok(_) => d.close(),
                    Err(e) => shell.show_error(&e.to_string()),
                }
            }
        });
        root.append(&separator());
        let name = entry("");
        name.set_placeholder_text(Some("Workflow name"));
        root.append(&name);
        let private = gtk::CheckButton::with_label("Own privately in the current workflow");
        root.append(&private);
        let create = gtk::Button::with_label("Create workflow");
        create.add_css_class("accent");
        create.set_halign(gtk::Align::End);
        create.set_sensitive(!workspaces.is_empty());
        root.append(&create);
        let weak = self.weak.clone();
        let d = dialog.clone();
        create.connect_clicked(move |_| {
            let Some(shell) = weak.upgrade() else {
                return;
            };
            let Some(destination) = workspaces.get(workspace.selected() as usize) else {
                return;
            };
            if name.text().trim().is_empty() {
                name.grab_focus();
                return;
            }
            let workflow = Workflow {
                row_settings: Default::default(),
                id: crate::model::id().into(),
                name: name.text().to_string(),
                workspace: Some(destination.clone()),
                visibility: if private.is_active() {
                    Visibility::Private
                } else {
                    Visibility::Public
                },
                owner: private.is_active().then(|| current.clone()),
                references: Default::default(),
                graph: Default::default(),
                authority: destination.host().into(),
                transfer: None,
            };
            match shell.orchestrate(Command::Create { workflow }, Some(current.0.clone())) {
                Ok(_) => d.close(),
                Err(e) => shell.show_error(&e.to_string()),
            }
        });
        dialog.set_child(Some(&gtk::ScrolledWindow::builder().child(&root).build()));
        dialog.present();
    }
    fn recipes(&self) {
        let state = self.engine.snapshot().state;
        let dialog = self.dialog("Recipes", 560, 500);
        let root = column(16);
        root.add_css_class("dialog-body");
        root.append(&label("Build once. Reuse the flow.", "dialog-title"));
        let intro = label(
            "Recipes capture this workspace’s graph. Instantiating creates fresh, idle nodes in the current directory.",
            "muted",
        );
        intro.set_wrap(true);
        root.append(&intro);
        for recipe in &state.recipes {
            let button = gtk::Button::with_label(&format!(
                "{}  ·  {} nodes   +",
                recipe.name,
                recipe.nodes.len()
            ));
            button.add_css_class("workspace-choice");
            let id = recipe.id.clone();
            let e = self.engine.clone();
            let d = dialog.clone();
            button.connect_clicked(move |_| {
                e.dispatch(Action::Instantiate { id: id.clone() });
                d.close();
            });
            root.append(&button);
        }
        if state.recipes.is_empty() {
            root.append(&label("No saved recipes yet.", "hint"));
        }
        root.append(&separator());
        let name = entry(&state.workspace().name);
        field(&root, "Save current graph as a recipe", &name);
        let save = gtk::Button::with_label("Save recipe");
        save.add_css_class("accent");
        root.append(&save);
        let ids: Vec<_> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .map(|n| n.id.clone())
            .collect();
        let e = self.engine.clone();
        let d = dialog.clone();
        let w = self.weak.clone();
        save.connect_clicked(move |_| {
            match e.request(Action::SaveRecipe {
                name: name.text().into(),
                nodes: ids.clone(),
            }) {
                Ok(_) => d.close(),
                Err(error) => {
                    if let Some(s) = w.upgrade() {
                        s.show_error(&error.to_string());
                    }
                }
            }
        });
        root.append(&label(
            "Share with: flowwm export <recipe-id> recipe.json",
            "hint",
        ));
        dialog.set_child(Some(&gtk::ScrolledWindow::builder().child(&root).build()));
        dialog.present();
    }
    fn watcher(&self, id: &str) {
        let state = self.engine.snapshot().state;
        let Some(source) = state.node(id) else {
            return;
        };
        let dialog = self.dialog("Metric watcher", 520, 460);
        let root = column(16);
        root.add_css_class("dialog-body");
        root.append(&label("Turn a signal into work.", "dialog-title"));
        root.append(&label(&source.title, "muted"));
        let threshold = gtk::SpinButton::with_range(-1_000_000.0, 1_000_000.0, 1.0);
        threshold.set_value(source.metric.samples.last().map(|s| s.value).unwrap_or(0.0));
        let operator = Choice::new();
        operator.append(Some("gt"), "Rises above");
        operator.append(Some("lt"), "Falls below");
        operator.set_active(Some(0));
        field(&root, "Trigger when the value", &operator.widget);
        field(&root, "Threshold", &threshold);
        let target = Choice::new();
        for n in state.nodes.iter().filter(|n| {
            n.workspace == state.selected_workspace && n.id != id && n.kind != NodeKind::Metric
        }) {
            target.append(Some(&n.id), &n.title);
        }
        target.set_active(Some(0));
        field(&root, "Queue this node", &target.widget);
        let note = label(
            "Creates a polling source connected to the selected node. It emits once per threshold crossing and rearms after the value returns to the other side.",
            "hint",
        );
        note.set_wrap(true);
        root.append(&note);
        let create = gtk::Button::with_label("Create watcher node");
        create.add_css_class("accent");
        root.append(&create);
        let e = self.engine.clone();
        let source = id.to_string();
        let d = dialog.clone();
        create.connect_clicked(move |_| {
            if let Some(target) = target.active_id() {
                e.dispatch(Action::Watch {
                    source: source.clone(),
                    target: target.to_string(),
                    threshold: threshold.value(),
                    above: operator.active_id().as_deref() == Some("gt"),
                });
                d.close();
            }
        });
        dialog.set_child(Some(&gtk::ScrolledWindow::builder().child(&root).build()));
        dialog.present();
    }
    fn help(&self) {
        if let Some(window) = self
            .app
            .windows()
            .into_iter()
            .find(|w| w.title().as_deref() == Some("Key bindings"))
        {
            window.present();
            return;
        }
        let dialog = self.dialog("Key bindings", 800, 640);
        let (root, search) = super::keybindings::content(crate::config::shortcuts());
        dialog.set_child(Some(&root));
        dialog.present();
        search.grab_focus();
    }
    fn dialog(&self, title: &str, width: i32, height: i32) -> gtk::Window {
        let dialog = gtk::Window::builder()
            .application(&self.app)
            .title(title)
            .transient_for(&self.window())
            .modal(true)
            .default_width(width)
            .default_height(height)
            .build();
        dialog.add_css_class("flow-dialog");
        if !self.windowed {
            dialog.init_layer_shell();
            dialog.set_namespace(Some("flowwm-dialog"));
            // Share DMS's modal layer so a newly opened clipboard can cover the editor.
            dialog.set_layer(if self.native_nodes {
                Layer::Top
            } else {
                Layer::Overlay
            });
            dialog.set_exclusive_zone(-1);
            dialog.set_keyboard_mode(KeyboardMode::Exclusive);
        }
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let d = dialog.clone();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if d.has_css_class("row-settings-window")
                    && let Some(popup) = d.child().and_downcast::<super::popup::Popup>()
                {
                    popup.escape(&d);
                } else {
                    d.close();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(key);
        dialog
    }
    fn capture(&self, path: PathBuf) -> Result<()> {
        let window = self
            .app
            .active_window()
            .filter(|window| {
                !self.native_nodes
                    || !self.graph_visible.get()
                    || window.has_css_class("flow-dialog")
            })
            .filter(|window| window.is_visible() && window.is_mapped())
            .unwrap_or_else(|| self.window().clone().upcast());
        let widget = window
            .child()
            .and_downcast::<super::popup::Popup>()
            .and_then(|popup| popup.body())
            .unwrap_or_else(|| window.clone().upcast());
        let paintable = gtk::WidgetPaintable::new(Some(&widget));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(
            &snapshot,
            paintable.intrinsic_width() as f64,
            paintable.intrinsic_height() as f64,
        );
        let node = snapshot.to_node().context("Window has not rendered yet")?;
        let renderer = gtk::gsk::CairoRenderer::new();
        renderer
            .realize(None::<&gdk::Surface>)
            .context("Create snapshot renderer")?;
        let texture = renderer.render_texture(&node, None);
        let result = texture.save_to_png(path).map_err(anyhow::Error::from);
        renderer.unrealize();
        result
    }
}

struct ChatUi {
    window: gtk::Window,
    id: Rc<RefCell<Option<String>>>,
    transcript: gtk::TextView,
    entry: gtk::Entry,
    status: gtk::Label,
    scroll: gtk::ScrolledWindow,
}

fn window_interface<'a>(
    state: &'a State,
    node: &Node,
) -> Option<&'a flow_core::providers::WindowInterface> {
    state
        .node_types
        .iter()
        .find(|definition| definition.id == crate::graph::type_id(node))
        .and_then(|definition| definition.window.as_ref())
}

fn apply_interface_fields(
    engine: &DesktopClient,
    id: &str,
    fields: &[(String, gtk::Entry)],
) -> Result<()> {
    let state = engine.snapshot().state;
    let mut node = state.node(id).context("Node is unavailable")?.clone();
    let mut changed = false;
    for (property, entry) in fields.iter().filter(|(_, entry)| entry.is_editable()) {
        let text = entry.text();
        let value =
            if crate::graph::property(&state, &node, property).is_some_and(|v| v.is_string()) {
                serde_json::Value::String(text.to_string())
            } else {
                match serde_json::from_str(text.as_str()) {
                    Ok(value) => value,
                    Err(_) => {
                        entry.add_css_class("error");
                        bail!("Enter a valid value for {property}");
                    }
                }
            };
        entry.remove_css_class("error");
        if node.flow.properties.get(property) != Some(&value) {
            node.flow.properties.insert(property.clone(), value);
            changed = true;
        }
    }
    if changed {
        engine.request(Action::Update {
            node: Box::new(node),
        })?;
    }
    Ok(())
}

/// Only instantiated from an extension's explicit, validated window contract.
struct ExtensionPanel {
    root: gtk::Box,
    status: gtk::Label,
    fields: Vec<(String, gtk::Entry)>,
    output: Option<(String, gtk::TextView, gtk::Box)>,
}
impl ExtensionPanel {
    fn new(shell: &Shell, node: &Node, interface: &flow_core::providers::WindowInterface) -> Self {
        let root = column(16);
        root.add_css_class("extension-body");
        let heading = row(12);
        let titles = column(4);
        titles.set_hexpand(true);
        titles.append(&label(&interface.title, "panel-title"));
        let status = label(node.status.label(), "status");
        titles.append(&status);
        heading.append(&titles);
        let graph = gtk::Button::from_icon_name("view-grid-symbolic");
        graph.set_tooltip_text(Some("Return to graph"));
        graph.set_valign(gtk::Align::Start);
        let weak = shell.weak.clone();
        graph.connect_clicked(move |_| {
            if let Some(shell) = weak.upgrade() {
                shell.show_nodes();
            }
        });
        heading.append(&graph);
        root.append(&heading);
        if !interface.description.is_empty() {
            let description = label(&interface.description, "field-label");
            description.set_wrap(true);
            description.set_max_width_chars(64);
            root.append(&description);
        }
        let mut fields = vec![];
        for spec in &interface.fields {
            let control = entry("");
            control.set_editable(spec.editable);
            if spec.editable {
                let engine = shell.engine.clone();
                let id = node.id.clone();
                let property = spec.property.clone();
                control.connect_activate(move |entry| {
                    let _ =
                        apply_interface_fields(&engine, &id, &[(property.clone(), entry.clone())]);
                });
            }
            let group = column(6);
            field(&group, &spec.label, &control);
            root.append(&group);
            fields.push((spec.property.clone(), control));
        }
        let output = interface.output.as_ref().map(|property| {
            let group = column(6);
            group.append(&label("Result", "field-label"));
            let view = output_view();
            view.add_css_class("extension-output");
            group.append(
                &gtk::ScrolledWindow::builder()
                    .child(&view)
                    .min_content_height(64)
                    .max_content_height(240)
                    .propagate_natural_height(true)
                    .build(),
            );
            root.append(&group);
            (property.clone(), view, group)
        });
        if !interface.actions.is_empty() {
            let actions = row(8);
            actions.set_halign(gtk::Align::End);
            for (index, action) in interface.actions.iter().enumerate() {
                let button = gtk::Button::with_label(&action.label);
                button.add_css_class(if index == 0 { "accent" } else { "quiet" });
                let engine = shell.engine.clone();
                let id = node.id.clone();
                let operation = action.operation.clone();
                let fields = fields.clone();
                let status = status.clone();
                button.connect_clicked(move |_| {
                    if matches!(
                        operation,
                        flow_core::providers::InterfaceOperation::Activate
                    ) && let Err(error) = apply_interface_fields(&engine, &id, &fields)
                    {
                        status.set_text(&error.to_string());
                        return;
                    }
                    use flow_core::providers::InterfaceOperation;
                    engine.dispatch(match &operation {
                        InterfaceOperation::Activate => Action::Run {
                            id: id.clone(),
                            downstream: false,
                        },
                        InterfaceOperation::Interrupt => Action::Interrupt { id: id.clone() },
                        InterfaceOperation::Emit { pin, args } => Action::Emit {
                            source: crate::graph::Endpoint {
                                node: id.clone(),
                                pin: pin.clone(),
                            },
                            args: args.clone(),
                        },
                    });
                });
                actions.append(&button);
            }
            root.append(&actions);
        }
        let panel = Self {
            root,
            status,
            fields,
            output,
        };
        panel.refresh(node, &shell.engine.snapshot().state);
        panel
    }
    fn refresh(&self, node: &Node, state: &State) {
        self.status.set_text(node.status.label());
        let value = |property: &str| {
            node.flow
                .values
                .get(property)
                .cloned()
                .or_else(|| crate::graph::property(state, node, property))
        };
        for (property, entry) in &self.fields {
            if entry.has_focus() {
                continue;
            }
            let text = value(property)
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_default();
            if entry.text() != text {
                entry.set_text(&text);
            }
        }
        if let Some((property, view, group)) = &self.output {
            let result = value(property);
            group.set_visible(result.as_ref().is_some_and(|v| !v.is_null()));
            let text = result
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .unwrap_or_else(|| serde_json::to_string_pretty(&v).unwrap_or_default())
                })
                .unwrap_or_default();
            set_buffer(view, &text);
        }
    }
}
fn output_view() -> gtk::TextView {
    let text = gtk::TextView::new();
    text.set_editable(false);
    text.set_cursor_visible(false);
    text.set_monospace(true);
    text.set_wrap_mode(gtk::WrapMode::WordChar);
    text.add_css_class("output-view");
    text
}
fn set_buffer(view: &gtk::TextView, text: &str) {
    let buffer = view.buffer();
    if buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), true)
        .as_str()
        != text
    {
        buffer.set_text(text);
    }
}
fn row(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}
fn column(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}
fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    if !class.is_empty() {
        label.add_css_class(class);
    }
    label.set_xalign(0.0);
    label
}
fn separator() -> gtk::Separator {
    gtk::Separator::new(gtk::Orientation::Vertical)
}
fn entry(text: &str) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.set_text(text);
    entry
}
fn field(root: &gtk::Box, name: &str, widget: &impl IsA<gtk::Widget>) {
    root.append(&label(name, "field-label"));
    root.append(widget);
}
fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
fn on_click(button: &gtk::Button, shell: &Rc<Shell>, callback: impl Fn(&Shell) + 'static) {
    let weak = Rc::downgrade(shell);
    button.connect_clicked(move |_| {
        if let Some(shell) = weak.upgrade() {
            callback(&shell);
        }
    });
}

// DropDown keeps selection IDs separate from their visible labels.
struct Choice {
    widget: gtk::DropDown,
    model: gtk::StringList,
    ids: RefCell<Vec<String>>,
}
impl Choice {
    fn new() -> Self {
        let model = gtk::StringList::new(&[]);
        let widget = gtk::DropDown::new(Some(model.clone()), None::<gtk::Expression>);
        Self {
            widget,
            model,
            ids: RefCell::new(vec![]),
        }
    }
    fn append(&self, id: Option<&str>, label: &str) {
        self.ids.borrow_mut().push(id.unwrap_or(label).into());
        self.model.append(label);
    }
    fn set_active(&self, index: Option<u32>) {
        self.widget
            .set_selected(index.unwrap_or(gtk::INVALID_LIST_POSITION));
    }
    fn active_id(&self) -> Option<String> {
        self.ids
            .borrow()
            .get(self.widget.selected() as usize)
            .cloned()
    }
}
