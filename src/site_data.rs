//! The Site Data Manager window, for looking at what sites have stored,
//! choosing what of it to preserve, and deleting it.
//!
//! Cookies are listed one by one. For the other kinds of data, such as local
//! storage, WebKit only says which sites have some, and only deletes all of a
//! kind for a site, so each of those is one row.
//!
//! What isn't preserved is deleted by `delete_unpreserved`, when BrowseGoo
//! quits and when it starts.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use webkit::WebsiteDataTypes as Types;
use webkit::soup;

use crate::block_sites;
use crate::closing;
use crate::preserved::Preserved;

/// Longer values are only shown below the list, when their row is selected.
const LONGEST_LISTED_VALUE: usize = 60;

/// How long to wait after cookies change before listing them again, as
/// pages often set several at once.
const REFRESH_DELAY: Duration = Duration::from_millis(300);

/// The kinds of data other than cookies, and what to call them.
const STORED_KINDS: [(Types, &str); 11] = [
    (Types::LOCAL_STORAGE, "Local storage"),
    (Types::SESSION_STORAGE, "Session storage"),
    (Types::INDEXEDDB_DATABASES, "IndexedDB"),
    (Types::DOM_CACHE, "Cache storage"),
    (Types::SERVICE_WORKER_REGISTRATIONS, "Service worker"),
    (Types::OFFLINE_APPLICATION_CACHE, "Application cache"),
    (Types::DISK_CACHE, "HTTP cache"),
    (Types::MEMORY_CACHE, "Memory cache"),
    (Types::HSTS_CACHE, "HSTS"),
    (Types::DEVICE_ID_HASH_SALT, "Device ID salt"),
    (Types::ITP, "Tracking prevention"),
];

const COOKIE_KIND: &str = "Cookie";

/// Every kind of entry, in the order of the type checkboxes.
fn kinds() -> impl Iterator<Item = &'static str> {
    std::iter::once(COOKIE_KIND).chain(STORED_KINDS.iter().map(|(_, name)| *name))
}

/// Which entries the list shows, besides what's typed in the filter field.
#[derive(Default)]
struct FilterOptions {
    /// Whether the text filters names, rather than sites.
    by_name: Cell<bool>,
    /// The kinds whose checkboxes are off.
    hidden_kinds: RefCell<HashSet<&'static str>>,
}

/// Where an entry came from, which is what deleting it needs.
enum Source {
    Cookie(soup::Cookie),
    Stored(webkit::WebsiteData, Types),
}

/// One row of the list.
struct Entry {
    /// The site, or for a cookie, the domain it's sent to, which starts with a
    /// dot if it's also sent to subdomains.
    host: String,
    kind: &'static str,
    name: String,
    value: String,
    /// More about the entry, as labels and text, for when it's selected.
    details: Vec<(&'static str, String)>,
    source: Source,
}

impl Entry {
    fn from_cookie(mut cookie: soup::Cookie) -> Self {
        let text = |text: Option<glib::GString>| text.map(String::from).unwrap_or_default();
        let host = text(cookie.domain());
        let name = text(cookie.name());
        let value = text(cookie.value());
        let path = text(cookie.path());
        let sent_to = match host.strip_prefix('.') {
            Some(domain) => format!("{domain} and its subdomains"),
            None => format!("{host} only"),
        };
        let expires = match cookie.expires() {
            Some(time) => format_time(&time),
            None => "When BrowseGoo quits".to_owned(),
        };
        let yes_no = |yes: bool, text: &str| if yes { text.to_owned() } else { "No".to_owned() };
        let same_site = match cookie.same_site_policy() {
            soup::SameSitePolicy::Strict => "Strict, only sent from the same site",
            soup::SameSitePolicy::Lax => "Lax, also sent when following a link from another site",
            soup::SameSitePolicy::None => "None, also sent from other sites",
            _ => "Unknown",
        };
        let details = vec![
            ("Sent to", sent_to),
            ("Path", path),
            ("Expires", expires),
            ("Secure", yes_no(cookie.is_secure(), "Yes, only sent over HTTPS")),
            ("HTTP only", yes_no(cookie.is_http_only(), "Yes, hidden from scripts")),
            ("SameSite", same_site.to_owned()),
        ];
        Self {
            host,
            kind: COOKIE_KIND,
            name,
            value,
            details,
            source: Source::Cookie(cookie),
        }
    }

