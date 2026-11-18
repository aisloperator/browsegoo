//! The Password Manager window, for looking at saved passwords, changing
//! them, and deleting them.
//!
//! A password is hidden until its Show Password button is pressed, each time a
//! record is selected. When shown, it can be selected and copied like any
//! text, but there's no button for copying it, as passwords left on the
//! clipboard can be read by other programs.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::closing;
use crate::passwords::{Passwords, Record, RecordId};

/// How a time is shown, in the local time zone.
pub fn format_time(seconds: i64) -> String {
    glib::DateTime::from_unix_local(seconds)
        .and_then(|time| time.format("%Y-%m-%d %H:%M"))
        .map(String::from)
        .unwrap_or_default()
}

fn format_last_used(record: &Record) -> String {
    record
        .last_used
        .map_or_else(|| "Never".to_owned(), format_time)
}

/// The record that a list item holds.
fn record_of(object: &glib::Object) -> std::cell::Ref<'_, Record> {
    object
        .downcast_ref::<glib::BoxedAnyObject>()
        .expect("the list holds records")
        .borrow::<Record>()
}

pub struct PasswordManager {
    window: gtk::Window,
    passwords: Rc<RefCell<Passwords>>,
    store: gio::ListStore,
    selection: gtk::SingleSelection,
    details: gtk::Grid,
    domain: gtk::Entry,
    username: gtk::Entry,
    password: gtk::Entry,
    show_password: gtk::Button,
    created: gtk::Label,
    edited: gtk::Label,
    last_used: gtk::Label,
    message: gtk::Label,
    count: gtk::Label,
    save: gtk::Button,
    revert: gtk::Button,
    delete: gtk::Button,
    /// The record whose details are shown, which stay as typed while the
    /// list is shown again.
    shown: Cell<Option<RecordId>>,
}

impl PasswordManager {
    pub fn new(parent: &gtk::ApplicationWindow, passwords: Rc<RefCell<Passwords>>) -> Rc<Self> {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let columns = gtk::ColumnView::builder()
            .show_column_separators(true)
            .build();
        columns.add_css_class("data-table");
        let sorted = gtk::SortListModel::new(Some(store.clone()), columns.sorter());
        let selection = gtk::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        columns.set_model(Some(&selection));

        let by_text = |text: fn(&Record) -> String| {
            move |a: &Record, b: &Record| text(a).to_lowercase().cmp(&text(b).to_lowercase())
        };
        let site = add_column(
            &columns,
            "Site",
            220,
            |r| r.domain.clone(),
            by_text(|r| r.domain.clone()),
        );
        let username = add_column(
            &columns,
            "Username",
            0,
            |r| r.username.clone(),
            by_text(|r| r.username.clone()),
        );
        username.set_expand(true);
        add_column(
            &columns,
            "Created",
            140,
            |r| format_time(r.created),
            |a, b| a.created.cmp(&b.created),
        );
        add_column(
            &columns,
            "Last Edited",
            140,
            |r| format_time(r.edited),
            |a, b| a.edited.cmp(&b.edited),
        );
        add_column(&columns, "Last Used", 140, format_last_used, |a, b| {
            a.last_used.cmp(&b.last_used)
        });
        columns.sort_by_column(Some(&site), gtk::SortType::Ascending);

        let scrolled = gtk::ScrolledWindow::builder()
            .child(&columns)
            .vexpand(true)
            .has_frame(true)
            .overlay_scrolling(false)
            .build();
        content.append(&scrolled);

        let details = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        let label = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(1.0)
                .css_classes(["dim-label"])
                .build()
        };
        let value = || gtk::Label::builder().xalign(0.0).selectable(true).build();
        let domain = gtk::Entry::builder().hexpand(true).build();
        let username_entry = gtk::Entry::builder().hexpand(true).build();
        // Not a `gtk::PasswordEntry`, whose peek icon would show it for as
        // long as it's toggled on. GTK doesn't copy from a hidden entry.
        let password = gtk::Entry::builder()
            .hexpand(true)
            .visibility(false)
            .input_purpose(gtk::InputPurpose::Password)
            .build();
        let show_password = gtk::Button::with_label("Show Password");
        let password_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        password_row.append(&password);
        password_row.append(&show_password);
        let created = value();
        let edited = value();
        let last_used = value();
        details.attach(&label("Site"), 0, 0, 1, 1);
        details.attach(&domain, 1, 0, 1, 1);
        details.attach(&label("Username"), 0, 1, 1, 1);
        details.attach(&username_entry, 1, 1, 1, 1);
        details.attach(&label("Password"), 0, 2, 1, 1);
        details.attach(&password_row, 1, 2, 1, 1);
        details.attach(&label("Created"), 0, 3, 1, 1);
        details.attach(&created, 1, 3, 1, 1);
        details.attach(&label("Last edited"), 0, 4, 1, 1);
        details.attach(&edited, 1, 4, 1, 1);
        details.attach(&label("Last used"), 0, 5, 1, 1);
        details.attach(&last_used, 1, 5, 1, 1);
        content.append(&details);

