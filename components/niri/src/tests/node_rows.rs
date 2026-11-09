use std::time::Duration;

use niri_config::Config;
use niri_ipc::node_row::{self, NodeRowScene, NodeWindow};
use smithay::backend::renderer::element::Element;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Rectangle};

use super::client::{ClientId, LayerConfigureProps};
use super::Fixture;
use crate::render_helpers::{RenderCtx, RenderTarget};

mod scene_client {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]
    #![allow(missing_docs, clippy::all)]
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("niri-ipc/protocols/flowwm-node-row-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("niri-ipc/protocols/flowwm-node-row-v1.xml");
}
use scene_client::flowwm_node_row_manager_v1::FlowwmNodeRowManagerV1;
wayland_client::delegate_noop!(super::client::State: FlowwmNodeRowManagerV1);

#[test]
fn node_rows_wayland_scenes_latch_chunks_only_on_the_matching_canvas_commit() {
    let mut f =
        Fixture::with_config(Config::parse_mem("workspace \"one\"\nworkspace \"two\"").unwrap());
    f.add_output(1, (1920, 1080));
    let ids: Vec<_> = f
        .niri()
        .layout
        .workspaces()
        .take(2)
        .map(|(_, _, ws)| ws.id().get())
        .collect();
    let client = f.add_client();
    let (first, _) = canvas(&mut f, client, ids[0]);
    let (second, _) = canvas(&mut f, client, ids[1]);
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|g| g.interface == "flowwm_node_row_manager_v1")
        .unwrap();
    let registry = c.display.get_registry(&c.qh, ());
    let manager: FlowwmNodeRowManagerV1 = registry.bind(global.name, 1, &c.qh, ());
    let scene = NodeRowScene {
        windows: (1..=300)
            .map(|id| NodeWindow {
                id,
                bounds: (id as f64 + 0.25, 30.5, 200., 120.),
                expanded: false,
                opacity: 1.,
            })
            .collect(),
        insets: [32, 0, 0, 0],
    };
    for (index, windows) in scene.windows.chunks(16).enumerate() {
        let chunk = NodeRowScene {
            windows: windows.to_vec(),
            insets: scene.insets,
        };
        manager.set_scene(
            &first,
            u32::from(index == 0),
            serde_json::to_string(&chunk).unwrap(),
        );
    }
    f.double_roundtrip(client);
    let current = |f: &mut Fixture, id| {
        f.niri()
            .layout
            .workspaces()
            .find(|(_, _, w)| w.id().get() == id)
            .unwrap()
            .2
            .node_tile
            .as_ref()
            .unwrap()
            .scene
            .clone()
    };
    assert_eq!(current(&mut f, ids[0]), NodeRowScene::default());
    f.client(client).window(&second).commit();
    f.double_roundtrip(client);
    assert_eq!(
        current(&mut f, ids[0]),
        NodeRowScene::default(),
        "another row must not consume a pending scene"
    );
    f.client(client).window(&first).commit();
    f.double_roundtrip(client);
    assert_eq!(current(&mut f, ids[0]), scene);
    manager.set_scene(
        &first,
        1,
        serde_json::to_string(&NodeRowScene::default()).unwrap(),
    );
    f.client(client).window(&first).commit();
    f.double_roundtrip(client);
    assert_eq!(
        current(&mut f, ids[0]),
        NodeRowScene::default(),
        "an empty scene clears all previews"
    );
    manager.destroy();
}

fn canvas(
    f: &mut Fixture,
    client: ClientId,
    workspace: u64,
) -> (wayland_client::protocol::wl_surface::WlSurface, WlSurface) {
    let win = f.client(client).create_window();
    win.xdg_toplevel.set_app_id(node_row::app_id(workspace));
    let surface = win.surface.clone();
    win.commit();
    f.double_roundtrip(client);
    let win = f.client(client).window(&surface);
    win.attach_new_buffer();
    win.set_size(1920, 1080);
    win.ack_last_and_commit();
    f.double_roundtrip(client);
    let ws = f
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, ws)| ws.id().get() == workspace)
        .unwrap()
        .2;
    (
        surface,
        ws.node_tile
            .as_ref()
            .unwrap()
            .canvas
            .toplevel()
            .unwrap()
            .wl_surface()
            .clone(),
    )
}

