//! The browser window, and its connection to Servo.
//!
//! GTK has no way to embed a native window for Servo to draw into, so Servo
//! renders to an offscreen software OpenGL surface, and each finished frame is
//! copied into a GTK texture.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use dpi::PhysicalSize;
use euclid::Scale;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use servo::{
    ClipboardDelegate, CreateNewWebViewRequest, Cursor, DeviceIntRect, DevicePoint, EventLoopWaker,
    InputEvent, KeyState, LoadStatus, MouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent,
    MouseMoveEvent, Opts, Preferences, RenderingContext, Servo, ServoBuilder,
    SoftwareRenderingContext, StringRequest, WebView, WebViewBuilder, WebViewDelegate, WheelDelta,
    WheelEvent, WheelMode,
};
use url::Url;

use crate::bookmarks::Bookmarks;
use crate::location::parse_location;
use crate::web_area::WebArea;
use crate::{VERSION, input};

const INITIAL_WIDTH: i32 = 1024;
const INITIAL_HEIGHT: i32 = 768;

/// How far one notch of a mouse wheel scrolls, in the units Servo's own example uses.
const WHEEL_NOTCH_PIXELS: f64 = 76.0;

const CREDIT: &str = "Created by AI Sloperator with Claude Code";
const WEBSITE: &str = "https://www.aisloperator.com/";
const LICENSE_STATEMENT: &str = "No license is given.";

thread_local! {
    /// The running browser, for Servo's waker to reach from other threads.
    static BROWSER: RefCell<Option<Rc<Browser>>> = const { RefCell::new(None) };
}

fn current_browser() -> Option<Rc<Browser>> {
    BROWSER.with_borrow(Clone::clone)
}

/// Lets Servo ask for `Servo::spin_event_loop` to be called on the main thread.
#[derive(Clone)]
struct Waker;

/// Whether a spin is already queued, so that wakes don't pile up in the main loop.
static SPIN_QUEUED: AtomicBool = AtomicBool::new(false);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        if !SPIN_QUEUED.swap(true, Ordering::AcqRel) {
            glib::MainContext::default().invoke(|| {
                SPIN_QUEUED.store(false, Ordering::Release);
                if let Some(browser) = current_browser() {
                    browser.spin();
                }
            });
        }
    }
}

struct Widgets {
    window: gtk::ApplicationWindow,
    web_area: WebArea,
    location: gtk::Entry,
    bookmark: gtk::ToggleButton,
    bookmark_list: gtk::ListBox,
    bookmarks_popover: gtk::Popover,
}

struct Browser {
    // These are options so that `shut_down` can release them in order.
    servo: RefCell<Option<Servo>>,
    webview: RefCell<Option<WebView>>,
    rendering_context: Rc<SoftwareRenderingContext>,
    widgets: Widgets,
    bookmarks: RefCell<Bookmarks>,
    needs_paint: Cell<bool>,
    pending_size: Cell<Option<PhysicalSize<u32>>>,
    pointer: Cell<DevicePoint>,
}

/// Opens the browser window at `start_url`.
pub fn open(app: &gtk::Application, config_dir: PathBuf, start_url: Url) {
    if current_browser().is_some() {
        return;
    }

    let size = PhysicalSize::new(INITIAL_WIDTH as u32, INITIAL_HEIGHT as u32);
    let rendering_context = match SoftwareRenderingContext::new(size) {
        Ok(context) => Rc::new(context),
        Err(error) => {
            eprintln!("Could not create a software OpenGL context: {error:?}");
            app.quit();
            return;
        }
    };
    if let Err(error) = rendering_context.make_current() {
        eprintln!("Could not use the software OpenGL context: {error:?}");
        app.quit();
        return;
    }

    let opts = Opts {
        config_dir: Some(config_dir.clone()),
        ..Default::default()
    };
    let preferences = Preferences {
        user_agent: format!("BrowseGoo/{VERSION}"),
        // Servo has these, but disables them by default. Sites that use
        // IntersectionObserver to reveal content as it scrolls into view would
        // otherwise stay partly blank, and ad loaders that expect `visualViewport`
        // give up without showing any ads.
        dom_intersection_observer_enabled: true,
        dom_visual_viewport_enabled: true,
        ..Default::default()
    };
    let servo = ServoBuilder::default()
        .opts(opts)
        .preferences(preferences)
        .event_loop_waker(Box::new(Waker))
        .build();
    servo.setup_logging();

    let bookmarks = Bookmarks::load(config_dir.join("bookmarks.tsv")).unwrap_or_else(|error| {
        // Carrying on would overwrite the bookmarks that we couldn't read.
        eprintln!("Could not read bookmarks: {error}");
        std::process::exit(1);
    });

    let browser = Rc::new(Browser {
        servo: RefCell::new(Some(servo)),
        webview: RefCell::new(None),
        rendering_context,
        widgets: build_widgets(app),
        bookmarks: RefCell::new(bookmarks),
        needs_paint: Cell::new(false),
        pending_size: Cell::new(None),
        pointer: Cell::new(DevicePoint::zero()),
    });
    BROWSER.set(Some(browser.clone()));

    browser.connect_widgets();
    browser.connect_actions(app);
    browser.connect_input();
    browser.rebuild_bookmark_list();

    let webview = browser.webview_builder(WebViewBuilder::new(
        browser.servo().as_ref().expect("Servo has just started"),
        browser.rendering_context.clone(),
    ));
    let webview = webview.url(start_url).build();
    webview.show();
    browser.webview.replace(Some(webview));

    gdk::GLContext::clear_current();
    browser.widgets.window.present();
    browser.widgets.web_area.grab_focus();
    browser.spin();
}

