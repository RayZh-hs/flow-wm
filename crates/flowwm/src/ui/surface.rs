//! Retained GSK canvas: cached text/edges, GPU textures and borders during window morphs.
use gtk::{glib, graphene, gsk, prelude::*, subclass::prelude::*};
use std::cell::RefCell;
type Renderer = Box<dyn Fn(&gtk::Snapshot, i32, i32)>;
pub struct RowSlide {
    pub outgoing: Option<gsk::RenderNode>,
    pub incoming: bool,
    /// +1 moves to a lower row: the current row leaves through the top.
    pub direction: f32,
    pub progress: f32,
}
mod imp {
    use super::*;
    #[derive(Default)]
    pub struct Surface {
        pub renderer: RefCell<Option<Renderer>>,
        pub frame: RefCell<Option<gsk::RenderNode>>,
        pub slide: RefCell<Option<RowSlide>>,
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
                let scene = gtk::Snapshot::new();
                if let Some(slide) = self.slide.borrow().as_ref() {
                    let height = obj.height() as f32;
                    scene.push_clip(&graphene::Rect::new(0.0, 0.0, obj.width() as f32, height));
                    if let Some(outgoing) = &slide.outgoing {
                        scene.save();
                        scene.translate(&graphene::Point::new(
                            0.0,
                            -slide.direction * slide.progress * height,
                        ));
                        scene.append_node(outgoing);
                        scene.restore();
                    }
                    if slide.incoming {
                        scene.save();
                        scene.translate(&graphene::Point::new(
                            0.0,
                            slide.direction * (1.0 - slide.progress) * height,
                        ));
                        render(&scene, obj.width(), obj.height());
                        scene.restore();
                    }
                    scene.pop();
                } else {
                    render(&scene, obj.width(), obj.height());
                }
                let frame = scene.to_node();
                if let Some(node) = &frame {
                    snapshot.append_node(node);
                }
                *self.frame.borrow_mut() = frame;
            }
        }
    }
}
glib::wrapper! {pub struct Surface(ObjectSubclass<imp::Surface>) @extends gtk::Widget, @implements gtk::Accessible,gtk::Buildable,gtk::ConstraintTarget;}
impl Surface {
    pub fn frame(&self) -> Option<gsk::RenderNode> {
        self.imp().frame.borrow().clone()
    }

    pub fn slide(&self, slide: Option<RowSlide>) {
        *self.imp().slide.borrow_mut() = slide;
        self.queue_draw();
    }

    pub fn slide_progress(&self, progress: f32) {
        if let Some(slide) = self.imp().slide.borrow_mut().as_mut() {
            slide.progress = progress;
        }
        self.queue_draw();
    }

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
