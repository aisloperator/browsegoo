//! The window for blocking the sites of entries selected in the Cookies and
//! Site Data window, which suggests rules for them to change if need be, and
//! adds them to the end of the user's own rules.

use std::collections::HashSet;

use gtk::prelude::*;

use crate::closing;

/// Opens the window, with a rule for each of `hosts`, over `parent`. `apply`
/// adds the rules as they are then, or says why it couldn't.
pub fn show<'a>(
    parent: &impl IsA<gtk::Window>,
    hosts: impl IntoIterator<Item = &'a str>,
    apply: impl Fn(&str) -> Result<(), String> + 'static,
) {
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
            "These rules block all requests to the selected sites and their subdomains. \
             With $important, they apply even where a filter list or another of your \
             rules makes an exception. Change them as you like before applying them, \
             which adds them to the end of your filter rules.",
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
    view.buffer().set_text(&block_rules(hosts));
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .has_frame(true)
        .overlay_scrolling(false)
        .build();
    content.append(&scrolled);

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .selectable(true)
        .build();
    let cancel = gtk::Button::with_label("Cancel");
    let apply_button = gtk::Button::builder()
        .label("Apply")
        .css_classes(["suggested-action"])
        .build();
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bottom.append(&status);
    bottom.append(&cancel);
    bottom.append(&apply_button);
    content.append(&bottom);

    let window = gtk::Window::builder()
        .title("Block Sites")
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(360)
        .child(&content)
        .build();
    closing::add_close_keys(&window);

    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let weak = window.downgrade();
    apply_button.connect_clicked(move |_| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let buffer = view.buffer();
        let (start, end) = buffer.bounds();
        let text = buffer.text(&start, &end, false);
        let result = if text.trim().is_empty() {
            Ok(())
        } else {
            apply(&text)
        };
        match result {
            Ok(()) => window.close(),
            Err(problem) => status.set_text(&problem),
        }
    });

    apply_button.grab_focus();
    window.present();
}

/// A rule blocking each host and its subdomains, leaving out repeats. A
/// cookie's domain starts with a dot when it's also sent to subdomains,
/// which the rules include anyway.
fn block_rules<'a>(hosts: impl IntoIterator<Item = &'a str>) -> String {
    let mut seen = HashSet::new();
    let mut rules = String::new();
    for host in hosts {
        let host = host.trim().trim_start_matches('.').to_lowercase();
        if !host.is_empty() && seen.insert(host.clone()) {
            rules.push_str(&format!("||{host}^$important\n"));
        }
    }
    rules
}

/// `rules` with `added` at the end, on lines of their own.
pub fn append_rules(rules: &str, added: &str) -> String {
    let mut text = rules.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(added);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_rule_per_host() {
        assert_eq!(
            block_rules([".foo.bar", "foo.bar", "Foo.Bar", "", "ads.example"]),
            "||foo.bar^$important\n||ads.example^$important\n"
        );
    }

    #[test]
    fn appending_starts_a_new_line() {
        assert_eq!(append_rules("", "b\n"), "b\n");
        assert_eq!(append_rules("a", "b"), "a\nb\n");
        assert_eq!(append_rules("a\n", "b\n"), "a\nb\n");
    }
}
