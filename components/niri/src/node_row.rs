//! A node canvas is workspace content, not a layer above the window layout.
//! Window tiles retain their normal sizing but use live client buffers in Nodes.
use niri_ipc::node_row::{self, NodeRowScene};
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::{PopupManager, Window, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::layout::workspace::{Workspace, WorkspaceRenderElement};
use crate::layout::{HitType, LayoutElement, LayoutElementRenderElement};
use crate::niri::{Niri, State};
use crate::niri_render_elements;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::surface::push_elements_from_surface_tree;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::utils::{is_mapped, send_scale_transform};

#[derive(Debug)]
pub(crate) struct NodeTile {
    pub canvas: Window,
    pub scene: NodeRowScene,
    pub mapped: bool,
    pub graph_focused: bool,
    configured_size: Size<i32, Logical>,
}

niri_render_elements! {
    NodeRenderElement<R> => {
        Canvas = smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<R>,
        Window = CropRenderElement<RelocateRenderElement<RescaleRenderElement<LayoutElementRenderElement<R>>>>,
        Popup = LayoutElementRenderElement<R>,
        Frame = crate::layout::tile::TileRenderElement<R>,
    }
}

impl<W: LayoutElement> Workspace<W> {
    pub fn nodes_visible(&self) -> bool {
        self.node_tile.as_ref().is_some_and(|t| t.mapped)
    }

    pub(crate) fn node_canvas_geometry(&self) -> Rectangle<f64, Logical> {
        let [top, right, bottom, left] = self
            .node_tile
            .as_ref()
            .map(|t| t.scene.insets)
            .unwrap_or_default();
        let area = self.working_area();
        let size = self.view_size();
        let x = area.loc.x.max(left as f64);
        let y = area.loc.y.max(top as f64);
        let end_x = (area.loc.x + area.size.w).min(size.w - right as f64);
        let end_y = (area.loc.y + area.size.h).min(size.h - bottom as f64);
        Rectangle::new(
            (x, y).into(),
            ((end_x - x).max(1.), (end_y - y).max(1.)).into(),
        )
    }

    pub(crate) fn refresh_node_tile(&mut self) {
        let geometry = self.node_canvas_geometry();
        let size = geometry.size.to_i32_round();
        let output = self.current_output().cloned();
        let scale = self.scale();
        let Some(tile) = self.node_tile.as_mut() else {
            return;
        };
        let top = tile.canvas.toplevel().unwrap();
        if tile.configured_size != size {
            tile.configured_size = size;
            top.with_pending_state(|state| {
                state.size = Some(size);
                state.bounds = Some(size);
            });
        }
        top.send_pending_configure();
        if let Some(output) = output {
            tile.canvas
                .output_enter(&output, Rectangle::from_size(size));
            with_states(top.wl_surface(), |states| {
                send_scale_transform(top.wl_surface(), states, scale, output.current_transform())
            });
        }
    }

    pub(crate) fn node_window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        let tile = self.node_tile.as_ref()?;
        let origin = self.node_canvas_geometry().loc;
        for entry in tile.scene.windows.iter().rev().filter(|w| w.expanded) {
            let (x, y, width, height) = entry.bounds;
            let rect = Rectangle::new(origin + Point::from((x, y)), (width, height).into());
            let Some(window) = self
                .windows()
                .find(|w| w.node_window_id() == Some(entry.id))
            else {
                continue;
            };
            if window.is_in_input_region(pos - rect.loc) {
                return Some((window, HitType::Input { win_pos: rect.loc }));
            }
        }
        None
    }

    pub(crate) fn render_nodes<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray: XrayPos,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        let Some(tile) = &self.node_tile else { return };
        let scale = Scale::from(self.scale().fractional_scale());
        let geometry = self.node_canvas_geometry();
        let canvas = tile.canvas.toplevel().unwrap().wl_surface();
        for entry in tile.scene.windows.iter().rev().filter(|w| w.expanded) {
            if let Some(window) = self
                .windows()
                .find(|w| w.node_window_id() == Some(entry.id))
            {
                let loc = geometry.loc + Point::from((entry.bounds.0, entry.bounds.1));
                window.render_popups(ctx.r(), loc, scale, 1., xray.offset(loc), &mut |elem| {
                    push(NodeRenderElement::Popup(elem).into())
                });
            }
        }
        for (popup, offset) in PopupManager::popups_for_surface(canvas) {
            let loc = geometry.loc + (offset - popup.geometry().loc).to_f64();
            push_elements_from_surface_tree(
                ctx.renderer,
                popup.wl_surface(),
                loc.to_physical_precise_round(scale),
                scale,
                1.,
                Kind::Unspecified,
                &mut |elem| push(NodeRenderElement::Canvas(elem).into()),
            );
        }
        for entry in tile.scene.windows.iter().filter(|w| w.expanded) {
            if let Some(tile) = self
                .tiles()
                .find(|tile| tile.window().node_window_id() == Some(entry.id))
            {
                if tile.window().sizing_mode().is_normal() {
                    let loc = geometry.loc + Point::from((entry.bounds.0, entry.bounds.1))
                        - tile.window_loc();
                    tile.render_frame(ctx.renderer, loc, true, &mut |elem| {
                        push(NodeRenderElement::Frame(elem).into())
                    });
                }
            }
        }
        // Decorations and graph gestures are above the client buffers; the canvas has preview holes.
        let loc = geometry.loc - tile.canvas.geometry().loc.to_f64();
        push_elements_from_surface_tree(
            ctx.renderer,
            canvas,
            loc.to_physical_precise_round(scale),
            scale,
            1.,
            Kind::Unspecified,
            &mut |elem| push(NodeRenderElement::Canvas(elem).into()),
        );
        for entry in tile.scene.windows.iter().rev() {
            let Some(window) = self
                .windows()
                .find(|w| w.node_window_id() == Some(entry.id))
            else {
                continue;
            };
            let size = window.size().to_f64();
            if size.w <= 0. || size.h <= 0. {
                continue;
            }
            let (x, y, width, height) = entry.bounds;
            let clip = Rectangle::new(geometry.loc + Point::from((x, y)), (width, height).into());
            if !clip.overlaps(Rectangle::from_size(self.view_size())) {
                continue;
            }
            let zoom = if entry.expanded {
                1.
            } else {
                (width / size.w).min(height / size.h)
            };
            let loc = geometry.loc
                + Point::from((
                    x + (width - size.w * zoom) / 2.,
                    y + (height - size.h * zoom) / 2.,
                ));
            let bounds = Rectangle::new(loc, size.upscale(zoom)).to_physical_precise_round(scale);
            window.set_offscreen_data(None);
            window.render_normal(
                ctx.r(),
                Point::default(),
                scale,
                entry.opacity,
                &mut |elem| {
                    let elem = RescaleRenderElement::from_element(elem, Point::default(), zoom);
                    let elem = RelocateRenderElement::from_element(
                        elem,
                        loc.to_physical_precise_round(scale),
                        Relocate::Relative,
                    );
                    if let Some(elem) = CropRenderElement::from_element(elem, scale, bounds) {
                        push(NodeRenderElement::Window(elem).into());
                    }
                },
            );
        }
    }
}

