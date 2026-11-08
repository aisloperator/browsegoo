//! The window for editing, reordering and deleting bookmarks.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::bookmarks::Bookmark;
use crate::location::parse_location;

/// A change to the bookmarks, which the browser makes and saves.
pub enum Edit {
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
    apply: Box<dyn Fn(Edit) -> Result<(), String>>,
    open: Box<dyn Fn(&str)>,
    items: RefCell<Vec<Bookmark>>,
    /// The bookmark the fields are for.
    editing: Cell<Option<usize>>,
    /// Which bookmark to select when the list is next shown.
    select_next: Cell<Option<usize>>,
    /// Set while the fields are being filled in, rather than edited.
    loading: Cell<bool>,
}

impl BookmarksEditor {
    /// Makes the window. `apply` makes changes, and `open` opens a URL.
    pub fn new(
        parent: &gtk::ApplicationWindow,
        apply: impl Fn(Edit) -> Result<(), String> + 'static,
        open: impl Fn(&str) + 'static,
    ) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .build();
        list.set_placeholder(Some(
            &gtk::Label::builder()
                .label("No bookmarks yet. The star button bookmarks the current page.")
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

        let move_up = gtk::Button::with_label("Move Up");
        let move_down = gtk::Button::with_label("Move Down");
        let delete = gtk::Button::builder()
            .label("Delete")
            .css_classes(["destructive-action"])
            .hexpand(true)
            .halign(gtk::Align::End)
            .build();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
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

        let editor = Rc::new(Self {
            window,
            list,
            title,
            address,
            error,
            move_up,
            move_down,
            delete,
            apply: Box::new(apply),
            open: Box::new(open),
            items: RefCell::new(Vec::new()),
            editing: Cell::new(None),
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

    fn save_fields(&self) {
        if self.loading.get() {
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
