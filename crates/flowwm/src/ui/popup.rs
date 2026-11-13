//! A popup stays mapped through its exit transition and can reverse while closing.
use super::theme::Theme;
use gtk::{gdk, glib, graphene, prelude::*, subclass::prelude::*};
use gtk4_layer_shell::{Edge, LayerShell};
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Popup {
        pub progress: Cell<f64>,
        pub shown: Cell<bool>,
        pub open_ms: Cell<u32>,
        pub close_ms: Cell<u32>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
        pub body: glib::WeakRef<gtk::Widget>,
        pub backdrop: RefCell<Option<gdk::RGBA>>,
        pub escaping_popover: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Popup {
        const NAME: &'static str = "FlowPopup";
        type Type = super::Popup;
        type ParentType = gtk::Box;
    }
    impl ObjectImpl for Popup {
        fn dispose(&self) {
            if let Some(tick) = self.tick.borrow_mut().take() {
                tick.remove();
            }
        }
    }
    impl BoxImpl for Popup {}
    impl WidgetImpl for Popup {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(body) = self.body.upgrade() {
                body.set_width_request((width - 24).clamp(1, 520));
            }
            self.parent_size_allocate(width, height, baseline);
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let progress = self.progress.get();
            if let Some(backdrop) = self.backdrop.borrow().as_ref() {
                let mut color = *backdrop;
                color.set_alpha(color.alpha() * progress as f32);
                snapshot.append_color(
                    &color,
                    &graphene::Rect::new(0., 0., obj.width() as f32, obj.height() as f32),
                );
            }
            let scale = (0.96 + 0.04 * progress) as f32;
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                obj.width() as f32 * (1. - scale) / 2.,
                obj.height() as f32 * (1. - scale) / 2.,
            ));
            snapshot.scale(scale, scale);
            snapshot.push_opacity(progress);
            self.parent_snapshot(snapshot);
            snapshot.pop();
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    pub struct Popup(ObjectSubclass<imp::Popup>) @extends gtk::Widget, gtk::Box,
    @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Popup {
    pub fn attach(window: &gtk::Window, body: &impl IsA<gtk::Widget>, theme: &Theme) {
        let popup: Self = glib::Object::builder()
            .property("orientation", gtk::Orientation::Vertical)
            .build();
        popup.imp().open_ms.set(theme.popup_open_ms);
        popup.imp().close_ms.set(theme.popup_close_ms);
        popup.imp().body.set(Some(body.as_ref()));
        *popup.imp().backdrop.borrow_mut() = Some(gdk::RGBA::new(
            theme.backdrop.0 as f32,
            theme.backdrop.1 as f32,
            theme.backdrop.2 as f32,
            theme.backdrop_opacity as f32,
        ));
        let overlay = gtk::Overlay::new();
        overlay.set_vexpand(true);
        let background = gtk::Box::new(gtk::Orientation::Vertical, 0);
        overlay.set_child(Some(&background));
        body.set_halign(gtk::Align::Center);
        body.set_valign(gtk::Align::Center);
        body.set_width_request(520);
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        overlay.add_overlay(body);
        overlay.set_measure_overlay(body, true);
        popup.append(&overlay);
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = popup.downgrade();
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(popup) = weak.upgrade() else {
                return;
            };
            let Some(window) = popup.root().and_downcast::<gtk::Window>() else {
                return;
            };
            let Some(body) = popup.body() else {
                return;
            };
            let inside = body.compute_bounds(&popup).is_some_and(|bounds| {
                bounds.contains_point(&graphene::Point::new(x as f32, y as f32))
            });
            if !inside {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                window.close();
                return;
            }
            let mut picked = popup.pick(x, y, gtk::PickFlags::DEFAULT);
            while let Some(widget) = picked {
                if widget.is::<gtk::Entry>()
                    || widget.is::<gtk::Button>()
                    || widget.is::<gtk::DropDown>()
                    || widget.is::<gtk::TextView>()
                {
                    return;
                }
                picked = widget.parent();
            }
            gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        });
        popup.add_controller(click);
        visit_popovers(body.as_ref(), &mut |popover| {
            let key = gtk::EventControllerKey::new();
            key.set_propagation_phase(gtk::PropagationPhase::Capture);
            let weak = popup.downgrade();
            key.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape
                    && let Some(popup) = weak.upgrade()
                {
                    popup.imp().escaping_popover.set(true);
                }
                glib::Propagation::Proceed
            });
            popover.add_controller(key);
            let weak = popup.downgrade();
            let window = window.downgrade();
            // GTK restores picker focus after `closed`; clear it on the next event-loop turn.
            popover.connect_local("closed", true, move |_| {
                if let Some(popup) = weak.upgrade()
                    && popup.imp().escaping_popover.replace(false)
                {
                    let window = window.clone();
                    glib::idle_add_local_once(move || {
                        if let Some(window) = window.upgrade() {
                            gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
                        }
                    });
                }
                None
            });
        });
        popup.connect_map(|popup| popup.reopen());
        window.set_child(Some(&popup));
        if window.is_layer_window() {
            window.set_default_size(0, 0);
            for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
                window.set_anchor(edge, true);
            }
        }
        window.connect_close_request(move |window| {
            if !window.is_mapped() || popup.imp().close_ms.get() == 0 {
                return glib::Propagation::Proceed;
            }
            popup.animate(false);
            glib::Propagation::Stop
        });
    }

    pub fn body(&self) -> Option<gtk::Widget> {
        self.imp().body.upgrade()
    }

    pub fn escape(&self, window: &gtk::Window) {
        let mut dismissed = false;
        if let Some(body) = self.body() {
            visit_popovers(&body, &mut |popover| {
                if popover.is_visible() {
                    dismissed = true;
                    self.imp().escaping_popover.set(true);
                    popover.popdown();
                }
            });
        }
        if dismissed {
            gtk::prelude::GtkWindowExt::set_focus(window, None::<&gtk::Widget>);
            return;
        }
        let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) else {
            window.close();
            return;
        };
        let mut active = Some(focus);
        while let Some(widget) = active {
            if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
                popover.popdown();
            }
            if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
                entry.select_region(entry.position(), entry.position());
            }
            active = widget.parent();
        }
        gtk::prelude::GtkWindowExt::set_focus(window, None::<&gtk::Widget>);
    }

    pub fn reopen(&self) {
        self.animate(true);
    }

    fn animate(&self, showing: bool) {
        let state = self.imp();
        if state.shown.replace(showing) == showing {
            return;
        }
        if let Some(tick) = state.tick.borrow_mut().take() {
            tick.remove();
        }
        let from = state.progress.get();
        let target = if showing { 1. } else { 0. };
        let duration = if showing {
            state.open_ms.get()
        } else {
            state.close_ms.get()
        };
        let duration = duration as f64 * (target - from).abs();
        if duration == 0. {
            state.progress.set(target);
            self.queue_draw();
            if !showing && let Some(window) = self.root().and_downcast::<gtk::Window>() {
                window.destroy();
            }
            return;
        }
        let started = Cell::new(None);
        // Start from the first frame, after mapping and cold font loading finish.
        *state.tick.borrow_mut() = Some(self.add_tick_callback(move |popup, clock| {
            let now = clock.frame_time();
            let start = started.get().unwrap_or_else(|| {
                started.set(Some(now));
                now
            });
            let progress = ((now - start) as f64 / (duration * 1000.)).clamp(0., 1.);
            let eased = if showing {
                super::motion::ease_out(progress)
            } else {
                progress.powi(3)
            };
            popup.imp().progress.set(from + (target - from) * eased);
            popup.queue_draw();
            if progress < 1. {
                return glib::ControlFlow::Continue;
            }
            popup.imp().tick.borrow_mut().take();
            if !showing && let Some(window) = popup.root().and_downcast::<gtk::Window>() {
                window.destroy();
            }
            glib::ControlFlow::Break
        }));
    }
}

fn visit_popovers(widget: &gtk::Widget, visit: &mut impl FnMut(&gtk::Popover)) {
    if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
        visit(popover);
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        visit_popovers(&widget, visit);
        child = widget.next_sibling();
    }
}