    /// An entry for each kind of data, other than cookies, that `data` has.
    fn from_stored(data: &webkit::WebsiteData) -> Vec<Self> {
        let host = data.name().map(String::from).unwrap_or_default();
        let types = data.types();
        STORED_KINDS
            .iter()
            .filter(|(kind, _)| types.contains(*kind))
            .map(|&(kind, kind_name)| {
                let size = data.size(kind);
                let value = if size > 0 { format_size(size) } else { String::new() };
                let details = vec![
                    ("Size", if size > 0 { value.clone() } else { "Not known".to_owned() }),
                    (
                        "Deleting",
                        format!("Removes all of this site's {}", kind_name.to_lowercase()),
                    ),
                ];
                Self {
                    host: host.clone(),
                    kind: kind_name,
                    name: String::new(),
                    value,
                    details,
                    source: Source::Stored(data.clone(), kind),
                }
            })
            .collect()
    }

    /// Identifies the entry from one listing to the next, to keep it selected,
    /// and from one run to the next, to preserve it.
    fn key(&self) -> String {
        let path = match &self.source {
            Source::Cookie(cookie) => cookie.clone().path().map(String::from).unwrap_or_default(),
            Source::Stored(..) => String::new(),
        };
        format!("{}\t{}\t{}\t{path}", self.host, self.kind, self.name)
    }

    /// The value for the list, which leaves out long ones.
    fn listed_value(&self) -> String {
        listed_value(&self.value)
    }

