//! Window images are captured and decoded away from GTK. Animation only reads cached surfaces.
use crate::{
    engine::{Control, Engine, UiCommand},
    model::State,
    niri::Desktop,
};
use gtk::{
    cairo, gdk,
    gdk_pixbuf::{InterpType, Pixbuf},
    gio, glib,
};
use std::{
    collections::{BTreeMap, HashMap},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    pub fn overlaps(self, other: Self) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }

    pub fn inset(self, amount: f64) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            width: (self.width - amount * 2.).max(0.),
            height: (self.height - amount * 2.).max(0.),
        }
    }
    pub fn interpolate(self, other: Self, t: f64) -> Self {
        if t <= 0. {
            return self;
        }
        if t >= 1. {
            return other;
        }
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
            width: self.width + (other.width - self.width) * t,
            height: self.height + (other.height - self.height) * t,
        }
    }
    pub fn integer(self) -> cairo::RectangleInt {
        cairo::RectangleInt::new(
            self.x.floor() as i32,
            self.y.floor() as i32,
            ((self.x + self.width).ceil() - self.x.floor()).max(0.0) as i32,
            ((self.y + self.height).ceil() - self.y.floor()).max(0.0) as i32,
        )
    }
}

pub fn rectangles(
    desktop: &Desktop,
    state: &State,
    insets: [i32; 4],
    width: f64,
    centered: bool,
) -> HashMap<String, Rect> {
    let Some(row) = desktop.workspaces.iter().find(|w| w.is_focused) else {
        return HashMap::new();
    };
    let mut windows: Vec<_> = desktop
        .windows
        .iter()
        .filter(|w| w.workspace_id == Some(row.id))
        .collect();
    windows.sort_by_key(|w| w.layout.pos_in_scrolling_layout);
    let focused = windows
        .iter()
        .find(|w| w.is_focused || Some(w.id) == row.active_window_id)
        .copied();
    let mut columns = BTreeMap::new();
    for w in &windows {
        if let Some((c, _)) = w.layout.pos_in_scrolling_layout {
            columns
                .entry(c)
                .and_modify(|size: &mut f64| *size = size.max(w.layout.tile_size.0))
                .or_insert(w.layout.tile_size.0);
        }
    }
    let gap = 16.0;
    let focused_column = focused
        .and_then(|w| w.layout.pos_in_scrolling_layout)
        .map(|p| p.0)
        .unwrap_or(1);
    let before: f64 = columns
        .iter()
        .filter(|(c, _)| **c < focused_column)
        .map(|(_, w)| w + gap)
        .sum();
    let total: f64 = columns.values().map(|w| w + gap).sum::<f64>() + gap;
    let focused_width = columns
        .get(&focused_column)
        .copied()
        .unwrap_or(width - 2.0 * gap);
    // Older Niri versions omit viewport coordinates. Centering is known after Enter.
    let offset = if centered {
        (width - focused_width) / 2.0 - before
    } else if total <= width {
        gap
    } else {
        // Niri's default "never" centering scrolls just far enough to reveal the
        // focused column. In particular, a narrow rightmost column is right aligned.
        gap - (before + focused_width - (width - 2.0 * gap)).max(0.0)
    };
    let mut starts = HashMap::new();
    let mut x = offset;
    for (column, w) in columns {
        starts.insert(column, x);
        x += w + gap;
    }
    let mut result = HashMap::new();
    let nodes: HashMap<_, _> = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
        .filter_map(|n| Some((n.window_id?, n)))
        .collect();
    let mut column_heights = HashMap::<usize, f64>::new();
    for w in windows {
        let (c, _) = w
            .layout
            .pos_in_scrolling_layout
            .unwrap_or((focused_column, 1));
        let above = column_heights.entry(c).or_default();
        let fallback_y = gap + *above;
        if w.layout.pos_in_scrolling_layout.is_some() {
            *above += w.layout.tile_size.1 + gap;
        }
        let Some(node) = nodes.get(&w.id) else {
            continue;
        };
        let (x, y) = w
            .layout
            .tile_pos_in_workspace_view
            .map(|(x, y)| (x - insets[3] as f64, y - insets[0] as f64))
            .unwrap_or((*starts.get(&c).unwrap_or(&gap), fallback_y));
        result.insert(
            node.id.clone(),
            Rect {
                x: x + w.layout.window_offset_in_tile.0,
                y: y + w.layout.window_offset_in_tile.1,
                width: w.layout.window_size.0 as f64,
                height: w.layout.window_size.1 as f64,
            },
        );
    }
    result
}

