//! Render extension GLSL into cached node-local pixels, independent of GSK's backend.
use super::theme::Theme;
use crate::model::Node;
use anyhow::{Context, Result, bail, ensure};
use flow_core::domain::{RendererContract, ShaderInput};
use glow::HasContext;
use gtk::{cairo, gdk, prelude::*};
use std::{cell::RefCell, collections::HashMap, ffi::c_void};

const VERTEX: &str = "#version 300 es\nprecision highp float;\nout vec2 flow_uv;\nvoid main() {\nvec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));\nflow_uv = vec2(p.x, 1.0 - p.y);\ngl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);\n}";
const PRELUDE: &str = "#version 300 es\nprecision highp float;\nin vec2 flow_uv;\nout vec4 flow_color;\nuniform vec2 flow_size;\nuniform vec2 flow_resolution;\nuniform vec4 flow_surface;\nuniform vec4 flow_primary;\nuniform vec4 flow_text;\nuniform float flow_status;\n";

type Pixels = Result<cairo::ImageSurface, String>;
#[derive(Default)]
struct Cache {
    engine: Option<Result<Engine, String>>,
    images: HashMap<String, (Vec<u8>, Pixels)>,
}
thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::default()); }

/// Restore GTK's context even when compilation, allocation or a uniform fails.
struct Current(Option<gdk::GLContext>);
impl Current {
    fn enter(context: &gdk::GLContext) -> Self {
        let previous = gdk::GLContext::current();
        context.make_current();
        Self(previous)
    }
}
impl Drop for Current {
    fn drop(&mut self) {
        if let Some(previous) = &self.0 {
            previous.make_current();
        } else {
            gdk::GLContext::clear_current();
        }
    }
}

struct Engine {
    context: gdk::GLContext,
    gl: glow::Context,
    // Epoxy is GTK's GL dispatch library. Its exported symbols are function pointers.
    _library: libloading::Library,
    programs: HashMap<String, Result<glow::Program, String>>,
}
impl Engine {
    fn new() -> Result<Self> {
        let display = gdk::Display::default().context("No display for node shaders")?;
        let context = display.create_gl_context()?;
        context.set_allowed_apis(gdk::GLAPI::GLES);
        context.set_required_version(3, 0);
        context
            .realize()
            .context("Node shaders require OpenGL ES 3.0")?;
        let _current = Current::enter(&context);
        // SAFETY: Epoxy lives for the GL dispatch table's entire lifetime. The
        // GDK context is current and every call stays on GTK's main thread.
        let (library, gl) = unsafe {
            let library = libloading::Library::new("libepoxy.so.0")?;
            let gl = glow::Context::from_loader_function(|name| {
                library
                    .get::<*const *const c_void>(format!("epoxy_{name}\0").as_bytes())
                    .map(|symbol| **symbol)
                    .unwrap_or(std::ptr::null())
            });
            (library, gl)
        };
        Ok(Self {
            context,
            gl,
            _library: library,
            programs: HashMap::new(),
        })
    }

