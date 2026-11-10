//! Surface-ordered node scenes. This avoids blocking the GTK frame on IPC and
//! prevents a scene from overtaking its canvas on a separate connection.
use std::{collections::HashSet, sync::Mutex};

use niri_ipc::node_row::NodeRowScene;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::compositor::with_states;

use super::raw::node_row::flowwm_node_row_manager_v1::{self, FlowwmNodeRowManagerV1};
use crate::niri::State;

#[derive(Default)]
pub(crate) struct PendingScene(pub Mutex<Option<StagedScene>>);

pub(crate) struct StagedScene {
    pub scene: NodeRowScene,
    ids: HashSet<u64>,
}
impl StagedScene {
    fn new(scene: NodeRowScene) -> Self {
        Self {
            ids: scene.windows.iter().map(|w| w.id).collect(),
            scene,
        }
    }
    fn append(&mut self, chunk: NodeRowScene) -> bool {
        if self.scene.windows.len() + chunk.windows.len() > 4096
            || chunk.windows.iter().any(|w| self.ids.contains(&w.id))
        {
            return false;
        }
        self.ids.extend(chunk.windows.iter().map(|w| w.id));
        self.scene.windows.extend(chunk.windows);
        true
    }
}

pub fn init(display: &DisplayHandle) {
    display.create_global::<State, FlowwmNodeRowManagerV1, _>(1, ());
}

impl GlobalDispatch<FlowwmNodeRowManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        manager: New<FlowwmNodeRowManagerV1>,
        _: &(),
        data: &mut DataInit<'_, Self>,
    ) {
        data.init(manager, ());
    }
}

impl Dispatch<FlowwmNodeRowManagerV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        manager: &FlowwmNodeRowManagerV1,
        request: flowwm_node_row_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            flowwm_node_row_manager_v1::Request::Destroy => (),
            flowwm_node_row_manager_v1::Request::SetScene {
                surface,
                reset,
                payload,
            } => {
                let scene = serde_json::from_str::<NodeRowScene>(&payload)
                    .ok()
                    .filter(|scene| scene.windows.len() <= 16 && scene.validate());
                let Some(scene) = scene else {
                    manager.post_error(0u32, "invalid node scene chunk");
                    return;
                };
                with_states(&surface, |states| {
                    states.data_map.insert_if_missing(PendingScene::default);
                    let pending = states.data_map.get::<PendingScene>().unwrap();
                    let mut pending = pending.0.lock().unwrap();
                    if reset != 0 {
                        *pending = Some(StagedScene::new(scene));
                    } else if let Some(pending) = pending.as_mut() {
                        if !pending.append(scene) {
                            manager.post_error(0u32, "invalid or oversized node scene");
                        }
                    } else {
                        manager.post_error(0u32, "node scene append without reset");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use niri_ipc::node_row::NodeWindow;

    #[test]
    fn node_rows_chunks_reject_duplicate_ids_and_enforce_the_scene_limit() {
        let chunk = |start, end| NodeRowScene {
            windows: (start..end)
                .map(|id| NodeWindow {
                    id,
                    bounds: (0., 0., 100., 100.),
                    expanded: false,
                    opacity: 1.,
                    presentation: None,
                })
                .collect(),
            insets: [0; 4],
        };
        let mut staged = StagedScene::new(chunk(1, 17));
        assert!(!staged.append(chunk(16, 32)));
        assert_eq!(staged.scene.windows.len(), 16);
        for start in (17..4097).step_by(16) {
            assert!(staged.append(chunk(start, start + 16)));
        }
        assert!(staged.scene.validate());
        assert!(!staged.append(chunk(4097, 4098)));
        assert_eq!(staged.scene.windows.len(), 4096);
    }
}
