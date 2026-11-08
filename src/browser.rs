//! The browser window, its tabs, and their connection to Servo.
//!
//! GTK has no way to embed a native window for Servo to draw into, so Servo
//! renders to an offscreen software OpenGL surface, and each finished frame is
//! copied into a GTK texture.
//!
//! Every tab has its own Servo `WebView`, but they all share that one surface.
//! Only the active tab's `WebView` is shown there, and each tab has its own
//! widget that keeps the last frame painted for it.

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
    SoftwareRenderingContext, StringRequest, WebView, WebViewBuilder, WebViewDelegate, WebViewId,
    WheelDelta, WheelEvent, WheelMode,
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

const NEW_TAB_TITLE: &str = "New Tab";

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
    tabs: gtk::Notebook,
    location: gtk::Entry,
    bookmark: gtk::ToggleButton,
    bookmark_list: gtk::ListBox,
    bookmarks_popover: gtk::Popover,
}

/// One browser tab. The state that the toolbar shows is kept for every tab, so
/// that it can be shown again when the tab becomes active.
struct Tab {
    webview: WebView,
    area: WebArea,
    label: gtk::Label,
    title: RefCell<Option<String>>,
    can_go_back: Cell<bool>,
    can_go_forward: Cell<bool>,
    progress: Cell<f64>,
    /// Whether this was opened as a blank tab, for typing a location into.
    opened_blank: bool,
}

impl Tab {
    fn id(&self) -> WebViewId {
        self.webview.id()
    }

    /// The URL to show in the location field, which is empty for a new tab.
    fn location_text(&self) -> String {
        match self.webview.url() {
            Some(url) if url.as_str() != "about:blank" => url.to_string(),
            _ => String::new(),
        }
    }

    fn display_title(&self) -> String {
        match self.title.borrow().as_deref() {
            Some(title) if !title.trim().is_empty() => title.trim().to_owned(),
            _ => match self.location_text() {
                url if url.is_empty() => NEW_TAB_TITLE.to_owned(),
                url => url,
            },
        }
    }

    fn update_label(&self) {
        self.label.set_text(&self.display_title());
        self.label.set_tooltip_text(Some(&self.display_title()));
    }
}

