//! The window for editing, reordering and deleting bookmarks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::bookmarks::Bookmark;
use crate::closing;
use crate::location::parse_location;

/// A change to the bookmarks, which the browser makes and saves.
pub enum Edit {
    Add {
        index: usize,
        url: String,
        title: String,
    },
    Change {
        index: usize,
        url: String,
        title: String,
    },
    Remove(usize),
    Move {
        from: usize,
        to: usize,
    },
}

pub struct BookmarksEditor {
    pub window: gtk::Window,
    list: gtk::ListBox,
    title: gtk::Entry,
    address: gtk::Entry,
    error: gtk::Label,
    move_up: gtk::Button,
    move_down: gtk::Button,
    delete: gtk::Button,
    add: gtk::Button,
    apply: Box<dyn Fn(Edit) -> Result<(), String>>,
    open: Box<dyn Fn(&str)>,
    current_page: Box<dyn Fn() -> Option<Bookmark>>,
    items: RefCell<Vec<Bookmark>>,
    /// The bookmark the fields are for.
    editing: Cell<Option<usize>>,
    /// Where a new bookmark goes, while the fields are for one.
    adding: Cell<Option<usize>>,
    /// Which bookmark to select when the list is next shown.
    select_next: Cell<Option<usize>>,
    /// Set while the fields are being filled in, rather than edited.
    loading: Cell<bool>,
}

impl BookmarksEditor {
    /// Makes the window. `apply` makes changes, `open` opens a URL, and
    /// `current_page` is what a new bookmark starts as.
    pub fn new(
        parent: &gtk::ApplicationWindow,
        apply: impl Fn(Edit) -> Result<(), String> + 'static,
        open: impl Fn(&str) + 'static,
        current_page: impl Fn() -> Option<Bookmark> + 'static,
    ) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .build();
        list.set_placeholder(Some(
            &gtk::Label::builder()
                .label("No bookmarks yet. Add one, or bookmark a page with the star button.")
                .wrap(true)
                .margin_top(24)
                .margin_bottom(24)
                .css_classes(["dim-label"])
                .build(),
        ));
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .has_frame(true)
            .build();

        let title = gtk::Entry::builder().hexpand(true).build();
        let address = gtk::Entry::builder()
            .hexpand(true)
            .input_purpose(gtk::InputPurpose::Url)
            .build();
        let fields = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        for (row, (name, entry)) in [("Title", &title), ("Address", &address)]
            .into_iter()
            .enumerate()
        {
            let label = gtk::Label::builder()
                .label(name)
                .xalign(1.0)
                .mnemonic_widget(entry)
                .build();
            fields.attach(&label, 0, row as i32, 1, 1);
            fields.attach(entry, 1, row as i32, 1, 1);
        }
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();

