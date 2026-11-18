//! The browser window, its tabs, and their WebKit views.
//!
//! The tabs are above the toolbar, in the title bar when the window draws its
//! own (see `decorations`). A `gtk::Notebook` puts each page right under the
//! tabs, with no room for a shared toolbar, so the notebook only provides the
//! tabs, with an empty placeholder as each one's page, and the pages
//! themselves are in a `gtk::Stack` below the toolbar.
//!
//! WebKit runs each page's content in separate, sandboxed processes, and does
//! its networking in another. This process only has the user interface.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use url::Url;
use webkit::prelude::*;

use crate::VERSION;
use crate::bookmarks::{Bookmark, Bookmarks};
use crate::bookmarks_editor::{BookmarksEditor, Edit};
use crate::block_sites;
use crate::closing;
use crate::decorations;
use crate::filter::BlockedSites;
use crate::find_bar::FindBar;
use crate::filter_lists::{self, FilterList, LISTS, ListStatus};
use crate::location::parse_location;
use crate::local_network::{self, LocalOrigin};
use crate::location_bar::LocationBar;
use crate::page_fonts;
use crate::password_manager::PasswordManager;
use crate::password_prompts::{self, PageMessage};
use crate::passwords::{self, Passwords};
use crate::preferences::PreferencesWindow;
use crate::privacy;
use crate::preserved::Preserved;
use crate::site_data::{self, SiteDataWindow};
use crate::socks_proxy::SocksProxy;
use crate::user_filters::UserFiltersWindow;

const INITIAL_WIDTH: i32 = 1024;
const INITIAL_HEIGHT: i32 = 768;

const NEW_TAB_TITLE: &str = "New Tab";

/// How long quitting waits for the site data that isn't preserved to be
/// deleted. What's left is deleted the next time BrowseGoo starts.
const DELETING_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long to wait after cookies change, or a page loads, before deleting
/// what blocked sites stored, as pages often store several things at once.
const DELETING_BLOCKED_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// How long after a page loads to look again for what blocked sites stored.
const DELETING_BLOCKED_AGAIN: std::time::Duration = std::time::Duration::from_secs(5);

/// How long what the user asked a tab to load counts as asked for, if WebKit
/// hasn't asked whether to go there by then. It can take a while, when a web
/// process is starting.
const USER_LOAD_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Finding text in a page counts at most this many matches.
pub const MOST_MATCHES: u32 = 1000;

const CREDIT: &str = "Created by AI Sloperator with Claude Code";
const WEBSITE: &str = "https://www.aisloperator.com/";
const LICENSE_STATEMENT: &str = "No license is given.";

/// What a page shows after its web process crashes.
const CRASHED_PAGE: &str = "<!doctype html><title>Crashed</title>\
    <body style=\"font: 16px sans-serif; margin: 3em\">\
    <h1 style=\"font-size: 1.5em\">This page crashed</h1>\
    <p>Press Reload to load it again.</p>";

/// The code of WebKit's policy error for a load that a content blocker
/// blocked, which WebKitGTK's headers leave out, but is WebKit's
/// `WebKitErrorBlockedByContentBlocker` on every platform.
const BLOCKED_BY_CONTENT_BLOCKER: i32 = 104;

/// What a tab shows instead of a page that a filter rule blocks.
fn blocked_page(url: &str) -> String {
    format!(
        "<!doctype html><title>Blocked</title>\
         <body style=\"font: 16px sans-serif; margin: 3em\">\
         <h1 style=\"font-size: 1.5em\">This page is blocked</h1>\
         <p>A filter rule blocks <span style=\"word-break: break-all\">{}</span>.</p>",
        escape_html(url)
    )
}

/// What a tab shows instead of a page on this computer or the local network,
/// which another page tried to open.
fn local_network_page(url: &str, from: &str) -> String {
    format!(
        "<!doctype html><title>Blocked</title>\
         <body style=\"font: 16px sans-serif; margin: 3em\">\
         <h1 style=\"font-size: 1.5em\">This page is on your computer or local network</h1>\
         <p>A page from <span style=\"word-break: break-all\">{}</span> tried to open \
         <span style=\"word-break: break-all\">{}</span>. BrowseGoo doesn't let pages \
         reach your computer or local network from other addresses.</p>\
         <p>If you meant to open it, press Reload.</p>",
        escape_html(from),
        escape_html(url)
    )
}