struct Browser {
    // This is an option so that `shut_down` can release it after the tabs.
    servo: RefCell<Option<Servo>>,
    /// In no particular order. The tab strip has the order the user sees.
    tabs: RefCell<Vec<Rc<Tab>>>,
    rendering_context: Rc<SoftwareRenderingContext>,
    widgets: Widgets,
    bookmarks: RefCell<Bookmarks>,
    needs_paint: Cell<bool>,
    pending_size: Cell<Option<PhysicalSize<u32>>>,
    pointer: Cell<DevicePoint>,
    /// Servo's event loop must not be spun again from inside itself.
    spinning: Cell<bool>,
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
        // Servo has this but disables it by default, and sites that use it to reveal
        // their content as it scrolls into view would otherwise stay partly blank.
        dom_intersection_observer_enabled: true,
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
        tabs: RefCell::new(Vec::new()),
        rendering_context,
        widgets: build_widgets(app),
        bookmarks: RefCell::new(bookmarks),
        needs_paint: Cell::new(false),
        pending_size: Cell::new(None),
        pointer: Cell::new(DevicePoint::zero()),
        spinning: Cell::new(false),
    });
    BROWSER.set(Some(browser.clone()));

    browser.connect_widgets();
    browser.connect_actions(app);
    browser.rebuild_bookmark_list();
    browser.open_tab(start_url);

    gdk::GLContext::clear_current();
    browser.widgets.window.present();
    browser.focus_page_or_location();
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

    let tabs = gtk::Notebook::builder()
        .scrollable(true)
        .show_border(false)
        .hexpand(true)
        .vexpand(true)
        .build();
    let new_tab = gtk::Button::builder()
        .icon_name("tab-new-symbolic")
        .tooltip_text("New tab (Ctrl+T)")
        .action_name("win.new-tab")
        .has_frame(false)
        .build();
    tabs.set_action_widget(&new_tab, gtk::PackType::End);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("BrowseGoo")
        .default_width(INITIAL_WIDTH)
        .default_height(INITIAL_HEIGHT)
        .titlebar(&header)
        .child(&tabs)
        .build();

    Widgets {
        window,
        tabs,
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

    fn active_tab(&self) -> Option<Rc<Tab>> {
        let notebook = &self.widgets.tabs;
        let page = notebook.nth_page(notebook.current_page())?;
        self.tabs
            .borrow()
            .iter()
            .find(|tab| tab.area.upcast_ref::<gtk::Widget>() == &page)
            .cloned()
    }

    fn tab(&self, id: WebViewId) -> Option<Rc<Tab>> {
        self.tabs
            .borrow()
            .iter()
            .find(|tab| tab.id() == id)
            .cloned()
    }

    fn is_active(&self, id: WebViewId) -> bool {
        self.active_tab().is_some_and(|tab| tab.id() == id)
    }

    /// The active tab's `WebView`.
    fn webview(&self) -> Option<WebView> {
        self.active_tab().map(|tab| tab.webview.clone())
    }

    fn webview_builder(self: &Rc<Self>, builder: WebViewBuilder) -> WebViewBuilder {
        builder
            .hidpi_scale_factor(Scale::new(self.widgets.window.scale_factor() as f32))
            .delegate(self.clone())
            .clipboard_delegate(self.clone())
    }

    /// Opens `url` in a new tab, and makes it the active tab.
    fn open_tab(self: &Rc<Self>, url: Url) {
        let Some(servo) = self.servo().clone() else {
            return;
        };
        let builder = WebViewBuilder::new(&servo, self.rendering_context.clone());
        let opened_blank = url.as_str() == "about:blank";
        self.add_tab(self.webview_builder(builder.url(url)).build(), opened_blank);
    }

    /// Adds a tab for `webview`, and makes it the active tab.
    fn add_tab(self: &Rc<Self>, webview: WebView, opened_blank: bool) {
        let id = webview.id();
        let area = WebArea::new();
        self.connect_input(&area, id);

        let label = gtk::Label::builder()
            .label(NEW_TAB_TITLE)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(12)
            .max_width_chars(24)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close tab (Ctrl+W)")
            .has_frame(false)
            .focus_on_click(false)
            .build();
        close.connect_clicked(move |_| close_tab_by_id(id));
        let tab_label = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab_label.append(&label);
        tab_label.append(&close);
        let middle_click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_MIDDLE)
            .build();
        middle_click.connect_released(move |_, _, _, _| close_tab_by_id(id));
        tab_label.add_controller(middle_click);

        let tab = Rc::new(Tab {
            webview,
            area,
            label,
            title: RefCell::new(None),
            can_go_back: Cell::new(false),
            can_go_forward: Cell::new(false),
            progress: Cell::new(0.0),
            opened_blank,
        });
        tab.update_label();

        // The tab must be known before the tab strip announces it as active.
        self.tabs.borrow_mut().push(tab.clone());
        let notebook = &self.widgets.tabs;
        let index = notebook.append_page(&tab.area, Some(&tab_label));
        notebook.set_tab_reorderable(&tab.area, true);
        notebook.set_current_page(Some(index));
    }

    fn close_tab(&self, id: WebViewId) {
        let Some(tab) = self.tab(id) else {
            return;
        };
        if self.tabs.borrow().len() == 1 {
            self.widgets.window.close();
            return;
        }
        self.tabs.borrow_mut().retain(|other| other.id() != id);
        // This makes another tab active, if this one was.
        self.widgets.tabs.detach_tab(&tab.area);
        // Dropping the last handle to the `WebView` closes it in Servo.
        drop(tab);
        Waker.wake();
    }

    /// Shows the newly active tab's page and state.
    fn activate_current_tab(&self) {
        let Some(active) = self.active_tab() else {
            return;
        };
        for tab in self.tabs.borrow().iter() {
            if tab.id() != active.id() {
                tab.webview.hide();
            }
        }
        active.webview.show();

        let title = active.title.borrow().clone();
        self.set_window_title(title.as_deref());
        self.widgets.location.set_text(&active.location_text());
        self.widgets
            .location
            .set_progress_fraction(active.progress.get());
        self.set_history_available(active.can_go_back.get(), active.can_go_forward.get());
        self.update_bookmark_button();
        self.focus_page_or_location();
        self.queue_resize();
        // Servo paints the newly shown page when it has a frame for it.
        self.needs_paint.set(true);
        Waker.wake();
    }

    /// Focuses the page, or the location field for a blank tab.
    fn focus_page_or_location(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        // A tab that's still loading has no URL yet, but isn't blank.
        if tab.opened_blank && tab.location_text().is_empty() {
            self.widgets.location.grab_focus();
        } else {
            tab.area.grab_focus();
        }
    }

    fn select_tab_by_offset(&self, offset: i32) {
        let notebook = &self.widgets.tabs;
        let count = notebook.n_pages() as i32;
        if count > 1 {
            let current = notebook.current_page().unwrap_or(0) as i32;
            let next = (current + offset).rem_euclid(count);
            notebook.set_current_page(Some(next as u32));
        }
    }

    fn set_window_title(&self, title: Option<&str>) {
        let window_title = match title.map(str::trim) {
            Some(title) if !title.is_empty() => format!("{title} - BrowseGoo"),
            _ => "BrowseGoo".to_owned(),
        };
        self.widgets.window.set_title(Some(&window_title));
    }

    fn is_editing_location(&self) -> bool {
        gtk::prelude::RootExt::focus(&self.widgets.window)
            .is_some_and(|focused| focused.is_ancestor(&self.widgets.location))
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
        if self.spinning.replace(true) {
            // Something Servo called asked for this, so it has to wait its turn.
            Waker.wake();
            return;
        }
        self.with_servo_gl(|| {
            if let (Some(size), Some(webview)) = (self.pending_size.take(), self.webview()) {
                webview.resize(size);
            }
            servo.spin_event_loop();
            if self.needs_paint.take() {
                self.paint();
            }
        });
        self.spinning.set(false);
    }

    fn paint(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        tab.webview.paint();

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
            tab.area.set_frame(frame.upcast());
        }
        self.rendering_context.present();
    }

    /// Stops Servo, which is also when it saves cookies.
    fn shut_down(&self) {
        self.with_servo_gl(|| {
            let tabs = self.tabs.take();
            drop(tabs);
            self.servo.replace(None);
        });
    }

    fn navigate(&self, url: Url) {
        if let Some(tab) = self.active_tab() {
            tab.webview.load(url);
            tab.area.grab_focus();
            self.spin();
        }
    }

    fn device_point(&self, x: f64, y: f64) -> DevicePoint {
        let scale = self.widgets.window.scale_factor() as f32;
        DevicePoint::new(x as f32 * scale, y as f32 * scale)
    }

    fn send_input(&self, id: WebViewId, event: InputEvent) {
        if let Some(tab) = self.tab(id) {
            tab.webview.notify_input_event(event);
            self.spin();
        }
    }

    /// Matches Servo's surface to the active tab's page area.
    fn queue_resize(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let area = &tab.area;
        if area.width() == 0 || area.height() == 0 {
            // It isn't laid out yet, and will say when it is.
            return;
        }
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

        widgets.window.connect_scale_factor_notify(|window| {
            if let Some(browser) = current_browser() {
                let scale = Scale::new(window.scale_factor() as f32);
                for tab in browser.tabs.borrow().iter() {
                    tab.webview.set_hidpi_scale_factor(scale);
                }
                browser.queue_resize();
            }
        });

        widgets.tabs.connect_page_notify(|_| {
            if let Some(browser) = current_browser() {
                browser.activate_current_tab();
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
        add_action("new-tab", |_| {
            // Opening a tab needs the shared handle, rather than a borrowed one.
            if let Some(browser) = current_browser() {
                browser.open_tab(Url::parse("about:blank").expect("about:blank is a valid URL"));
                browser.spin();
            }
        });
        add_action("close-tab", |browser| {
            if let Some(tab) = browser.active_tab() {
                browser.close_tab(tab.id());
            }
        });
        add_action("next-tab", |browser| browser.select_tab_by_offset(1));
        add_action("previous-tab", |browser| browser.select_tab_by_offset(-1));
        add_action("toggle-bookmark", Browser::toggle_bookmark);
        add_action("about", Browser::show_about);
        add_action("quit", |browser| browser.widgets.window.close());

        app.set_accels_for_action("win.back", &["<Alt>Left"]);
        app.set_accels_for_action("win.forward", &["<Alt>Right"]);
        app.set_accels_for_action("win.reload", &["<Control>r", "F5"]);
        app.set_accels_for_action("win.focus-location", &["<Control>l"]);
        app.set_accels_for_action("win.toggle-bookmark", &["<Control>d"]);
        app.set_accels_for_action("win.quit", &["<Control>q"]);
        app.set_accels_for_action("win.new-tab", &["<Control>t"]);
        app.set_accels_for_action("win.close-tab", &["<Control>w"]);
        app.set_accels_for_action("win.next-tab", &["<Control>Tab", "<Control>Page_Down"]);
        app.set_accels_for_action(
            "win.previous-tab",
            &[
                "<Control><Shift>Tab",
                "<Control>ISO_Left_Tab",
                "<Control>Page_Up",
            ],
        );

        self.set_history_available(false, false);
    }

    /// Sends input on a tab's page area to that tab's `WebView`.
    fn connect_input(&self, area: &WebArea, id: WebViewId) {
        area.connect_resized(move || {
            if let Some(browser) = current_browser() {
                if browser.is_active(id) {
                    browser.queue_resize();
                }
            }
        });

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_, x, y| {
            if let Some(browser) = current_browser() {
                let point = browser.device_point(x, y);
                browser.pointer.set(point);
                browser.send_input(id, InputEvent::MouseMove(MouseMoveEvent::new(point.into())));
            }
        });
        motion.connect_leave(move |_| {
            if let Some(browser) = current_browser() {
                browser.send_input(
                    id,
                    InputEvent::MouseLeftViewport(MouseLeftViewportEvent::default()),
                );
            }
        });
        area.add_controller(motion);

        let clicks = gtk::GestureClick::builder().button(0).build();
        let send_click = move |gesture: &gtk::GestureClick, action, x, y| {
            if let Some(browser) = current_browser() {
                let point = browser.device_point(x, y);
                browser.send_input(
                    id,
                    InputEvent::MouseButton(MouseButtonEvent::new(
                        action,
                        input::mouse_button(gesture.current_button()),
                        point.into(),
                    )),
                );
            }
        };
        clicks.connect_pressed(move |gesture, _, x, y| {
            if let Some(widget) = gesture.widget() {
                widget.grab_focus();
            }
            send_click(gesture, MouseButtonAction::Down, x, y);
        });
        clicks.connect_released(move |gesture, _, x, y| {
            send_click(gesture, MouseButtonAction::Up, x, y);
        });
        area.add_controller(clicks);

        let scrolling = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scrolling.connect_scroll(move |controller, dx, dy| {
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
                        let scale = browser.widgets.window.scale_factor() as f64;
                        (-dx * scale, -dy * scale, WheelMode::DeltaPixel)
                    }
                };
                let delta = WheelDelta { x, y, z: 0.0, mode };
                let point = browser.pointer.get().into();
                browser.send_input(id, InputEvent::Wheel(WheelEvent::new(delta, point)));
            }
            glib::Propagation::Stop
        });
        area.add_controller(scrolling);

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if let Some(browser) = current_browser() {
                browser.send_input(id, input::keyboard_event(KeyState::Down, key, modifiers));
            }
            glib::Propagation::Stop
        });
        keys.connect_key_released(move |_, key, _, modifiers| {
            if let Some(browser) = current_browser() {
                browser.send_input(id, input::keyboard_event(KeyState::Up, key, modifiers));
            }
        });
        area.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| {
            if let Some(tab) = current_browser().and_then(|browser| browser.tab(id)) {
                tab.webview.set_focused(true);
            }
        });
        focus.connect_leave(move |_| {
            if let Some(tab) = current_browser().and_then(|browser| browser.tab(id)) {
                tab.webview.set_focused(false);
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
    fn notify_new_frame_ready(&self, webview: WebView) {
        // This arrives while Servo is busy, so it's painted when `spin` regains control.
        if self.is_active(webview.id()) {
            self.needs_paint.set(true);
        }
    }

    fn notify_url_changed(&self, webview: WebView, _url: Url) {
        let Some(tab) = self.tab(webview.id()) else {
            return;
        };
        tab.update_label();
        if self.is_active(tab.id()) {
            if !self.is_editing_location() {
                self.widgets.location.set_text(&tab.location_text());
            }
            self.update_bookmark_button();
        }
    }

    fn notify_page_title_changed(&self, webview: WebView, title: Option<String>) {
        let Some(tab) = self.tab(webview.id()) else {
            return;
        };
        tab.title.replace(title.clone());
        tab.update_label();
        if self.is_active(tab.id()) {
            self.set_window_title(title.as_deref());
        }
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        let Some(tab) = self.tab(webview.id()) else {
            return;
        };
        let progress = match status {
            LoadStatus::Started => 0.2,
            LoadStatus::HeadParsed => 0.6,
            LoadStatus::Complete | LoadStatus::Stopped => 0.0,
        };
        tab.progress.set(progress);
        if self.is_active(tab.id()) {
            self.widgets.location.set_progress_fraction(progress);
        }
    }

    fn notify_history_changed(&self, webview: WebView, entries: Vec<Url>, current: usize) {
        let Some(tab) = self.tab(webview.id()) else {
            return;
        };
        tab.can_go_back.set(current > 0);
        tab.can_go_forward.set(current + 1 < entries.len());
        if self.is_active(tab.id()) {
            self.set_history_available(tab.can_go_back.get(), tab.can_go_forward.get());
        }
    }

    fn notify_cursor_changed(&self, webview: WebView, cursor: Cursor) {
        if let Some(tab) = self.tab(webview.id()) {
            tab.area
                .set_cursor_from_name(Some(input::cursor_name(cursor)));
        }
    }

    /// A page that opens a new window, such as with a `target="_blank"` link,
    /// gets a new tab instead.
    fn request_create_new(&self, _parent: WebView, request: CreateNewWebViewRequest) {
        let Some(browser) = current_browser() else {
            return;
        };
        let webview = browser
            .webview_builder(request.builder(self.rendering_context.clone()))
            .build();
        browser.add_tab(webview, false);
    }

    /// A page closed itself, such as with `window.close()`.
    fn notify_closed(&self, webview: WebView) {
        let id = webview.id();
        // This arrives while Servo is busy, and closing a tab needs Servo.
        glib::idle_add_local_once(move || close_tab_by_id(id));
    }
}

fn close_tab_by_id(id: WebViewId) {
    if let Some(browser) = current_browser() {
        browser.close_tab(id);
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
