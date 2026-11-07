//! Native node canvases use an xdg toplevel content role. They never join the
//! application's window list or the compositor's ordinary scrolling columns.
use gtk::{glib::translate::ToGlibPtr, prelude::*};
use niri_ipc::{Request, Response, node_row::NodeRowScene, socket::Socket};
use std::{cell::RefCell, ffi::CString, rc::Rc};

unsafe extern "C" {
    fn gdk_wayland_toplevel_set_application_id(
        surface: *mut gtk::gdk::ffi::GdkSurface,
        application_id: *const libc::c_char,
    );
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

#[derive(Clone, Default)]
pub struct Publisher {
    state: Rc<RefCell<Option<(Socket, u64, NodeRowScene)>>>,
}
impl Publisher {
    pub fn invalidate(&self) {
        self.state.borrow_mut().take();
    }
    pub fn publish(&self, workspace: u64, scene: NodeRowScene) {
        let mut state = self.state.borrow_mut();
        if state
            .as_ref()
            .is_some_and(|(_, id, previous)| *id == workspace && *previous == scene)
        {
            return;
        }
        let socket = state
            .take()
            .map(|(socket, _, _)| socket)
            .or_else(|| Socket::connect().ok());
        let Some(mut socket) = socket else { return };
        if matches!(
            socket.send(Request::SetNodeRow {
                workspace_id: workspace,
                scene: scene.clone()
            }),
            Ok(Ok(Response::Handled))
        ) {
            *state = Some((socket, workspace, scene));
        }
    }
}