    /// Whether the site, or with `by_name` the name, contains `filter`, which
    /// must be lowercase.
    fn matches(&self, filter: &str, by_name: bool) -> bool {
        let field = if by_name { &self.name } else { &self.host };
        field.to_lowercase().contains(filter)
    }
}

fn listed_value(value: &str) -> String {
    match value.chars().count() {
        length if length > LONGEST_LISTED_VALUE => format!("({length} characters)"),
        _ => value.to_owned(),
    }
}

/// The order of the list until a column header is clicked: by site, with
/// cookies first, then by name.
fn sort_entries(entries: &mut [Entry]) {
    let kind_order = |kind: &str| {
        STORED_KINDS
            .iter()
            .position(|(_, name)| *name == kind)
            .map_or(0, |position| position + 1)
    };
    entries.sort_by(|a, b| {
        sort_text(&a.host)
            .cmp(&sort_text(&b.host))
            .then(kind_order(a.kind).cmp(&kind_order(b.kind)))
            .then(a.name.cmp(&b.name))
    });
}

/// How text sorts, which ignores case and a cookie domain's leading dot.
fn sort_text(text: &str) -> String {
    text.trim_start_matches('.').to_lowercase()
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if bytes < 1024 {
        return if bytes == 1 { "1 byte".to_owned() } else { format!("{bytes} bytes") };
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

fn format_time(time: &glib::DateTime) -> String {
    time.to_local()
        .and_then(|time| time.format("%Y-%m-%d %H:%M"))
        .map(String::from)
        .unwrap_or_default()
}

/// The entry that a list item holds.
fn entry_of(object: &glib::Object) -> std::cell::Ref<'_, Entry> {
    object
        .downcast_ref::<glib::BoxedAnyObject>()
        .expect("the list holds entries")
        .borrow::<Entry>()
}

pub struct SiteDataWindow {
    window: gtk::Window,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    search: gtk::SearchEntry,
    details: gtk::Grid,
    count: gtk::Label,
    delete: gtk::Button,
    block: gtk::Button,
    /// Adds rules to the end of the user's own.
    add_rules: Rc<dyn Fn(&str) -> Result<(), String>>,
    cookies: webkit::CookieManager,
    data: webkit::WebsiteDataManager,
    /// Counts listings, so that only the newest one is shown.
    generation: Cell<u64>,
    refresh_scheduled: Cell<bool>,
    deleting: Cell<bool>,
}

impl SiteDataWindow {
    pub fn new(
        parent: &gtk::ApplicationWindow,
        cookies: webkit::CookieManager,
        data: webkit::WebsiteDataManager,
        preserved: Rc<RefCell<Preserved>>,
        add_rules: impl Fn(&str) -> Result<(), String> + 'static,
    ) -> Rc<Self> {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Filter")
            .hexpand(true)
            .build();
        let by_site = gtk::CheckButton::builder()
            .label("Site")
            .active(true)
            .tooltip_text("Filter by site")
            .build();
        let by_name = gtk::CheckButton::builder()
            .label("Name")
            .group(&by_site)
            .tooltip_text("Filter by name")
            .build();
        let refresh = gtk::Button::with_label("Refresh");
        refresh.set_tooltip_text(Some(
            "Cookies are listed again as they change, but other data only when \
             this window is opened or switched to, or with this button",
        ));
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top.append(&search);
        top.append(&by_site);
        top.append(&by_name);
        top.append(&refresh);
        content.append(&top);

        // Wraps onto more lines when the window is too narrow for them.
        let kind_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(12)
            .row_spacing(2)
            .max_children_per_line(kinds().count() as u32)
            .build();
        let kind_checks: Vec<(&'static str, gtk::CheckButton)> = kinds()
            .map(|kind| {
                let check = gtk::CheckButton::builder().label(kind).active(true).build();
                kind_box.insert(&check, -1);
                // The checkbox takes the focus, rather than the box around it.
                if let Some(child) = check.parent() {
                    child.set_focusable(false);
                }
                (kind, check)
            })
            .collect();
        content.append(&kind_box);

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let options = Rc::new(FilterOptions::default());
        let filter = {
            let search = search.downgrade();
            let options = options.clone();
            gtk::CustomFilter::new(move |object| {
                let entry = entry_of(object);
                if options.hidden_kinds.borrow().contains(entry.kind) {
                    return false;
                }
                let text = search.upgrade().map(|search| search.text().to_lowercase());
                text.is_none_or(|text| entry.matches(text.trim(), options.by_name.get()))
            })
        };
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let columns = gtk::ColumnView::builder()
            .show_column_separators(true)
            .build();
        // GTK's compact rows for tables of data. Setting the classes instead
        // would remove the ones GTK gives it.
        columns.add_css_class("data-table");
        let sorted = gtk::SortListModel::new(Some(filtered), columns.sorter());
        let selection = gtk::MultiSelection::new(Some(sorted));
        columns.set_model(Some(&selection));

        add_preserve_column(&columns, &preserved);
        let site = add_column(&columns, "Site", 200, |entry| entry.host.clone());
        add_column(&columns, "Type", 130, |entry| entry.kind.to_owned());
        add_column(&columns, "Name", 160, |entry| entry.name.clone());
        let value = add_column(&columns, "Value", 0, |entry| entry.listed_value());
        value.set_expand(true);
        columns.sort_by_column(Some(&site), gtk::SortType::Ascending);

        let scrolled = gtk::ScrolledWindow::builder()
            .child(&columns)
            .vexpand(true)
            .has_frame(true)
            // Scrollbars that show whenever there's more, rather than only
            // when the pointer moves, which some themes don't hint at.
            .overlay_scrolling(false)
            .build();
        content.append(&scrolled);

        let details = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(12)
            .build();
        // A fixed height, so that the list doesn't change size with the selection.
        let details_scrolled = gtk::ScrolledWindow::builder()
            .child(&details)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .height_request(180)
            .overlay_scrolling(false)
            .build();
        content.append(&details_scrolled);

        let count = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        let delete = gtk::Button::builder()
            .label("Delete")
            .sensitive(false)
            .css_classes(["destructive-action"])
            .build();
        let block = gtk::Button::builder()
            .label("Block…")
            .sensitive(false)
            .tooltip_text("Write filter rules that block the selected items' sites")
            .build();
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bottom.append(&count);
        bottom.append(&block);
        bottom.append(&delete);
        content.append(&bottom);

        let window = gtk::Window::builder()
            .title("Site Data Manager")
            .transient_for(parent)
            .default_width(820)
            .default_height(680)
            .hide_on_close(true)
            .child(&content)
            .build();
        closing::add_close_keys(&window);
        let close = closing::close_button(&window);
        bottom.append(&close);

        let this = Rc::new(Self {
            window,
            store,
            selection,
            search,
            details,
            count,
            delete,
            block,
            add_rules: Rc::new(add_rules),
            cookies,
            data,
            generation: Cell::new(0),
            refresh_scheduled: Cell::new(false),
            deleting: Cell::new(false),
        });

        let weak = Rc::downgrade(&this);
        let with = move |action: fn(&Rc<Self>)| {
            let weak: Weak<Self> = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    action(&this);
                }
            }
        };

        let changed = with(|this| {
            this.show_details();
            this.show_count();
        });
        let refilter = Rc::new(move || {
            filter.changed(gtk::FilterChange::Different);
            changed();
        });
        let again = refilter.clone();
        this.search.connect_search_changed(move |_| again());
        let again = refilter.clone();
        let options_now = options.clone();
        by_name.connect_toggled(move |by_name| {
            options_now.by_name.set(by_name.is_active());
            again();
        });
        for (kind, check) in kind_checks {
            let again = refilter.clone();
            let options = options.clone();
            check.connect_toggled(move |check| {
                let mut hidden = options.hidden_kinds.borrow_mut();
                if check.is_active() {
                    hidden.remove(kind);
                } else {
                    hidden.insert(kind);
                }
                drop(hidden);
                again();
            });
        }
        let changed = with(|this| {
            this.show_details();
            this.show_count();
        });
        this.selection.connect_selection_changed(move |_, _, _| changed());
        let changed = with(|this| this.show_count());
        this.selection.connect_items_changed(move |_, _, _, _| changed());

        let refresh_now = with(Self::refresh);
        refresh.connect_clicked(move |_| refresh_now());
        let delete_selected = with(Self::delete_selected);
        this.delete.connect_clicked(move |_| delete_selected());
        let block_selected = with(Self::block_selected);
        this.block.connect_clicked(move |_| block_selected());

        let delete_selected = with(Self::delete_selected);
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Delete"),
            Some(gtk::CallbackAction::new(move |_, _| {
                delete_selected();
                glib::Propagation::Stop
            })),
        ));
        columns.add_controller(shortcuts);