impl Niri {
    pub(crate) fn stage_node_row(&mut self, id: u64, scene: NodeRowScene) -> Result<(), String> {
        let ws = self
            .layout
            .workspaces_mut()
            .find(|ws| ws.id().get() == id)
            .ok_or("node workspace no longer exists")?;
        ws.pending_node_scene = Some(scene);
        Ok(())
    }

    pub(crate) fn remove_node_canvas(&mut self, surface: &WlSurface) -> bool {
        let mut removed = false;
        for ws in self.layout.workspaces_mut() {
            if ws
                .node_tile
                .as_ref()
                .is_some_and(|t| t.canvas.wl_surface().as_deref() == Some(surface))
            {
                ws.node_tile = None;
                ws.pending_node_scene = None;
                removed = true;
                break;
            }
        }
        if removed {
            self.queue_redraw_all();
        }
        removed
    }

    pub(crate) fn node_canvas_focus(&self) -> Option<WlSurface> {
        if self.layout.is_overview_open() {
            return None;
        }
        let ws = self.layout.active_workspace()?;
        let tile = ws.node_tile.as_ref().filter(|t| t.mapped)?;
        let focused = self.layout.focus().and_then(|w| w.node_window_id());
        if !tile.graph_focused
            && tile
                .scene
                .windows
                .iter()
                .any(|w| w.expanded && Some(w.id) == focused)
        {
            return None;
        }
        Some(tile.canvas.toplevel().unwrap().wl_surface().clone())
    }

