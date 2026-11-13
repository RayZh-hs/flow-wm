//! A popup stays mapped through its exit transition and can reverse while closing.
use gtk::{glib, graphene, prelude::*, subclass::prelude::*};
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
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let progress = self.progress.get();
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
    pub fn attach(window: &gtk::Window, body: &impl IsA<gtk::Widget>, open_ms: u32, close_ms: u32) {
        let popup: Self = glib::Object::builder()
            .property("orientation", gtk::Orientation::Vertical)
            .build();
        popup.imp().open_ms.set(open_ms);
        popup.imp().close_ms.set(close_ms);
        popup.append(body);
        popup.connect_map(|popup| popup.reopen());
        window.set_child(Some(&popup));
        window.connect_close_request(move |window| {
            if !window.is_mapped() || popup.imp().close_ms.get() == 0 {
                return glib::Propagation::Proceed;
            }
            popup.animate(false);
            glib::Propagation::Stop
        });
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
