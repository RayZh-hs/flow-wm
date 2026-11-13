//! Native node canvases use an xdg toplevel content role. They never join the
//! application's window list or the compositor's ordinary scrolling columns.
use gtk::{gdk, glib::translate::ToGlibPtr, prelude::*};
use niri_ipc::{Request, Response, node_row::NodeRowScene, socket::Socket};
use std::{cell::RefCell, ffi::CString, rc::Rc};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_surface},
};

unsafe extern "C" {
    fn gdk_wayland_display_get_wl_display(display: *mut gdk::ffi::GdkDisplay) -> *mut libc::c_void;
    fn gdk_wayland_surface_get_wl_surface(surface: *mut gdk::ffi::GdkSurface) -> *mut libc::c_void;
    fn gdk_wayland_toplevel_set_application_id(
        surface: *mut gtk::gdk::ffi::GdkSurface,
        application_id: *const libc::c_char,
    );
}

pub(super) fn connection(display: &gdk::Display) -> Option<Connection> {
    if !display.type_().name().contains("Wayland") {
        return None;
    }
    let pointer = unsafe { gdk_wayland_display_get_wl_display(display.to_glib_none().0) };
    if pointer.is_null() {
        return None;
    }
    // The borrowed backend never owns or disconnects GTK's wl_display.
    let backend =
        unsafe { wayland_backend::sys::client::Backend::from_foreign_display(pointer.cast()) };
    Some(Connection::from_backend(backend))
}

pub(super) fn surface(
    connection: &Connection,
    surface: &gdk::Surface,
) -> Option<wl_surface::WlSurface> {
    let pointer = unsafe { gdk_wayland_surface_get_wl_surface(surface.to_glib_none().0) };
    if pointer.is_null() {
        return None;
    }
    // GTK retains ownership of the foreign proxy.
    let id = unsafe {
        wayland_backend::client::ObjectId::from_ptr(
            wl_surface::WlSurface::interface(),
            pointer.cast(),
        )
    }
    .ok()?;
    wl_surface::WlSurface::from_id(connection, id).ok()
}

pub fn bind(window: &gtk::ApplicationWindow, workspace: u64) {
    let id = CString::new(niri_ipc::node_row::app_id(workspace)).unwrap();
    window.connect_local("map", true, move |values| {
        let window = values[0].get::<gtk::ApplicationWindow>().unwrap();
        if let Some(surface) = window.surface() {
            // Called only after probing the native contract, on a Wayland display.
            unsafe {
                gdk_wayland_toplevel_set_application_id(surface.to_glib_none().0, id.as_ptr());
            }
        }
        None
    });
}

pub fn interface(window: &gtk::ApplicationWindow, node: &str) {
    let id = CString::new(format!("dev.flowwm.interface.v1.{node}"))
        .expect("Node ID cannot contain NUL");
    window.connect_local("map", true, move |values| {
        let window = values[0].get::<gtk::ApplicationWindow>().unwrap();
        if let Some(surface) = window.surface()
            && surface.display().type_().name().contains("Wayland")
        {
            unsafe {
                gdk_wayland_toplevel_set_application_id(surface.to_glib_none().0, id.as_ptr());
            }
        }
        None
    });
}

mod protocol {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]
    #![allow(missing_docs, clippy::all)]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!(
            "../../components/niri/niri-ipc/protocols/flowwm-node-row-v1.xml"
        );
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!(
        "../../components/niri/niri-ipc/protocols/flowwm-node-row-v1.xml"
    );
}
use protocol::flowwm_node_row_manager_v1::{self, FlowwmNodeRowManagerV1};
struct WaylandState;
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<FlowwmNodeRowManagerV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &FlowwmNodeRowManagerV1,
        _: flowwm_node_row_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
struct WaylandPublisher {
    manager: FlowwmNodeRowManagerV1,
    connection: Connection,
    queue: EventQueue<WaylandState>,
    // Keep the foreign display alive until all protocol proxies have gone away.
    _display: gdk::Display,
}
impl WaylandPublisher {
    fn new(surface: &gdk::Surface) -> Option<Self> {
        let display = surface.display();
        let connection = connection(&display)?;
        let (globals, queue) = registry_queue_init::<WaylandState>(&connection).ok()?;
        let manager = globals.bind(&queue.handle(), 1..=1, ()).ok()?;
        Some(Self {
            manager,
            connection,
            queue,
            _display: display,
        })
    }
    fn publish(&mut self, surface: &gdk::Surface, scene: &NodeRowScene) -> bool {
        let Some(surface) = self::surface(&self.connection, surface) else {
            return false;
        };
        let chunks = scene.windows.chunks(16);
        if scene.windows.is_empty() {
            self.manager
                .set_scene(&surface, 1, serde_json::to_string(scene).unwrap());
        } else {
            for (index, windows) in chunks.enumerate() {
                let chunk = NodeRowScene {
                    windows: windows.to_vec(),
                    insets: scene.insets,
                };
                self.manager.set_scene(
                    &surface,
                    u32::from(index == 0),
                    serde_json::to_string(&chunk).unwrap(),
                );
            }
        }
        // Dispatch only events already read by GTK; never wait for the compositor.
        let _ = self.queue.dispatch_pending(&mut WaylandState);
        true
    }
}
impl Drop for WaylandPublisher {
    fn drop(&mut self) {
        self.manager.destroy();
    }
}
#[derive(Default)]
struct PublishState {
    last: Option<(u64, NodeRowScene)>,
    legacy: Option<Socket>,
    wayland: Option<WaylandPublisher>,
    probed: bool,
}
#[derive(Clone, Default)]
pub struct Publisher {
    state: Rc<RefCell<PublishState>>,
}
impl Publisher {
    pub fn invalidate(&self) {
        self.state.borrow_mut().last = None;
    }
    pub fn publish(&self, surface: Option<&gdk::Surface>, workspace: u64, scene: NodeRowScene) {
        let mut state = self.state.borrow_mut();
        if state
            .last
            .as_ref()
            .is_some_and(|(id, previous)| *id == workspace && *previous == scene)
        {
            return;
        }
        if !state.probed
            && let Some(surface) = surface
        {
            state.wayland = WaylandPublisher::new(surface);
            state.probed = true;
        }
        if let Some(publisher) = state.wayland.as_mut()
            && surface.is_some_and(|surface| publisher.publish(surface, &scene))
        {
            state.last = Some((workspace, scene));
            return;
        }
        // Compatibility for compositors that expose the original IPC-only contract.
        let socket = state.legacy.take().or_else(|| Socket::connect().ok());
        let Some(mut socket) = socket else { return };
        if matches!(
            socket.send(Request::SetNodeRow {
                workspace_id: workspace,
                scene: scene.clone()
            }),
            Ok(Ok(Response::Handled))
        ) {
            state.last = Some((workspace, scene));
            state.legacy = Some(socket);
        }
    }
}
