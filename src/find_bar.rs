//! The bar at the bottom of the window for finding text in the page, which
//! slides up below the page rather than covering it.

use gtk::prelude::*;

pub struct FindBar {
    pub revealer: gtk::Revealer,
    pub entry: gtk::SearchEntry,
    pub match_case: gtk::CheckButton,
    pub previous: gtk::Button,
    pub next: gtk::Button,
    pub close: gtk::Button,
    status: gtk::Label,
}

impl FindBar {
    pub fn new() -> Self {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Find in page")
            .width_chars(28)
            .search_delay(100)
            .build();
        let previous = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Previous match (Shift+Enter)")
            .build();
        let next = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("Next match (Enter)")
            .build();
        let buttons = gtk::Box::builder().css_classes(["linked"]).build();
        buttons.append(&previous);
        buttons.append(&next);
        let match_case = gtk::CheckButton::with_label("Match Case");
        let status = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close (Escape)")
            .css_classes(["flat"])
            .build();

        let bar = gtk::Box::builder()
            .spacing(8)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(8)
            .margin_end(8)
            .build();
        bar.append(&entry);
        bar.append(&buttons);
        bar.append(&match_case);
        bar.append(&status);
        bar.append(&close);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&bar);

        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .child(&content)
            .build();

        Self {
            revealer,
            entry,
            match_case,
            previous,
            next,
            close,
            status,
        }
    }

    pub fn is_open(&self) -> bool {
        self.revealer.reveals_child()
    }

    /// Shows the bar, ready to type what to find.
    pub fn open(&self) {
        self.revealer.set_reveal_child(true);
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    pub fn close(&self) {
        self.revealer.set_reveal_child(false);
        self.show_matches(None);
    }

    pub fn text(&self) -> String {
        self.entry.text().to_string()
    }

    /// Shows how many matches were found, if the text was looked for.
    pub fn show_matches(&self, matches: Option<u32>) {
        let text = match matches {
            None => String::new(),
            Some(0) => "Phrase not found".to_owned(),
            Some(1) => "1 match".to_owned(),
            Some(count) if count >= crate::browser::MOST_MATCHES => {
                format!("More than {} matches", crate::browser::MOST_MATCHES - 1)
            }
            Some(count) => format!("{count} matches"),
        };
        self.status.set_text(&text);
        if matches == Some(0) {
            self.entry.add_css_class("error");
        } else {
            self.entry.remove_css_class("error");
        }
    }
}
