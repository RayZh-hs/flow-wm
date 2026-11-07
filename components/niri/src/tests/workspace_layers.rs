use std::time::Duration;

use niri_config::Config;
use niri_ipc::workspace_layer;
use smithay::backend::renderer::element::Element;
use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use smithay::utils::{Physical, Point, Rectangle};

use super::client::{ClientId, LayerConfigureProps};
use super::Fixture;
use crate::render_helpers::{RenderCtx, RenderTarget};

fn map_row(f: &mut Fixture, client: ClientId, workspace: u64) -> LayerSurface {
    let namespace = workspace_layer::namespace(workspace, "test");
    let layer = f
        .client(client)
        .create_layer(None, Layer::Overlay, &namespace);
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top | Anchor::Bottom),
        exclusive_zone: Some(-1),
        kb_interactivity: Some(KeyboardInteractivity::Exclusive),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(client);
    let layer = f.client(client).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(1920, 1080);
    layer.ack_last_and_commit();
    f.double_roundtrip(client);
    let output = f.niri_output(1);
    let layers = layer_map_for_output(&output);
    let mapped = layers
        .layers()
        .find(|layer| layer.namespace() == namespace)
        .unwrap()
        .clone();
    mapped
}

fn rendered_rows(f: &mut Fixture) -> Vec<Rectangle<i32, Physical>> {
    let output = f.niri_output(1);
    let state = f.niri_state();
    state.niri.update_render_elements(None);
    let layers = layer_map_for_output(&output);
    state
        .backend
        .headless()
        .with_primary_renderer(|renderer| {
            let mut result = Vec::new();
            state.niri.render_workspace_layers(
                RenderCtx {
                    renderer,
                    target: RenderTarget::Output,
                    xray: None,
                },
                &output,
                &layers,
                &mut |elem| result.push(elem.geometry(1.0.into())),
            );
            result
        })
        .unwrap()
}

#[test]
fn egl_workspace_layers_follow_live_gestures_and_reversed_swipes() {
    let config = Config::parse_mem("workspace \"first\"\nworkspace \"second\"").unwrap();
    let mut f = Fixture::with_config(config);
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (1920, 1080));
    let (first_index, first) = f
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, w)| w.name().map(String::as_str) == Some("first"))
        .map(|(_, i, w)| (i, w.id().get()))
        .unwrap();
    let (second_index, second) = f
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, w)| w.name().map(String::as_str) == Some("second"))
        .map(|(_, i, w)| (i, w.id().get()))
        .unwrap();
    f.niri().layout.switch_workspace(first_index);
    f.niri_complete_animations();
    let client = f.add_client();
    let first_layer = map_row(&mut f, client, first);
    let second_layer = map_row(&mut f, client, second);
    let output = f.niri_output(1);
    assert!(f.niri().workspace_layer_active(&first_layer));
    assert!(!f.niri().workspace_layer_active(&second_layer));
    assert_eq!(
        f.niri().seat.get_keyboard().unwrap().current_focus(),
        Some(first_layer.wl_surface().clone())
    );
    assert_eq!(rendered_rows(&mut f).len(), 1);

    // A fixed shell panel keeps its visual/input/focus priority over the row.
    let panel = f
        .client(client)
        .create_layer(None, Layer::Top, "system-panel");
    let panel_surface = panel.surface.clone();
    panel.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
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
    let hit = f.niri().contents_under((100.0, 10.0).into()).layer.unwrap();
    assert_eq!(hit.namespace(), "system-panel");
    assert_eq!(
        f.niri().seat.get_keyboard().unwrap().current_focus(),
        Some(hit.wl_surface().clone())
    );
    let panel = f.client(client).layer(&panel_surface);
    panel.attach_null();
    panel.commit();
    f.double_roundtrip(client);

    f.niri()
        .layout
        .workspace_switch_gesture_begin(&output, true);
    f.niri()
        .layout
        .workspace_switch_gesture_update(120.0, Duration::from_millis(20), true);
    // The active row has not changed, yet both rows already move with the fingers.
    assert_eq!(
        f.niri().layout.active_workspace().unwrap().id().get(),
        first
    );
    let geometry: Vec<_> = f
        .niri()
        .layout
        .monitor_for_output(&output)
        .unwrap()
        .workspaces_with_render_geo()
        .map(|(ws, geo)| (ws.id().get(), geo))
        .collect();
    assert_eq!(geometry.len(), 2);
    let render = rendered_rows(&mut f);
    assert_eq!(render.len(), 2);
    for (workspace, geo) in geometry {
        let point = Point::from((100.0, geo.loc.y.max(0.0) + 20.0));
        let expected = if workspace == first {
            &first_layer
        } else {
            &second_layer
        };
        assert_eq!(
            f.niri().contents_under(point).layer.as_ref(),
            Some(expected)
        );
        assert!(render.iter().any(|rect| rect.loc.y == geo.loc.y as i32));
    }
    f.niri()
        .layout
        .workspace_switch_gesture_update(-120.0, Duration::from_millis(40), true);
    f.niri().layout.workspace_switch_gesture_end(Some(true));
    f.niri_complete_animations();
    assert_eq!(
        f.niri().layout.active_workspace().unwrap().id().get(),
        first
    );
    assert_eq!(rendered_rows(&mut f).len(), 1);
    assert_eq!(
        f.niri()
            .contents_under((100.0, 100.0).into())
            .layer
            .as_ref(),
        Some(&first_layer)
    );

    f.niri().layout.switch_workspace(second_index);
    f.niri_complete_animations();
    f.double_roundtrip(client);
    assert_eq!(
        f.niri().seat.get_keyboard().unwrap().current_focus(),
        Some(second_layer.wl_surface().clone())
    );
    assert_eq!(
        f.niri()
            .contents_under((100.0, 100.0).into())
            .layer
            .as_ref(),
        Some(&second_layer)
    );
    assert_eq!(rendered_rows(&mut f).len(), 1);
}

#[test]
fn workspace_layers_reject_missing_rows_and_invalid_namespaces() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let client = f.add_client();
    for namespace in [
        "niri-workspace-v1:0:test",
        "niri-workspace-v1:42:",
        "niri-workspace-v1:18446744073709551615:test",
    ] {
        let layer = f
            .client(client)
            .create_layer(None, Layer::Overlay, namespace);
        let surface = layer.surface.clone();
        layer.set_configure_props(LayerConfigureProps {
            anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top | Anchor::Bottom),
            ..Default::default()
        });
        layer.commit();
        f.double_roundtrip(client);
        assert!(f.client(client).layer(&surface).close_requested);
    }
}
