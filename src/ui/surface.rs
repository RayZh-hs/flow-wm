//! Retained GSK canvas: cached text/edges, GPU textures and borders during window morphs.
use gtk::{glib, prelude::*, subclass::prelude::*};
use std::cell::RefCell;
type Renderer = Box<dyn Fn(&gtk::Snapshot, i32, i32)>;
mod imp {
    use super::*;
    #[derive(Default)]
    pub struct Surface {
        pub renderer: RefCell<Option<Renderer>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Surface {
        const NAME: &'static str = "FlowGraphSurface";
        type Type = super::Surface;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for Surface {}
    impl WidgetImpl for Surface {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (100, 800, -1, -1)
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(render) = self.renderer.borrow().as_ref() {
                let obj = self.obj();
                render(snapshot, obj.width(), obj.height());
            }
        }
    }
}
glib::wrapper! {pub struct Surface(ObjectSubclass<imp::Surface>) @extends gtk::Widget, @implements gtk::Accessible,gtk::Buildable,gtk::ConstraintTarget;}
impl Surface {
    pub fn new(render: impl Fn(&gtk::Snapshot, i32, i32) + 'static) -> Self {
        let widget: Self = glib::Object::builder()
            .property("hexpand", true)
            .property("vexpand", true)
            .property("focusable", true)
            .build();
        *widget.imp().renderer.borrow_mut() = Some(Box::new(render));
        widget
    }
}
