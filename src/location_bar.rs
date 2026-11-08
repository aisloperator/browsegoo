//! Completion in the location field: the rest of a bookmarked address is filled
//! in as it's typed, and matching bookmarks are listed below the field.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::bookmarks::Bookmark;
use crate::completion::{self, Autofill};

const MAX_SUGGESTIONS: usize = 8;

pub struct LocationBar {
    entry: gtk::Entry,
    popover: gtk::Popover,
    list: gtk::ListBox,
    bookmarks: Box<dyn Fn() -> Vec<Bookmark>>,
    open: Box<dyn Fn(&str)>,
    /// What was actually typed, without any filled-in completion.
    typed: RefCell<String>,
    autofill: RefCell<Option<Autofill>>,
    suggestions: RefCell<Vec<Bookmark>>,
    /// Set while the text is being changed other than by typing.
    setting_text: Cell<bool>,
}

impl LocationBar {
    /// Adds completion to `entry`, from `bookmarks`, calling `open` with a URL
    /// when a suggestion is chosen.
    pub fn new(
        entry: &gtk::Entry,
        bookmarks: impl Fn() -> Vec<Bookmark> + 'static,
        open: impl Fn(&str) + 'static,
    ) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .focusable(false)
            .build();
        let popover = gtk::Popover::builder()
            .child(&list)
            .position(gtk::PositionType::Bottom)
            .has_arrow(false)
            // The field keeps the keyboard focus while this is open.
            .autohide(false)
            .focusable(false)
            .build();
        popover.set_parent(entry);
        let attached = popover.clone();
        entry.connect_destroy(move |_| attached.unparent());

