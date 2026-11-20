//! The preferences window, for the fonts that pages can use, the User-Agent,
//! and filter lists.

use std::collections::BTreeMap;
use std::time::UNIX_EPOCH;

use gtk::glib;
use gtk::prelude::*;

use crate::closing;
use crate::filter_lists::{LISTS, ListStatus, Settings};
use crate::user_agent::Choice;

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
    /// Whether to limit the fonts that pages can use, from the next start.
    pub limit_fonts: gtk::CheckButton,
    /// Which User-Agent pages get, in the order of `Choice::ALL`.
    pub user_agent: gtk::DropDown,
}

impl PreferencesWindow {
    /// `fonts_note` says why the fonts that pages can use aren't limited as
    /// chosen, if they aren't.
    pub fn new(
        parent: &gtk::ApplicationWindow,
        settings: &Settings,
        limit_fonts: bool,
        fonts_note: Option<&str>,
        user_agent: Choice,
    ) -> Self {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let section = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .css_classes(["title-4"])
                .build()
        };
        let explain = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .wrap(true)
                .margin_start(28)
                .css_classes(["dim-label", "caption"])
                .build()
        };
        content.append(&section("Fonts"));
        let limit_fonts = gtk::CheckButton::builder()
            .label("Limit the fonts that pages can use")
            .active(limit_fonts)
            .build();
        content.append(&limit_fonts);
        content.append(&explain(
            "Pages can only use DejaVu, Liberation and Noto Color Emoji, and fonts \
             they provide themselves, so that they can't tell which fonts are \
             installed, which would help to tell you apart from other people. Text \
             in other scripts, such as Chinese, Japanese, Korean or Thai, may not \
             show unless the page provides a font for it. A change takes effect \
             when BrowseGoo is started again.",
        ));
        if let Some(note) = fonts_note {
            let note = explain(note);
            note.remove_css_class("dim-label");
            note.add_css_class("warning");
            content.append(&note);
        }

        content.append(&section("User-Agent"));
        let labels: Vec<&str> = Choice::ALL.iter().map(|choice| choice.label()).collect();
        let user_agent_choices = gtk::DropDown::from_strings(&labels);
        user_agent_choices.set_halign(gtk::Align::Start);
        user_agent_choices.set_selected(
            Choice::ALL
                .iter()
                .position(|&choice| choice == user_agent)
                .unwrap_or(0) as u32,
        );
        content.append(&user_agent_choices);
        content.append(&explain(
            "What pages are told this browser is, in requests and to their \
             scripts. Actual says BrowseGoo and its version, but not the window \
             system, operating system or hardware. The others are for developing \
             sites that treat browsers differently: WebKit's default says it's \
             Safari, and the others are those of the latest Chrome and Firefox on \
             64-bit Windows 11. With WebKit's default, WebKit can tell some sites \
             it's another browser, among its fixes for particular sites, which \
             are off with the others so that every page gets the same one. A \
             change takes effect when a page is next loaded.",
        ));

        let heading = gtk::Label::builder()
            .label("Filter Lists")
            .xalign(0.0)
            .css_classes(["title-4"])
            .build();
        let intro = gtk::Label::builder()
            .label(
                "Requests that the enabled lists match are blocked. Enabled lists \
                 that haven't been downloaded yet are downloaded when BrowseGoo \
                 starts. Otherwise, lists are only downloaded when you press \
                 Update Lists.",
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
        closing::add_close_keys(&window);
        let close = closing::close_button(&window);
        close.set_halign(gtk::Align::End);
        close.set_margin_top(6);
        content.append(&close);

        Self {
            window,
            rows,
            update,
            update_status,
            limit_fonts,
            user_agent: user_agent_choices,
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
    if let Some(problem) = &status.problem {
        return format!("Downloaded {when}, but it couldn't be used: {problem}");
    }
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