    fn compile(&self, source: &str) -> Result<glow::Program> {
        // SAFETY: Called only with this engine's context current. Each GL
        // object is deleted on error; successful programs are owned by Engine.
        unsafe {
            let gl = &self.gl;
            let program = gl.create_program().map_err(anyhow::Error::msg)?;
            for (stage, source) in [
                (glow::VERTEX_SHADER, VERTEX),
                (glow::FRAGMENT_SHADER, source),
            ] {
                let shader = match gl.create_shader(stage) {
                    Ok(shader) => shader,
                    Err(error) => {
                        gl.delete_program(program);
                        bail!("{error}");
                    }
                };
                gl.shader_source(shader, source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let error = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    gl.delete_program(program);
                    bail!("Shader compilation: {error}");
                }
                gl.attach_shader(program, shader);
                gl.delete_shader(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                let error = gl.get_program_info_log(program);
                gl.delete_program(program);
                bail!("Shader linking: {error}");
            }
            Ok(program)
        }
    }

    fn render(
        &mut self,
        renderer: &RendererContract,
        uniforms: &[(String, f32)],
        theme: &Theme,
        status: f32,
        width: i32,
        height: i32,
    ) -> Result<cairo::ImageSurface> {
        let _current = Current::enter(&self.context);
        let mut source = PRELUDE.to_owned();
        for name in renderer.uniforms.keys() {
            source.push_str(&format!("uniform float {name};\n"));
        }
        source.push_str("#line 1\n");
        source.push_str(&renderer.fragment);
        source.push_str("\nvoid main() { flow_color = flow_draw(flow_uv); }\n");
        if !self.programs.contains_key(&source) {
            if self.programs.len() >= 32 {
                self.clear_programs();
            }
            let compiled = self.compile(&source).map_err(|e| e.to_string());
            self.programs.insert(source.clone(), compiled);
        }
        let program = *self.programs[&source]
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // SAFETY: All objects belong to the current context. The pixel buffer
        // has exactly width*height*4 bytes. Resources are released on every path.
        unsafe {
            let gl = &self.gl;
            let texture = gl.create_texture().map_err(anyhow::Error::msg)?;
            let framebuffer = match gl.create_framebuffer() {
                Ok(value) => value,
                Err(error) => {
                    gl.delete_texture(texture);
                    bail!("{error}");
                }
            };
            let result = (|| -> Result<Vec<u8>> {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    width,
                    height,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::NEAREST as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::NEAREST as i32,
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                ensure!(
                    gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE,
                    "Cannot allocate node shader framebuffer"
                );
                gl.viewport(0, 0, width, height);
                gl.use_program(Some(program));
                gl.uniform_2_f32(
                    gl.get_uniform_location(program, "flow_size").as_ref(),
                    renderer.size[0] as f32,
                    renderer.size[1] as f32,
                );
                gl.uniform_2_f32(
                    gl.get_uniform_location(program, "flow_resolution").as_ref(),
                    width as f32,
                    height as f32,
                );
                for (name, color) in [
                    (
                        "flow_surface",
                        theme.node_background(crate::model::Status::Idle),
                    ),
                    ("flow_primary", theme.primary),
                    ("flow_text", theme.text),
                ] {
                    gl.uniform_4_f32(
                        gl.get_uniform_location(program, name).as_ref(),
                        color.0 as f32,
                        color.1 as f32,
                        color.2 as f32,
                        1.,
                    );
                }
                gl.uniform_1_f32(
                    gl.get_uniform_location(program, "flow_status").as_ref(),
                    status,
                );
                for (name, value) in uniforms {
                    gl.uniform_1_f32(gl.get_uniform_location(program, name).as_ref(), *value);
                }
                let vao = gl.create_vertex_array().map_err(anyhow::Error::msg)?;
                gl.bind_vertex_array(Some(vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                gl.delete_vertex_array(vao);
                let mut pixels = vec![0; width as usize * height as usize * 4];
                gl.read_pixels(
                    0,
                    0,
                    width,
                    height,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixels)),
                );
                let error = gl.get_error();
                ensure!(error == glow::NO_ERROR, "Node shader GL error: {error:#x}");
                Ok(pixels)
            })();
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.use_program(None);
            gl.delete_framebuffer(framebuffer);
            gl.delete_texture(texture);
            let pixels = result?;
            let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)?;
            let stride = surface.stride() as usize;
            {
                let mut data = surface.data()?;
                for y in 0..height as usize {
                    for x in 0..width as usize {
                        let from = ((height as usize - 1 - y) * width as usize + x) * 4;
                        let rgba = &pixels[from..from + 4];
                        let alpha = rgba[3] as u32;
                        let premul = |c: u8| (c as u32 * alpha + 127) / 255;
                        let argb = (alpha << 24)
                            | (premul(rgba[0]) << 16)
                            | (premul(rgba[1]) << 8)
                            | premul(rgba[2]);
                        let to = y * stride + x * 4;
                        data[to..to + 4].copy_from_slice(&argb.to_ne_bytes());
                    }
                }
            }
            Ok(surface)
        }
    }
    fn clear_programs(&mut self) {
        for (_, program) in self.programs.drain() {
            if let Ok(program) = program {
                // SAFETY: callers make this context current before deleting.
                unsafe {
                    self.gl.delete_program(program);
                }
            }
        }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let _current = Current::enter(&self.context);
        self.clear_programs();
    }
}

fn inputs(node: &Node, renderer: &RendererContract) -> Result<Vec<(String, f32)>> {
    renderer
        .uniforms
        .iter()
        .map(|(uniform, binding)| {
            let value = match binding {
                ShaderInput::Constant { value } => *value,
                ShaderInput::Property { name } | ShaderInput::Output { name } => {
                    let record = if matches!(binding, ShaderInput::Property { .. }) {
                        &node.flow.properties
                    } else {
                        &node.flow.values
                    };
                    let value = record
                        .get(name)
                        .with_context(|| format!("Shader input {name} is unavailable"))?;
                    if let Some(value) = value.as_bool() {
                        f64::from(value)
                    } else {
                        value.as_f64().with_context(|| {
                            format!("Shader input {name} must be a number or Boolean")
                        })?
                    }
                }
            } as f32;
            ensure!(
                value.is_finite(),
                "Shader uniform {uniform} must fit a finite float"
            );
            Ok((uniform.clone(), value))
        })
        .collect()
}