/// What a tab shows instead of a page whose name is a public one, but is for an
/// address on this computer or the local network.
fn public_name_page(url: &str) -> String {
    format!(
        "<!doctype html><title>Blocked</title>\
         <body style=\"font: 16px sans-serif; margin: 3em\">\
         <h1 style=\"font-size: 1.5em\">This name is for your computer or local network</h1>\
         <p>The name in <span style=\"word-break: break-all\">{}</span> isn't one for \
         a local network, but it's for an address on your computer or local network. \
         BrowseGoo doesn't connect to those, as a site could use its own name to reach \
         them.</p>\
         <p>If this is your own server, use its address, such as 127.0.0.1, or a name \
         ending in .localhost, .local, .lan, .home.arpa, .internal or .test.</p>",
        escape_html(url)
    )
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Why a tab shows a page of BrowseGoo's own in place of one.
#[derive(Clone, Debug, PartialEq)]
enum StandInReason {
    Crashed,
    Blocked,
    /// A page from this address tried to open a local one.
    LocalNetwork(String),
    /// The page's name isn't a local one, but is for a local address.
    PublicNameForLocalAddress,
}

thread_local! {
    /// The running browser, for signal handlers to reach.
    static BROWSER: RefCell<Option<Rc<Browser>>> = const { RefCell::new(None) };
}

fn current_browser() -> Option<Rc<Browser>> {
    BROWSER.with_borrow(Clone::clone)
}

fn user_agent() -> String {
    format!("BrowseGoo/{VERSION}")
}

/// Where BrowseGoo keeps things, which are created before the browser opens.
#[derive(Clone)]
pub struct Dirs {
    /// Bookmarks and settings.
    pub config: PathBuf,
    /// Cookies, and what sites store, such as with `localStorage`.
    pub data: PathBuf,
    /// Filter lists, and WebKit's cache.
    pub cache: PathBuf,
}

struct Widgets {
    window: gtk::ApplicationWindow,
    tabs: gtk::Notebook,
    pages: gtk::Stack,
    location: gtk::Entry,
    location_bar: Rc<LocationBar>,
    filter_button: gtk::MenuButton,
    bookmark: gtk::ToggleButton,
    bookmark_list: gtk::ListBox,
    bookmarks_popover: gtk::Popover,
    find_bar: FindBar,
    /// Shown while the active tab has a password entered that could be saved.
    save_password: gtk::Button,
}

/// Identifies a tab, as WebKit views have no IDs of their own.
type TabId = u64;

/// One browser tab.
struct Tab {
    id: TabId,
    webview: webkit::WebView,
    /// The filters this tab uses, which are its own so that they can be off.
    content: webkit::UserContentManager,
    /// This tab's page in the tab strip, which only stands in for the view.
    placeholder: gtk::Box,
    label: gtk::Label,
    /// Whether this was opened as a blank tab, for typing a location into.
    opened_blank: bool,
    filtering: Cell<bool>,
    /// The tab whose page opened this one, if any.
    opener: Option<TabId>,
    /// The address of the page that crashed or is blocked, if one of
    /// BrowseGoo's own pages is shown instead.
    stand_in: RefCell<Option<String>>,
    /// Whether that page of BrowseGoo's own is about to load, which isn't
    /// leaving it.
    loading_stand_in: Cell<bool>,
    /// The password last entered on a page in this tab, which could be saved.
    entered_password: RefCell<Option<EnteredPassword>>,
    /// The page shown, once it's started arriving, which is the one that
    /// leaving it comes from.
    committed: RefCell<Option<Url>>,
    /// What the user asked this tab to load, and when, until WebKit asks
    /// whether to go there, as only the user can open a local address from
    /// another one. WebKit can ask after the user asks for something else.
    user_loads: RefCell<Vec<(Url, Instant)>>,
    /// Whether the user pressed Back or Forward, until the tab starts going.
    user_history: Cell<bool>,
}

/// A password entered in a form, which BrowseGoo offers to save.
struct EnteredPassword {
    domain: String,
    username: String,
    password: String,
}

impl Tab {
    /// The URL to show in the location field, which is empty for a new tab.
    fn location_text(&self) -> String {
        if let Some(url) = self.stand_in.borrow().as_ref() {
            return url.clone();
        }
        match self.webview.uri() {
            Some(url) if url != "about:blank" => url.to_string(),
            _ => String::new(),
        }
    }

    fn display_title(&self) -> String {
        match self.webview.title() {
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

    /// The page shown, as a bookmark of it would be.
    fn page(&self) -> Option<Bookmark> {
        let url = self.location_text();
        if url.is_empty() {
            return None;
        }
        let title = match self.webview.title() {
            Some(title) if !title.trim().is_empty() => title.to_string(),
            _ => url.clone(),
        };
        Some(Bookmark { url, title })
    }

    /// Reloads the page, or tries loading it again after it crashed or was
    /// blocked. A forced reload fetches everything again instead of using the
    /// cache.
    fn reload(&self, force: bool) {
        match self.stand_in.take() {
            Some(url) => self.load_for_user(&url),
            None if force => self.webview.reload_bypass_cache(),
            None => self.webview.reload(),
        }
    }

    /// Loads `url` because the user asked for it, such as from the location
    /// field, which can open a local address from any page.
    fn load_for_user(&self, url: &str) {
        if let Ok(parsed) = Url::parse(url) {
            self.user_loads.borrow_mut().push((parsed, Instant::now()));
        }
        self.webview.load_uri(url);
    }

    /// Whether the navigation to `target` is one the user asked for, which
    /// it's then no longer waiting for.
    fn take_user_navigation(&self, target: &Url, kind: webkit::NavigationType) -> bool {
        if kind == webkit::NavigationType::BackForward && self.user_history.take() {
            return true;
        }
        let mut loads = self.user_loads.borrow_mut();
        loads.retain(|(_, asked)| asked.elapsed() < USER_LOAD_TIME_LIMIT);
        match loads.iter().position(|(url, _)| url == target) {
            Some(position) => {
                loads.remove(position);
                true
            }
            None => false,
        }
    }

    /// Shows a page of BrowseGoo's own instead of the one at `url`.
    fn show_stand_in(&self, url: String, reason: StandInReason) {
        let html = match reason {
            StandInReason::Crashed => CRASHED_PAGE.to_owned(),
            StandInReason::Blocked => blocked_page(&url),
            StandInReason::LocalNetwork(from) => local_network_page(&url, &from),
            StandInReason::PublicNameForLocalAddress => public_name_page(&url),
        };
        self.stand_in.replace(Some(url));
        self.loading_stand_in.set(true);
        self.webview.load_alternate_html(&html, "about:blank", None);
    }


    /// How far the page has loaded, for the location field, which shows
    /// nothing once it's done.
    fn progress(&self) -> f64 {
        if self.webview.is_loading() {
            self.webview.estimated_load_progress().max(0.1)
        } else {
            0.0
        }
    }
}

struct Browser {
    /// In no particular order. The tab strip has the order the user sees.
    tabs: RefCell<Vec<Rc<Tab>>>,
    next_tab_id: Cell<TabId>,
    widgets: Widgets,
    dirs: Dirs,
    network_session: webkit::NetworkSession,
    settings: webkit::Settings,
    bookmarks: RefCell<Bookmarks>,
    /// The cookies and site data that aren't deleted on quitting.
    preserved: Rc<RefCell<Preserved>>,
    passwords: Rc<RefCell<Passwords>>,
    /// Whether the fonts that pages can use are limited.
    page_fonts: page_fonts::State,
    /// Whether the site data is being deleted, before quitting.
    quitting: Cell<bool>,
    /// Where WebKit keeps the compiled filter lists.
    filter_store: webkit::UserContentFilterStore,
    /// The compiled filter lists in use.
    filters: RefCell<Vec<webkit::UserContentFilter>>,
    /// Whether the filter lists have been loaded since starting.
    filters_loaded: Cell<bool>,
    /// The content blocker that keeps pages from reaching local addresses,
    /// which every tab uses, even when it isn't filtered.
    local_filter: RefCell<Option<webkit::UserContentFilter>>,
    /// The local hosts and ports that pages on them can reach, as the user
    /// has gone to them.
    local_origins: RefCell<Vec<LocalOrigin>>,
    /// Counts compilations of the local network rules, so that only the
    /// newest one is used.
    local_generation: Cell<u64>,
    /// WebKit connects through it, so that public names can't be used to
    /// reach local addresses.
    proxy: SocksProxy,
    filter_settings: RefCell<filter_lists::Settings>,
    list_statuses: RefCell<BTreeMap<&'static str, ListStatus>>,
    /// Counts filter reloads, so that only the newest one is used.
    filter_generation: Cell<u64>,
    /// The sites that the filter lists in use block entirely, whose cookies
    /// and site data are deleted.
    blocked_sites: RefCell<Rc<BlockedSites>>,
    /// Whether deleting what blocked sites stored is waiting to start, and
    /// whether that includes data other than cookies.
    blocked_deleting_scheduled: Cell<Option<bool>>,
    /// Whether it's being deleted.
    blocked_deleting: Cell<bool>,
    /// The tab whose page was last searched with the find bar.
    find_tab: Cell<Option<TabId>>,
    updating_lists: Cell<bool>,
    /// Whether lists are being downloaded, and what the preferences say
    /// about it, for when they're opened.
    list_download_state: RefCell<(bool, String)>,
    /// The page to load once the filters are first ready, so that it's filtered.
    start_page: RefCell<Option<(TabId, Url)>>,
    preferences: RefCell<Option<PreferencesWindow>>,
    bookmarks_editor: RefCell<Option<Rc<BookmarksEditor>>>,
    site_data: RefCell<Option<Rc<SiteDataWindow>>>,
    password_manager: RefCell<Option<Rc<PasswordManager>>>,
    user_filters: RefCell<Option<Rc<UserFiltersWindow>>>,
}

/// Opens the browser window at `start_url`, once the cookies and site data
/// that aren't preserved are deleted, in case BrowseGoo didn't quit cleanly.
pub fn open(app: &gtk::Application, dirs: Dirs, start_url: Url, page_fonts: page_fonts::State) {
    if current_browser().is_some() {
        return;
    }
    // Before any web process starts, so that they can read the font settings.
    if let (page_fonts::State::Limited { .. }, Some(context)) =
        (&page_fonts, webkit::WebContext::default())
    {
        context.add_path_to_sandbox(page_fonts::config_dir(&dirs.cache), true);
    }

    let network_session = webkit::NetworkSession::new(
        dirs.data.to_str(),
        dirs.cache.join("web").to_str(),
    );
    // Every connection goes through it, including to local addresses.
    let proxy = SocksProxy::start().unwrap_or_else(|error| {
        // Carrying on would let public names reach local addresses.
        eprintln!("Could not start BrowseGoo's proxy: {error}");
        std::process::exit(1);
    });
    network_session.set_proxy_settings(
        webkit::NetworkProxyMode::Custom,
        Some(&webkit::NetworkProxySettings::new(Some(&proxy.uri), &[])),
    );
    if let Some(cookies) = network_session.cookie_manager() {
        if let Some(path) = dirs.data.join("cookies.sqlite").to_str() {
            cookies.set_persistent_storage(path, webkit::CookiePersistentStorage::Sqlite);
        }
    }
    let preserved = Preserved::load(dirs.config.join("preserved_site_data.tsv"))
        .unwrap_or_else(|error| {
            // Carrying on would delete what was meant to be preserved.
            eprintln!("Could not read which site data to preserve: {error}");
            std::process::exit(1);
        });
    let preserved = Rc::new(RefCell::new(preserved));

    // Nothing is loaded until this is done, and the application waits for it,
    // though it has no window yet.
    let hold = app.hold();
    let app = app.clone();
    glib::spawn_future_local(async move {
        if let (Some(cookies), Some(data)) = (
            network_session.cookie_manager(),
            network_session.website_data_manager(),
        ) {
            site_data::delete_unpreserved(&cookies, &data, &preserved).await;
        }
        open_window(&app, dirs, start_url, network_session, preserved, page_fonts, proxy);
        drop(hold);
    });
}

fn open_window(
    app: &gtk::Application,
    dirs: Dirs,
    start_url: Url,
    network_session: webkit::NetworkSession,
    preserved: Rc<RefCell<Preserved>>,
    page_fonts: page_fonts::State,
    proxy: SocksProxy,
) {
    let settings = webkit::Settings::new();
    settings.set_user_agent(Some(&user_agent()));

    let bookmarks = Bookmarks::load(dirs.config.join("bookmarks.tsv")).unwrap_or_else(|error| {
        // Carrying on would overwrite the bookmarks that we couldn't read.
        eprintln!("Could not read bookmarks: {error}");
        std::process::exit(1);
    });
    let filter_settings = filter_lists::Settings::load(dirs.config.join("filter_lists.tsv"))
        .unwrap_or_else(|error| {
            // Carrying on would overwrite the settings that we couldn't read.
            eprintln!("Could not read filter list settings: {error}");
            std::process::exit(1);
        });
    let passwords = Passwords::load(dirs.config.join("passwords.tsv")).unwrap_or_else(|error| {
        // Carrying on would overwrite the passwords that we couldn't read.
        eprintln!("Could not read saved passwords: {error}");
        std::process::exit(1);
    });
    let filter_store = webkit::UserContentFilterStore::new(
        &filter_lists::compiled_dir(&dirs.cache).to_string_lossy(),
    );

    let browser = Rc::new(Browser {
        tabs: RefCell::new(Vec::new()),
        next_tab_id: Cell::new(1),
        widgets: build_widgets(app),
        dirs,
        network_session,
        settings,
        bookmarks: RefCell::new(bookmarks),
        preserved,
        passwords: Rc::new(RefCell::new(passwords)),
        page_fonts,
        quitting: Cell::new(false),
        filter_store,
        filters: RefCell::new(Vec::new()),
        filters_loaded: Cell::new(false),
        local_filter: RefCell::new(None),
        local_origins: RefCell::new(Vec::new()),
        local_generation: Cell::new(0),
        proxy,
        filter_settings: RefCell::new(filter_settings),
        list_statuses: RefCell::new(BTreeMap::new()),
        filter_generation: Cell::new(0),
        blocked_sites: RefCell::new(Rc::new(BlockedSites::default())),
        blocked_deleting_scheduled: Cell::new(None),
        blocked_deleting: Cell::new(false),
        find_tab: Cell::new(None),
        updating_lists: Cell::new(false),
        list_download_state: RefCell::new((false, String::new())),
        start_page: RefCell::new(None),
        preferences: RefCell::new(None),
        bookmarks_editor: RefCell::new(None),
        site_data: RefCell::new(None),
        password_manager: RefCell::new(None),
        user_filters: RefCell::new(None),
    });
    BROWSER.set(Some(browser.clone()));

    browser.connect_widgets();
    browser.connect_actions(app);
    browser.rebuild_bookmark_list();
    if start_url.as_str() == "about:blank" {
        browser.open_tab(None);
    } else {
        let tab = browser.add_tab(None, false, None, true);
        browser.widgets.location.set_text(start_url.as_str());
        browser.start_page.replace(Some((tab.id, start_url)));
    }
    browser.reload_filters();
    browser.download_missing_lists();
    browser.compile_local_filter(None);

    browser.widgets.window.present();
    browser.focus_page_or_location();
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
    let main_menu = gio::Menu::new();
    main_menu.append(Some("Bookmarks"), Some("win.edit-bookmarks"));
    main_menu.append(Some("Password Manager"), Some("win.passwords"));
    main_menu.append(Some("Site Data Manager"), Some("win.site-data"));
    main_menu.append(Some("Your Filter Rules"), Some("win.user-filters"));
    main_menu.append(Some("Preferences"), Some("win.preferences"));
    main_menu.append(Some("About BrowseGoo"), Some("win.about"));
    let main_menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main menu")
        .menu_model(&main_menu)
        .build();

    let filter_menu = gio::Menu::new();
    filter_menu.append(Some("Filter This Tab"), Some("win.filter-tab"));
    filter_menu.append(Some("Your Filter Rules"), Some("win.user-filters"));
    filter_menu.append(Some("Preferences"), Some("win.preferences"));
    let filter_button = gtk::MenuButton::builder()
        .icon_name("security-high-symbolic")
        .menu_model(&filter_menu)
        .build();

    let location = gtk::Entry::builder()
        .hexpand(true)
        .input_purpose(gtk::InputPurpose::Url)
        .placeholder_text("Enter a location")
        .build();

    // Only while there's a password to save, so it's out of the way otherwise.
    let save_password = gtk::Button::builder()
        .icon_name("dialog-password-symbolic")
        .tooltip_text("Save the password entered on this page")
        .action_name("win.save-password")
        .visible(false)
        .build();

    let bookmark = gtk::ToggleButton::builder()
        .icon_name("browsegoo-star-outline-symbolic")
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

    let toolbar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .css_classes(["toolbar"])
        .build();
    for widget in [
        back.upcast_ref::<gtk::Widget>(),
        forward.upcast_ref(),
        reload.upcast_ref(),
        location.upcast_ref(),
        save_password.upcast_ref(),
        filter_button.upcast_ref(),
        bookmark.upcast_ref(),
        bookmarks_button.upcast_ref(),
        main_menu_button.upcast_ref(),
    ] {
        toolbar.append(widget);
    }

    let tabs = gtk::Notebook::builder()
        .scrollable(true)
        .show_border(false)
        .hexpand(true)
        .valign(gtk::Align::End)
        .build();
    let new_tab = gtk::Button::builder()
        .icon_name("tab-new-symbolic")
        .tooltip_text("New tab (Ctrl+T)")
        .action_name("win.new-tab")
        .has_frame(false)
        .build();
    tabs.set_action_widget(&new_tab, gtk::PackType::End);

    let pages = gtk::Stack::builder().hexpand(true).vexpand(true).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let title_bar = if decorations::draw_own_title_bar() {
        // The empty parts of the title bar still move the window, as usual.
        let title_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        title_bar.append(&tabs);
        title_bar.append(&gtk::WindowControls::new(gtk::PackType::End));
        Some(gtk::WindowHandle::builder().child(&title_bar).build())
    } else {
        // The window manager's title bar is above, so the tabs are at the top
        // of the window's content.
        content.append(&tabs);
        None
    };
    content.append(&toolbar);
    content.append(&pages);
    let find_bar = FindBar::new();
    content.append(&find_bar.revealer);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("BrowseGoo")
        .default_width(INITIAL_WIDTH)
        .default_height(INITIAL_HEIGHT)
        .child(&content)
        .build();
    if let Some(title_bar) = &title_bar {
        window.set_titlebar(Some(title_bar));
    }

    Widgets {
        window,
        tabs,
        pages,
        find_bar,
        location_bar: LocationBar::new(
            &location,
            || {
                current_browser()
                    .map(|browser| browser.bookmarks.borrow().items().to_vec())
                    .unwrap_or_default()
            },
            |url| {
                if let (Some(browser), Some(url)) = (current_browser(), parse_location(url)) {
                    browser.navigate(url);
                }
            },
        ),
        location,
        bookmark,
        bookmark_list,
        bookmarks_popover,
        filter_button,
        save_password,
    }
}

impl Browser {
    fn active_tab(&self) -> Option<Rc<Tab>> {
        let notebook = &self.widgets.tabs;
        let page = notebook.nth_page(notebook.current_page())?;
        self.tabs
            .borrow()
            .iter()
            .find(|tab| tab.placeholder.upcast_ref::<gtk::Widget>() == &page)
            .cloned()
    }

    fn tab(&self, id: TabId) -> Option<Rc<Tab>> {
        self.tabs.borrow().iter().find(|tab| tab.id == id).cloned()
    }

    fn is_active(&self, id: TabId) -> bool {
        self.active_tab().is_some_and(|tab| tab.id == id)
    }

    /// The active tab's view.
    fn webview(&self) -> Option<webkit::WebView> {
        self.active_tab().map(|tab| tab.webview.clone())
    }

    /// Opens a new tab, at `url` or blank, and makes it the active tab.
    fn open_tab(self: &Rc<Self>, url: Option<Url>) -> Rc<Tab> {
        let tab = self.add_tab(None, url.is_none(), None, true);
        if let Some(url) = url {
            tab.webview.load_uri(url.as_str());
        }
        tab
    }

    /// Adds a tab, whose page the `opener` tab may have opened, and makes it the
    /// active tab if `activate`. A page that opens another, such as with
    /// `window.open()`, needs the new one to be `related` to its own view.
    fn add_tab(
        self: &Rc<Self>,
        related: Option<&webkit::WebView>,
        opened_blank: bool,
        opener: Option<TabId>,
        activate: bool,
    ) -> Rc<Tab> {
        let id = self.next_tab_id.get();
        self.next_tab_id.set(id + 1);

        let content = webkit::UserContentManager::new();
        let builder = webkit::WebView::builder()
            .settings(&self.settings)
            .user_content_manager(&content)
            .hexpand(true)
            .vexpand(true);
        // A related view shares the other's network session.
        let webview = match related {
            Some(related) => builder.related_view(related).build(),
            None => builder.network_session(&self.network_session).build(),
        };

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
            id,
            webview,
            content,
            placeholder: gtk::Box::new(gtk::Orientation::Vertical, 0),
            label,
            opened_blank,
            filtering: Cell::new(true),
            opener,
            stand_in: RefCell::new(None),
            loading_stand_in: Cell::new(false),
            entered_password: RefCell::new(None),
            committed: RefCell::new(None),
            user_loads: RefCell::new(Vec::new()),
            user_history: Cell::new(false),
        });
        tab.update_label();
        self.apply_filters(&tab);
        privacy::add_scripts(&tab.content);
        password_prompts::watch_pages(&tab.content, move |message| {
            if let Some(browser) = current_browser() {
                browser.page_password_message(id, message);
            }
        });
        self.connect_webview(&tab);

        // The tab must be known before the tab strip announces it as active.
        self.tabs.borrow_mut().push(tab.clone());
        self.widgets.pages.add_child(&tab.webview);
        let position = opener.and_then(|opener| self.position_for_tab_from(opener));
        let notebook = &self.widgets.tabs;
        let index = notebook.insert_page(&tab.placeholder, Some(&tab_label), position);
        notebook.set_tab_reorderable(&tab.placeholder, true);
        if activate {
            notebook.set_current_page(Some(index));
        }
        tab
    }

    /// Keeps the tab and toolbar up to date with a tab's page, and handles what
    /// the page asks of the browser.
    fn connect_webview(&self, tab: &Tab) {
        let id = tab.id;
        let webview = &tab.webview;

        webview.connect_title_notify(move |_| {
            if let Some(browser) = current_browser() {
                browser.page_changed(id);
            }
        });
        webview.connect_uri_notify(move |_| {
            if let Some(browser) = current_browser() {
                browser.page_changed(id);
            }
        });
        let show_progress = move || {
            if let Some(browser) = current_browser() {
                if let (true, Some(tab)) = (browser.is_active(id), browser.tab(id)) {
                    browser
                        .widgets
                        .location
                        .set_progress_fraction(tab.progress());
                }
            }
        };
        webview.connect_estimated_load_progress_notify(move |_| show_progress());
        webview.connect_is_loading_notify(move |_| show_progress());

        webview.connect_load_changed(move |_, event| {
            let Some(browser) = current_browser() else {
                return;
            };
            let Some(tab) = browser.tab(id) else {
                return;
            };
            match event {
                webkit::LoadEvent::Started => {
                    if !tab.loading_stand_in.take() {
                        tab.stand_in.replace(None);
                    }
                    tab.user_history.set(false);
                }
                webkit::LoadEvent::Committed => {
                    let url = tab.webview.uri().and_then(|url| Url::parse(&url).ok());
                    tab.committed.replace(url);
                }
                // WebKit doesn't say when site data other than cookies
                // changes, so it's looked at as pages load, and again a
                // little later, as the HTTP cache is written afterwards.
                webkit::LoadEvent::Finished => {
                    browser.delete_blocked_soon(true);
                    glib::timeout_add_local_once(DELETING_BLOCKED_AGAIN, || {
                        if let Some(browser) = current_browser() {
                            browser.delete_blocked_soon(true);
                        }
                    });
                }
                _ => {}
            }
        });
        if let Some(history) = webview.back_forward_list() {
            // The bindings leave this signal out, as it has untyped arguments.
            history.connect_local("changed", false, move |_| {
                if let Some(browser) = current_browser() {
                    browser.page_changed(id);
                }
                None
            });
        }

        // A link clicked with the middle button, or with Ctrl, opens in a new
        // tab in the background, or in the foreground if Shift is also held.
        webview.connect_decide_policy(move |_, decision, kind| {
            if !matches!(
                kind,
                webkit::PolicyDecisionType::NavigationAction
                    | webkit::PolicyDecisionType::NewWindowAction
            ) {
                return false;
            }
            let Some(decision) = decision.downcast_ref::<webkit::NavigationPolicyDecision>() else {
                return false;
            };
            let Some(action) = decision.navigation_action() else {
                return false;
            };
            if let Some(browser) = current_browser() {
                if browser.decide_local_navigation(id, decision, &action) {
                    return true;
                }
            }
            let modifiers = gdk::ModifierType::from_bits_truncate(action.modifiers());
            let new_tab = action.navigation_type() == webkit::NavigationType::LinkClicked
                && (action.mouse_button() == gdk::BUTTON_MIDDLE
                    || modifiers.contains(gdk::ModifierType::CONTROL_MASK));
            let url = action
                .request()
                .and_then(|request| request.uri())
                .and_then(|url| Url::parse(&url).ok());
            let (true, Some(url), Some(browser)) = (new_tab, url, current_browser()) else {
                return false;
            };
            decision.ignore();
            let activate = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
            let tab = browser.add_tab(None, false, Some(id), activate);
            tab.webview.load_uri(url.as_str());
            true
        });

        // A page that a filter rule blocks fails to load, saying so.
        webview.connect_load_failed(move |_, _, url, error| {
            let Some(tab) = current_browser().and_then(|browser| browser.tab(id)) else {
                return false;
            };
            if error.matches(webkit::PolicyError::__Unknown(BLOCKED_BY_CONTENT_BLOCKER)) {
                tab.show_stand_in(url.to_owned(), StandInReason::Blocked);
                return true;
            }
            let refused = current_browser().is_some_and(|browser| {
                Url::parse(url).ok().is_some_and(|url| {
                    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
                        return false;
                    };
                    browser.proxy.refused(host, port)
                })
            });
            if refused {
                tab.show_stand_in(url.to_owned(), StandInReason::PublicNameForLocalAddress);
                return true;
            }
            false
        });

        // A page that opens a new window, such as with a `target="_blank"` link
        // or `window.open()`, gets a new tab instead.
        webview.connect_create(move |webview, _| {
            let browser = current_browser()?;
            let tab = browser.add_tab(Some(webview), false, Some(id), true);
            Some(tab.webview.clone().upcast())
        });

        // A page closed itself, such as with `window.close()`.
        webview.connect_close(move |_| {
            // Not while WebKit is still telling us.
            glib::idle_add_local_once(move || close_tab_by_id(id));
        });

        if let Some(finder) = webview.find_controller() {
            let show = move |matches| {
                if let Some(browser) = current_browser() {
                    if browser.find_tab.get() == Some(id) {
                        browser.widgets.find_bar.show_matches(Some(matches));
                    }
                }
            };
            let found = show.clone();
            finder.connect_found_text(move |_, matches| found(matches));
            finder.connect_failed_to_find_text(move |_| show(0));
        }

        webview.connect_web_process_terminated(move |_, reason| {
            if reason == webkit::WebProcessTerminationReason::TerminatedByApi {
                return;
            }
            let Some(tab) = current_browser().and_then(|browser| browser.tab(id)) else {
                return;
            };
            let url = tab.location_text();
            if reason == webkit::WebProcessTerminationReason::ExceededMemoryLimit {
                eprintln!("A page used too much memory, and was stopped: {url}");
            } else {
                eprintln!("A page crashed: {url}");
            }
            tab.show_stand_in(url, StandInReason::Crashed);
        });
    }

    /// Shows a change in a tab's page, such as a new address or title.
    fn page_changed(&self, id: TabId) {
        let Some(tab) = self.tab(id) else {
            return;
        };
        tab.update_label();
        if !self.is_active(id) {
            return;
        }
        self.set_window_title(tab.webview.title().as_deref());
        if !self.is_editing_location() {
            self.widgets.location_bar.set_text(&tab.location_text());
        }
        self.set_history_available(tab.webview.can_go_back(), tab.webview.can_go_forward());
        self.update_bookmark_button();
    }

    /// Where a tab opened from the `opener` tab goes: after it, and after the
    /// tabs it already opened, so that they stay in the order they were opened.
    fn position_for_tab_from(&self, opener: TabId) -> Option<u32> {
        let notebook = &self.widgets.tabs;
        let mut position = notebook.page_num(&self.tab(opener)?.placeholder)? + 1;
        while let Some(page) = notebook.nth_page(Some(position)) {
            let opened_by_same_tab = self.tabs.borrow().iter().any(|tab| {
                tab.placeholder.upcast_ref::<gtk::Widget>() == &page && tab.opener == Some(opener)
            });
            if !opened_by_same_tab {
                break;
            }
            position += 1;
        }
        Some(position)
    }

    fn close_tab(&self, id: TabId) {
        let Some(tab) = self.tab(id) else {
            return;
        };
        if self.tabs.borrow().len() == 1 {
            self.widgets.window.close();
            return;
        }
        self.tabs.borrow_mut().retain(|other| other.id != id);
        // Ends the page now, such as a video playing in it, rather than only
        // once nothing refers to its view any more. Muting silences it at
        // once, and the blank page replaces it, stopping its sound and scripts.
        tab.webview.set_is_muted(true);
        tab.webview.stop_loading();
        tab.webview.load_uri("about:blank");
        // This makes another tab active, if this one was.
        self.widgets.tabs.detach_tab(&tab.placeholder);
        // Which also ends the page, once nothing else refers to its view.
        self.widgets.pages.remove(&tab.webview);
    }

    /// Shows the newly active tab's page and state.
    fn activate_current_tab(&self) {
        let Some(active) = self.active_tab() else {
            return;
        };
        self.widgets.pages.set_visible_child(&active.webview);
        self.set_window_title(active.webview.title().as_deref());
        self.widgets.location_bar.set_text(&active.location_text());
        self.widgets
            .location
            .set_progress_fraction(active.progress());
        self.set_history_available(active.webview.can_go_back(), active.webview.can_go_forward());
        self.update_bookmark_button();
        self.update_filter_button();
        self.update_save_password_button();
        if self.widgets.find_bar.is_open() {
            self.find(false);
        }
        self.focus_page_or_location();
    }

    fn find_options(&self, backwards: bool) -> u32 {
        let mut options = webkit::FindOptions::WRAP_AROUND;
        if !self.widgets.find_bar.match_case.is_active() {
            options |= webkit::FindOptions::CASE_INSENSITIVE;
        }
        if backwards {
            options |= webkit::FindOptions::BACKWARDS;
        }
        options.bits()
    }

    /// Stops showing matches in the tab last searched, if there is one.
    fn finish_finding(&self) {
        let tab = self.find_tab.take().and_then(|id| self.tab(id));
        if let Some(finder) = tab.and_then(|tab| tab.webview.find_controller()) {
            finder.search_finish();
        }
    }

    /// Looks for what's in the find bar in the active tab's page, from the
    /// start, as when it's typed.
    fn find(&self, backwards: bool) {
        self.finish_finding();
        let text = self.widgets.find_bar.text();
        let Some(tab) = self.active_tab() else {
            return;
        };
        let Some(finder) = tab.webview.find_controller() else {
            return;
        };
        if text.is_empty() {
            self.widgets.find_bar.show_matches(None);
            return;
        }
        self.find_tab.set(Some(tab.id));
        finder.search(&text, self.find_options(backwards), MOST_MATCHES);
    }

    /// Goes to the next or previous match, opening the find bar if it's closed.
    fn find_again(&self, backwards: bool) {
        let bar = &self.widgets.find_bar;
        if !bar.is_open() {
            bar.open();
        }
        let Some(tab) = self.active_tab() else {
            return;
        };
        let finder = tab.webview.find_controller();
        let searched = finder.as_ref().and_then(|finder| finder.search_text());
        let same_search = self.find_tab.get() == Some(tab.id)
            && searched.is_some_and(|searched| searched == bar.text());
        match finder {
            Some(finder) if same_search && backwards => finder.search_previous(),
            Some(finder) if same_search => finder.search_next(),
            _ => self.find(backwards),
        }
    }

    fn close_find_bar(&self) {
        self.finish_finding();
        self.widgets.find_bar.close();
        if let Some(webview) = self.webview() {
            webview.grab_focus();
        }
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
            tab.webview.grab_focus();
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

    fn navigate(&self, url: Url) {
        if let Some(tab) = self.active_tab() {
            tab.stand_in.replace(None);
            tab.load_for_user(url.as_str());
            tab.webview.grab_focus();
        }
    }

    fn connect_widgets(self: &Rc<Self>) {
        let widgets = &self.widgets;

        if let Some(cookies) = self.network_session.cookie_manager() {
            cookies.connect_changed(|_| {
                if let Some(browser) = current_browser() {
                    browser.delete_blocked_soon(false);
                }
            });
        }

        let find_bar = &widgets.find_bar;
        let with_browser = |action: fn(&Browser)| {
            move || {
                if let Some(browser) = current_browser() {
                    action(&browser);
                }
            }
        };
        let find = with_browser(|browser| browser.find(false));
        find_bar.entry.connect_search_changed(move |_| find());
        let find = with_browser(|browser| browser.find(false));
        find_bar.match_case.connect_toggled(move |_| find());
        let next = with_browser(|browser| browser.find_again(false));
        find_bar.entry.connect_activate(move |_| next());
        let next = with_browser(|browser| browser.find_again(false));
        find_bar.next.connect_clicked(move |_| next());
        let previous = with_browser(|browser| browser.find_again(true));
        find_bar.previous.connect_clicked(move |_| previous());
        let close = with_browser(Browser::close_find_bar);
        find_bar.close.connect_clicked(move |_| close());
        let close = with_browser(Browser::close_find_bar);
        find_bar.entry.connect_stop_search(move |_| close());
        // Shift+Enter, for the previous match.
        let keys = gtk::EventControllerKey::new();
        let previous = with_browser(|browser| browser.find_again(true));
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let enter = matches!(key, gdk::Key::Return | gdk::Key::KP_Enter);
            if enter && modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                previous();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        find_bar.entry.add_controller(keys);

        widgets.tabs.connect_page_notify(|_| {
            if let Some(browser) = current_browser() {
                browser.activate_current_tab();
            }
        });

        widgets.location.connect_activate(|entry| {
            if let Some(browser) = current_browser() {
                let text = browser.widgets.location_bar.activation_text();
                match parse_location(&text) {
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

        widgets.window.connect_close_request(|_| match current_browser() {
            Some(browser) => {
                browser.quit();
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
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
            if let Some(tab) = browser.active_tab() {
                tab.user_history.set(true);
                tab.webview.go_back();
            }
        });
        add_action("forward", |browser| {
            if let Some(tab) = browser.active_tab() {
                tab.user_history.set(true);
                tab.webview.go_forward();
            }
        });
        add_action("reload", |browser| {
            if let Some(tab) = browser.active_tab() {
                tab.reload(false);
            }
        });
        add_action("force-reload", |browser| {
            if let Some(tab) = browser.active_tab() {
                tab.reload(true);
            }
        });
        add_action("focus-location", |browser| {
            browser.widgets.location.grab_focus();
        });
        add_action("new-tab", |_| {
            // Opening a tab needs the shared handle, rather than a borrowed one.
            if let Some(browser) = current_browser() {
                browser.open_tab(None);
            }
        });
        add_action("close-tab", |browser| {
            if let Some(tab) = browser.active_tab() {
                browser.close_tab(tab.id);
            }
        });
        add_action("next-tab", |browser| browser.select_tab_by_offset(1));
        add_action("previous-tab", |browser| browser.select_tab_by_offset(-1));
        add_action("toggle-bookmark", Browser::toggle_bookmark);
        add_action("about", Browser::show_about);
        add_action("site-data", Browser::show_site_data);
        add_action("passwords", Browser::show_password_manager);
        add_action("save-password", Browser::ask_to_save_password);
        add_action("find", |browser| browser.widgets.find_bar.open());
        add_action("find-next", |browser| browser.find_again(false));
        add_action("find-previous", |browser| browser.find_again(true));
        add_action("user-filters", Browser::show_user_filters);
        add_action("edit-bookmarks", |_| {
            // The window needs the shared handle, rather than a borrowed one.
            if let Some(browser) = current_browser() {
                browser.show_bookmarks_editor();
            }
        });
        add_action("preferences", |_| {
            // The window needs the shared handle, rather than a borrowed one.
            if let Some(browser) = current_browser() {
                browser.show_preferences();
            }
        });

        let filter_tab = gio::SimpleAction::new_stateful("filter-tab", None, &true.to_variant());
        filter_tab.connect_activate(|_, _| {
            if let Some(browser) = current_browser() {
                if let Some(tab) = browser.active_tab() {
                    tab.filtering.set(!tab.filtering.get());
                    browser.apply_filters(&tab);
                }
                browser.update_filter_button();
            }
        });
        window.add_action(&filter_tab);
        add_action("quit", |browser| browser.widgets.window.close());

        app.set_accels_for_action("win.back", &["<Alt>Left"]);
        app.set_accels_for_action("win.forward", &["<Alt>Right"]);
        app.set_accels_for_action("win.reload", &["<Control>r", "F5"]);
        app.set_accels_for_action("win.force-reload", &["<Control><Shift>r", "<Shift>F5"]);
        app.set_accels_for_action("win.focus-location", &["<Control>l"]);
        app.set_accels_for_action("win.find", &["<Control>f"]);
        app.set_accels_for_action("win.find-next", &["<Control>g", "F3"]);
        app.set_accels_for_action("win.find-previous", &["<Control><Shift>g", "<Shift>F3"]);
        app.set_accels_for_action("win.toggle-bookmark", &["<Control>d"]);
        app.set_accels_for_action("win.quit", &["<Control>q"]);
        app.set_accels_for_action("win.edit-bookmarks", &["<Control><Shift>o"]);
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
        if let Some(page) = self.active_tab().and_then(|tab| tab.page()) {
            let result = self.bookmarks.borrow_mut().toggle(&page.url, &page.title);
            if let Err(error) = result {
                eprintln!("Could not save bookmarks: {error}");
            }
        }
        self.bookmarks_changed();
    }

    /// Updates everything that shows the bookmarks.
    fn bookmarks_changed(&self) {
        self.rebuild_bookmark_list();
        self.update_bookmark_button();
        let editor = self.bookmarks_editor.borrow().clone();
        if let Some(editor) = editor {
            let items = self.bookmarks.borrow().items().to_vec();
            editor.show_bookmarks(&items);
        }
    }

    fn show_user_filters(&self) {
        if self.user_filters.borrow().is_none() {
            let rules = filter_lists::load_user_rules(&self.dirs.config)
                .map_err(|error| error.to_string());
            let window = UserFiltersWindow::new(&self.widgets.window, rules, |text| {
                let Some(browser) = current_browser() else {
                    return Ok(());
                };
                filter_lists::save_user_rules(&browser.dirs.config, text)
                    .map_err(|error| format!("Your rules couldn't be saved: {error}"))?;
                browser.reload_filters();
                Ok(())
            });
            self.user_filters.replace(Some(window));
            self.show_user_filters_status();
        }
        if let Some(window) = self.user_filters.borrow().as_ref() {
            window.present();
        }
    }

    /// Adds `added` to the end of the user's rules, and starts using them.
    fn add_user_rules(&self, added: &str) -> Result<(), String> {
        let rules = filter_lists::load_user_rules(&self.dirs.config)
            .map_err(|error| format!("Your rules couldn't be read: {error}"))?;
        let rules = block_sites::append_rules(&rules, added);
        filter_lists::save_user_rules(&self.dirs.config, &rules)
            .map_err(|error| format!("Your rules couldn't be saved: {error}"))?;
        if let Some(window) = self.user_filters.borrow().as_ref() {
            window.show_added(&rules, added);
        }
        self.reload_filters();
        Ok(())
    }

    fn show_list_statuses(&self, statuses: BTreeMap<&'static str, ListStatus>) {
        self.list_statuses.replace(statuses);
        if let Some(preferences) = self.preferences.borrow().as_ref() {
            preferences.show_statuses(&self.list_statuses.borrow());
        }
        self.show_user_filters_status();
    }

    fn show_user_filters_status(&self) {
        if let Some(window) = self.user_filters.borrow().as_ref() {
            let statuses = self.list_statuses.borrow();
            window.show_status(statuses.get(filter_lists::USER_RULES_ID));
        }
    }

    /// Deletes the cookies and site data that aren't preserved, and then
    /// closes the window, which quits.
    fn quit(&self) {
        if self.quitting.replace(true) {
            return;
        }
        for window in gtk::Window::list_toplevels() {
            window.set_visible(false);
        }
        // So that pages can't store anything more while it's being deleted.
        for tab in self.tabs.borrow().iter() {
            tab.webview.terminate_web_process();
        }
        let window = self.widgets.window.clone();
        let cookies = self.network_session.cookie_manager();
        let data = self.network_session.website_data_manager();
        let preserved = self.preserved.clone();
        glib::spawn_future_local(async move {
            if let (Some(cookies), Some(data)) = (cookies, data) {
                let deleting = site_data::delete_unpreserved(&cookies, &data, &preserved);
                if glib::future_with_timeout(DELETING_TIME_LIMIT, deleting).await.is_err() {
                    eprintln!("Quitting before all the site data that isn't preserved was deleted");
                }
            }
            // The tabs refer back to the browser, through their signal handlers.
            BROWSER.take();
            // Closing a hidden window might do nothing, so it's destroyed,
            // which leaves the application with no window, so it quits.
            window.destroy();
        });
    }

    /// Deletes the cookies that blocked sites stored, shortly, and with
    /// `data`, the sites' other data, unless they're preserved.
    fn delete_blocked_soon(&self, data: bool) {
        let scheduled = self.blocked_deleting_scheduled.get();
        self.blocked_deleting_scheduled
            .set(Some(data || scheduled == Some(true)));
        if scheduled.is_some() {
            return;
        }
        glib::timeout_add_local_once(DELETING_BLOCKED_DELAY, || {
            if let Some(browser) = current_browser() {
                browser.delete_blocked();
            }
        });
    }

    fn delete_blocked(&self) {
        if self.quitting.get() {
            return;
        }
        // Deleting cookies changes them, which schedules this again, to wait
        // for until this is done.
        if self.blocked_deleting.get() {
            glib::timeout_add_local_once(DELETING_BLOCKED_DELAY, || {
                if let Some(browser) = current_browser() {
                    browser.delete_blocked();
                }
            });
            return;
        }
        let Some(data) = self.blocked_deleting_scheduled.take() else {
            return;
        };
        let (Some(cookies), Some(manager)) = (
            self.network_session.cookie_manager(),
            self.network_session.website_data_manager(),
        ) else {
            return;
        };
        self.blocked_deleting.set(true);
        let preserved = self.preserved.clone();
        let blocked = self.blocked_sites.borrow().clone();
        glib::spawn_future_local(async move {
            let manager = data.then_some(&manager);
            site_data::delete_blocked(&cookies, manager, &preserved, &blocked).await;
            if let Some(browser) = current_browser() {
                browser.blocked_deleting.set(false);
            }
        });
    }

    fn show_site_data(&self) {
        if self.site_data.borrow().is_none() {
            let (Some(cookies), Some(data)) = (
                self.network_session.cookie_manager(),
                self.network_session.website_data_manager(),
            ) else {
                return;
            };
            let window = SiteDataWindow::new(
                &self.widgets.window,
                cookies,
                data,
                self.preserved.clone(),
                |rules| match current_browser() {
                    Some(browser) => browser.add_user_rules(rules),
                    None => Ok(()),
                },
            );
            self.site_data.replace(Some(window));
        }
        let window = self.site_data.borrow().clone();
        if let Some(window) = window {
            window.present();
        }
    }

    fn show_password_manager(&self) {
        if self.password_manager.borrow().is_none() {
            let window = PasswordManager::new(&self.widgets.window, self.passwords.clone());
            self.password_manager.replace(Some(window));
        }
        let window = self.password_manager.borrow().clone();
        if let Some(window) = window {
            window.present();
        }
    }

    /// Shows the saved passwords again in the Password Manager, if it's open.
    fn passwords_changed(&self) {
        let window = self.password_manager.borrow().clone();
        if let Some(window) = window {
            window.refresh();
        }
    }

    /// Handles what the script in a tab's page says about its forms. Only the
    /// page's own domain is used, and only when the tab is still on it.
    fn page_password_message(&self, id: TabId, message: PageMessage) {
        let Some(tab) = self.tab(id) else {
            return;
        };
        let url = tab.webview.uri().map(String::from).unwrap_or_default();
        let Some(domain) = passwords::domain_of(&url) else {
            return;
        };
        match message {
            PageMessage::Entered { host, username, password } => {
                if host != domain || self.passwords.borrow().contains(&domain, &username, &password) {
                    return;
                }
                tab.entered_password.replace(Some(EnteredPassword {
                    domain,
                    username,
                    password,
                }));
                if self.is_active(id) {
                    self.update_save_password_button();
                }
            }
            PageMessage::Clicked { host, area } => {
                if host != domain || !self.is_active(id) {
                    return;
                }
                let records = self.passwords.borrow().for_domain(&domain);
                if records.is_empty() {
                    return;
                }
                let secure = url.starts_with("https:");
                password_prompts::show_menu(&tab.webview, &area, &records, secure, move |record| {
                    if let Some(browser) = current_browser() {
                        browser.fill_password(id, record);
                    }
                });
            }
        }
    }

    /// Fills in the saved password that was chosen from the menu in a tab.
    fn fill_password(&self, id: TabId, record: passwords::RecordId) {
        let (Some(tab), Some(record)) = (self.tab(id), self.passwords.borrow().get(record).cloned())
        else {
            return;
        };
        password_prompts::fill(&tab.webview, &record, move || {
            let Some(browser) = current_browser() else {
                return;
            };
            if let Err(error) = browser.passwords.borrow_mut().mark_used(record.id) {
                eprintln!("Could not save when a password was used: {error}");
            }
            browser.passwords_changed();
        });
    }

    fn update_save_password_button(&self) {
        let entered = self
            .active_tab()
            .and_then(|tab| tab.entered_password.borrow().as_ref().map(|entered| entered.domain.clone()));
        let button = &self.widgets.save_password;
        if let Some(domain) = &entered {
            button.set_tooltip_text(Some(&format!("Save the password entered on {domain}")));
        }
        button.set_visible(entered.is_some());
    }

    /// Asks whether to save the password entered in the active tab, which is
    /// then no longer offered, whether it's saved or not.
    fn ask_to_save_password(&self) {
        let Some(entered) = self.active_tab().and_then(|tab| tab.entered_password.take()) else {
            return;
        };
        self.update_save_password_button();
        let domain = entered.domain.clone();
        password_prompts::ask_to_save(
            &self.widgets.window,
            &entered.domain,
            &entered.username,
            &entered.password,
            move |username, password| {
                let Some(browser) = current_browser() else {
                    return Ok(());
                };
                let added = browser.passwords.borrow_mut().add(&domain, username, password);
                added.map_err(|error| format!("Could not save the password: {error}"))?;
                browser.passwords_changed();
                Ok(())
            },
        );
    }

    fn show_bookmarks_editor(self: &Rc<Self>) {
        if self.bookmarks_editor.borrow().is_none() {
            let editor = BookmarksEditor::new(
                &self.widgets.window,
                |edit| match current_browser() {
                    Some(browser) => browser.edit_bookmarks(edit),
                    None => Ok(()),
                },
                |url| {
                    if let (Some(browser), Some(url)) = (current_browser(), parse_location(url)) {
                        browser.navigate(url);
                    }
                },
                || current_browser().and_then(|browser| browser.current_page()),
            );
            editor.show_bookmarks(self.bookmarks.borrow().items());
            self.bookmarks_editor.replace(Some(editor));
        }
        let editor = self.bookmarks_editor.borrow().clone();
        if let Some(editor) = editor {
            editor.present();
        }
    }

    /// The active tab's page, as a bookmark of it would be.
    fn current_page(&self) -> Option<Bookmark> {
        let page = self.active_tab()?.page()?;
        let is_web_page = Url::parse(&page.url)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
        is_web_page.then_some(page)
    }

    fn edit_bookmarks(&self, edit: Edit) -> Result<(), String> {
        let result = {
            let mut bookmarks = self.bookmarks.borrow_mut();
            let saved = |result: std::io::Result<()>| {
                result.map_err(|error| format!("Could not save bookmarks: {error}"))
            };
            match edit {
                Edit::Add { index, url, title } => bookmarks.insert(index, &url, &title),
                Edit::Change { index, url, title } => bookmarks.edit(index, &url, &title),
                Edit::Remove(index) => saved(bookmarks.remove(index)),
                Edit::Move { from, to } => saved(bookmarks.move_item(from, to)),
            }
        };
        if result.is_ok() {
            // Not otherwise, which would replace what was typed in the editor.
            self.bookmarks_changed();
        }
        result
    }

    /// Shows whether the active tab is filtered. WebKit doesn't say what its
    /// filters block, as blocked requests never reach this process.
    fn update_filter_button(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let filtering = tab.filtering.get();
        if let Some(action) = self
            .widgets
            .window
            .lookup_action("filter-tab")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_state(&filtering.to_variant());
        }

        let button = &self.widgets.filter_button;
        button.set_icon_name(if filtering {
            "security-high-symbolic"
        } else {
            "security-low-symbolic"
        });
        let tooltip = match (filtering, self.filters.borrow().len()) {
            (false, _) => "Filtering is off for this tab".to_owned(),
            (true, 0) => "Filtering is on, but no filter lists are in use yet".to_owned(),
            (true, 1) => "Filtering is on, with 1 filter list".to_owned(),
            (true, count) => format!("Filtering is on, with {count} filter lists"),
        };
        button.set_tooltip_text(Some(&tooltip));
    }

    /// Converts the cached lists for WebKit, in the background, and compiles
    /// those that changed. Then it uses them in every tab.
    fn reload_filters(&self) {
        let generation = self.filter_generation.get() + 1;
        self.filter_generation.set(generation);
        let enabled = self.filter_settings.borrow().enabled_lists();
        let cache_dir = self.dirs.cache.clone();
        let config_dir = self.dirs.config.clone();
        let store = self.filter_store.clone();
        glib::spawn_future_local(async move {
            let is_current = || {
                current_browser().is_some_and(|browser| browser.filter_generation.get() == generation)
            };
            let directory = cache_dir.clone();
            let converted = gio::spawn_blocking(move || {
                filter_lists::convert_lists(&directory, &config_dir, &enabled)
            })
            .await;
            let Ok((converted, mut statuses, blocked)) = converted else {
                if let Some(browser) = current_browser() {
                    browser.filters_loaded.set(true);
                    browser.load_start_page();
                }
                return;
            };
            if !is_current() {
                return;
            }
            // Compiling can take a while, so show what was downloaded now,
            // and any problems compiling once it's done.
            if let Some(browser) = current_browser() {
                browser.show_list_statuses(statuses.clone());
            }

            let mut filters = Vec::new();
            for list in converted {
                let id = list.id;
                let compiled = if filter_lists::is_compiled(&cache_dir, &list) {
                    store.load_future(id).await.ok()
                } else {
                    None
                };
                let filter = match compiled {
                    Some(filter) => Ok(filter),
                    None => {
                        // Compiling takes a while, and WebKit does it on another thread.
                        let source = glib::Bytes::from_owned(list.json.clone().into_bytes());
                        let saved = store.save_future(id, &source).await;
                        if saved.is_ok() {
                            if let Err(error) = filter_lists::mark_compiled(&cache_dir, &list) {
                                eprintln!("Could not record compiling {id}: {error}");
                            }
                        }
                        saved
                    }
                };
                match filter {
                    Ok(filter) => filters.push(filter),
                    Err(error) => {
                        if let Some(status) = statuses.get_mut(id) {
                            status.problem = Some(error.message().to_owned());
                        }
                    }
                }
                if !is_current() {
                    return;
                }
            }

            let Some(browser) = current_browser() else {
                return;
            };
            browser.filters.replace(filters);
            for tab in browser.tabs.borrow().iter() {
                browser.apply_filters(tab);
            }
            browser.blocked_sites.replace(Rc::new(blocked));
            browser.delete_blocked_soon(true);
            browser.show_list_statuses(statuses);
            browser.update_filter_button();
            browser.filters_loaded.set(true);
            browser.load_start_page();
        });
    }

    /// Loads the page that waited for the filters, if it's still there, once
    /// they and the local network rules are ready.
    fn load_start_page(&self) {
        if !self.filters_loaded.get() || self.local_filter.borrow().is_none() {
            return;
        }
        let Some((id, url)) = self.start_page.take() else {
            return;
        };
        if let Some(tab) = self.tab(id) {
            if tab.webview.uri().is_none() {
                tab.load_for_user(url.as_str());
            }
        }
    }

    /// Decides whether a tab can go to a local address, which only the user, or
    /// a page on the same host and port, can open. Returns whether it decided.
    fn decide_local_navigation(
        &self,
        id: TabId,
        decision: &webkit::NavigationPolicyDecision,
        action: &webkit::NavigationAction,
    ) -> bool {
        let Some(tab) = self.tab(id) else {
            return false;
        };
        let target = action
            .request()
            .and_then(|request| request.uri())
            .and_then(|url| Url::parse(&url).ok());
        let Some(target) = target else {
            return false;
        };
        let Some(origin) = LocalOrigin::of(&target) else {
            return false;
        };
        let kind = action.navigation_type();
        let user_asked = tab.take_user_navigation(&target, kind);
        let opener = tab
            .opener
            .and_then(|opener| self.tab(opener))
            .and_then(|opener| opener.committed.borrow().clone());
        let from = [tab.committed.borrow().clone(), opener];
        if !local_network::may_navigate(&target, user_asked, &from) {
            decision.ignore();
            // WebKit doesn't say whether it's for the whole tab or a frame in
            // it, so the tab only says so when the user followed a link or
            // sent a form, and would expect something to happen, or when it
            // has no page yet. Pages trying it themselves are just refused.
            let followed = action.is_user_gesture()
                && matches!(
                    kind,
                    webkit::NavigationType::LinkClicked | webkit::NavigationType::FormSubmitted
                );
            if followed || tab.committed.borrow().is_none() {
                let from = from
                    .iter()
                    .flatten()
                    .next()
                    .map_or_else(|| "another address".to_owned(), |url| url.origin().ascii_serialization());
                tab.show_stand_in(target.to_string(), StandInReason::LocalNetwork(from));
            }
            return true;
        }
        if self.local_origins.borrow().contains(&origin) {
            return false;
        }
        // The rules must allow it before WebKit loads it.
        let decision = decision.clone();
        self.compile_local_filter(Some((origin, Box::new(move || decision.use_()))));
        true
    }

    /// Compiles the local network rules, with `adding` allowed too, if given,
    /// and then gives them to every tab, and calls its function.
    fn compile_local_filter(&self, adding: Option<(LocalOrigin, Box<dyn FnOnce()>)>) {
        let (then, origins) = {
            let mut origins = self.local_origins.borrow_mut();
            let then = adding.map(|(origin, then)| {
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
                then
            });
            (then, origins.clone())
        };
        let generation = self.local_generation.get() + 1;
        self.local_generation.set(generation);
        let store = self.filter_store.clone();
        let rules = glib::Bytes::from_owned(local_network::rules(&origins).into_bytes());
        glib::spawn_future_local(async move {
            let compiled = store.save_future(local_network::FILTER_ID, &rules).await;
            let Some(browser) = current_browser() else {
                return;
            };
            match compiled {
                Ok(filter) => {
                    if browser.local_generation.get() == generation {
                        browser.local_filter.replace(Some(filter));
                        for tab in browser.tabs.borrow().iter() {
                            browser.apply_filters(tab);
                        }
                    }
                }
                Err(error) => {
                    // Carrying on would let pages reach local addresses.
                    eprintln!("Could not compile the local network rules: {error}");
                    std::process::exit(1);
                }
            }
            if let Some(then) = then {
                then();
            }
            browser.load_start_page();
        });
    }

    /// Gives a tab the filters in use, or none if it isn't filtered. They apply
    /// from the next page it loads.
    fn apply_filters(&self, tab: &Tab) {
        tab.content.remove_all_filters();
        if let Some(filter) = self.local_filter.borrow().as_ref() {
            tab.content.add_filter(filter);
        }
        if tab.filtering.get() {
            for filter in self.filters.borrow().iter() {
                tab.content.add_filter(filter);
            }
        }
    }

    fn show_preferences(self: &Rc<Self>) {
        if self.preferences.borrow().is_none() {
            let preferences = PreferencesWindow::new(
                &self.widgets.window,
                &self.filter_settings.borrow(),
                page_fonts::is_chosen(&self.dirs.config),
                self.page_fonts.note().as_deref(),
            );
            preferences.limit_fonts.connect_toggled(|check| {
                let Some(browser) = current_browser() else {
                    return;
                };
                let limited = check.is_active();
                if let Err(error) = page_fonts::choose(&browser.dirs.config, limited) {
                    eprintln!("Could not save the font setting: {error}");
                    check.set_active(!limited);
                }
            });
            for row in &preferences.rows {
                let id = row.id;
                row.check.connect_toggled(move |check| {
                    let Some(browser) = current_browser() else {
                        return;
                    };
                    let Some(list) = LISTS.iter().find(|list| list.id == id) else {
                        return;
                    };
                    let result = browser
                        .filter_settings
                        .borrow_mut()
                        .set_enabled(list, check.is_active());
                    if let Err(error) = result {
                        eprintln!("Could not save filter list settings: {error}");
                    }
                    browser.reload_filters();
                });
            }
            preferences.update.connect_clicked(|_| {
                if let Some(browser) = current_browser() {
                    browser.update_lists();
                }
            });
            preferences.show_statuses(&self.list_statuses.borrow());
            let (updating, message) = &*self.list_download_state.borrow();
            preferences.show_update_state(*updating, message);
            self.preferences.replace(Some(preferences));
        }
        if let Some(preferences) = self.preferences.borrow().as_ref() {
            preferences.window.present();
        }
    }

    /// Downloads the enabled lists, in the background.
    fn update_lists(&self) {
        let lists = self.filter_settings.borrow().enabled_lists();
        if lists.is_empty() {
            if let Some(preferences) = self.preferences.borrow().as_ref() {
                preferences.show_update_state(false, "No lists are enabled.");
            }
            return;
        }
        self.download_lists(lists, "Updating\u{2026}", "Updated", "update");
    }

    /// Downloads the enabled lists that have never been downloaded, such as
    /// when BrowseGoo is first run, in the background. Lists that have been
    /// downloaded are left as they are, until Update Lists is pressed.
    fn download_missing_lists(&self) {
        let cache_dir = &self.dirs.cache;
        let lists: Vec<&'static FilterList> = self
            .filter_settings
            .borrow()
            .enabled_lists()
            .into_iter()
            .filter(|list| !filter_lists::cache_path(cache_dir, list).exists())
            .collect();
        if !lists.is_empty() {
            self.download_lists(lists, "Downloading\u{2026}", "Downloaded", "download");
        }
    }

    /// Downloads `lists` in the background, saying so in the preferences with
    /// `doing`, `done` and `verb`, and then uses them.
    fn download_lists(
        &self,
        lists: Vec<&'static FilterList>,
        doing: &'static str,
        done: &'static str,
        verb: &'static str,
    ) {
        if self.updating_lists.replace(true) {
            return;
        }
        self.list_download_state
            .replace((true, doing.to_owned()));
        if let Some(preferences) = self.preferences.borrow().as_ref() {
            preferences.show_update_state(true, doing);
        }

        let cache_dir = self.dirs.cache.clone();
        let user_agent = user_agent();
        glib::spawn_future_local(async move {
            let results = gio::spawn_blocking(move || {
                lists
                    .iter()
                    .map(|list| {
                        (
                            list.name,
                            filter_lists::update(&cache_dir, list, &user_agent),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            let Some(browser) = current_browser() else {
                return;
            };
            browser.updating_lists.set(false);
            let message = match results {
                Err(_) => format!("Could not {verb} the lists."),
                Ok(results) => {
                    let failures: Vec<String> = results
                        .iter()
                        .filter_map(|(name, result)| {
                            result
                                .as_ref()
                                .err()
                                .map(|error| format!("Could not {verb} {name}: {error}."))
                        })
                        .collect();
                    if failures.is_empty() {
                        match results.len() {
                            1 => format!("{done} 1 list."),
                            count => format!("{done} {count} lists."),
                        }
                    } else {
                        for failure in &failures {
                            eprintln!("{failure}");
                        }
                        failures.join("\n")
                    }
                }
            };
            browser.list_download_state.replace((false, message.clone()));
            if let Some(preferences) = browser.preferences.borrow().as_ref() {
                preferences.show_update_state(false, &message);
            }
            browser.reload_filters();
        });
    }

    fn update_bookmark_button(&self) {
        let is_bookmarked = self
            .active_tab()
            .and_then(|tab| tab.webview.uri())
            .is_some_and(|url| self.bookmarks.borrow().contains(&url));
        let button = &self.widgets.bookmark;
        button.set_active(is_bookmarked);
        // BrowseGoo's own stars, as the theme's empty one is faded, like a
        // disabled button's.
        button.set_icon_name(if is_bookmarked {
            "browsegoo-star-filled-symbolic"
        } else {
            "browsegoo-star-outline-symbolic"
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
        let about = gtk::AboutDialog::builder()
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
            .build();
        closing::add_close_keys(&about);
        about.present();
    }
}

fn close_tab_by_id(id: TabId) {
    if let Some(browser) = current_browser() {
        browser.close_tab(id);
    }
}