pub struct Pixels {
    pub key: String,
    data: Vec<u8>,
    width: i32,
    height: i32,
    stride: i32,
}
impl Pixels {
    pub fn texture(&self) -> gdk::Texture {
        use gtk::prelude::*;
        gdk::MemoryTexture::new(
            self.width,
            self.height,
            gdk::MemoryFormat::B8g8r8a8Premultiplied,
            &glib::Bytes::from_owned(self.data.clone()),
            self.stride as usize,
        )
        .upcast()
    }
    pub fn surface(self) -> Option<(String, cairo::ImageSurface)> {
        let surface = cairo::ImageSurface::create_for_data(
            self.data,
            cairo::Format::ARgb32,
            self.width,
            self.height,
            self.stride,
        )
        .ok()?;
        Some((self.key, surface))
    }
}
fn decode(key: String, file: &std::path::Path, max_width: i32) -> Option<Pixels> {
    let image = Pixbuf::from_file(file).ok()?;
    let image = if image.width() > max_width {
        image.scale_simple(
            max_width,
            (image.height() as f64 * max_width as f64 / image.width() as f64)
                .round()
                .max(1.0) as i32,
            InterpType::Bilinear,
        )?
    } else {
        image
    };
    let (width, height) = (image.width(), image.height());
    let stride = cairo::Format::ARgb32.stride_for_width(width as u32).ok()?;
    let mut data = vec![0; stride as usize * height as usize];
    let source = image.read_pixel_bytes();
    let source = source.as_ref();
    let channels = image.n_channels() as usize;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let s = y * image.rowstride() as usize + x * channels;
            let d = y * stride as usize + x * 4;
            let a = if channels == 4 { source[s + 3] } else { 255 };
            let pixel = ((a as u32) << 24)
                | ((source[s] as u32 * a as u32 / 255) << 16)
                | ((source[s + 1] as u32 * a as u32 / 255) << 8)
                | (source[s + 2] as u32 * a as u32 / 255);
            data[d..d + 4].copy_from_slice(&pixel.to_ne_bytes());
        }
    }
    Some(Pixels {
        key,
        data,
        width,
        height,
        stride,
    })
}