fn build_widgets(app: &gtk::Application) -> Widgets {
    let icon_button = |icon: &str, tooltip: &str, action: &str| {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .action_name(action)
            .build()
    };

    let back = icon_button("go-previous-symbolic", "Back (Alt+Left)", "win.back");
    let forward = icon_button("go-next-symbolic", "Forward (Alt+Right)", "win.forward");
    let reload = icon_button("view-refresh-symbolic", "Reload (Ctrl+R)", "win.reload");
    let about = icon_button("help-about-symbolic", "About BrowseGoo", "win.about");

    let location = gtk::Entry::builder()
        .hexpand(true)
        .input_purpose(gtk::InputPurpose::Url)
        .placeholder_text("Enter a location")
        .build();

    let bookmark = gtk::ToggleButton::builder()
        .icon_name("non-starred-symbolic")
        .tooltip_text("Bookmark this page (Ctrl+D)")
        .build();

    let bookmark_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    bookmark_list.set_placeholder(Some(&gtk::Label::new(Some("No bookmarks yet"))));
    let bookmark_scroller = gtk::ScrolledWindow::builder()
        .child(&bookmark_list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .propagate_natural_width(true)
        .max_content_height(480)
        .build();
    let bookmarks_popover = gtk::Popover::builder().child(&bookmark_scroller).build();
    let bookmarks_button = gtk::MenuButton::builder()
        .icon_name("user-bookmarks-symbolic")
        .tooltip_text("Bookmarks")
        .popover(&bookmarks_popover)
        .build();

    let header = gtk::HeaderBar::new();
    header.pack_start(&back);
    header.pack_start(&forward);
    header.pack_start(&reload);
    header.set_title_widget(Some(&location));
    header.pack_end(&about);
    header.pack_end(&bookmarks_button);
    header.pack_end(&bookmark);

    let web_area = WebArea::new();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("BrowseGoo")
        .default_width(INITIAL_WIDTH)
        .default_height(INITIAL_HEIGHT)
        .titlebar(&header)
        .child(&web_area)
        .build();

    Widgets {
        window,
        web_area,
        location,
        bookmark,
        bookmark_list,
        bookmarks_popover,
    }
}

impl Browser {
    fn servo(&self) -> std::cell::Ref<'_, Option<Servo>> {
        self.servo.borrow()
    }

    fn webview(&self) -> Option<WebView> {
        self.webview.borrow().clone()
    }

    fn webview_builder(self: &Rc<Self>, builder: WebViewBuilder) -> WebViewBuilder {
        builder
            .hidpi_scale_factor(Scale::new(self.widgets.web_area.scale_factor() as f32))
            .delegate(self.clone())
            .clipboard_delegate(self.clone())
    }

    /// Runs `work`, which may use OpenGL, on Servo's OpenGL context.
    ///
    /// GTK may also be using OpenGL on this thread, and it remembers which
    /// context it believes is current, so it has to be told to forget that.
    fn with_servo_gl(&self, work: impl FnOnce()) {
        if let Err(error) = self.rendering_context.make_current() {
            eprintln!("Could not use the software OpenGL context: {error:?}");
            return;
        }
        work();
        gdk::GLContext::clear_current();
    }

    /// Lets Servo do whatever it has waiting, and shows any new frame.
    fn spin(&self) {
        let Some(servo) = self.servo().clone() else {
            return;
        };
        self.with_servo_gl(|| {
            if let (Some(size), Some(webview)) = (self.pending_size.take(), self.webview()) {
                webview.resize(size);
            }
            servo.spin_event_loop();
            if self.needs_paint.take() {
                self.paint();
            }
        });
    }

    fn paint(&self) {
        let Some(webview) = self.webview() else {
            return;
        };
        webview.paint();

        let size = self.rendering_context.size2d().to_i32();
        if let Some(image) = self
            .rendering_context
            .read_to_image(DeviceIntRect::from_size(size))
        {
            let (width, height) = image.dimensions();
            let frame = gdk::MemoryTexture::new(
                width as i32,
                height as i32,
                gdk::MemoryFormat::R8g8b8a8Premultiplied,
                &glib::Bytes::from_owned(image.into_raw()),
                width as usize * 4,
            );
            self.widgets.web_area.set_frame(frame.upcast());
        }
        self.rendering_context.present();
    }

    /// Stops Servo, which is also when it saves cookies.
    fn shut_down(&self) {
        self.with_servo_gl(|| {
            self.webview.replace(None);
            self.servo.replace(None);
        });
    }

    fn navigate(&self, url: Url) {
        if let Some(webview) = self.webview() {
            webview.load(url);
            self.widgets.web_area.grab_focus();
            self.spin();
        }
    }

    fn device_point(&self, x: f64, y: f64) -> DevicePoint {
        let scale = self.widgets.web_area.scale_factor() as f32;
        DevicePoint::new(x as f32 * scale, y as f32 * scale)
    }

    fn send_input(&self, event: InputEvent) {
        if let Some(webview) = self.webview() {
            webview.notify_input_event(event);
            self.spin();
        }
    }

    fn queue_resize(&self) {
        let area = &self.widgets.web_area;
        let scale = area.scale_factor();
        let size = PhysicalSize::new(
            (area.width() * scale).max(1) as u32,
            (area.height() * scale).max(1) as u32,
        );
        if size != self.rendering_context.size() {
            self.pending_size.set(Some(size));
            // This is during GTK's layout, which is no time to do any rendering.
            Waker.wake();
        }
    }

    fn connect_widgets(self: &Rc<Self>) {
        let widgets = &self.widgets;

        widgets.web_area.connect_resized(|| {
            if let Some(browser) = current_browser() {
                browser.queue_resize();
            }
        });
        widgets.web_area.connect_scale_factor_notify(|_| {
            if let Some(browser) = current_browser() {
                if let Some(webview) = browser.webview() {
                    let scale = browser.widgets.web_area.scale_factor() as f32;
                    webview.set_hidpi_scale_factor(Scale::new(scale));
                }
                browser.queue_resize();
            }
        });

        widgets.location.connect_activate(|entry| {
            if let Some(browser) = current_browser() {
                match parse_location(&entry.text()) {
                    Some(url) => browser.navigate(url),
                    None => entry.error_bell(),
                }
            }
        });

        widgets.bookmark.connect_clicked(|_| {
            if let Some(browser) = current_browser() {
                browser.toggle_bookmark();
            }
        });

        widgets.bookmark_list.connect_row_activated(|_, row| {
            let Some(browser) = current_browser() else {
                return;
            };
            let index = row.index() as usize;
            let url = browser
                .bookmarks
                .borrow()
                .items()
                .get(index)
                .map(|b| b.url.clone());
            browser.widgets.bookmarks_popover.popdown();
            if let Some(url) = url.as_deref().and_then(parse_location) {
                browser.navigate(url);
            }
        });

        widgets.window.connect_close_request(|_| {
            if let Some(browser) = BROWSER.take() {
                browser.shut_down();
            }
            glib::Propagation::Proceed
        });
    }

    fn connect_actions(self: &Rc<Self>, app: &gtk::Application) {
        let window = &self.widgets.window;
        let add_action = |name: &str, activate: fn(&Browser)| {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| {
                if let Some(browser) = current_browser() {
                    activate(&browser);
                }
            });
            window.add_action(&action);
        };

        add_action("back", |browser| {
            if let Some(webview) = browser.webview() {
                webview.go_back(1);
                browser.spin();
            }
        });
        add_action("forward", |browser| {
            if let Some(webview) = browser.webview() {
                webview.go_forward(1);
                browser.spin();
            }
        });
        add_action("reload", |browser| {
            if let Some(webview) = browser.webview() {
                webview.reload();
                browser.spin();
            }
        });
        add_action("focus-location", |browser| {
            browser.widgets.location.grab_focus();
        });
        add_action("toggle-bookmark", Browser::toggle_bookmark);
        add_action("about", Browser::show_about);
        add_action("quit", |browser| browser.widgets.window.close());

        app.set_accels_for_action("win.back", &["<Alt>Left"]);
        app.set_accels_for_action("win.forward", &["<Alt>Right"]);
        app.set_accels_for_action("win.reload", &["<Control>r", "F5"]);
        app.set_accels_for_action("win.focus-location", &["<Control>l"]);
        app.set_accels_for_action("win.toggle-bookmark", &["<Control>d"]);
        app.set_accels_for_action("win.quit", &["<Control>q"]);

        self.set_history_available(false, false);
    }

    fn connect_input(self: &Rc<Self>) {
        let area = &self.widgets.web_area;

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(|_, x, y| {
            if let Some(browser) = current_browser() {
                let point = browser.device_point(x, y);
                browser.pointer.set(point);
                browser.send_input(InputEvent::MouseMove(MouseMoveEvent::new(point.into())));
            }
        });
        motion.connect_leave(|_| {
            if let Some(browser) = current_browser() {
                browser.send_input(InputEvent::MouseLeftViewport(
                    MouseLeftViewportEvent::default(),
                ));
            }
        });
        area.add_controller(motion);

        let clicks = gtk::GestureClick::builder().button(0).build();
        let send_click = |gesture: &gtk::GestureClick, action, x, y| {
            if let Some(browser) = current_browser() {
                let point = browser.device_point(x, y);
                browser.send_input(InputEvent::MouseButton(MouseButtonEvent::new(
                    action,
                    input::mouse_button(gesture.current_button()),
                    point.into(),
                )));
            }
        };
        clicks.connect_pressed(move |gesture, _, x, y| {
            if let Some(browser) = current_browser() {
                browser.widgets.web_area.grab_focus();
            }
            send_click(gesture, MouseButtonAction::Down, x, y);
        });
        clicks.connect_released(move |gesture, _, x, y| {
            send_click(gesture, MouseButtonAction::Up, x, y);
        });
        area.add_controller(clicks);

        let scrolling = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scrolling.connect_scroll(|controller, dx, dy| {
            if let Some(browser) = current_browser() {
                // GTK's deltas are the way the pointer moved, and Servo's are the way the
                // view moves.
                let (x, y, mode) = match controller.unit() {
                    gdk::ScrollUnit::Wheel => (
                        -dx * WHEEL_NOTCH_PIXELS,
                        -dy * WHEEL_NOTCH_PIXELS,
                        WheelMode::DeltaLine,
                    ),
                    _ => {
                        let scale = browser.widgets.web_area.scale_factor() as f64;
                        (-dx * scale, -dy * scale, WheelMode::DeltaPixel)
                    }
                };
                let delta = WheelDelta { x, y, z: 0.0, mode };
                let point = browser.pointer.get().into();
                browser.send_input(InputEvent::Wheel(WheelEvent::new(delta, point)));
            }
            glib::Propagation::Stop
        });
        area.add_controller(scrolling);

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(|_, key, _, modifiers| {
            if let Some(browser) = current_browser() {
                browser.send_input(input::keyboard_event(KeyState::Down, key, modifiers));
            }
            glib::Propagation::Stop
        });
        keys.connect_key_released(|_, key, _, modifiers| {
            if let Some(browser) = current_browser() {
                browser.send_input(input::keyboard_event(KeyState::Up, key, modifiers));
            }
        });
        area.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(|_| {
            if let Some(webview) = current_browser().and_then(|browser| browser.webview()) {
                webview.set_focused(true);
            }
        });
        focus.connect_leave(|_| {
            if let Some(webview) = current_browser().and_then(|browser| browser.webview()) {
                webview.set_focused(false);
            }
        });
        area.add_controller(focus);
    }

    /// Enables the actions, which also takes care of their buttons and shortcuts.
    fn set_history_available(&self, can_go_back: bool, can_go_forward: bool) {
        let set_enabled = |name, enabled| {
            let action = self.widgets.window.lookup_action(name);
            if let Some(action) = action.and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        };
        set_enabled("back", can_go_back);
        set_enabled("forward", can_go_forward);
    }

    fn toggle_bookmark(&self) {
        if let Some(webview) = self.webview() {
            if let Some(url) = webview.url() {
                let title = webview.page_title().unwrap_or_else(|| url.to_string());
                let result = self.bookmarks.borrow_mut().toggle(url.as_str(), &title);
                if let Err(error) = result {
                    eprintln!("Could not save bookmarks: {error}");
                }
                self.rebuild_bookmark_list();
            }
        }
        self.update_bookmark_button();
    }

    fn update_bookmark_button(&self) {
        let is_bookmarked = self
            .webview()
            .and_then(|webview| webview.url())
            .is_some_and(|url| self.bookmarks.borrow().contains(url.as_str()));
        let button = &self.widgets.bookmark;
        button.set_active(is_bookmarked);
        button.set_icon_name(if is_bookmarked {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        });
    }

    fn rebuild_bookmark_list(&self) {
        let list = &self.widgets.bookmark_list;
        list.remove_all();
        for bookmark in self.bookmarks.borrow().items() {
            let label = gtk::Label::builder()
                .label(&bookmark.title)
                .tooltip_text(&bookmark.url)
                .xalign(0.0)
                .max_width_chars(48)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            list.append(&label);
        }
    }

    fn show_about(&self) {
        gtk::AboutDialog::builder()
            .transient_for(&self.widgets.window)
            .modal(true)
            .program_name("BrowseGoo")
            .version(VERSION)
            .comments(CREDIT)
            .website(WEBSITE)
            .website_label(WEBSITE)
            .authors(["AI Sloperator with Claude Code"])
            .license_type(gtk::License::Custom)
            .license(LICENSE_STATEMENT)
            .build()
            .present();
    }
}