        // Other data has no signal for changes, so it's listed again whenever
        // the window is switched to.
        let refresh_now = with(Self::refresh);
        this.window.connect_is_active_notify(move |window| {
            if window.is_active() {
                refresh_now();
            }
        });
        let schedule = with(Self::schedule_refresh);
        this.cookies.connect_changed(move |_| schedule());

        this.show_details();
        this
    }

    pub fn present(self: &Rc<Self>) {
        self.window.present();
        self.refresh();
    }

    /// Lists the data again shortly, if the window is open.
    fn schedule_refresh(self: &Rc<Self>) {
        if !self.window.is_visible() || self.refresh_scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(REFRESH_DELAY, move || {
            if let Some(this) = weak.upgrade() {
                this.refresh_scheduled.set(false);
                this.refresh();
            }
        });
    }

    /// Lists the data again, keeping what was selected selected.
    fn refresh(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        let cookies = self.cookies.clone();
        let data = self.data.clone();
        glib::spawn_future_local(async move {
            let cookies = cookies.all_cookies_future().await;
            let stored = data.fetch_future(Types::ALL & !Types::COOKIES).await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.generation.get() != generation {
                return;
            }
            let mut entries = Vec::new();
            let mut problems = Vec::new();
            match cookies {
                Ok(cookies) => entries.extend(cookies.into_iter().map(Entry::from_cookie)),
                Err(error) => problems.push(format!("Could not list cookies: {error}")),
            }
            match stored {
                Ok(stored) => entries.extend(stored.iter().flat_map(Entry::from_stored)),
                Err(error) => problems.push(format!("Could not list site data: {error}")),
            }
            for problem in &problems {
                eprintln!("{problem}");
            }
            this.show_entries(entries);
        });
    }

    fn show_entries(&self, mut entries: Vec<Entry>) {
        sort_entries(&mut entries);
        let selected: HashSet<String> = self
            .selected()
            .iter()
            .map(|object| entry_of(object).key())
            .collect();
        let objects: Vec<glib::BoxedAnyObject> =
            entries.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objects);

        if !selected.is_empty() {
            let keep = gtk::Bitset::new_empty();
            for position in 0..self.selection.n_items() {
                if let Some(object) = self.selection.item(position) {
                    if selected.contains(&entry_of(&object).key()) {
                        keep.add(position);
                    }
                }
            }
            self.selection.set_selection(&keep, &keep);
        }
        self.show_details();
        self.show_count();
    }

    /// The selected entries, as their list items.
    fn selected(&self) -> Vec<glib::Object> {
        let positions = self.selection.selection();
        (0..positions.size())
            .filter_map(|index| self.selection.item(positions.nth(index as u32)))
            .collect()
    }

    fn show_count(&self) {
        let total = self.store.n_items();
        let shown = self.selection.n_items();
        let items = |count: u32| match count {
            1 => "1 item".to_owned(),
            count => format!("{count} items"),
        };
        let mut text = match (total, shown == total) {
            (0, _) => "Nothing is stored".to_owned(),
            (_, true) => items(total),
            (_, false) => format!("{} of {}", shown, items(total)),
        };
        let selected = self.selection.selection().size();
        if selected > 1 {
            text.push_str(&format!(", {selected} selected"));
        }
        self.count.set_text(&text);
        self.delete.set_sensitive(selected > 0 && !self.deleting.get());
        self.block.set_sensitive(selected > 0);
    }

    /// Shows more about the selected entry, if just one is selected.
    fn show_details(&self) {
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        let selected = self.selected();
        let [object] = selected.as_slice() else {
            let hint = gtk::Label::builder()
                .label("Select an item to see more about it.")
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build();
            self.details.attach(&hint, 0, 0, 1, 1);
            return;
        };
        let entry = entry_of(object);
        let mut rows = vec![("Site", entry.host.clone()), ("Type", entry.kind.to_owned())];
        if !entry.name.is_empty() {
            rows.push(("Name", entry.name.clone()));
        }
        if matches!(entry.source, Source::Cookie(_)) {
            rows.push(("Value", entry.value.clone()));
        }
        rows.extend(entry.details.iter().cloned());
        for (row, (label, text)) in rows.into_iter().enumerate() {
            let label = gtk::Label::builder()
                .label(label)
                .xalign(1.0)
                .yalign(0.0)
                .css_classes(["dim-label"])
                .build();
            let text = gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .hexpand(true)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .build();
            // Hyphens added where a value wraps would look like part of it.
            let attributes = gtk::pango::AttrList::new();
            attributes.insert(gtk::pango::AttrInt::new_insert_hyphens(false));
            text.set_attributes(Some(&attributes));
            self.details.attach(&label, 0, row as i32, 1, 1);
            self.details.attach(&text, 1, row as i32, 1, 1);
        }
    }

    /// Opens a window with rules for blocking the selected entries' sites,
    /// which leaves them selected, to delete next if need be.
    fn block_selected(self: &Rc<Self>) {
        let selected = self.selected();
        if selected.is_empty() {
            return;
        }
        let hosts: Vec<String> = selected
            .iter()
            .map(|object| entry_of(object).host.clone())
            .collect();
        let add_rules = self.add_rules.clone();
        block_sites::show(&self.window, hosts.iter().map(String::as_str), move |rules| {
            add_rules(rules)
        });
    }

    fn delete_selected(self: &Rc<Self>) {
        let selected = self.selected();
        if selected.is_empty() || self.deleting.replace(true) {
            return;
        }
        self.delete.set_sensitive(false);
        let weak = Rc::downgrade(self);
        let cookies = self.cookies.clone();
        let data = self.data.clone();
        glib::spawn_future_local(async move {
            for object in selected {
                let result = match &entry_of(&object).source {
                    Source::Cookie(cookie) => cookies.delete_cookie_future(cookie).await,
                    Source::Stored(site, kind) => remove_data(&data, std::slice::from_ref(site), *kind).await,
                };
                if let Err(error) = result {
                    eprintln!("Could not delete {}: {error}", entry_of(&object).key());
                }
            }
            if let Some(this) = weak.upgrade() {
                this.deleting.set(false);
                this.refresh();
            }
        });
    }
}

