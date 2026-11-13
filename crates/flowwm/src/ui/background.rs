//! Shared compositor blur for GTK panels and native tooltip surfaces.
use super::native;
use gtk::{gdk, glib, prelude::*};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_compositor, wl_region, wl_registry},
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};

#[derive(Default)]
struct State {
    blur: bool,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
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
impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            state.blur = matches!(flags, WEnum::Value(flags) if flags.contains(ext_background_effect_manager_v1::Capability::Blur));
        }
    }
}
delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_region::WlRegion);
delegate_noop!(State: ignore ExtBackgroundEffectSurfaceV1);

struct Client {
    manager: ExtBackgroundEffectManagerV1,
    compositor: wl_compositor::WlCompositor,
    events: RefCell<(EventQueue<State>, State)>,
    connection: Connection,
    _display: gdk::Display,
}
impl Client {
    fn new(display: &gdk::Display) -> Option<Rc<Self>> {
        let connection = native::connection(display)?;
        let (globals, queue) = registry_queue_init::<State>(&connection).ok()?;
        let manager = globals.bind(&queue.handle(), 1..=1, ()).ok()?;
        let compositor = globals.bind(&queue.handle(), 1..=1, ()).ok()?;
        let client = Self {
            manager,
            compositor,
            events: RefCell::new((queue, State::default())),
            connection,
            _display: display.clone(),
        };
        // Probe once per display, including the manager's advertised capabilities.
        {
            let (queue, state) = &mut *client.events.borrow_mut();
            queue.roundtrip(state).ok()?;
        }
        Some(Rc::new(client))
    }
    fn supported(&self) -> bool {
        let (queue, state) = &mut *self.events.borrow_mut();
        // GTK reads the display; subsequent frames only drain already queued events.
        let _ = queue.dispatch_pending(state);
        state.blur
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.manager.destroy();
    }
}
thread_local! {
    static CLIENTS: RefCell<HashMap<gdk::Display, Option<Rc<Client>>>> = RefCell::new(HashMap::new());
}

pub struct Style {
    pub enabled: bool,
    pub radius: f64,
    pub scale: f64,
}

pub fn gtk_blur() -> bool {
    // From 4.23.3, GDK owns the protocol object even without a backdrop filter.
    gtk::check_version(4, 23, 3).is_none()
}

struct Effect {
    surface: Option<ExtBackgroundEffectSurfaceV1>,
    client: Rc<Client>,
    last: Option<Vec<gdk::Rectangle>>,
    widget: glib::WeakRef<gtk::Widget>,
}
impl Drop for Effect {
    fn drop(&mut self) {
        if let Some(widget) = self.widget.upgrade() {
            widget.remove_css_class("flow-blur");
        }
        if let Some(surface) = &self.surface {
            surface.destroy();
        }
    }
}
impl Effect {
    fn new(surface: &gdk::Surface) -> Option<Self> {
        let display = surface.display();
        let client = CLIENTS.with(|clients| {
            clients
                .borrow_mut()
                .entry(display.clone())
                .or_insert_with(|| Client::new(&display))
                .clone()
        })?;
        let surface = if gtk_blur() {
            None
        } else {
            let wl_surface = native::surface(&client.connection, surface)?;
            Some(client.manager.get_background_effect(
                &wl_surface,
                &client.events.borrow().0.handle(),
                (),
            ))
        };
        Some(Self {
            surface,
            client,
            last: None,
            widget: glib::WeakRef::new(),
        })
    }
    fn update(&mut self, widget: &gtk::Widget, style: Style) {
        let Some(native) = widget.native() else {
            return;
        };
        let Ok(native_widget) = native.clone().dynamic_cast::<gtk::Widget>() else {
            return;
        };
        self.widget.set(Some(&native_widget));
        let supported = self.client.supported();
        if supported {
            native_widget.add_css_class("flow-blur");
        } else {
            native_widget.remove_css_class("flow-blur");
        }
        let Some(surface) = &self.surface else {
            return;
        };
        let rects = if style.enabled && supported {
            let Some(bounds) = widget.compute_bounds(&native_widget) else {
                return;
            };
            let (dx, dy) = native.surface_transform();
            let scale = style.scale;
            rounded_region(
                (bounds.x() as f64 * scale + native_widget.width() as f64 * (1. - scale) / 2. - dx)
                    .round() as i32,
                (bounds.y() as f64 * scale + native_widget.height() as f64 * (1. - scale) / 2. - dy)
                    .round() as i32,
                (bounds.width() as f64 * scale).round() as i32,
                (bounds.height() as f64 * scale).round() as i32,
                style.radius * scale,
            )
        } else {
            Vec::new()
        };
        if self.last.as_ref() == Some(&rects) {
            return;
        }
        if rects.is_empty() {
            surface.set_blur_region(None);
        } else {
            let region = self
                .client
                .compositor
                .create_region(&self.client.events.borrow().0.handle(), ());
            for rect in &rects {
                region.add(rect.x(), rect.y(), rect.width(), rect.height());
            }
            surface.set_blur_region(Some(&region));
            region.destroy();
        }
        self.last = Some(rects);
        // GTK commits the pending region together with its next rendered buffer.
    }
}