pub fn image(node: &Node, renderer: &RendererContract, theme: &Theme, scale: f64) -> Pixels {
    let scale = scale.clamp(0.125, 4.);
    let width = (renderer.size[0] * scale).ceil() as i32;
    let height = (renderer.size[1] * scale).ceil() as i32;
    let key = serde_json::to_vec(&(
        renderer,
        &node.flow.properties,
        &node.flow.values,
        node.status,
        theme.surface_high.rgb(),
        theme.primary.rgb(),
        theme.text.rgb(),
        width,
        height,
    ))
    .expect("shader cache key");
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((old, image)) = cache.images.get(&node.id)
            && *old == key
        {
            return image.clone();
        }
        let image = (|| -> Result<_> {
            renderer.validate()?;
            let uniforms = inputs(node, renderer)?;
            let engine = cache
                .engine
                .get_or_insert_with(|| Engine::new().map_err(|e| e.to_string()));
            let engine = engine.as_mut().map_err(|e| anyhow::anyhow!("{e}"))?;
            engine.render(
                renderer,
                &uniforms,
                theme,
                node.status as u8 as f32,
                width,
                height,
            )
        })()
        .map_err(|e| format!("{e:#}"));
        if let Err(error) = &image {
            eprintln!("Node {} shader: {error}", node.id);
        }
        let bytes = |image: &Pixels| {
            image
                .as_ref()
                .map_or(0, |image| image.stride() as usize * image.height() as usize)
        };
        let retained_bytes: usize = cache.images.values().map(|(_, image)| bytes(image)).sum();
        if cache.images.len() >= 128 || retained_bytes + bytes(&image) > 64 * 1024 * 1024 {
            cache.images.clear();
        }
        cache.images.insert(node.id.clone(), (key, image.clone()));
        image
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeKind, Position};
    use serde_json::json;

    fn pixel(image: &cairo::ImageSurface, x: usize, y: usize) -> u32 {
        let mut copy =
            cairo::ImageSurface::create(cairo::Format::ARgb32, image.width(), image.height())
                .unwrap();
        {
            let c = cairo::Context::new(&copy).unwrap();
            c.set_source_surface(image, 0., 0.).unwrap();
            c.paint().unwrap();
        }
        let stride = copy.stride() as usize;
        let data = copy.data().unwrap();
        let offset = y * stride + x * 4;
        u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    #[ignore = "requires a GTK display and GLES 3; run with smoke_rendering.py"]
    fn shaders_render_live_uniforms_alpha_and_errors_without_changing_the_gtk_context() {
        gtk::init().unwrap();
        let mut node = Node::new("test", NodeKind::Extension, ".".into(), Position::default());
        let renderer: RendererContract = serde_json::from_value(json!({
            "size":[128,96],
            "fragment":"vec4 flow_draw(vec2 uv) { return vec4(u_red, uv.y, u_blue, 0.5); }",
            "uniforms":{"u_red":{"source":"property","name":"red"},"u_blue":{"source":"output","name":"blue"}}
        })).unwrap();
        node.flow.properties.insert("red".into(), json!(1.));
        node.flow.values.insert("blue".into(), json!(0.));
        let current = gdk::Display::default()
            .unwrap()
            .create_gl_context()
            .unwrap();
        current.realize().unwrap();
        current.make_current();
        let theme = Theme::default();
        let first = image(&node, &renderer, &theme, 2.).unwrap();
        assert_eq!((first.width(), first.height()), (256, 192));
        let top = pixel(&first, 128, 2);
        let bottom = pixel(&first, 128, 189);
        assert_eq!(top >> 24, 128);
        assert_eq!((top >> 16) & 255, 128);
        assert!(
            bottom & 0xff00 > top & 0xff00,
            "UV origin must be the top left"
        );
        assert_eq!(gdk::GLContext::current(), Some(current.clone()));
        assert_eq!(
            image(&node, &renderer, &theme, 2.).unwrap().to_raw_none(),
            first.to_raw_none(),
            "Reuse unchanged shader pixels"
        );
        node.flow.properties.insert("red".into(), json!(false));
        node.flow.values.insert("blue".into(), json!(1.));
        let changed = image(&node, &renderer, &theme, 2.).unwrap();
        assert_eq!(pixel(&changed, 128, 2) & 0xff00ff, 128);
        let mut broken = renderer.clone();
        broken.fragment = "not valid GLSL".into();
        assert!(
            image(&node, &broken, &theme, 1.)
                .unwrap_err()
                .contains("compilation")
        );
        assert_eq!(gdk::GLContext::current(), Some(current.clone()));
        node.flow.values.clear();
        assert!(
            image(&node, &renderer, &theme, 1.)
                .unwrap_err()
                .contains("blue is unavailable")
        );
        gdk::GLContext::clear_current();
    }
}