/// Deletes all cookies and site data, except what's preserved.
pub async fn delete_unpreserved(
    cookies: &webkit::CookieManager,
    data: &webkit::WebsiteDataManager,
    preserved: &RefCell<Preserved>,
) {
    let is_preserved = |entry: &Entry| preserved.borrow().contains(&entry.key());
    match cookies.all_cookies_future().await {
        Ok(all) => {
            for entry in all.into_iter().map(Entry::from_cookie) {
                if let (false, Source::Cookie(cookie)) = (is_preserved(&entry), &entry.source) {
                    if let Err(error) = cookies.delete_cookie_future(cookie).await {
                        eprintln!("Could not delete {}: {error}", entry.key());
                    }
                }
            }
        }
        Err(error) => eprintln!("Could not list cookies to delete: {error}"),
    }
    match data.fetch_future(Types::ALL & !Types::COOKIES).await {
        Ok(stored) => {
            let entries: Vec<Entry> = stored
                .iter()
                .flat_map(Entry::from_stored)
                .filter(|entry| !is_preserved(entry))
                .collect();
            // One request for each kind, for all the sites that have it.
            for (kind, kind_name) in STORED_KINDS {
                let sites: Vec<webkit::WebsiteData> = entries
                    .iter()
                    .filter_map(|entry| match &entry.source {
                        Source::Stored(site, _) if entry.kind == kind_name => Some(site.clone()),
                        _ => None,
                    })
                    .collect();
                if sites.is_empty() {
                    continue;
                }
                if let Err(error) = remove_data(data, &sites, kind).await {
                    eprintln!("Could not delete {}: {error}", kind_name.to_lowercase());
                }
            }
        }
        Err(error) => eprintln!("Could not list site data to delete: {error}"),
    }
}