        let message = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();
        content.append(&message);

        let count = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        let revert = gtk::Button::builder()
            .label("Revert")
            .tooltip_text("Undo the changes made here to this record")
            .build();
        let save = gtk::Button::builder()
            .label("Save Changes")
            .css_classes(["suggested-action"])
            .build();
        let delete = gtk::Button::builder()
            .label("Delete…")
            .css_classes(["destructive-action"])
            .build();
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bottom.append(&count);
        bottom.append(&delete);
        bottom.append(&revert);
        bottom.append(&save);
        content.append(&bottom);

        let window = gtk::Window::builder()
            .title("Password Manager")
            .transient_for(parent)
            .default_width(820)
            .default_height(600)
            .hide_on_close(true)
            .child(&content)
            .build();
        closing::add_close_keys(&window);
        bottom.append(&closing::close_button(&window));

        let this = Rc::new(Self {
            window,
            passwords,
            store,
            selection,
            details,
            domain,
            username: username_entry,
            password,
            show_password,
            created,
            edited,
            last_used,
            message,
            count,
            save,
            revert,
            delete,
            shown: Cell::new(None),
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
        let changed = with(|this| this.show_selected());
        this.selection
            .connect_selection_changed(move |_, _, _| changed());
        for entry in [&this.domain, &this.username, &this.password] {
            let edited = with(|this| this.update_buttons());
            entry.connect_changed(move |_| edited());
            let save = with(Self::save_changes);
            entry.connect_activate(move |_| save());
        }
        let toggle = with(Self::toggle_password);
        this.show_password.connect_clicked(move |_| toggle());
        let save = with(Self::save_changes);
        this.save.connect_clicked(move |_| save());
        let revert = with(Self::revert);
        this.revert.connect_clicked(move |_| revert());
        let delete = with(Self::delete_selected);
        this.delete.connect_clicked(move |_| delete());
        // Shown again only with its button, once the window is open again.
        let hide = with(|this| this.hide_password());
        this.window.connect_visible_notify(move |window| {
            if !window.is_visible() {
                hide();
            }
        });

        this.refresh();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Lists the records again, as they're saved, keeping the selected one
    /// selected, and what's typed for it.
    pub fn refresh(&self) {
        let records: Vec<glib::BoxedAnyObject> = self
            .passwords
            .borrow()
            .records()
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        let shown = self.shown.get();
        self.store.splice(0, self.store.n_items(), &records);
        let position = (0..self.selection.n_items()).find(|&position| {
            self.selection
                .item(position)
                .is_some_and(|object| Some(record_of(&object).id) == shown)
        });
        self.selection
            .set_selected(position.unwrap_or(gtk::INVALID_LIST_POSITION));
        self.show_selected();
        let text = match self.store.n_items() {
            0 => "No passwords are saved".to_owned(),
            1 => "1 password".to_owned(),
            count => format!("{count} passwords"),
        };
        self.count.set_text(&text);
    }

    fn selected(&self) -> Option<Record> {
        self.selection
            .selected_item()
            .map(|object| record_of(&object).clone())
    }

    /// Shows the selected record's details. Another record's are shown with
    /// its password hidden.
    fn show_selected(&self) {
        let record = self.selected();
        let id = record.as_ref().map(|record| record.id);
        if id != self.shown.get() {
            self.shown.set(id);
            self.hide_password();
            self.show_message(None);
            let text = |text: Option<&str>| text.unwrap_or_default().to_owned();
            self.domain
                .set_text(&text(record.as_ref().map(|r| r.domain.as_str())));
            self.username
                .set_text(&text(record.as_ref().map(|r| r.username.as_str())));
            self.password
                .set_text(&text(record.as_ref().map(|r| r.password.as_str())));
        }
        let time = |time: fn(&Record) -> String| record.as_ref().map(time).unwrap_or_default();
        self.created.set_text(&time(|r| format_time(r.created)));
        self.edited.set_text(&time(|r| format_time(r.edited)));
        self.last_used.set_text(&time(format_last_used));
        self.details.set_sensitive(record.is_some());
        self.delete.set_sensitive(record.is_some());
        self.update_buttons();
    }

    /// Whether what's typed differs from the selected record.
    fn is_edited(&self) -> bool {
        self.selected().is_some_and(|record| {
            self.domain.text() != record.domain
                || self.username.text() != record.username
                || self.password.text() != record.password
        })
    }

    fn update_buttons(&self) {
        let edited = self.is_edited();
        self.save.set_sensitive(edited);
        self.revert.set_sensitive(edited);
    }

    fn hide_password(&self) {
        self.password.set_visibility(false);
        self.show_password.set_label("Show Password");
    }

    fn toggle_password(self: &Rc<Self>) {
        if !self.password.property::<bool>("visibility") {
            self.password.set_visibility(true);
            self.show_password.set_label("Hide Password");
        } else {
            self.hide_password();
        }
    }

    fn show_message(&self, message: Option<&str>) {
        self.message.set_text(message.unwrap_or_default());
        self.message.set_visible(message.is_some());
    }

    fn save_changes(self: &Rc<Self>) {
        let Some(record) = self.selected() else {
            return;
        };
        if !self.is_edited() {
            return;
        }
        let result = self.passwords.borrow_mut().edit(
            record.id,
            &self.domain.text(),
            &self.username.text(),
            &self.password.text(),
        );
        match result {
            Ok(()) => {
                self.refresh();
                // As saved, such as with the domain in lowercase.
                self.show_selected_again();
            }
            Err(error) => self.show_message(Some(&error)),
        }
    }

    /// Shows the selected record's details as saved, with its password hidden.
    fn show_selected_again(&self) {
        self.shown.set(None);
        self.show_selected();
    }

    fn revert(self: &Rc<Self>) {
        self.show_selected_again();
    }

    fn delete_selected(self: &Rc<Self>) {
        let Some(record) = self.selected() else {
            return;
        };
        let who = match record.username.as_str() {
            "" => "with no username".to_owned(),
            username => format!("for {username}"),
        };
        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message("Delete this saved password?")
            .detail(format!(
                "The password {who} on {} will be deleted.",
                record.domain
            ))
            .buttons(["Cancel", "Delete"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(this) = weak.upgrade() else {
                return;
            };
            if dialog.choose_future(Some(&this.window)).await != Ok(1) {
                return;
            }
            let result = this.passwords.borrow_mut().remove(record.id);
            match result {
                Ok(()) => this.refresh(),
                Err(error) => {
                    this.show_message(Some(&format!("Could not save passwords: {error}")))
                }
            }
        });
    }
}

/// Adds a column that shows `text` for each record, and sorts with `compare`.
fn add_column(
    columns: &gtk::ColumnView,
    title: &str,
    width: i32,
    text: fn(&Record) -> String,
    compare: impl Fn(&Record, &Record) -> Ordering + 'static,
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
        if let (Some(object), Some(label)) =
            (item.item(), item.child().and_downcast::<gtk::Label>())
        {
            let text = text(&record_of(&object));
            label.set_tooltip_text((!text.is_empty()).then_some(text.as_str()));
            label.set_text(&text);
        }
    });
    let sorter = gtk::CustomSorter::new(move |a, b| compare(&record_of(a), &record_of(b)).into());
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