impl WebViewDelegate for Browser {
    fn notify_new_frame_ready(&self, _webview: WebView) {
        // This arrives while Servo is busy, so it's painted when `spin` regains control.
        self.needs_paint.set(true);
    }

    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        let location = &self.widgets.location;
        let is_editing = gtk::prelude::RootExt::focus(&self.widgets.window)
            .is_some_and(|focused| focused.is_ancestor(location));
        if !is_editing {
            location.set_text(url.as_str());
        }
        self.update_bookmark_button();
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        let window_title = match title.as_deref() {
            Some(title) if !title.is_empty() => format!("{title} - BrowseGoo"),
            _ => "BrowseGoo".to_owned(),
        };
        self.widgets.window.set_title(Some(&window_title));
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        let progress = match status {
            LoadStatus::Started => 0.2,
            LoadStatus::HeadParsed => 0.6,
            LoadStatus::Complete | LoadStatus::Stopped => 0.0,
        };
        self.widgets.location.set_progress_fraction(progress);
    }

    fn notify_history_changed(&self, _webview: WebView, entries: Vec<Url>, current: usize) {
        self.set_history_available(current > 0, current + 1 < entries.len());
    }

    fn notify_cursor_changed(&self, _webview: WebView, cursor: Cursor) {
        self.widgets
            .web_area
            .set_cursor_from_name(Some(input::cursor_name(cursor)));
    }

    /// This window shows one page at a time, so a page that opens a new window
    /// takes over this one.
    fn request_create_new(&self, _parent: WebView, request: CreateNewWebViewRequest) {
        let Some(browser) = current_browser() else {
            return;
        };
        let webview = browser
            .webview_builder(request.builder(self.rendering_context.clone()))
            .build();
        webview.show();
        webview.set_focused(true);
        if let Some(previous) = self.webview.replace(Some(webview)) {
            // The page that asked for this may still be in the middle of asking.
            glib::idle_add_local_once(move || drop(previous));
        }
    }
}

impl ClipboardDelegate for Browser {
    fn clear(&self, _webview: WebView) {
        self.widgets.window.clipboard().set_text("");
    }

    fn get_text(&self, _webview: WebView, request: StringRequest) {
        self.widgets
            .window
            .clipboard()
            .read_text_async(None::<&gio::Cancellable>, move |result| match result {
                Ok(text) => request.success(text.unwrap_or_default().into()),
                Err(error) => request.failure(error.to_string()),
            });
    }

    fn set_text(&self, _webview: WebView, new_contents: String) {
        self.widgets.window.clipboard().set_text(&new_contents);
    }
}