/// Deletes one kind of data that sites have stored.
fn remove_data(
    manager: &webkit::WebsiteDataManager,
    sites: &[webkit::WebsiteData],
    kind: Types,
) -> impl Future<Output = Result<(), glib::Error>> + use<> {
    let sites = sites.to_vec();
    gio::GioFuture::new(manager, move |manager, cancellable, send| {
        let sites: Vec<&webkit::WebsiteData> = sites.iter().collect();
        manager.remove(kind, &sites, Some(cancellable), move |result| {
            send.resolve(result);
        });
    })
}

/// Adds the column of checkboxes for preserving entries, which sorts the
/// preserved ones first.
fn add_preserve_column(columns: &gtk::ColumnView, preserved: &Rc<RefCell<Preserved>>) {
    let factory = gtk::SignalListItemFactory::new();
    let saving = preserved.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let check = gtk::CheckButton::builder()
            .halign(gtk::Align::Center)
            .tooltip_text("Keep this when BrowseGoo quits")
            .build();
        let item_now = item.downgrade();
        let preserved = saving.clone();
        // Binding an entry sets the checkbox to what's saved, which changes nothing.
        check.connect_toggled(move |check| {
            let Some(object) = item_now.upgrade().and_then(|item| item.item()) else {
                return;
            };
            let key = entry_of(&object).key();
            let result = preserved.borrow_mut().set(&key, check.is_active());
            if let Err(error) = result {
                eprintln!("Could not save which site data to preserve: {error}");
                check.set_active(!check.is_active());
            }
        });
        item.set_child(Some(&check));
    });
    let showing = preserved.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let (Some(object), Some(check)) =
            (item.item(), item.child().and_downcast::<gtk::CheckButton>())
        {
            let key = entry_of(&object).key();
            let preserve = showing.borrow().contains(&key);
            check.set_active(preserve);
        }
    });
    let sorting = preserved.clone();
    let sorter = gtk::CustomSorter::new(move |a, b| {
        let preserved = sorting.borrow();
        let is_preserved = |object: &glib::Object| preserved.contains(&entry_of(object).key());
        is_preserved(b).cmp(&is_preserved(a)).into()
    });
    let column = gtk::ColumnViewColumn::builder()
        .title("Preserve")
        .factory(&factory)
        .sorter(&sorter)
        .resizable(true)
        .build();
    columns.append_column(&column);
}

