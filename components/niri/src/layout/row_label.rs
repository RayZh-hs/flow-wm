use std::cell::RefCell;

use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::{EllipsizeMode, FontDescription};
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Physical, Point, Transform};

use crate::render_helpers::renderer::NiriRenderer;

#[derive(Debug, Default)]
pub(super) struct RowLabel {
    cached: RefCell<Option<Cached>>,
}

#[derive(Debug)]
struct Cached {
    text: String,
    scale: i32,
    width: i32,
    buffer: Option<MemoryRenderBuffer>,
}

impl RowLabel {
    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        text: &str,
        scale: f64,
        width: f64,
        location: Point<i32, Physical>,
        alpha: f32,
    ) -> Option<MemoryRenderBufferRenderElement<R>> {
        let scale = scale.ceil() as i32;
        let width = width.floor().max(1.) as i32;
        // A single bounded cache follows renames, output scale and available width.
        let text: String = text.chars().take(512).collect();
        let mut cached = self.cached.borrow_mut();
        if cached
            .as_ref()
            .is_none_or(|c| c.text != text || c.scale != scale || c.width != width)
        {
            let buffer = render_buffer(&text, scale, width)
                .inspect_err(|err| tracing::warn!("error rendering row name: {err:?}"))
                .ok();
            *cached = Some(Cached {
                text,
                scale,
                width,
                buffer,
            });
        }
        let buffer = cached.as_ref()?.buffer.as_ref()?;
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location.to_f64(),
            buffer,
            Some(alpha),
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
    }
}

fn render_buffer(text: &str, scale: i32, max_width: i32) -> anyhow::Result<MemoryRenderBuffer> {
    let padding = 8 * scale;
    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    let mut font = FontDescription::from_string("sans medium 16px");
    font.set_absolute_size((16 * scale * pangocairo::pango::SCALE) as f64);
    layout.set_font_description(Some(&font));
    layout.set_single_paragraph_mode(true);
    layout.set_ellipsize(EllipsizeMode::End);
    layout.set_width(((max_width * scale - padding * 2).max(1)) * pangocairo::pango::SCALE);
    layout.set_text(text);
    let (width, height) = layout.pixel_size();
    let width = width + padding * 2;
    let height = height + padding * 2;
    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    let radius = f64::from(padding);
    for (x, y, angle) in [
        (f64::from(width) - radius, radius, -90.),
        (f64::from(width) - radius, f64::from(height) - radius, 0.),
        (radius, f64::from(height) - radius, 90.),
        (radius, radius, 180.),
    ] {
        cr.arc(
            x,
            y,
            radius,
            angle * std::f64::consts::PI / 180.,
            (angle + 90.) * std::f64::consts::PI / 180.,
        );
    }
    cr.close_path();
    cr.set_source_rgba(0.08, 0.08, 0.08, 0.9);
    cr.fill()?;
    cr.move_to(radius, radius);
    cr.set_source_rgb(1., 1., 1.);
    pangocairo::functions::update_layout(&cr, &layout);
    pangocairo::functions::show_layout(&cr, &layout);
    drop(cr);
    let data = surface.take_data()?;
    Ok(MemoryRenderBuffer::from_slice(
        &data,
        Fourcc::Argb8888,
        (width, height),
        scale,
        Transform::Normal,
        None,
    ))
}
