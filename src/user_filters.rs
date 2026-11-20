//! The window for editing the user's own filter rules, which are used along
//! with the filter lists.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use crate::closing;
use crate::filter;
use crate::filter_lists::ListStatus;

/// At most this many line numbers are listed as not applied.
const MOST_LINES_LISTED: usize = 10;

pub struct UserFiltersWindow {
    window: gtk::Window,
    buffer: gtk::TextBuffer,
    apply: gtk::Button,
    status: gtk::Label,
    /// The text as last applied, to tell whether it's been changed since.
    applied: RefCell<String>,
    /// What's known about the applied rules, once they've been converted.
    applied_status: RefCell<String>,
    /// False if the rules couldn't be read, as saving would replace them.
    editable: Cell<bool>,
}

impl UserFiltersWindow {
    /// A window for editing `rules`, the text of the rules' file, if it could be
    /// read. `apply` saves new rules and starts using them.
    pub fn new(
        parent: &gtk::ApplicationWindow,
        rules: Result<String, String>,
        apply: impl Fn(&str) -> Result<(), String> + 'static,
    ) -> Rc<Self> {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let intro = gtk::Label::builder()
            .label(
                "Your own rules, used along with the filter lists, one per line. They're in \
                 the Adblock Plus format that the lists and uBlock Origin use, such as \
                 ||ads.example.com^ to block a site, or @@||example.com^ to make an \
                 exception. Lines that start with ! are comments.",
            )
            .xalign(0.0)
            .wrap(true)
            .build();
        content.append(&intro);

        let view = gtk::TextView::builder()
            .monospace(true)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .has_frame(true)
            // Scrollbars that show whenever there's more, as in the site data window.
            .overlay_scrolling(false)
            .build();
        content.append(&scrolled);

        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .selectable(true)
            .build();
        let apply_button = gtk::Button::builder()
            .label("Apply")
            .sensitive(false)
            .css_classes(["suggested-action"])
            .build();
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bottom.append(&status);
        bottom.append(&apply_button);
        content.append(&bottom);

        let window = gtk::Window::builder()
            .title("Your Filter Rules")
            .transient_for(parent)
            .default_width(640)
            .default_height(480)
            // Closing keeps what was typed, for when it's opened again.
            .hide_on_close(true)
            .child(&content)
            .build();
        closing::add_close_keys(&window);
        bottom.append(&closing::close_button(&window));

        let buffer = view.buffer();
        let this = Rc::new(Self {
            window,
            buffer: buffer.clone(),
            apply: apply_button.clone(),
            status,
            applied: RefCell::new(String::new()),
            applied_status: RefCell::new(String::new()),
            editable: Cell::new(true),
        });
        match rules {
            Ok(text) => {
                buffer.set_text(&text);
                this.applied.replace(text);
            }
            Err(problem) => {
                view.set_editable(false);
                this.editable.set(false);
                this.applied_status
                    .replace(format!("Your rules couldn't be read: {problem}"));
            }
        }
        buffer.place_cursor(&buffer.start_iter());

        let weak = Rc::downgrade(&this);
        buffer.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.show_state();
            }
        });
        let weak = Rc::downgrade(&this);
        apply_button.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let text = this.text();
            match apply(&text) {
                Ok(()) => {
                    this.applied.replace(text);
                    this.applied_status.replace("Applying…".to_owned());
                }
                Err(problem) => {
                    this.applied_status.replace(problem);
                }
            }
            this.show_state();
        });

        this.show_state();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Shows that `added` was added to the end of the rules, which are now
    /// `applied`. Any changes not yet applied are kept.
    pub fn show_added(&self, applied: &str, added: &str) {
        if !self.editable.get() {
            return;
        }
        // Inserted rather than set, to keep the cursor where it was.
        let text = self.text();
        let appended = crate::block_sites::append_rules(&text, added);
        self.applied.replace(applied.to_owned());
        self.applied_status.replace("Applying…".to_owned());
        self.buffer
            .insert(&mut self.buffer.end_iter(), &appended[text.len()..]);
        self.show_state();
    }

    fn text(&self) -> String {
        let (start, end) = self.buffer.bounds();
        self.buffer.text(&start, &end, false).to_string()
    }

    fn has_changes(&self) -> bool {
        *self.applied.borrow() != self.text()
    }

    /// Shows what's known about the applied rules, once they're in use.
    pub fn show_status(&self, status: Option<&ListStatus>) {
        let text = match status {
            Some(ListStatus {
                problem: Some(problem),
                ..
            }) => format!("Your rules couldn't be used: {problem}"),
            Some(ListStatus {
                stats: Some(stats), ..
            }) => {
                let not_applied = filter::lines_not_applied(&self.applied.borrow());
                match (stats.rules, not_applied.is_empty()) {
                    (0, true) => "You have no rules.".to_owned(),
                    (rules, _) => {
                        let using = match rules {
                            1 => "Using 1 rule.".to_owned(),
                            count => format!("Using {count} rules."),
                        };
                        format!(
                            "{using}{} They apply from each tab's next page load.",
                            describe_lines_not_applied(&not_applied)
                        )
                    }
                }
            }
            _ => String::new(),
        };
        self.applied_status.replace(text);
        self.show_state();
    }

    fn show_state(&self) {
        let changed = self.editable.get() && self.has_changes();
        self.apply.set_sensitive(changed);
        let applied = self.applied_status.borrow();
        let text = match (changed, applied.is_empty()) {
            (true, _) => "Your changes aren't applied yet.",
            (false, false) => applied.as_str(),
            (false, true) => "",
        };
        self.status.set_text(text);
    }
}

/// Says which lines aren't applied, if any.
fn describe_lines_not_applied(lines: &[usize]) -> String {
    let listed: Vec<String> = lines
        .iter()
        .take(MOST_LINES_LISTED)
        .map(usize::to_string)
        .collect();
    let more = lines.len().saturating_sub(MOST_LINES_LISTED);
    match (lines.len(), more) {
        (0, _) => String::new(),
        (1, _) => format!(
            " Line {} isn't applied, as that kind of rule isn't supported.",
            listed[0]
        ),
        (_, 0) => format!(
            " Lines {} aren't applied, as those kinds of rules aren't supported.",
            listed.join(", ")
        ),
        (_, more) => format!(
            " Lines {} and {more} more aren't applied, as those kinds of rules aren't \
             supported.",
            listed.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_not_applied_are_listed() {
        assert_eq!(describe_lines_not_applied(&[]), "");
        assert!(describe_lines_not_applied(&[3]).starts_with(" Line 3 isn't"));
        assert!(describe_lines_not_applied(&[3, 5]).starts_with(" Lines 3, 5 aren't"));
        let many: Vec<usize> = (1..=12).collect();
        assert!(
            describe_lines_not_applied(&many)
                .starts_with(" Lines 1, 2, 3, 4, 5, 6, 7, 8, 9, 10 and 2 more")
        );
    }
}