        let bar = Rc::new(Self {
            entry: entry.clone(),
            popover,
            list,
            bookmarks: Box::new(bookmarks),
            open: Box::new(open),
            typed: RefCell::new(String::new()),
            autofill: RefCell::new(None),
            suggestions: RefCell::new(Vec::new()),
            setting_text: Cell::new(false),
        });
        bar.connect();
        bar
    }

    fn connect(self: &Rc<Self>) {
        let bar = Rc::downgrade(self);
        self.entry.connect_changed(move |entry| {
            let Some(bar) = bar.upgrade() else {
                return;
            };
            if bar.setting_text.get() {
                return;
            }
            // Changing the text from inside its own change notification isn't safe.
            let text = entry.text().to_string();
            let bar = Rc::downgrade(&bar);
            glib::idle_add_local_once(move || {
                if let Some(bar) = bar.upgrade() {
                    bar.text_typed(text);
                }
            });
        });

        let bar = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(bar) = bar.upgrade() else {
                return;
            };
            let url = bar
                .suggestions
                .borrow()
                .get(row.index() as usize)
                .map(|bookmark| bookmark.url.clone());
            bar.hide_suggestions();
            if let Some(url) = url {
                bar.set_text(&url);
                (bar.open)(&url);
            }
        });

        // In the capture phase, to come before the field's own key handling.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let bar = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(bar) = bar.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let plain = !modifiers.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SHIFT_MASK,
            );
            match key {
                gdk::Key::Tab if plain && bar.accept_completion() => glib::Propagation::Stop,
                gdk::Key::Down | gdk::Key::KP_Down if bar.popover.is_visible() => {
                    bar.move_selection(1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up | gdk::Key::KP_Up if bar.popover.is_visible() => {
                    bar.move_selection(-1);
                    glib::Propagation::Stop
                }
                gdk::Key::Escape if bar.popover.is_visible() => {
                    bar.hide_suggestions();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.entry.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let bar = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(bar) = bar.upgrade() {
                bar.hide_suggestions();
            }
        });
        self.entry.add_controller(focus);
    }

    /// Sets the text other than by typing, such as to show a page's address.
    pub fn set_text(&self, text: &str) {
        self.setting_text.set(true);
        self.entry.set_text(text);
        self.setting_text.set(false);
        self.typed.replace(text.to_owned());
        self.autofill.replace(None);
        self.hide_suggestions();
    }

    /// The location to go to when the field is activated, which for a
    /// filled-in completion is the bookmark's own URL, with its scheme.
    pub fn activation_text(&self) -> String {
        let text = self.entry.text().to_string();
        let autofill_url = self
            .autofill
            .borrow()
            .as_ref()
            .filter(|autofill| autofill.text == text)
            .map(|autofill| autofill.url.clone());
        self.hide_suggestions();
        autofill_url.unwrap_or(text)
    }

    fn text_typed(&self, text: String) {
        if self.entry.text() != text.as_str() {
            // More has been typed since.
            return;
        }
        let previous = self.typed.replace(text.clone());
        let bookmarks = (self.bookmarks)();

        // Only typing more at the end fills in, so that deleting a completion
        // doesn't bring it straight back.
        let typed_more =
            text.len() > previous.len() && self.entry.position() == text.chars().count() as i32;
        let autofill = typed_more
            .then(|| completion::autofill(&text, &bookmarks))
            .flatten();
        if let Some(autofill) = &autofill {
            self.setting_text.set(true);
            self.entry.set_text(&autofill.text);
            self.entry.select_region(text.chars().count() as i32, -1);
            self.setting_text.set(false);
        }
        self.autofill.replace(autofill);

        let suggestions: Vec<Bookmark> =
            completion::suggestions(&text, &bookmarks, MAX_SUGGESTIONS)
                .into_iter()
                .cloned()
                .collect();
        self.show_suggestions(suggestions);
    }

    /// Accepts a filled-in completion, if there is one.
    fn accept_completion(&self) -> bool {
        let text = self.entry.text();
        let has_completion = self.entry.selection_bounds().is_some_and(|(_, end)| {
            end == text.chars().count() as i32 && self.autofill.borrow().is_some()
        });
        if has_completion {
            self.entry.set_position(-1);
            self.typed.replace(text.to_string());
        }
        has_completion
    }

    fn show_suggestions(&self, suggestions: Vec<Bookmark>) {
        self.list.remove_all();
        for bookmark in &suggestions {
            let title = gtk::Label::builder()
                .label(&bookmark.title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let url = gtk::Label::builder()
                .label(&bookmark.url)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["dim-label", "caption"])
                .build();
            let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
            row.append(&title);
            row.append(&url);
            self.list.append(&row);
            if let Some(row) = self.list.last_child() {
                // So that clicking it doesn't take the focus from the field.
                row.set_focusable(false);
            }
        }
        let has_suggestions = !suggestions.is_empty();
        self.suggestions.replace(suggestions);

        if has_suggestions && self.entry_has_focus() {
            let width = self.entry.width();
            self.list.set_size_request(width, -1);
            self.popover.set_pointing_to(Some(&gdk::Rectangle::new(
                0,
                0,
                width,
                self.entry.height(),
            )));
            self.popover.popup();
        } else {
            self.popover.popdown();
        }
    }

    /// Whether the keyboard focus is in the field, which is really in a child of it.
    fn entry_has_focus(&self) -> bool {
        let entry: &gtk::Widget = self.entry.upcast_ref();
        self.entry
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focused| &focused == entry || focused.is_ancestor(entry))
    }

    fn hide_suggestions(&self) {
        self.popover.popdown();
        self.list.unselect_all();
    }

    /// Moves through the suggestions, showing the selected one's address in the
    /// field, and what was typed when going back past the first.
    fn move_selection(&self, offset: i32) {
        let count = self.suggestions.borrow().len() as i32;
        let current = self.list.selected_row().map_or(-1, |row| row.index());
        let next = (current + offset).clamp(-1, count - 1);
        let text = match self.list.row_at_index(next) {
            Some(row) => {
                self.list.select_row(Some(&row));
                self.suggestions.borrow()[next as usize].url.clone()
            }
            None => {
                self.list.unselect_all();
                self.typed.borrow().clone()
            }
        };
        self.setting_text.set(true);
        self.entry.set_text(&text);
        self.entry.set_position(-1);
        self.setting_text.set(false);
        self.autofill.replace(None);
    }
}