pub struct Captures {
    tx: Option<mpsc::Sender<(Desktop, bool)>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    pub ready: Arc<Mutex<Vec<Pixels>>>,
}
impl Captures {
    pub fn new(engine: Engine) -> Self {
        let (tx, rx) = mpsc::channel::<(Desktop, bool)>();
        let ready = Arc::new(Mutex::new(vec![]));
        let output = ready.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let directory = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(format!("flowwm-captures-{}", crate::model::id()));
            if std::fs::create_dir(&directory).is_err() {
                return;
            }
            let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
            let mut wallpaper_key = None;
            let mut last = HashMap::<u64, Instant>::new();
            while let Ok((mut desktop, mut force)) = rx.recv() {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                while let Ok((new, refresh)) = rx.try_recv() {
                    desktop = new;
                    force |= refresh;
                }
                let monitor = desktop
                    .workspaces
                    .iter()
                    .find(|w| w.is_focused)
                    .and_then(|w| w.output.as_deref());
                let file = wallpaper_file(monitor);
                let key = file.as_ref().map(|p| {
                    (
                        p.clone(),
                        std::fs::metadata(p).ok().and_then(|m| m.modified().ok()),
                    )
                });
                if key != wallpaper_key {
                    if let Some(file) = file
                        && let Some(image) = decode("wallpaper".into(), &file, 3200)
                    {
                        output.lock().unwrap().push(image);
                        engine.dispatch(Control::Ui {
                            command: UiCommand::RefreshImages,
                        });
                    }
                    wallpaper_key = key;
                }
                let row = desktop
                    .workspaces
                    .iter()
                    .find(|w| w.is_focused)
                    .map(|w| w.id);
                let mut windows: Vec<_> = desktop
                    .windows
                    .iter()
                    .filter(|w| w.workspace_id == row)
                    .collect();
                windows.sort_by_key(|w| !w.is_focused);
                for window in windows.into_iter().take(20) {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    if !force
                        && last
                            .get(&window.id)
                            .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
                    {
                        continue;
                    }
                    let file = directory.join(format!("{}.png", window.id));
                    let start = Instant::now();
                    let decoded = capture_window(window.id, &file)
                        .and_then(|_| decode(window.id.to_string(), &file, 1440));
                    engine.performance.record("window_capture", start.elapsed());
                    let _ = std::fs::remove_file(&file);
                    if let Some(image) = decoded {
                        output.lock().unwrap().push(image);
                        last.insert(window.id, Instant::now());
                        engine.dispatch(Control::Ui {
                            command: UiCommand::RefreshImages,
                        });
                    }
                }
            }
            let _ = std::fs::remove_dir_all(directory);
        });
        Self {
            tx: Some(tx),
            ready,
            stop,
            worker: Some(worker),
        }
    }
    pub fn request(&self, desktop: Desktop, force: bool) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((desktop, force));
        }
    }
}

impl Drop for Captures {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

// Niri's ScreenshotWindow action also writes the clipboard and shows a notification.
// Its Mutter-compatible one-frame screencast avoids both side effects.
fn capture_window(id: u64, file: &std::path::Path) -> Option<()> {
    use glib::variant::ToVariant;
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            let connection =
                gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).ok()?;
            let call = |object: &str,
                        interface: &str,
                        method: &str,
                        parameters: Option<&glib::Variant>| {
                connection
                    .call_sync(
                        Some("org.gnome.Mutter.ScreenCast"),
                        object,
                        interface,
                        method,
                        parameters,
                        None,
                        gio::DBusCallFlags::NONE,
                        600,
                        None::<&gio::Cancellable>,
                    )
                    .ok()
            };
            let properties = HashMap::<String, glib::Variant>::new();
            let reply = call(
                "/org/gnome/Mutter/ScreenCast",
                "org.gnome.Mutter.ScreenCast",
                "CreateSession",
                Some(&(properties,).to_variant()),
            )?;
            let session = reply.child_value(0).str()?.to_string();
            let result = (|| {
                let properties = HashMap::from([
                    ("window-id".to_string(), id.to_variant()),
                    ("cursor-mode".to_string(), 0u32.to_variant()),
                ]);
                let reply = call(
                    &session,
                    "org.gnome.Mutter.ScreenCast.Session",
                    "RecordWindow",
                    Some(&(properties,).to_variant()),
                )?;
                let stream = reply.child_value(0).str()?.to_string();
                let node = std::rc::Rc::new(std::cell::Cell::new(None));
                let ready = node.clone();
                let subscription = connection.subscribe_to_signal(
                    Some("org.gnome.Mutter.ScreenCast"),
                    Some("org.gnome.Mutter.ScreenCast.Stream"),
                    Some("PipeWireStreamAdded"),
                    Some(&stream),
                    None,
                    gio::DBusSignalFlags::NONE,
                    move |signal| {
                        if let Some((id,)) = signal.parameters.get::<(u32,)>() {
                            ready.set(Some(id));
                        }
                    },
                );
                let started = call(
                    &session,
                    "org.gnome.Mutter.ScreenCast.Session",
                    "Start",
                    None,
                );
                let start = Instant::now();
                while started.is_some()
                    && node.get().is_none()
                    && start.elapsed() < Duration::from_millis(600)
                {
                    while context.pending() {
                        context.iteration(false);
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                drop(subscription);
                let node = node.get()?;
                let mut process = std::process::Command::new("gst-launch-1.0")
                    .args(["-q", "pipewiresrc"])
                    .arg(format!("path={node}"))
                    .args([
                        "num-buffers=1",
                        "!",
                        "videoconvert",
                        "!",
                        "video/x-raw,format=RGBA",
                        "!",
                        "pngenc",
                        "compression-level=0",
                        "!",
                        "filesink",
                    ])
                    .arg(format!("location={}", file.display()))
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .ok()?;
                let start = Instant::now();
                loop {
                    match process.try_wait() {
                        Ok(Some(status)) => return status.success().then_some(()),
                        Ok(None) if start.elapsed() < Duration::from_millis(900) => {
                            thread::sleep(Duration::from_millis(3))
                        }
                        _ => {
                            let _ = process.kill();
                            let _ = process.wait();
                            return None;
                        }
                    }
                }
            })();
            let _ = call(
                &session,
                "org.gnome.Mutter.ScreenCast.Session",
                "Stop",
                None,
            );
            result
        })
        .ok()
        .flatten()
}