        let add = gtk::Button::with_label("Add");
        let move_up = gtk::Button::with_label("Move Up");
        let move_down = gtk::Button::with_label("Move Down");
        let delete = gtk::Button::builder()
            .label("Delete")
            .css_classes(["destructive-action"])
            .build();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.append(&add);
        buttons.append(&move_up);
        buttons.append(&move_down);
        buttons.append(&delete);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&scroller);
        content.append(&fields);
        content.append(&error);
        content.append(&buttons);

        let window = gtk::Window::builder()
            .title("Bookmarks")
            .transient_for(parent)
            .default_width(600)
            .default_height(460)
            .hide_on_close(true)
            .child(&content)
            .build();
        closing::add_close_keys(&window);
        let close = closing::close_button(&window);
        close.set_hexpand(true);
        close.set_halign(gtk::Align::End);
        buttons.append(&close);

        let editor = Rc::new(Self {
            window,
            list,
            title,
            address,
            error,
            move_up,
            move_down,
            delete,
            add,
            apply: Box::new(apply),
            open: Box::new(open),
            current_page: Box::new(current_page),
            items: RefCell::new(Vec::new()),
            editing: Cell::new(None),
            adding: Cell::new(None),
            select_next: Cell::new(None),
            loading: Cell::new(false),
        });
        editor.connect();
        editor
    }

    fn connect(self: &Rc<Self>) {
        let editor = Rc::downgrade(self);
        self.list.connect_row_selected(move |_, row| {
            if let Some(editor) = editor.upgrade() {
                editor.load(row.map(|row| row.index() as usize));
            }
        });

        let editor = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(editor) = editor.upgrade() else {
                return;
            };
            let url = editor
                .items
                .borrow()
                .get(row.index() as usize)
                .map(|item| item.url.clone());
            if let Some(url) = url {
                (editor.open)(&url);
            }
        });

        // Edits are made when a field is left or Enter is pressed in it, rather
        // than with every key, which would save half-typed addresses.
        for entry in [&self.title, &self.address] {
            let editor = Rc::downgrade(self);
            entry.connect_activate(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.save_fields();
                }
            });
            let focus = gtk::EventControllerFocus::new();
            let editor = Rc::downgrade(self);
            focus.connect_leave(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.save_fields();
                }
            });
            entry.add_controller(focus);
        }

        // Closing keeps a change being typed, as leaving its field would, and
        // otherwise drops it, so it isn't still there when the window reopens.
        let editor = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(editor) = editor.upgrade() {
                editor.save_fields();
                editor.load(editor.editing.get());
            }
            glib::Propagation::Proceed
        });

        let editor = Rc::downgrade(self);
        self.add.connect_clicked(move |_| {
            if let Some(editor) = editor.upgrade() {
                editor.start_adding();
            }
        });

        let buttons = [(&self.move_up, -1), (&self.move_down, 1), (&self.delete, 0)];
        for (button, offset) in buttons {
            let editor = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(editor) = editor.upgrade() else {
                    return;
                };
                let Some(index) = editor.editing.get() else {
                    return;
                };
                let (edit, next) = if offset == 0 {
                    let last = editor.items.borrow().len().saturating_sub(2);
                    (Edit::Remove(index), index.min(last))
                } else {
                    let to = index.saturating_add_signed(offset);
                    (Edit::Move { from: index, to }, to)
                };
                editor.select_next.set(Some(next));
                editor.make(edit);
            });
        }
    }

    /// Shows the bookmarks, such as after they change.
    pub fn show_bookmarks(&self, items: &[Bookmark]) {
        let selected = self
            .select_next
            .take()
            .or(self.editing.get())
            .map(|index| index.min(items.len().saturating_sub(1)));
        self.items.replace(items.to_vec());

        self.loading.set(true);
        self.list.remove_all();
        for item in items {
            let title = gtk::Label::builder()
                .label(&item.title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let url = gtk::Label::builder()
                .label(&item.url)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["dim-label", "caption"])
                .build();
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(6)
                .margin_end(6)
                .build();
            row.append(&title);
            row.append(&url);
            self.list.append(&row);
        }
        self.loading.set(false);

        if self.adding.get().is_some() && selected.is_none() {
            // Keep what's being typed for a new bookmark.
            return;
        }
        let row = selected.and_then(|index| self.list.row_at_index(index as i32));
        self.list.select_row(row.as_ref());
        self.load(row.map(|row| row.index() as usize));
    }

    /// Fills in the fields for a bookmark, or empties them.
    fn load(&self, index: Option<usize>) {
        if self.loading.get() {
            return;
        }
        let item = index.and_then(|index| self.items.borrow().get(index).cloned());
        let index = item.as_ref().and(index);
        self.editing.set(index);
        self.adding.set(None);

        self.loading.set(true);
        self.title
            .set_text(item.as_ref().map_or("", |item| item.title.as_str()));
        self.address
            .set_text(item.as_ref().map_or("", |item| item.url.as_str()));
        self.loading.set(false);
        self.show_error(None);

        let count = self.items.borrow().len();
        let selected = index.is_some();
        self.title.set_sensitive(selected);
        self.address.set_sensitive(selected);
        self.delete.set_sensitive(selected);
        self.move_up
            .set_sensitive(index.is_some_and(|index| index > 0));
        self.move_down
            .set_sensitive(index.is_some_and(|index| index + 1 < count));
    }

    /// Empties the fields for a new bookmark, or fills them in with the current
    /// page if it isn't bookmarked. It goes after the selected one, if any.
    fn start_adding(&self) {
        let index = self
            .editing
            .get()
            .map_or(self.items.borrow().len(), |index| index + 1);
        self.list.unselect_all();
        self.editing.set(None);
        self.adding.set(Some(index));

        let page = (self.current_page)()
            .filter(|page| !self.items.borrow().iter().any(|item| item.url == page.url));
        self.loading.set(true);
        self.title
            .set_text(page.as_ref().map_or("", |page| page.title.as_str()));
        self.address
            .set_text(page.as_ref().map_or("", |page| page.url.as_str()));
        self.loading.set(false);
        self.show_error(None);

        self.title.set_sensitive(true);
        self.address.set_sensitive(true);
        self.delete.set_sensitive(false);
        self.move_up.set_sensitive(false);
        self.move_down.set_sensitive(false);
        if page.is_some() {
            self.title.grab_focus();
        } else {
            self.address.grab_focus();
        }
    }

    fn save_fields(&self) {
        if self.loading.get() {
            return;
        }
        if let Some(index) = self.adding.get() {
            self.save_new(index);
            return;
        }
        let Some(index) = self.editing.get() else {
            return;
        };
        let Some(item) = self.items.borrow().get(index).cloned() else {
            return;
        };
        let title = self.title.text().to_string();
        let address = self.address.text().to_string();
        if title == item.title && address == item.url {
            return;
        }
        let Some(url) = parse_location(&address) else {
            self.show_error(Some("That isn't a valid address."));
            return;
        };
        self.select_next.set(Some(index));
        self.make(Edit::Change {
            index,
            url: url.to_string(),
            title,
        });
    }

    /// Adds the new bookmark once it has an address. Until then, as when moving
    /// from the title to the address, there's nothing to complain about.
    fn save_new(&self, index: usize) {
        let address = self.address.text().trim().to_owned();
        if address.is_empty() {
            return;
        }
        let Some(url) = parse_location(&address) else {
            self.show_error(Some("That isn't a valid address."));
            return;
        };
        let title = self.title.text().trim().to_owned();
        let title = if title.is_empty() {
            url.to_string()
        } else {
            title
        };
        self.select_next.set(Some(index));
        self.make(Edit::Add {
            index,
            url: url.to_string(),
            title,
        });
    }

    fn make(&self, edit: Edit) {
        if let Err(error) = (self.apply)(edit) {
            self.select_next.set(None);
            self.show_error(Some(&error));
        }
    }

    fn show_error(&self, error: Option<&str>) {
        self.error.set_text(error.unwrap_or_default());
        self.error.set_visible(error.is_some());
    }

    pub fn present(&self) {
        self.window.present();
        // Show the list scrolled to whatever's selected, once it's laid out.
        let list = self.list.clone();
        glib::idle_add_local_once(move || {
            if let Some(row) = list.selected_row() {
                row.grab_focus();
            }
        });
    }
}
