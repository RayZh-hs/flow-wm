use anyhow::{Context, Result, bail};
use niri_ipc::{Action, Request, Response, WorkspaceReferenceArg, socket::Socket};
use serde::{Deserialize, Serialize};
use std::{
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Desktop {
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub windows_ready: bool,
    #[serde(default)]
    pub workspaces_ready: bool,
    pub connected: bool,
    /// A transport failure alone does not end the compositor's session.
    #[serde(default)]
    pub session_ended: bool,
    pub windows: Vec<niri_ipc::Window>,
    pub workspaces: Vec<niri_ipc::Workspace>,
    pub error: Option<String>,
}
impl Desktop {
    pub fn activate_row(&mut self, id: u64, focused: bool) {
        let Some(row) = self.workspaces.iter().find(|w| w.id == id).cloned() else {
            return;
        };
        for workspace in &mut self.workspaces {
            if focused {
                workspace.is_focused = workspace.id == id;
            }
            if workspace.output == row.output {
                workspace.is_active = workspace.id == id;
            }
        }
        if focused {
            for window in &mut self.windows {
                window.is_focused = Some(window.id) == row.active_window_id;
            }
        }
    }
    pub fn apply_event(&mut self, event: niri_ipc::Event) -> bool {
        use niri_ipc::Event;
        let synchronize = matches!(
            &event,
            Event::WindowsChanged { .. }
                | Event::WindowOpenedOrChanged { .. }
                | Event::WindowClosed { .. }
                | Event::WindowFocusChanged { .. }
                | Event::WindowUrgencyChanged { .. }
                | Event::WorkspacesChanged { .. }
                | Event::WorkspaceActivated { .. }
        );
        let d = self;
        match event {
            Event::WindowsChanged { windows } => {
                d.windows = windows;
                d.windows_ready = true;
            }
            Event::WindowOpenedOrChanged { window } => {
                if window.is_focused {
                    for w in &mut d.windows {
                        w.is_focused = false;
                    }
                }
                if let Some(w) = d.windows.iter_mut().find(|w| w.id == window.id) {
                    *w = window;
                } else {
                    d.windows.push(window);
                }
            }
            Event::WindowClosed { id } => d.windows.retain(|w| w.id != id),
            Event::WindowLayoutsChanged { changes } => {
                for (id, layout) in changes {
                    if let Some(window) = d.windows.iter_mut().find(|w| w.id == id) {
                        window.layout = layout;
                    }
                }
            }
            Event::WindowFocusChanged { id } => {
                for w in &mut d.windows {
                    w.is_focused = Some(w.id) == id;
                }
            }
            Event::WindowUrgencyChanged { id, urgent } => {
                if let Some(w) = d.windows.iter_mut().find(|w| w.id == id) {
                    w.is_urgent = urgent;
                }
            }
            Event::WorkspacesChanged { workspaces } => {
                d.workspaces = workspaces;
                d.workspaces_ready = true;
            }
            Event::WorkspaceActivated { id, focused } => {
                d.activate_row(id, focused);
            }
            Event::WorkspaceActiveWindowChanged {
                workspace_id,
                active_window_id,
            } => {
                if let Some(row) = d.workspaces.iter_mut().find(|w| w.id == workspace_id) {
                    row.active_window_id = active_window_id;
                }
            }
            _ => {}
        }
        synchronize
    }
}

pub fn request(request: Request) -> Result<Response> {
    // Commands are short lived; the live event stream uses its own connection.
    let mut socket =
        Socket::connect().context("NIRI_SOCKET is unavailable; start FlowWM inside Niri")?;
    socket.send(request)?.map_err(anyhow::Error::msg)
}

pub fn node_rows_supported() -> bool {
    matches!(
        request(Request::NodeRowSupport),
        Ok(Response::NodeRowSupport { version: 1 })
    )
}
pub fn focus(id: u64) -> Result<()> {
    request(Request::Action(Action::FocusWindow { id }))?;
    Ok(())
}
pub fn move_to_workspace(id: u64, index: u8) -> Result<()> {
    request(Request::Action(Action::MoveWindowToWorkspace {
        window_id: Some(id),
        reference: WorkspaceReferenceArg::Index(index),
        focus: false,
    }))?;
    Ok(())
}
pub fn navigate(direction: &str) -> Result<()> {
    let action = match direction {
        "left" => Action::FocusColumnLeft {},
        "right" => Action::FocusColumnRight {},
        "up" => Action::FocusWindowUp {},
        "down" => Action::FocusWindowDown {},
        _ => bail!("Unknown direction"),
    };
    request(Request::Action(action))?;
    Ok(())
}
pub fn move_window(direction: &str) -> Result<()> {
    let action = match direction {
        "left" => Action::MoveColumnLeft {},
        "right" => Action::MoveColumnRight {},
        "up" => Action::MoveWindowUp {},
        "down" => Action::MoveWindowDown {},
        _ => bail!("Unknown direction"),
    };
    request(Request::Action(action))?;
    Ok(())
}
/// A persistent logical workspace gets a named Niri workspace on first expansion.
/// Only an empty workspace is named; existing user workspaces are never repurposed.
pub fn activate_workspace(logical_id: &str, create: bool) -> Result<()> {
    let name = format!("flowwm-{}", &logical_id[..logical_id.len().min(8)]);
    let Response::Workspaces(workspaces) = request(Request::Workspaces)? else {
        bail!("Unexpected Niri workspace response");
    };
    if workspaces.iter().any(|w| w.name.as_deref() == Some(&name)) {
        request(Request::Action(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Name(name),
        }))?;
    } else if create {
        let output = workspaces
            .iter()
            .find(|w| w.is_focused)
            .and_then(|w| w.output.as_ref());
        let empty = workspaces
            .iter()
            .filter(|w| {
                w.output.as_ref() == output && w.active_window_id.is_none() && w.name.is_none()
            })
            .max_by_key(|w| w.idx)
            .context("No empty Niri workspace is available on this output")?;
        request(Request::Action(Action::SetWorkspaceName {
            name: name.clone(),
            workspace: Some(WorkspaceReferenceArg::Index(empty.idx)),
        }))?;
        request(Request::Action(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Name(name),
        }))?;
    }
    Ok(())
}
pub fn listen(stop: Arc<AtomicBool>) -> Arc<Mutex<Desktop>> {
    // The desktop provider already publishes reconciled bindings to the shell.
    // Keep native geometry here without dispatching a second action (and
    // presentation save) for every compositor event.
    listen_observed(stop, Arc::new(|_| {}))
}
pub fn listen_observed(
    stop: Arc<AtomicBool>,
    observer: Arc<dyn Fn(Desktop) + Send + Sync>,
) -> Arc<Mutex<Desktop>> {
    let desktop = Arc::new(Mutex::new(Desktop::default()));
    let shared = desktop.clone();
    thread::spawn(move || {
        let mut process = None;
        while !stop.load(Ordering::Relaxed) {
            if let Err(error) = event_stream(&shared, &stop, &observer, &mut process) {
                let mut state = shared.lock().unwrap();
                state.connected = false;
                state.session_ended = process
                    .as_ref()
                    .is_some_and(|process| process.has_exited().unwrap_or(false));
                state.error = Some(error.to_string());
                let snapshot = state.clone();
                drop(state);
                observer(snapshot);
            }
            for _ in 0..20 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
    desktop
}
fn event_stream(
    desktop: &Mutex<Desktop>,
    stop: &AtomicBool,
    observer: &Arc<dyn Fn(Desktop) + Send + Sync>,
    process: &mut Option<CompositorProcess>,
) -> Result<()> {
    let mut socket = Socket::connect()?;
    *process = Some(CompositorProcess::observe(&socket)?);
    socket
        .send(Request::EventStream)?
        .map_err(anyhow::Error::msg)?;
    {
        let mut d = desktop.lock().unwrap();
        d.connected = true;
        d.session_ended = false;
        d.windows_ready = false;
        d.workspaces_ready = false;
        d.session = std::env::var("NIRI_SOCKET").unwrap_or_default();
        d.error = None;
    }
    let mut next = socket.read_events();
    while !stop.load(Ordering::Relaxed) {
        let event = next()?;
        let mut d = desktop.lock().unwrap();
        let synchronize = d.apply_event(event);
        if synchronize {
            let snapshot = d.clone();
            drop(d);
            observer(snapshot);
        }
    }
    Ok(())
}

/// Keep a stable reference to the IPC server, independently of its event stream.
struct CompositorProcess(OwnedFd);

impl CompositorProcess {
    fn observe(socket: &impl AsFd) -> std::io::Result<Self> {
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                socket.as_fd().as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, credentials.pid, 0) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(unsafe { OwnedFd::from_raw_fd(raw as i32) }))
    }

    fn has_exited(&self) -> std::io::Result<bool> {
        let mut fd = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut fd, 1, 0) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fd.revents & (libc::POLLIN | libc::POLLHUP) != 0)
    }
}
