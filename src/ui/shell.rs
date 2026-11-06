use super::canvas::{self, CanvasAction, View};
use super::{Options, theme::Theme};
use crate::{
    engine::{Control, Engine, Presentation, UiCommand},
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
    engine: Engine,
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
        let provider = gtk::CssProvider::new(); provider.load_from_string(&Theme::load().css());
        if let Some(display) = gdk::Display::default() { gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION); }
        if let Some(settings) = gtk::Settings::default() {
            if let Some(property) = settings.find_property("gtk-interface-color-scheme")
                && let Some(class) = glib::EnumClass::with_type(property.value_type())
                && let Some(value) = class.to_value_by_nick("dark") {
                settings.set_property_from_value("gtk-interface-color-scheme", &value);
            } else { settings.set_gtk_application_prefer_dark_theme(true); }
        }
        let stop = Arc::new(AtomicBool::new(false)); let desktop = niri::listen(stop.clone(), options.integrated.then(|| engine.clone()));
        let shell = Shell::build(app, engine.clone(), macros.clone(), options, desktop);
        gtk::prelude::WidgetExt::realize(&shell.window);
        if let Some(renderer) = shell.window.renderer() { eprintln!("FlowWM renderer: {}", renderer.type_().name()); }
        for path in Theme::paths() {
            if let Some(parent) = path.parent() {
                let directory = gio::File::for_path(parent);
                if let Ok(monitor) = directory.monitor_directory(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>) {
                    let w = shell.weak.clone(); let provider = provider.clone();
                    monitor.connect_changed(move |_, file, _, _| {
                        if file.basename().is_some_and(|name| name.as_os_str() == "dms-colors.json" || name.as_os_str() == "settings.json") && let Some(shell) = w.upgrade() {
                            let theme = Theme::load(); provider.load_from_string(&theme.css()); shell.view.borrow_mut().theme = theme; shell.apply_insets(); shell.canvas.queue_draw();
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
        std::thread::spawn(move || { if signals.forever().next().is_some() { signal_engine.dispatch(Control::Ui { command: UiCommand::Quit }); } });
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
    panel: NodePanel,
}
struct Shell {
    app: gtk::Application,
    window: gtk::ApplicationWindow,
    bar: gtk::ApplicationWindow,
    engine: Engine,
    macros: Vec<Macro>,
    desktop: Arc<Mutex<Desktop>>,
    canvas: gtk::DrawingArea,
    view: Rc<RefCell<View>>,
    inspector: gtk::Box,
    panel: RefCell<Option<(String, NodePanel)>>,
    workspace_label: gtk::Label,
    workspace_path: gtk::Label,
    footer: gtk::Label,
    connection: gtk::Label,
    bar_status: gtk::Label,
    error: gtk::Label,
    windows: RefCell<HashMap<String, NodeWindow>>,
    revision: Cell<u64>,
    weak: Weak<Shell>,
    last_workspace: RefCell<String>,
    windowed: bool,
    theme_monitors: RefCell<Vec<gio::FileMonitor>>,
    integrated: bool,
    graph_visible: Cell<bool>,
    transition_serial: Cell<u64>,
    first_frame: Cell<bool>,
    pending_focus: RefCell<Option<String>>,
    return_window: Cell<Option<u64>>,
    bar_geometry: RefCell<HashMap<String, [i32; 4]>>,
    graph_content: gtk::Box,
    collapse_layer: gtk::Fixed,
    ghosts: RefCell<Vec<super::collapse::Ghost>>,
    reveal: Cell<f64>,
    paint_started: Cell<Option<std::time::Instant>>,
    frame_monitor: Cell<bool>,
    pinned_rows: RefCell<HashSet<u64>>,
}
impl Shell {
    fn build(
        app: &gtk::Application,
        engine: Engine,
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
        if !windowed {
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
        let collapse_layer = gtk::Fixed::new();
        collapse_layer.set_can_target(false);
        collapse_layer.set_overflow(gtk::Overflow::Hidden);
        collapse_layer.set_hexpand(true);
        collapse_layer.set_vexpand(true);
        surface.add_overlay(&collapse_layer);
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
        fit_button.set_tooltip_text(Some("Frame all nodes · H"));
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
        let help_button = gtk::Button::with_label("?  Shortcuts");
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
                window: window.clone(),
                bar: bar.clone(),
                engine: engine.clone(),
                macros,
                desktop,
                canvas,
                view,
                inspector,
                panel: RefCell::new(None),
                workspace_label,
                workspace_path,
                footer,
                connection,
                bar_status,
                error,
                windows: RefCell::new(HashMap::new()),
                revision: Cell::new(u64::MAX),
                weak: weak.clone(),
                last_workspace: RefCell::new(String::new()),
                windowed,
                theme_monitors: RefCell::new(vec![]),
                integrated: options.integrated,
                graph_visible: Cell::new(false),
                transition_serial: Cell::new(0),
                first_frame: Cell::new(true),
                pending_focus: RefCell::new(None),
                return_window: Cell::new(None),
                bar_geometry: RefCell::new(HashMap::new()),
                graph_content: root.clone(),
                collapse_layer: collapse_layer.clone(),
                ghosts: RefCell::new(vec![]),
                reveal: Cell::new(1.0),
                paint_started: Cell::new(None),
                frame_monitor: Cell::new(false),
                pinned_rows: RefCell::new(HashSet::new()),
            }
        });
        on_click(&workspace_button, &shell, |s| s.workspace_dialog());
        on_click(&add_button, &shell, |s| s.palette());
        on_click(&windows_button, &shell, |s| s.show_windows());
        on_click(&show_nodes, &shell, |s| s.show_nodes());
        on_click(&nodes_button, &shell, |s| s.fit());
        on_click(&fit_button, &shell, |s| s.fit());
        on_click(&run_button, &shell, |s| {
            s.engine.dispatch(Control::RunWorkspace)
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
        let w = shell.weak.clone();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::p {
                if let Some(s) = w.upgrade() {
                    s.dms_launcher(Some("nodes"));
                }
                return glib::Propagation::Stop;
            }
            if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::Tab {
                if let Some(s) = w.upgrade() {
                    s.show_windows();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);
        shell.apply_insets();
        shell.refresh();
        shell
    }
    fn canvas_action(&self, action: CanvasAction) {
        match action {
            CanvasAction::Select(id) => self.select(id),
            CanvasAction::Open(id) => self.open(&id),
            CanvasAction::Add => self.palette(),
            CanvasAction::Edit(id) => self.edit(&id),
            CanvasAction::Delete(id) => self.engine.dispatch(Control::Delete { id }),
            CanvasAction::Run(id) => self.engine.dispatch(Control::Run {
                id,
                downstream: false,
            }),
            CanvasAction::Windows => self.show_windows(),
        }
    }
    fn select(&self, id: Option<String>) {
        self.view.borrow_mut().selected = id.clone();
        if self.panel.borrow().as_ref().map(|(id, _)| id.clone()) == id {
            return;
        }
        clear(&self.inspector);
        if let Some(id) = id
            && let Some(node) = self.engine.snapshot().state.node(&id)
        {
            let panel = NodePanel::new(self, node, false);
            self.inspector.append(&panel.root);
            *self.panel.borrow_mut() = Some((id, panel));
            self.inspector.set_visible(true);
        } else {
            *self.panel.borrow_mut() = None;
            self.inspector.set_visible(false);
        }
        self.canvas.queue_draw();
    }
    fn fit_immediate(&self) {
        canvas::fit_immediate(&self.canvas, &self.view, &self.engine.snapshot().state);
    }
    fn fit(&self) {
        canvas::fit_animated(&self.canvas, &self.view, &self.engine.snapshot().state);
    }
    fn apply_insets(&self) {
        if self.windowed || !self.integrated {
            return;
        }
        let margins = self.current_insets();
        for (edge, margin) in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left]
            .into_iter()
            .zip(margins)
        {
            self.window.set_margin(edge, margin);
        }
        self.window.add_css_class("integrated");
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
    fn transition(&self, visible: bool, after: Option<Box<dyn FnOnce()>>) {
        let started = std::time::Instant::now();
        self.transition_serial.set(self.transition_serial.get() + 1);
        let serial = self.transition_serial.get();
        self.graph_visible.set(visible);
        if !visible {
            canvas::suspend(&self.view);
        }
        let presentation = self.engine.presentation();
        self.engine.set_presentation(Presentation {
            graph_visible: visible,
            lookup_return: presentation.lookup_return,
        });
        if !self.windowed {
            self.window.set_keyboard_mode(if visible {
                KeyboardMode::Exclusive
            } else {
                KeyboardMode::None
            });
        }
        let target = if visible { 1.0 } else { 0.0 };
        if visible {
            self.bar.set_visible(false);
            if !self.window.is_visible() {
                self.reveal.set(0.0);
                self.graph_content.set_opacity(0.0);
                self.window.set_opacity(1.0);
                self.window.present();
            }
            self.canvas.grab_focus();
        }
        if !self.window.is_visible() || (self.reveal.get() - target).abs() < 0.001 {
            self.reveal.set(target);
            self.graph_content.set_opacity(target);
            if !visible {
                self.paint_started.set(None);
                self.window.set_visible(false);
            }
            if let Some(after) = after {
                after();
            }
            return;
        }
        if !self.frame_monitor.get()
            && let Some(clock) = self.window.frame_clock()
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
            self.frame_monitor.set(true);
        }
        for ghost in self.ghosts.borrow().iter() {
            ghost.widget.set_visible(true);
        }
        let from = self.reveal.get();
        let weak = self.weak.clone();
        let after = RefCell::new(after);
        let previous = Cell::new(started);
        self.window.add_tick_callback(move |_, _| {
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
            if visible && shell.first_frame.get() && shell.canvas.width() > 0 {
                shell.fit_immediate();
                shell.first_frame.set(false);
                gtk::prelude::GtkWindowExt::set_focus(&shell.window, Some(&shell.canvas));
                shell.canvas.grab_focus();
                if let Some(id) = shell.pending_focus.borrow_mut().take() {
                    canvas::focus_node(
                        &shell.canvas,
                        &shell.view,
                        &shell.engine.snapshot().state,
                        &id,
                    );
                }
            }
            let duration = shell.view.borrow().theme.motion_ms as f64;
            let progress = if duration == 0.0 {
                1.0
            } else {
                (started.elapsed().as_secs_f64() * 1000.0 / duration).clamp(0.0, 1.0)
            };
            let eased = 1.0 - (1.0 - progress).powi(3);
            let reveal = from + (target - from) * eased;
            shell.reveal.set(reveal);
            shell.graph_content.set_opacity(reveal);
            super::collapse::update(
                &shell.collapse_layer,
                &shell.ghosts.borrow(),
                &shell.engine.snapshot().state,
                &shell.view.borrow(),
                reveal,
            );
            if progress >= 1.0 {
                shell.engine.performance.record(
                    if visible {
                        "transition_open"
                    } else {
                        "transition_close"
                    },
                    started.elapsed(),
                );
                for ghost in shell.ghosts.borrow().iter() {
                    ghost.widget.set_visible(false);
                }
                let weak = shell.weak.clone();
                let after = after.borrow_mut().take();
                glib::idle_add_local_once(move || {
                    let Some(shell) = weak.upgrade() else {
                        return;
                    };
                    if shell.transition_serial.get() != serial {
                        return;
                    }
                    if !visible {
                        shell.window.set_visible(false);
                    }
                    if let Some(after) = after {
                        after();
                    }
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
        if self.integrated && !was_visible && !self.engine.presentation().lookup_return {
            let desktop = self.desktop.lock().unwrap().clone();
            let display = gtk::prelude::WidgetExt::display(&self.window);
            let output = desktop
                .workspaces
                .iter()
                .find(|w| w.is_focused)
                .and_then(|w| w.output.as_ref());
            let monitor = (0..display.monitors().n_items())
                .filter_map(|i| display.monitors().item(i)?.downcast::<gdk::Monitor>().ok())
                .find(|m| m.connector().as_deref() == output.map(String::as_str));
            let geometry = monitor
                .map(|m| m.geometry())
                .unwrap_or_else(|| gtk::gdk::Rectangle::new(0, 0, 1920, 1080));
            let insets = self.current_insets();
            *self.ghosts.borrow_mut() = super::collapse::create(
                &self.collapse_layer,
                &desktop,
                &self.engine.snapshot().state,
                insets,
                (geometry.width() - insets[1] - insets[3]) as f64,
                (geometry.height() - insets[0] - insets[2]) as f64,
            );
        }
        if !was_visible
            && let Some(window) = self
                .desktop
                .lock()
                .unwrap()
                .windows
                .iter()
                .find(|w| w.is_focused)
        {
            self.return_window.set(Some(window.id));
        }
        self.engine.set_presentation(Presentation {
            graph_visible: true,
            lookup_return: false,
        });
        self.refresh();
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
        if let Err(error) = self.engine.request(Control::SyncDesktop {
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
            self.engine.request(Control::BindRow {
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
        self.hide_graph(true);
    }
    fn hide_graph(&self, restore_focus: bool) {
        let was_visible = self.graph_visible.get();
        self.engine.set_presentation(Presentation {
            graph_visible: false,
            lookup_return: false,
        });
        if restore_focus
            && was_visible
            && let Some(id) = self.return_window.get()
            && self
                .desktop
                .lock()
                .unwrap()
                .windows
                .iter()
                .any(|w| w.id == id)
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
    fn dms_launcher(&self, mode: Option<&str>) {
        let query = mode.map(|mode| match mode {
            "create" => "@ + ",
            "recipes" => "@ # ",
            _ => "@ ",
        });
        let restore = self.graph_visible.get() || self.engine.presentation().lookup_return;
        self.engine.set_presentation(Presentation {
            graph_visible: false,
            lookup_return: restore,
        });
        self.transition(false, None);
        self.bar.set_visible(false);
        std::thread::spawn(move || {
            let mut command = std::process::Command::new("dms");
            command.args(["ipc", "call", "spotlight"]);
            if let Some(query) = query {
                command.args(["openQuery", query]);
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
            if let Err(error) = self.engine.request(Control::SelectWorkspace {
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
                            &shell.canvas,
                            &shell.view,
                            &shell.engine.snapshot().state,
                            &id,
                        );
                    }
                });
            }
        }
    }
    fn control(&self, command: UiCommand) {
        match command {
            UiCommand::Toggle => {
                if self.graph_visible.get() {
                    self.show_windows();
                } else {
                    self.show_nodes();
                }
            }
            UiCommand::Nodes => self.show_nodes(),
            UiCommand::Windows => self.show_windows(),
            UiCommand::Quit => self.app.quit(),
            UiCommand::Notes => {
                self.show_nodes();
                self.notes();
            }
            UiCommand::Recipes => {
                self.show_nodes();
                self.recipes();
            }
            UiCommand::RunGroup => self.engine.dispatch(Control::RunWorkspace),
            UiCommand::Help => {
                self.show_nodes();
                self.help();
            }
            UiCommand::BarGeometry { output, insets } => {
                self.bar_geometry
                    .borrow_mut()
                    .insert(output, insets.map(|n| n.clamp(0, 512)));
                self.apply_insets();
            }
            UiCommand::Open { id } => self.open(&id),
            UiCommand::Select { id } => self.select(Some(id)),
            UiCommand::Focus { id, expand } => self.focus_node(&id, expand),
            UiCommand::Search { mode } => self.dms_launcher(Some(&mode)),
            UiCommand::Launcher => self.dms_launcher(None),
            UiCommand::ResumeLookup => {
                if self.engine.presentation().lookup_return {
                    self.show_nodes();
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
                match self.engine.request(Control::SelectWorkspace { id }) {
                    Ok(_) => {
                        self.refresh();
                        self.show_nodes();
                        self.fit();
                    }
                    Err(error) => self.show_error(&error.to_string()),
                }
            }
            UiCommand::Recipe { id } => match self.engine.request(Control::Instantiate { id }) {
                Ok(_) => {
                    self.refresh();
                    self.show_nodes();
                    self.fit();
                }
                Err(error) => self.show_error(&error.to_string()),
            },
            UiCommand::Edit { id } => self.edit(&id),
            UiCommand::Palette => self.palette(),
            UiCommand::Fit => self.fit(),
            UiCommand::Dismiss => {
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
                    let id = canvas::navigate(
                        &self.canvas,
                        &self.view,
                        &self.engine.snapshot().state,
                        &direction,
                    );
                    self.select(id);
                    self.canvas.queue_draw();
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
        let edges: usize = visible.iter().map(|n| n.dependencies.len()).sum();
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
        let workspace_changed = *self.last_workspace.borrow() != state.selected_workspace;
        if workspace_changed {
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
            self.view.borrow_mut().workspace = state.selected_workspace.clone();
            self.select(None);
            self.fit();
        }
        let selected = self.view.borrow().selected.clone();
        if let Some(id) = selected {
            if let Some(node) = state.node(&id) {
                if let Some((_, panel)) = &*self.panel.borrow() {
                    panel.refresh(node, state);
                }
            } else {
                self.select(None);
            }
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
        self.canvas.queue_draw();
    }
    fn show_error(&self, text: &str) {
        self.error.set_text(text);
        self.error.set_visible(true);
    }
    fn open(&self, id: &str) {
        let state = self.engine.snapshot().state;
        let Some(node) = state.node(id) else {
            return;
        };
        if node.kind == NodeKind::Window {
            let desktop = self.desktop.lock().unwrap().clone();
            if let Some(window) = desktop.windows.iter().find(|w| {
                node.window_id == Some(w.id)
                    || node
                        .window_match
                        .as_ref()
                        .is_some_and(|selector| window_selector(w) == *selector)
            }) {
                self.hide_graph(false);
                if let Err(error) = niri::focus(window.id) {
                    self.show_nodes();
                    self.show_error(&error.to_string());
                }
                return;
            }
            self.edit(id);
            return;
        }
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
        if let Some(window) = self.windows.borrow().get(id) {
            self.hide_graph(false);
            window.window.present();
            return;
        }
        let panel = NodePanel::new(self, node, true);
        let window = gtk::ApplicationWindow::builder()
            .application(&self.app)
            .title(&node.title)
            .default_width(720)
            .default_height(850)
            .build();
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
        root.append(&label(
            "Tab  add node     ·     Enter  create     ·     Esc  cancel",
            "hint",
        ));
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
        let Some(m) = self.macros.get(index) else {
            return;
        };
        let state = self.engine.snapshot().state;
        let view = self.view.borrow();
        let center = view.world(
            self.canvas.width() as f64 / 2.0,
            self.canvas.height() as f64 / 2.0,
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
        if matches!(m.kind, NodeKind::Agent | NodeKind::Shell | NodeKind::Gate)
            && let Some(selected) = view.selected.as_ref()
        {
            node.dependencies.push(selected.clone());
        }
        let id = node.id.clone();
        drop(view);
        match self.engine.request(Control::Create {
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
        if node.status.active() {
            self.show_error("Stop this node before changing its parameters.");
            return;
        }
        let dialog = self.dialog("Node parameters", 640, 760);
        let root = column(14);
        root.add_css_class("dialog-body");
        root.append(&label(
            &format!("{} parameters", node.kind.label()),
            "dialog-title",
        ));
        let title = entry(&node.title);
        field(&root, "Name", &title);
        let directory = entry(&node.cwd.to_string_lossy());
        field(&root, "Working directory", &directory);
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
        field(
            &root,
            if node.kind == NodeKind::Agent {
                "Prompt"
            } else if matches!(node.kind, NodeKind::Shell | NodeKind::Metric) {
                "Shell script"
            } else {
                "Instructions"
            },
            &editor_scroll,
        );
        if matches!(node.kind, NodeKind::Shell | NodeKind::Metric) {
            let info = label(
                "Runs as your user. Upstream results are JSON in $FLOWWM_INPUTS.",
                "hint",
            );
            info.set_wrap(true);
            root.append(&info);
        }
        let writable = gtk::CheckButton::with_label("Allow Codex to edit files in this directory");
        writable.set_active(node.writable);
        if node.kind == NodeKind::Agent {
            root.append(&writable);
        }
        let review = gtk::CheckButton::with_label("Ask me to review the result before continuing");
        review.set_active(node.review);
        if matches!(node.kind, NodeKind::Agent | NodeKind::Shell) {
            root.append(&review);
        }
        let condition = Choice::new();
        condition.append(Some("success"), "All inputs succeed");
        condition.append(Some("failure"), "Any input fails · recovery branch");
        condition.set_active_id(Some(if node.run_when == RunWhen::Success {
            "success"
        } else {
            "failure"
        }));
        field(&root, "Run when", &condition.widget);
        let interval = gtk::SpinButton::with_range(1.0, 86400.0, 1.0);
        interval.set_value(node.metric.interval_secs as f64);
        let unit = entry(&node.metric.unit);
        if node.kind == NodeKind::Metric {
            field(&root, "Sample every (seconds)", &interval);
            field(&root, "Unit", &unit);
        }
        let window_choice = Choice::new();
        if node.kind == NodeKind::Window {
            for window in &self.desktop.lock().unwrap().windows {
                window_choice.append(
                    Some(&window_selector(window)),
                    &format!(
                        "{} · {}",
                        window.app_id.as_deref().unwrap_or("app"),
                        window.title.as_deref().unwrap_or("Untitled")
                    ),
                );
            }
            if let Some(selector) = &node.window_match {
                window_choice.set_active_id(Some(selector));
            }
            field(&root, "Native window", &window_choice.widget);
        }
        if !node.dependencies.is_empty() {
            root.append(&label("INPUT CONNECTIONS", "eyebrow"));
            for input in &node.dependencies {
                let line = row(8);
                let title = state
                    .node(input)
                    .map(|n| n.title.as_str())
                    .unwrap_or("Missing node");
                let name = label(title, "muted");
                name.set_hexpand(true);
                line.append(&name);
                let remove = gtk::Button::with_label("Disconnect");
                remove.add_css_class("flat");
                let e = self.engine.clone();
                let source = input.clone();
                let target = node.id.clone();
                let line_copy = line.clone();
                remove.connect_clicked(move |_| {
                    e.dispatch(Control::Disconnect {
                        source: source.clone(),
                        target: target.clone(),
                    });
                    line_copy.set_visible(false);
                });
                line.append(&remove);
                root.append(&line);
            }
        }
        let actions = row(10);
        actions.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        let save = gtk::Button::with_label("Save node");
        save.add_css_class("accent");
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
            node.title = title.text().to_string();
            node.cwd = PathBuf::from(directory.text().as_str());
            let b = editor.buffer();
            node.command = b.text(&b.start_iter(), &b.end_iter(), true).to_string();
            node.writable = writable.is_active();
            node.review = review.is_active();
            node.run_when = if condition.active_id().as_deref() == Some("failure") {
                RunWhen::Failure
            } else {
                RunWhen::Success
            };
            node.metric.interval_secs = interval.value() as u64;
            node.metric.unit = unit.text().to_string();
            node.window_match = window_choice.active_id().map(|s| s.to_string());
            // Respect connections disconnected while this dialog was open.
            if let Some(current) = s.engine.snapshot().state.node(&node.id) {
                node.dependencies = current.dependencies.clone();
            }
            match s.engine.request(Control::Update {
                node: Box::new(node),
            }) {
                Ok(_) => {
                    d.close();
                    s.rebuild_panel();
                }
                Err(error) => s.show_error(&error.to_string()),
            }
        });
        let scroll = gtk::ScrolledWindow::builder().child(&root).build();
        dialog.set_child(Some(&scroll));
        dialog.present();
    }
    fn rebuild_panel(&self) {
        let selected = self.view.borrow().selected.clone();
        *self.panel.borrow_mut() = None;
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
            e.dispatch(Control::Notes {
                text: b.text(&b.start_iter(), &b.end_iter(), true).to_string(),
            });
            d.close();
        });
        dialog.set_child(Some(&root));
        dialog.present();
    }
    fn workspace_dialog(&self) {
        let state = self.engine.snapshot().state;
        let dialog = self.dialog("Workspaces", 520, 460);
        let root = column(14);
        root.add_css_class("dialog-body");
        root.append(&label("Workspaces", "dialog-title"));
        for workspace in &state.workspaces {
            let button = gtk::Button::with_label(&format!(
                "{}  {}",
                if workspace.id == state.selected_workspace {
                    "●"
                } else {
                    "○"
                },
                workspace.name
            ));
            button.add_css_class("workspace-choice");
            let e = self.engine.clone();
            let id = workspace.id.clone();
            let d = dialog.clone();
            button.connect_clicked(move |_| {
                e.dispatch(Control::SelectWorkspace { id: id.clone() });
                d.close();
            });
            root.append(&button);
        }
        root.append(&separator());
        let name = entry("");
        name.set_placeholder_text(Some("New workspace name"));
        root.append(&name);
        let cwd = entry(&state.workspace().cwd.to_string_lossy());
        field(&root, "Working directory", &cwd);
        let create = gtk::Button::with_label("Create workspace");
        create.add_css_class("accent");
        root.append(&create);
        let e = self.engine.clone();
        let d = dialog.clone();
        let w = self.weak.clone();
        create.connect_clicked(move |_| {
            match e.request(Control::CreateWorkspace {
                name: name.text().to_string(),
                cwd: PathBuf::from(cwd.text().as_str()),
            }) {
                Ok(_) => d.close(),
                Err(error) => {
                    if let Some(s) = w.upgrade() {
                        s.show_error(&error.to_string());
                    }
                }
            }
        });
        let scroll = gtk::ScrolledWindow::builder().child(&root).build();
        dialog.set_child(Some(&scroll));
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
                e.dispatch(Control::Instantiate { id: id.clone() });
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
            match e.request(Control::SaveRecipe {
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
            "Triggers once per threshold crossing. It rearms after the metric returns to the other side. The target’s normal input conditions still apply.",
            "hint",
        );
        note.set_wrap(true);
        root.append(&note);
        let create = gtk::Button::with_label("Enable watcher");
        create.add_css_class("accent");
        root.append(&create);
        let e = self.engine.clone();
        let source = id.to_string();
        let d = dialog.clone();
        create.connect_clicked(move |_| {
            if let Some(target) = target.active_id() {
                e.dispatch(Control::Watch {
                    watcher: Watcher {
                        id: crate::model::id(),
                        source: source.clone(),
                        target: target.to_string(),
                        threshold: threshold.value(),
                        above: operator.active_id().as_deref() == Some("gt"),
                        enabled: true,
                        armed: true,
                        last_triggered: None,
                    },
                });
                d.close();
            }
        });
        for watcher in state.watchers.iter().filter(|w| w.source == id) {
            let row = row(10);
            row.append(&label(
                &format!(
                    "{} {} → {}",
                    if watcher.above { ">" } else { "<" },
                    watcher.threshold,
                    state
                        .node(&watcher.target)
                        .map(|n| n.title.as_str())
                        .unwrap_or("node")
                ),
                "muted",
            ));
            let remove = gtk::Button::with_label("Remove");
            let enabled = gtk::Switch::new();
            enabled.set_valign(gtk::Align::Center);
            enabled.set_active(watcher.enabled);
            let e = self.engine.clone();
            let template = watcher.clone();
            enabled.connect_active_notify(move |switch| {
                let mut watcher = template.clone();
                watcher.enabled = switch.is_active();
                watcher.armed = true;
                e.dispatch(Control::Watch { watcher });
            });
            row.append(&enabled);
            let e = self.engine.clone();
            let id = watcher.id.clone();
            let r = row.clone();
            remove.connect_clicked(move |_| {
                e.dispatch(Control::DeleteWatcher { id: id.clone() });
                r.set_visible(false);
            });
            row.append(&remove);
            root.append(&row);
        }
        dialog.set_child(Some(&gtk::ScrolledWindow::builder().child(&root).build()));
        dialog.present();
    }
    fn help(&self) {
        let dialog = self.dialog("FlowWM shortcuts", 560, 560);
        let root = column(18);
        root.add_css_class("dialog-body");
        root.append(&label("Stay in your flow.", "dialog-title"));
        for (key, action) in [
            ("Tab", "Create a node or use an extension macro"),
            ("Arrow keys / Super + arrows", "Navigate the graph"),
            ("Shift + arrows", "Move the selected node on the grid"),
            (
                "Enter / double click",
                "Expand a node into a native tiled window",
            ),
            ("Escape", "Return to Niri windows"),
            ("H / Home", "Frame the entire graph"),
            ("E / R / Delete", "Edit / run / delete the selected node"),
            ("Drag a bottom port", "Connect it to a downstream node"),
            ("Mouse wheel", "Zoom the graph"),
            ("Middle drag / background drag", "Pan the graph"),
        ] {
            let line = row(24);
            let k = label(key, "shortcut");
            k.set_width_chars(24);
            line.append(&k);
            let a = label(action, "muted");
            a.set_wrap(true);
            line.append(&a);
            root.append(&line);
        }
        dialog.set_child(Some(&root));
        dialog.present();
    }
    fn dialog(&self, title: &str, width: i32, height: i32) -> gtk::Window {
        let dialog = gtk::Window::builder()
            .application(&self.app)
            .title(title)
            .transient_for(&self.window)
            .modal(true)
            .default_width(width)
            .default_height(height)
            .build();
        dialog.add_css_class("flow-dialog");
        if !self.windowed {
            dialog.init_layer_shell();
            dialog.set_namespace(Some("flowwm-dialog"));
            dialog.set_layer(Layer::Overlay);
            dialog.set_exclusive_zone(-1);
            dialog.set_keyboard_mode(KeyboardMode::Exclusive);
        }
        let key = gtk::EventControllerKey::new();
        let d = dialog.clone();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                d.close();
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
            .unwrap_or_else(|| self.window.clone().upcast());
        let widget: gtk::Widget = window.clone().upcast();
        let paintable = gtk::WidgetPaintable::new(Some(&widget));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, widget.width() as f64, widget.height() as f64);
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

struct NodePanel {
    root: gtk::Box,
    status: gtk::Label,
    title: gtk::Label,
    directory: gtk::Label,
    command: gtk::Label,
    output: gtk::TextView,
    logs: gtk::TextView,
    actions: gtk::Box,
    engine: Engine,
    id: String,
    handoff: gtk::Entry,
    previous_status: Cell<Status>,
}
impl NodePanel {
    fn new(shell: &Shell, node: &Node, expanded: bool) -> Self {
        let root = column(0);
        root.add_css_class("node-panel");
        root.set_vexpand(true);
        let top = row(10);
        top.add_css_class("panel-heading");
        top.append(&label(
            if expanded {
                "NODE WINDOW"
            } else {
                "PARAMETERS"
            },
            "eyebrow",
        ));
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top.append(&spacer);
        if !expanded {
            let close = gtk::Button::from_icon_name("window-close-symbolic");
            close.add_css_class("flat");
            let w = shell.weak.clone();
            close.connect_clicked(move |_| {
                if let Some(s) = w.upgrade() {
                    s.select(None);
                }
            });
            top.append(&close);
        } else {
            let graph = gtk::Button::with_label("↖  Graph");
            graph.add_css_class("flat");
            let w = shell.weak.clone();
            graph.connect_clicked(move |_| {
                if let Some(s) = w.upgrade() {
                    s.show_nodes();
                }
            });
            top.append(&graph);
        }
        root.append(&top);
        let summary = column(12);
        summary.add_css_class("panel-summary");
        let title = label(&node.title, "panel-title");
        title.set_max_width_chars(if expanded { 64 } else { 27 });
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        summary.append(&title);
        let status = label(node.status.label(), "status");
        status.set_xalign(0.0);
        summary.append(&status);
        let directory = label(&node.cwd.to_string_lossy(), "path");
        directory.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        directory.set_width_chars(1);
        directory.set_xalign(0.0);
        summary.append(&directory);
        let command = label(&node.command, "command-preview");
        command.set_wrap(true);
        command.set_max_width_chars(if expanded { 100 } else { 34 });
        command.set_xalign(0.0);
        command.set_lines(5);
        command.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if !(expanded && node.kind == NodeKind::Human) {
            summary.append(&command);
        }
        let tools = row(8);
        let edit = gtk::Button::with_label("Edit parameters");
        edit.add_css_class("quiet");
        let w = shell.weak.clone();
        let id = node.id.clone();
        edit.connect_clicked(move |_| {
            if let Some(s) = w.upgrade() {
                s.edit(&id);
            }
        });
        if !node.desktop_window {
            tools.append(&edit);
        }
        if !expanded {
            let open = gtk::Button::from_icon_name("view-fullscreen-symbolic");
            open.add_css_class("quiet");
            open.set_tooltip_text(Some("Expand as a native window"));
            let w = shell.weak.clone();
            let id = node.id.clone();
            open.connect_clicked(move |_| {
                if let Some(s) = w.upgrade() {
                    s.open(&id);
                }
            });
            tools.append(&open);
        }
        summary.append(&tools);
        if node.kind == NodeKind::Metric {
            let watch = gtk::Button::with_label("⌁  Add or manage watcher");
            watch.add_css_class("quiet");
            let w = shell.weak.clone();
            let id = node.id.clone();
            watch.connect_clicked(move |_| {
                if let Some(s) = w.upgrade() {
                    s.watcher(&id);
                }
            });
            summary.append(&watch);
        }
        root.append(&summary);
        let actions = row(8);
        actions.add_css_class("node-actions");
        root.append(&actions);
        let handoff = entry("");
        handoff.set_placeholder_text(Some("Handoff note (optional)"));
        handoff.set_margin_start(20);
        handoff.set_margin_end(20);
        handoff.set_margin_bottom(14);
        handoff.set_visible(matches!(node.kind, NodeKind::Human | NodeKind::Gate));
        root.append(&handoff);
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        notebook.add_css_class("output-tabs");
        let output = output_view();
        if node.kind == NodeKind::Human {
            output.set_editable(true);
            output.set_cursor_visible(true);
            let engine = shell.engine.clone();
            let id = node.id.clone();
            output.buffer().connect_changed(move |buffer| {
                engine.dispatch(Control::HumanProgress {
                    id: id.clone(),
                    text: buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), true)
                        .to_string(),
                });
            });
        }
        let activity = output_view();
        notebook.append_page(
            &gtk::ScrolledWindow::builder()
                .child(&output)
                .vexpand(true)
                .build(),
            Some(&label("Output", "")),
        );
        notebook.append_page(
            &gtk::ScrolledWindow::builder()
                .child(&activity)
                .vexpand(true)
                .build(),
            Some(&label("Activity", "")),
        );
        root.append(&notebook);
        let footer = label(
            if node.kind == NodeKind::Human {
                "Your work is a parallel branch of the workflow."
            } else {
                "Closing this window keeps the work running."
            },
            "panel-hint",
        );
        footer.set_wrap(true);
        footer.set_max_width_chars(if expanded { 100 } else { 36 });
        root.append(&footer);
        let panel = Self {
            root,
            status,
            title,
            directory,
            command,
            output,
            logs: activity,
            actions,
            engine: shell.engine.clone(),
            id: node.id.clone(),
            handoff,
            previous_status: Cell::new(Status::Stopped),
        };
        panel.refresh(node, &shell.engine.snapshot().state);
        panel
    }
    fn refresh(&self, node: &Node, state: &State) {
        self.title.set_text(&node.title);
        self.directory.set_text(&node.cwd.to_string_lossy());
        self.command.set_text(&node.command);
        self.status.set_text(&format!(
            "{}  ·  {}",
            node.kind.label(),
            node.status.label()
        ));
        let output = if node.kind == NodeKind::Human {
            node.output.clone()
        } else if node.desktop_window {
            if node.window_id.is_some() { "This application is an active node on its Niri row.\n\nPress Enter to return to its window. Super+Tab collapses the row into the graph without stopping any work." } else { "This application window has closed. Its node remains in the graph." }.into()
        } else if node.output.trim().is_empty() {
            if node.status == Status::Queued
                && node
                    .dependencies
                    .iter()
                    .filter_map(|id| state.node(id))
                    .any(|n| n.status == Status::Failed)
                && node.run_when == RunWhen::Success
            {
                "An upstream node raised an issue. This branch is paused until it is resolved."
                    .to_string()
            } else if node.status == Status::Review {
                let inputs: Vec<_> = node
                    .dependencies
                    .iter()
                    .filter_map(|id| state.node(id))
                    .map(|n| format!("{}\n{}", n.title, n.output))
                    .collect();
                format!("Waiting for your decision.\n\n{}", inputs.join("\n\n"))
            } else if node.kind == NodeKind::Human {
                node.command.clone()
            } else {
                "Output will appear here when this node runs.".into()
            }
        } else if node.kind == NodeKind::Agent {
            crate::engine::agent_result(&node.output)
                .map(|result| format!("{}\n\n{}", result.summary, result.artifacts.join("\n")))
                .unwrap_or_else(|_| node.output.clone())
        } else {
            node.output.clone()
        };
        if node.kind != NodeKind::Human || !self.output.has_focus() {
            set_buffer(&self.output, &output);
        }
        let activity: String = node
            .logs
            .iter()
            .map(|l| format!("[{}] {}\n{}\n\n", l.time % 86400, l.kind, l.text))
            .collect();
        set_buffer(&self.logs, &activity);
        if self.previous_status.get() == node.status && self.actions.first_child().is_some() {
            return;
        }
        self.previous_status.set(node.status);
        clear(&self.actions);
        if node.desktop_window {
            if node.window_id.is_none() {
                self.actions.append(&label("Window closed", "muted"));
                return;
            }
            let open = gtk::Button::with_label("Return to window");
            open.add_css_class("accent");
            let engine = self.engine.clone();
            let id = self.id.clone();
            open.connect_clicked(move |_| {
                engine.dispatch(Control::Ui {
                    command: UiCommand::Open { id: id.clone() },
                })
            });
            self.actions.append(&open);
            return;
        }
        if node.status == Status::Review
            || (matches!(node.kind, NodeKind::Human | NodeKind::Window)
                && node.status == Status::Running)
        {
            let approve = gtk::Button::with_label(if node.kind == NodeKind::Human {
                "✓  Mark done"
            } else {
                "✓  Approve & continue"
            });
            approve.add_css_class("accent");
            let e = self.engine.clone();
            let id = self.id.clone();
            let handoff = self.handoff.clone();
            approve.connect_clicked(move |_| {
                let text = handoff.text().to_string();
                e.dispatch(Control::Approve {
                    id: id.clone(),
                    output: (!text.is_empty()).then_some(text),
                });
            });
            self.actions.append(&approve);
            if node.status == Status::Review {
                let reject = gtk::Button::with_label("Reject");
                reject.add_css_class("quiet");
                let e = self.engine.clone();
                let id = self.id.clone();
                reject.connect_clicked(move |_| e.dispatch(Control::Reject { id: id.clone() }));
                self.actions.append(&reject);
            }
        } else if node.status.active() {
            let stop = gtk::Button::with_label("□  Stop");
            stop.add_css_class("quiet");
            let e = self.engine.clone();
            let id = self.id.clone();
            stop.connect_clicked(move |_| e.dispatch(Control::Stop { id: id.clone() }));
            self.actions.append(&stop);
        } else {
            let run = gtk::Button::with_label("▷  Run node");
            run.add_css_class("accent");
            let e = self.engine.clone();
            let id = self.id.clone();
            run.connect_clicked(move |_| {
                e.dispatch(Control::Run {
                    id: id.clone(),
                    downstream: false,
                })
            });
            self.actions.append(&run);
            let branch = gtk::Button::with_label("Run branch");
            branch.add_css_class("quiet");
            let e = self.engine.clone();
            let id = self.id.clone();
            branch.connect_clicked(move |_| {
                e.dispatch(Control::Run {
                    id: id.clone(),
                    downstream: true,
                })
            });
            self.actions.append(&branch);
        }
    }
}
fn window_selector(window: &niri_ipc::Window) -> String {
    crate::groups::selector(window)
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
    fn set_active_id(&self, id: Option<&str>) {
        let index = self
            .ids
            .borrow()
            .iter()
            .position(|s| Some(s.as_str()) == id);
        self.widget.set_selected(
            index
                .map(|i| i as u32)
                .unwrap_or(gtk::INVALID_LIST_POSITION),
        );
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