/// Observe scheduled frames only; hidden and idle surfaces request no frames.
pub fn attach(widget: &impl IsA<gtk::Widget>, style: impl Fn() -> Style + 'static) {
    let widget = widget.as_ref();
    let style = Rc::new(style);
    let active = Rc::new(RefCell::new(
        None::<(gdk::FrameClock, Vec<glib::SignalHandlerId>)>,
    ));
    let mapped = active.clone();
    widget.connect_map(move |widget| {
        let Some(surface) = widget.native().and_then(|native| native.surface()) else {
            return;
        };
        let Some(effect) = Effect::new(&surface) else {
            return;
        };
        let Some(clock) = widget.frame_clock() else {
            return;
        };
        let legacy = effect.surface.is_some();
        let effect = RefCell::new(effect);
        let weak = widget.downgrade();
        let style = style.clone();
        let refresh = Rc::new(move || {
            if let Some(widget) = weak.upgrade() {
                effect.borrow_mut().update(&widget, style());
            }
        });
        let layout = refresh.clone();
        let mut handlers = vec![clock.connect_local("layout", true, move |_| {
            layout();
            None
        })];
        if legacy {
            // Tick callbacks update the animated scale without requesting layout.
            handlers.push(clock.connect_local("update", true, move |_| {
                refresh();
                None
            }));
        }
        *mapped.borrow_mut() = Some((clock, handlers));
    });
    widget.connect_unmap(move |_| {
        if let Some((clock, handlers)) = active.borrow_mut().take() {
            for handler in handlers {
                clock.disconnect(handler);
            }
        }
    });
}

fn rounded_region(x: i32, y: i32, width: i32, height: i32, radius: f64) -> Vec<gdk::Rectangle> {
    if width <= 0 || height <= 0 {
        return Vec::new();
    }
    let radius = radius.clamp(0., width.min(height) as f64 / 2.);
    let mut rects: Vec<gdk::Rectangle> = Vec::new();
    for row in 0..height {
        let dy = (radius - (row.min(height - row - 1) as f64 + 0.5)).max(0.);
        let inset = (radius - (radius * radius - dy * dy).sqrt()).round() as i32;
        if let Some(last) = rects.last_mut()
            && last.x() == x + inset
        {
            last.set_height(last.height() + 1);
        } else {
            rects.push(gdk::Rectangle::new(
                x + inset,
                y + row,
                width - 2 * inset,
                1,
            ));
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_region_rounds_corners_and_clamps_small_or_empty_surfaces() {
        for (width, height, radius) in [(520, 560, 16.), (25, 13, 50.), (30, 40, 0.)] {
            let rects = rounded_region(20, 30, width, height, radius);
            assert_eq!(rects.iter().map(|r| r.height()).sum::<i32>(), height);
            assert!(
                rects
                    .iter()
                    .all(|r| r.x() >= 20 && r.width() > 0 && r.x() + r.width() <= 20 + width)
            );
            assert_eq!(rects.first().unwrap().y(), 30);
            assert_eq!(
                rects.last().unwrap().y() + rects.last().unwrap().height(),
                30 + height
            );
            assert_eq!(
                rects.first().unwrap().width(),
                rects.last().unwrap().width()
            );
            if radius == 0. {
                assert_eq!(rects, [gdk::Rectangle::new(20, 30, width, height)]);
            } else {
                assert!(rects.first().unwrap().x() > 20);
                assert!(rects.iter().any(|r| r.width() == width));
            }
        }
        assert!(rounded_region(0, 0, 0, 40, 16.).is_empty());
    }
}