fn wallpaper_file(output: Option<&str>) -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("FLOWWM_WALLPAPER") {
        return Some(file.into());
    }
    let base = directories::BaseDirs::new()?;
    let session = std::fs::read(base.state_dir()?.join("DankMaterialShell/session.json")).ok()?;
    let session: serde_json::Value = serde_json::from_slice(&session).ok()?;
    let per_output = if session["perMonitorWallpaper"].as_bool() == Some(true) {
        output
            .and_then(|o| session["monitorWallpapers"].get(o))
            .and_then(|v| v.as_str())
    } else {
        None
    };
    let file = per_output.or_else(|| session["wallpaperPath"].as_str())?;
    Some(PathBuf::from(file.strip_prefix("file://").unwrap_or(file)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeKind, Position};
    #[test]
    fn geometry_keeps_full_offscreen_surfaces_and_honors_exact_compositor_coordinates() {
        let mut state = State::empty("/tmp".into());
        let mut desktop = Desktop {
            connected: true,
            ..Desktop::default()
        };
        desktop.workspaces.push(serde_json::from_value(serde_json::json!({"id":1,"idx":1,"name":null,"output":"DP-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":3})).unwrap());
        for (id, width) in [(1, 1000), (2, 1000), (3, 500)] {
            desktop.windows.push(serde_json::from_value(serde_json::json!({"id":id,"workspace_id":1,"title":"Window","app_id":"test","pid":null,"is_focused":id==3,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[id,1],"tile_size":[width,900],"window_size":[width,900],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[0,0]},"focus_timestamp":null})).unwrap());
            let mut node = Node::new(
                &state.selected_workspace,
                NodeKind::Window,
                "/tmp".into(),
                Position::default(),
            );
            node.window_id = Some(id);
            state.nodes.push(node);
        }
        let rects = rectangles(&desktop, &state, [44, 0, 0, 0], 1600.0, false);
        assert_eq!(
            rects[&state.nodes[2].id].x, 1084.0,
            "a narrow focused column is right aligned before centering"
        );
        assert!(rects[&state.nodes[0].id].x < 0.0);
        assert_eq!(rects[&state.nodes[0].id].width, 1000.0);
        let centered = rectangles(&desktop, &state, [44, 0, 0, 0], 1600.0, true);
        assert_eq!(centered[&state.nodes[2].id].x, 550.0);
        desktop.windows[0].layout.tile_pos_in_workspace_view = Some((-412.0, 60.0));
        let exact = rectangles(&desktop, &state, [44, 0, 0, 0], 1600.0, true);
        assert_eq!(exact[&state.nodes[0].id].x, -412.0);
        assert_eq!(exact[&state.nodes[0].id].y, 16.0);
    }
}