fn render(f: &mut Fixture) -> Vec<Rectangle<i32, Physical>> {
    render_opacity(f)
        .into_iter()
        .map(|(rect, _)| rect)
        .collect()
}

fn render_opacity(f: &mut Fixture) -> Vec<(Rectangle<i32, Physical>, f32)> {
    let output = f.niri_output(1);
    let state = f.niri_state();
    state.niri.update_render_elements(None);
    state
        .backend
        .headless()
        .with_primary_renderer(|renderer| {
            let mon = state.niri.layout.monitor_for_output(&output).unwrap();
            let mut result = Vec::new();
            mon.render_workspaces(
                RenderCtx {
                    renderer,
                    target: RenderTarget::Output,
                    xray: None,
                },
                true,
                &mut |elem| result.push((elem.geometry(1.0.into()), elem.alpha())),
            );
            result
        })
        .unwrap()
}

#[test]
fn node_rows_use_canvas_geometry_when_gtk_sets_the_role_after_initial_commit() {
    let mut f =
        Fixture::with_config(Config::parse_mem(r#"output "headless-1" { scale 1.75; }"#).unwrap());
    f.add_output(1, (3200, 2000));
    let workspace = f.niri().layout.active_workspace().unwrap().id().get();
    let client = f.add_client();
    let panel = f.client(client).create_layer(None, Layer::Top, "panel");
    let panel_surface = panel.surface.clone();
    panel.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 44)),
        exclusive_zone: Some(44),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&panel_surface);
    panel.attach_new_buffer();
    panel.set_size(1829, 44);
    panel.ack_last_and_commit();
    f.double_roundtrip(client);

    let win = f.client(client).create_window();
    let surface = win.surface.clone();
    // GTK sends its ordinary application ID and empty commit before our map handler sets the role.
    win.xdg_toplevel.set_app_id("dev.flowwm.Shell".into());
    win.commit();
    win.xdg_toplevel.set_app_id(node_row::app_id(workspace));
    f.double_roundtrip(client);

    let win = f.client(client).window(&surface);
    assert_eq!(
        win.configures_received
            .iter()
            .map(|(_, c)| c.size)
            .collect::<Vec<_>>(),
        [(1829, 1099)],
        "the first buffer must fill the canvas instead of a temporary half-width window"
    );
    win.attach_new_buffer();
    win.set_size(1829, 1099);
    win.ack_last_and_commit();
    f.double_roundtrip(client);
    assert!(f.niri().layout.active_workspace().unwrap().nodes_visible());
    assert_eq!(f.niri().layout.windows().count(), 0);
}