    pub(crate) fn focus_node_canvas(&mut self, surface: Option<&WlSurface>) {
        for ws in self.layout.workspaces_mut() {
            if let Some(tile) = &mut ws.node_tile {
                tile.graph_focused = surface == Some(tile.canvas.toplevel().unwrap().wl_surface());
            }
        }
    }

    pub(crate) fn node_canvas_under(
        &self,
        output: &Output,
        point: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.layout.is_overview_open() {
            return None;
        }
        let mon = self.layout.monitor_for_output(output)?;
        let (ws, geo) = mon
            .workspaces_with_render_geo()
            .find(|(_, geo)| geo.contains(point))?;
        let tile = ws.node_tile.as_ref().filter(|t| t.mapped)?;
        let loc = geo.loc + ws.node_canvas_geometry().loc;
        tile.canvas
            .surface_under(point - loc, WindowSurfaceType::ALL)
            .map(|(s, offset)| (s, loc + offset.to_f64()))
    }
}

impl State {
    pub(crate) fn node_row_handle_commit(&mut self, surface: &WlSurface, root: &WlSurface) -> bool {
        let known = self.niri.layout.workspaces().any(|(_, _, ws)| {
            ws.node_tile
                .as_ref()
                .is_some_and(|t| t.canvas.wl_surface().as_deref() == Some(root))
        });
        if !known {
            if surface != root {
                return false;
            }
            let Some(unmapped) = self.niri.unmapped_windows.get(root) else {
                return false;
            };
            let top = unmapped.window.toplevel().unwrap();
            let app_id = with_states(root, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .app_id
                    .clone()
            });
            let Some(app_id) = app_id.filter(|id| id.starts_with(node_row::PREFIX)) else {
                return false;
            };
            let Some(id) = node_row::workspace_id(&app_id) else {
                top.send_close();
                return true;
            };
            let valid = self
                .niri
                .layout
                .workspaces()
                .any(|(_, _, ws)| ws.id().get() == id && ws.node_tile.is_none());
            if !valid {
                top.send_close();
                return true;
            }
            let unmapped = self.niri.unmapped_windows.remove(root).unwrap();
            let ws = self
                .niri
                .layout
                .workspaces_mut()
                .find(|ws| ws.id().get() == id)
                .unwrap();
            ws.node_tile = Some(NodeTile {
                canvas: unmapped.window,
                scene: NodeRowScene::default(),
                mapped: false,
                graph_focused: true,
                configured_size: Size::default(),
            });
            ws.refresh_node_tile();
            ws.node_tile
                .as_ref()
                .unwrap()
                .canvas
                .toplevel()
                .unwrap()
                .send_pending_configure();
            self.niri.layout.ensure_node_row_space();
        }
        if surface == root {
            let ws = self
                .niri
                .layout
                .workspaces_mut()
                .find(|ws| {
                    ws.node_tile
                        .as_ref()
                        .is_some_and(|t| t.canvas.wl_surface().as_deref() == Some(root))
                })
                .unwrap();
            let tile = ws.node_tile.as_mut().unwrap();
            tile.canvas.on_commit();
            let was_mapped = tile.mapped;
            tile.mapped = is_mapped(root);
            if was_mapped && !tile.mapped {
                tile.configured_size = Size::default();
            }
            if let Some(scene) = ws.pending_node_scene.take() {
                tile.scene = scene;
            }
            ws.refresh_node_tile();
        }
        if let Some(output) = self.niri.output_for_root(root).cloned() {
            self.niri.queue_redraw(&output);
        }
        true
    }
}
