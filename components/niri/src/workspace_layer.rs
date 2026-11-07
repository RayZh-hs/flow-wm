use niri_ipc::workspace_layer;
use smithay::desktop::{LayerMap, LayerSurface, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::niri::{scale_relocate_crop, Niri, OutputRenderElements};
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;

pub(crate) struct WorkspaceLayerHit {
    pub surface: WlSurface,
    pub position: Point<f64, Logical>,
    pub layer: LayerSurface,
}

impl Niri {
    pub(crate) fn workspace_layer_under(
        &self,
        output: &Output,
        point: Point<f64, Logical>,
        layers: &LayerMap,
    ) -> Option<WorkspaceLayerHit> {
        for layer in layers.layers_on(Layer::Overlay).rev() {
            if workspace_layer::workspace_id(layer.namespace()).is_none() {
                continue;
            }
            let Some(mapped) = self.mapped_layer_surfaces.get(layer) else {
                continue;
            };
            let Some(offset) = self.workspace_layer_offset(layer, output, point) else {
                continue;
            };
            let loc = layers.layer_geometry(layer)?.loc.to_f64() + offset + mapped.bob_offset();
            if let Some((surface, offset)) =
                layer.surface_under(point - loc, WindowSurfaceType::ALL)
            {
                return Some(WorkspaceLayerHit {
                    surface,
                    position: offset.to_f64() + loc,
                    layer: layer.clone(),
                });
            }
        }
        None
    }

    pub(crate) fn workspace_layer_active(&self, layer: &LayerSurface) -> bool {
        let Some(id) = workspace_layer::workspace_id(layer.namespace()) else {
            return true;
        };
        !self.layout.is_overview_open()
            && self
                .layout
                .active_workspace()
                .is_some_and(|ws| ws.id().get() == id)
    }

    pub(crate) fn workspace_layer_offset(
        &self,
        layer: &LayerSurface,
        output: &Output,
        point: Point<f64, Logical>,
    ) -> Option<Point<f64, Logical>> {
        let Some(id) = workspace_layer::workspace_id(layer.namespace()) else {
            return Some(Point::default());
        };
        if self.layout.is_overview_open() {
            return None;
        }
        let mon = self.layout.monitor_for_output(output)?;
        mon.workspaces_with_render_geo()
            .find(|(ws, geo)| ws.id().get() == id && geo.contains(point))
            .map(|(_, geo)| geo.loc)
    }

    pub(crate) fn render_workspace_layers<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        output: &Output,
        layers: &LayerMap,
        push: &mut dyn FnMut(OutputRenderElements<R>),
    ) {
        let Some(mon) = self.layout.monitor_for_output(output) else {
            return;
        };
        let scale = Scale::from(output.current_scale().fractional_scale());
        let zoom = mon.overview_zoom();
        for (ws, geo) in mon.workspaces_with_render_geo() {
            for surface in layers.layers_on(Layer::Overlay).rev() {
                if workspace_layer::workspace_id(surface.namespace()) != Some(ws.id().get()) {
                    continue;
                }
                let Some(mapped) = self.mapped_layer_surfaces.get(surface) else {
                    continue;
                };
                let Some(layer_geo) = layers.layer_geometry(surface) else {
                    continue;
                };
                let loc = layer_geo.loc.to_f64();
                let xray = XrayPos::new(geo.loc, zoom).offset(loc);
                let mut transformed = |elem| {
                    if let Some(elem) = scale_relocate_crop(elem, scale, zoom, geo) {
                        push(elem.into());
                    }
                };
                mapped.render_popups(ctx.r(), None, loc, xray, &mut transformed);
                mapped.render_normal(ctx.r(), None, loc, xray, &mut transformed);
            }
        }
    }
}