#[test]
fn egl_node_rows_replace_windows_and_follow_swipes_overview_and_lifecycle() {
    let mut f = Fixture::with_config(
        Config::parse_mem("workspace \"first\"\nworkspace \"second\"").unwrap(),
    );
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (1920, 1080));
    let rows: Vec<_> = f
        .niri()
        .layout
        .workspaces()
        .take(2)
        .map(|(_, i, ws)| (i, ws.id().get()))
        .collect();
    let (first_idx, first) = rows[0];
    let (_, second) = rows[1];
    f.niri().layout.switch_workspace(first_idx);
    f.niri_complete_animations();
    let client = f.add_client();
    // Keep a real window on the row; it must not be drawn or accept input behind Nodes.
    let win = f.client(client).create_window();
    let win_surface = win.surface.clone();
    win.commit();
    f.double_roundtrip(client);
    let win = f.client(client).window(&win_surface);
    win.attach_new_buffer();
    win.set_size(800, 600);
    win.ack_last_and_commit();
    f.double_roundtrip(client);
    f.niri_complete_animations();
    let window_id = f.niri().layout.windows().next().unwrap().1.id().get();
    f.niri().layout.center_window(None);
    f.niri_complete_animations();
    let ws = f.niri().layout.active_workspace().unwrap();
    let (_, position, _) = ws.tiles_with_render_positions().next().unwrap();
    let (_, ipc) = ws.tiles_with_ipc_layouts().next().unwrap();
    assert_eq!(
        ipc.tile_pos_in_workspace_view,
        Some((position.x, position.y))
    );
    assert!(
        position.x > 100.,
        "centering must produce a nonzero viewport offset"
    );
    let (first_client, first_surface) = canvas(&mut f, client, first);
    let (_, second_surface) = canvas(&mut f, client, second);
    assert_eq!(
        f.niri().layout.windows().count(),
        1,
        "canvases are workspace content, never application tiles"
    );
    assert_eq!(layer_map_for_output(&f.niri_output(1)).layers().count(), 0);
    assert_eq!(
        f.niri().seat.get_keyboard().unwrap().current_focus(),
        Some(first_surface.clone())
    );
    assert_eq!(
        render(&mut f).len(),
        1,
        "the window/tab layout must not render behind the graph"
    );
    assert_eq!(
        f.niri()
            .contents_under((100., 100.).into())
            .surface
            .unwrap()
            .0,
        first_surface
    );
    assert!(f
        .niri()
        .contents_under((100., 100.).into())
        .window
        .is_none());

    f.niri()
        .stage_node_row(
            first,
            NodeRowScene {
                windows: vec![NodeWindow {
                    id: window_id,
                    bounds: (100., 100., 200., 150.),
                    expanded: false,
                    opacity: 1.,
                }],
                insets: [0; 4],
            },
        )
        .unwrap();
    assert_eq!(
        render(&mut f).len(),
        1,
        "geometry must wait for the matching canvas buffer commit"
    );
    f.client(client).window(&first_client).commit();
    f.double_roundtrip(client);
    let elements = render(&mut f);
    assert_eq!(
        elements.len(),
        3,
        "one live projection, its dimmed backdrop, and graph content, without the tab view"
    );
    assert!(elements.iter().any(|r| r.size.w == 200 && r.size.h == 150));

    f.niri()
        .stage_node_row(
            first,
            NodeRowScene {
                windows: vec![NodeWindow {
                    id: window_id,
                    bounds: (100., 100., 800., 600.),
                    expanded: true,
                    opacity: 1.,
                }],
                insets: [0; 4],
            },
        )
        .unwrap();
    f.client(client).window(&first_client).commit();
    f.double_roundtrip(client);
    assert!(f
        .niri()
        .contents_under((120., 120.).into())
        .window
        .is_some());
    assert_eq!(
        f.niri()
            .contents_under((20., 20.).into())
            .surface
            .unwrap()
            .0,
        first_surface
    );
    let window = f.niri().layout.windows().next().unwrap().1.window.clone();
    f.niri_state().focus_window(&window);
    f.double_roundtrip(client);
    assert_eq!(
        f.niri().seat.get_keyboard().unwrap().current_focus(),
        Some(window.toplevel().unwrap().wl_surface().clone())
    );
    f.niri().focus_node_canvas(Some(&first_surface));
    f.double_roundtrip(client);
    assert_eq!(
        f.niri().seat.get_keyboard().unwrap().current_focus(),
        Some(first_surface.clone())
    );
    // A client can still have its old size while a new scene is committed.
    // Keep its origin fixed and clip both rendering and input to the scene.
    for (width, height) in [(1000., 700.), (400., 300.)] {
        f.niri()
            .stage_node_row(
                first,
                NodeRowScene {
                    windows: vec![NodeWindow {
                        id: window_id,
                        bounds: (101.25, 102.5, width, height),
                        expanded: true,
                        opacity: 1.,
                    }],
                    insets: [0; 4],
                },
            )
            .unwrap();
        f.client(client).window(&first_client).commit();
        f.double_roundtrip(client);
        let elements = render(&mut f);
        assert!(elements.iter().any(|r| r.loc.x == 101 && r.loc.y == 103
            && r.size.w == width.min(800.) as i32 && r.size.h == height.min(600.) as i32),
            "expanded content must stay at the scene origin with no extra layout frame: {elements:?}");
        assert!(f
            .niri()
            .contents_under((120., 120.).into())
            .window
            .is_some());
        if width < 800. {
            assert!(
                f.niri()
                    .contents_under((700., 200.).into())
                    .window
                    .is_none(),
                "clipped client pixels must not receive input outside the scene rectangle"
            );
        }
    }
    f.niri()
        .stage_node_row(
            first,
            NodeRowScene {
                windows: vec![NodeWindow {
                    id: window_id,
                    bounds: (100., 100., 200., 150.),
                    expanded: false,
                    opacity: 1.,
                }],
                insets: [0; 4],
            },
        )
        .unwrap();
    f.client(client).window(&first_client).commit();
    f.double_roundtrip(client);

    let output = f.niri_output(1);
    f.niri()
        .layout
        .workspace_switch_gesture_begin(&output, true);
    f.niri()
        .layout
        .workspace_switch_gesture_update(120., Duration::from_millis(20), true);
    let geos: Vec<_> = f
        .niri()
        .layout
        .monitor_for_output(&output)
        .unwrap()
        .workspaces_with_render_geo()
        .map(|(ws, geo)| (ws.id().get(), geo))
        .collect();
    assert_eq!(geos.len(), 2);
    let moving = render_opacity(&mut f);
    assert!(
        moving.iter().any(|(_, alpha)| *alpha < 0.01),
        "canvas text fades while swiping"
    );
    assert!(
        moving.iter().any(|(_, alpha)| *alpha > 0.99),
        "native window content stays visible"
    );
    let elements = render(&mut f);
    assert_eq!(elements.len(), 4);
    for (id, geo) in &geos {
        let point = (100., geo.loc.y.max(0.) + 20.).into();
        let surface = if *id == first {
            &first_surface
        } else {
            &second_surface
        };
        assert_eq!(&f.niri().contents_under(point).surface.unwrap().0, surface);
        assert!(
            elements
                .iter()
                .all(|r| geos.iter().any(|(_, geo)| r.loc.y as f64 >= geo.loc.y
                    && (r.loc.y + r.size.h) as f64 <= geo.loc.y + geo.size.h)),
            "node tiles must be cropped to their own workspace: {elements:?}, {geos:?}"
        );
    }
    f.niri()
        .layout
        .workspace_switch_gesture_update(-120., Duration::from_millis(40), true);
    f.niri().layout.workspace_switch_gesture_end(Some(true));
    f.niri_complete_animations();
    assert_eq!(
        f.niri().layout.active_workspace().unwrap().id().get(),
        first
    );
    assert_eq!(render(&mut f).len(), 3);
    f.niri().layout.open_overview();
    f.niri_complete_animations();
    let faded = render_opacity(&mut f);
    assert!(
        faded.iter().any(|(_, alpha)| *alpha < 0.01),
        "canvas text is hidden in overview"
    );
    assert!(
        faded.iter().any(|(_, alpha)| *alpha > 0.99),
        "native windows remain visible in overview"
    );
    let overview = render(&mut f);
    assert!(
        overview.len() >= 3,
        "overview includes each native graph and its live projections"
    );
    assert!(overview.iter().all(|r| r.size.w < 1920));
    assert!(f.niri().node_canvas_focus().is_none());
    assert!(f
        .niri()
        .node_canvas_under(&output, (100., 100.).into())
        .is_none());
    f.niri().layout.close_overview();
    f.niri_complete_animations();

    // Null commits return the row to native Windows; remapping restores Nodes.
    f.client(client).window(&first_client).attach_null();
    f.client(client).window(&first_client).commit();
    f.double_roundtrip(client);
    assert!(!f.niri().layout.active_workspace().unwrap().nodes_visible());
    assert!(f.niri().node_canvas_focus().is_none());
    f.client(client).window(&first_client).commit();
    f.double_roundtrip(client);
    f.client(client).window(&first_client).attach_new_buffer();
    f.client(client).window(&first_client).ack_last_and_commit();
    f.double_roundtrip(client);
    assert!(f.niri().layout.active_workspace().unwrap().nodes_visible());
    f.client(client)
        .window(&first_client)
        .xdg_toplevel
        .destroy();
    f.double_roundtrip(client);
    assert!(f
        .niri()
        .layout
        .active_workspace()
        .unwrap()
        .node_tile
        .is_none());
    assert_eq!(f.niri().layout.windows().count(), 1);
}