/// Adds a column that shows `text` for each entry, and sorts by it.
fn add_column(
    columns: &gtk::ColumnView,
    title: &str,
    width: i32,
    text: fn(&Entry) -> String,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let (Some(object), Some(label)) = (item.item(), item.child().and_downcast::<gtk::Label>())
        {
            let text = text(&entry_of(&object));
            label.set_tooltip_text((!text.is_empty()).then_some(text.as_str()));
            label.set_text(&text);
        }
    });
    let sorter = gtk::CustomSorter::new(move |a, b| {
        sort_text(&text(&entry_of(a)))
            .cmp(&sort_text(&text(&entry_of(b))))
            .into()
    });
    let column = gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .sorter(&sorter)
        .resizable(true)
        .build();
    if width > 0 {
        column.set_fixed_width(width);
    }
    columns.append_column(&column);
    column
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_are_left_out_of_the_list() {
        assert_eq!(listed_value("abc"), "abc");
        let long = "x".repeat(LONGEST_LISTED_VALUE + 1);
        assert_eq!(listed_value(&long), format!("({} characters)", LONGEST_LISTED_VALUE + 1));
        let fits = "é".repeat(LONGEST_LISTED_VALUE);
        assert_eq!(listed_value(&fits), fits);
    }

    #[test]
    fn filtering_by_site_or_name() {
        let entry = Entry::from_cookie(soup::Cookie::new("SessionID", "1", ".Example.com", "/", 60));
        assert!(entry.matches("example", false));
        assert!(!entry.matches("session", false));
        assert!(entry.matches("session", true));
        assert!(!entry.matches("example", true));
        assert!(entry.matches("", true));
    }

    #[test]
    fn every_kind_has_a_checkbox() {
        let kinds: Vec<_> = kinds().collect();
        assert_eq!(kinds.len(), STORED_KINDS.len() + 1);
        assert_eq!(kinds.iter().collect::<HashSet<_>>().len(), kinds.len());
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(1), "1 byte");
        assert_eq!(format_size(1023), "1023 bytes");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn sorting_ignores_the_leading_dot_and_case() {
        assert_eq!(sort_text(".Example.com"), "example.com");
        assert!(sort_text(".b.example") > sort_text("A.example"));
    }
}
