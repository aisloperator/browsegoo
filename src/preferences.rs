//! The preferences window, which for now is about filter lists.

use std::collections::BTreeMap;
use std::time::UNIX_EPOCH;

use gtk::glib;
use gtk::prelude::*;

use crate::filter_lists::{LISTS, ListStatus, Settings};

pub struct ListRow {
    pub id: &'static str,
    pub check: gtk::CheckButton,
    status: gtk::Label,
}

pub struct PreferencesWindow {
    pub window: gtk::Window,
    pub rows: Vec<ListRow>,
    pub update: gtk::Button,
    update_status: gtk::Label,
}

impl PreferencesWindow {
    pub fn new(parent: &gtk::ApplicationWindow, settings: &Settings) -> Self {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let heading = gtk::Label::builder()
            .label("Filter Lists")
            .xalign(0.0)
            .css_classes(["title-4"])
            .build();
        let intro = gtk::Label::builder()
            .label(
                "Requests that the enabled lists match are blocked. Lists are only \
                 downloaded when you press Update Lists.",
            )
            .xalign(0.0)
            .wrap(true)
            .build();
        content.append(&heading);
        content.append(&intro);

        let mut rows = Vec::new();
        for list in &LISTS {
            let check = gtk::CheckButton::builder()
                .label(list.name)
                .active(settings.is_enabled(list))
                .build();
            let details = |text: &str| {
                gtk::Label::builder()
                    .label(text)
                    .xalign(0.0)
                    .margin_start(28)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .css_classes(["dim-label", "caption"])
                    .build()
            };
            let url = details(list.url);
            url.set_selectable(true);
            url.set_tooltip_text(Some(list.url));
            // The status is worth reading in full.
            let status = details("");
            status.set_ellipsize(gtk::pango::EllipsizeMode::None);
            status.set_wrap(true);

            let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
            row.append(&check);
            row.append(&url);
            row.append(&status);
            content.append(&row);
            rows.push(ListRow {
                id: list.id,
                check,
                status,
            });
        }

        let update_status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .build();
        let update = gtk::Button::with_label("Update Lists");
        let update_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        update_row.set_margin_top(6);
        update_row.append(&update_status);
        update_row.append(&update);
        content.append(&update_row);

        let window = gtk::Window::builder()
            .title("BrowseGoo Preferences")
            .transient_for(parent)
            .default_width(560)
            .hide_on_close(true)
            .child(&content)
            .build();

        Self {
            window,
            rows,
            update,
            update_status,
        }
    }

    pub fn show_statuses(&self, statuses: &BTreeMap<&'static str, ListStatus>) {
        for row in &self.rows {
            let status = statuses.get(row.id).cloned().unwrap_or_default();
            row.status.set_text(&describe(&status));
        }
    }

    pub fn show_update_state(&self, updating: bool, message: &str) {
        self.update.set_sensitive(!updating);
        self.update_status.set_text(message);
    }
}

fn describe(status: &ListStatus) -> String {
    let Some(downloaded) = status.downloaded else {
        return "Not downloaded yet".to_owned();
    };
    let when = downloaded
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|age| glib::DateTime::from_unix_local(age.as_secs() as i64).ok())
        .and_then(|time| time.format("%Y-%m-%d %H:%M").ok())
        .map_or_else(|| "at an unknown time".to_owned(), |time| time.to_string());
    match status.stats {
        Some(stats) => format!(
            "Downloaded {when}. Using {} rules, skipping {} unsupported ones.",
            with_separators(stats.rules),
            with_separators(stats.unsupported)
        ),
        None => format!("Downloaded {when}"),
    }
}

fn with_separators(number: usize) -> String {
    let digits = number.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::with_separators;

    #[test]
    fn thousands_separators() {
        assert_eq!(with_separators(0), "0");
        assert_eq!(with_separators(999), "999");
        assert_eq!(with_separators(1000), "1,000");
        assert_eq!(with_separators(106768), "106,768");
        assert_eq!(with_separators(1234567), "1,234,567");
    }
}