#[test]
fn node_rows_respect_panels_even_with_a_fullscreen_window_and_reject_duplicate_roles() {
    let mut f = Fixture::with_config(Config::parse_mem("workspace \"first\"").unwrap());
    f.add_output(1, (1920, 1080));
    let client = f.add_client();
    let workspace = f.niri().layout.active_workspace().unwrap().id().get();
    let win = f.client(client).create_window();
    let fullscreen_surface = win.surface.clone();
    win.set_fullscreen(None);
    win.commit();
    f.double_roundtrip(client);
    let win = f.client(client).window(&fullscreen_surface);
    win.attach_new_buffer();
    win.set_size(1920, 1080);
    win.ack_last_and_commit();
    f.double_roundtrip(client);
    f.niri_complete_animations();
    assert!(f
        .niri()
        .layout
        .active_workspace()
        .unwrap()
        .render_above_top_layer());
    let (surface, server_surface) = canvas(&mut f, client, workspace);
    for binding in [
        node_row::app_id(workspace),
        node_row::app_id(u64::MAX),
        format!("{}0", node_row::PREFIX),
    ] {
        let win = f.client(client).create_window();
        let surface = win.surface.clone();
        win.xdg_toplevel.set_app_id(binding);
        win.commit();
        f.double_roundtrip(client);
        assert!(f.client(client).window(&surface).close_requested);
    }
    let panel = f.client(client).create_layer(None, Layer::Top, "panel");
    let panel_surface = panel.surface.clone();
    panel.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        exclusive_zone: Some(50),
        kb_interactivity: Some(KeyboardInteractivity::Exclusive),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&panel_surface);
    panel.attach_new_buffer();
    panel.set_size(1920, 50);
    panel.ack_last_and_commit();
    f.double_roundtrip(client);
    let ws = f.niri().layout.active_workspace().unwrap();
    assert_eq!(ws.node_canvas_geometry().loc.y, 50.);
    assert!(!ws.render_above_top_layer());
    assert!(f.niri().contents_under((100., 10.).into()).layer.is_some());
    assert_ne!(
        f.niri().seat.get_keyboard().unwrap().current_focus(),
        Some(server_surface)
    );
    let win = f.client(client).window(&surface);
    assert_eq!(win.configures_received.last().unwrap().1.size, (1920, 1030));
}

#[test]
fn node_rows_keep_an_empty_trailing_row_and_follow_output_removal() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let client = f.add_client();
    let workspace = f.niri().layout.active_workspace().unwrap().id().get();
    let (_, surface) = canvas(&mut f, client, workspace);
    let output = f.niri_output(1);
    let rows: Vec<_> = f
        .niri()
        .layout
        .workspaces()
        .filter(|(mon, _, _)| mon.is_some_and(|m| m.output() == &output))
        .map(|(_, _, ws)| ws)
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.last().unwrap().node_tile.is_none());
    f.add_output(2, (160, 720));
    let second = f.niri_output(2);
    f.niri().remove_output(&output);
    f.double_roundtrip(client);
    assert_eq!(f.niri().output_for_root(&surface), Some(&second));
    let ws = f
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, ws)| ws.id().get() == workspace)
        .unwrap()
        .2;
    assert!(ws.nodes_visible());
    assert_eq!(ws.node_canvas_geometry().size.w, 160.);
    assert_eq!(ws.node_canvas_geometry().size.h, 720.);
}
