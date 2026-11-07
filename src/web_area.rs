//! The GTK widget that displays the frames Servo renders.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

mod imp {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    pub struct WebArea {
        pub frame: RefCell<Option<gdk::Texture>>,
        pub on_resize: RefCell<Option<Box<dyn Fn()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WebArea {
        const NAME: &'static str = "BrowseGooWebArea";
        type Type = super::WebArea;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for WebArea {}

    impl WidgetImpl for WebArea {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if let Some(on_resize) = self.on_resize.borrow().as_ref() {
                on_resize();
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(frame) = self.frame.borrow().as_ref() {
                let widget = self.obj();
                let bounds =
                    graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32);
                snapshot.append_texture(frame, &bounds);
            }
        }
    }
}

glib::wrapper! {
    pub struct WebArea(ObjectSubclass<imp::WebArea>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl WebArea {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("focusable", true)
            .property("hexpand", true)
            .property("vexpand", true)
            .build()
    }

    pub fn connect_resized(&self, on_resize: impl Fn() + 'static) {
        self.imp().on_resize.replace(Some(Box::new(on_resize)));
    }

    pub fn set_frame(&self, frame: gdk::Texture) {
        self.imp().frame.replace(Some(frame));
        self.queue_draw();
    }
}
