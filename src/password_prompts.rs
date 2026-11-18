//! What connects saved passwords to pages: the script that watches their
//! forms, the window that asks whether to save a password that was entered,
//! and the menu of saved passwords to fill in.
//!
//! Nothing is filled in on a page unless the user chooses a password from the
//! menu, each time.

use gtk::prelude::*;
use gtk::{gdk, glib};
use webkit::javascriptcore;
use webkit::prelude::*;

use crate::closing;
use crate::password_manager::format_time;
use crate::passwords::{Record, RecordId};

/// The JavaScript world that the script runs in, apart from the page's.
const WORLD: &str = "browsegoo-passwords";
/// What the script sends its messages to, which only its world has.
const HANDLER: &str = "browsegooPasswords";
const SCRIPT: &str = include_str!("passwords.js");

/// What the script says about a page.
#[derive(Debug, PartialEq)]
pub enum PageMessage {
    /// A password was entered in a form, on `host`.
    Entered {
        host: String,
        username: String,
        password: String,
    },
    /// A username or password field was clicked. It's at `area` in the view.
    Clicked { host: String, area: gdk::Rectangle },
}

/// Adds the script to a tab's pages, and calls `received` with what it says.
pub fn watch_pages(content: &webkit::UserContentManager, received: impl Fn(PageMessage) + 'static) {
    content.add_script(&webkit::UserScript::for_world(
        SCRIPT,
        // A login form in a frame from another site would need that site's
        // passwords, so only the top frame's are filled in, for its site.
        webkit::UserContentInjectedFrames::TopFrame,
        webkit::UserScriptInjectionTime::Start,
        WORLD,
        &[],
        &[],
    ));
    if !content.register_script_message_handler(HANDLER, Some(WORLD)) {
        eprintln!("Could not watch pages for passwords");
        return;
    }
    content.connect_script_message_received(Some(HANDLER), move |_, value| {
        if let Some(message) = parse_message(value) {
            received(message);
        }
    });
}

fn parse_message(value: &javascriptcore::Value) -> Option<PageMessage> {
    if !value.is_object() {
        return None;
    }
    let text = |name: &str| {
        value
            .object_get_property(name)
            .filter(javascriptcore::Value::is_string)
            .map(|value| value.to_str().to_string())
    };
    let number = |name: &str| {
        value
            .object_get_property(name)
            .filter(javascriptcore::Value::is_number)
            .map(|value| value.to_double())
            .filter(|number| number.is_finite())
    };
    let host = text("host").filter(|host| !host.is_empty())?;
    match text("kind")?.as_str() {
        "entered" => Some(PageMessage::Entered {
            host,
            username: text("username")?,
            password: text("password").filter(|password| !password.is_empty())?,
        }),
        "clicked" => Some(PageMessage::Clicked {
            host,
            area: gdk::Rectangle::new(
                number("x")?.round() as i32,
                number("y")?.round() as i32,
                number("width")?.round().max(1.0) as i32,
                number("height")?.round().max(1.0) as i32,
            ),
        }),
        _ => None,
    }
}

/// Fills in the form whose field was last clicked with `record`, if the page
/// is still on its domain, and calls `filled` if it was.
pub fn fill(webview: &webkit::WebView, record: &Record, filled: impl FnOnce() + 'static) {
    let arguments = glib::VariantDict::new(None);
    arguments.insert("host", &record.domain);
    arguments.insert("username", &record.username);
    arguments.insert("password", &record.password);
    webview.call_async_javascript_function(
        "return browsegooFill(host, username, password);",
        Some(&arguments.end()),
        Some(WORLD),
        None,
        None::<&gtk::gio::Cancellable>,
        move |result| match result {
            Ok(value) if value.to_boolean() => filled(),
            Ok(_) => {}
            Err(error) => eprintln!("Could not fill in a password: {error}"),
        },
    );
}

/// Shows the menu of saved passwords for the page's site, below the field at
/// `area` in `webview`, and calls `chosen` with the one chosen, if any.
/// `records` must be in the order to list them.
pub fn show_menu(
    webview: &webkit::WebView,
    area: &gdk::Rectangle,
    records: &[Record],
    secure: bool,
    chosen: impl Fn(RecordId) + 'static,
) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    let title = gtk::Label::builder()
        .label("Saved passwords for this site")
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    content.append(&title);
    if !secure {
        let warning = gtk::Label::builder()
            .label("This page isn't secure. What's filled in could be seen\non the network.")
            .xalign(0.0)
            .css_classes(["warning"])
            .build();
        content.append(&warning);
    }

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Browse)
        .css_classes(["boxed-list"])
        .build();
    for record in records {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(6)
            .margin_end(6)
            .build();
        let username = match record.username.as_str() {
            "" => "(no username)",
            username => username,
        };
        row.append(
            &gtk::Label::builder()
                .label(username)
                .xalign(0.0)
                .max_width_chars(40)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        if records.len() > 1 {
            let when = if record.edited > record.created {
                format!("Edited {}", format_time(record.edited))
            } else {
                format!("Saved {}", format_time(record.created))
            };
            row.append(
                &gtk::Label::builder()
                    .label(when)
                    .xalign(0.0)
                    .css_classes(["dim-label", "caption"])
                    .build(),
            );
        }
        list.append(&row);
    }
    content.append(&list);

    let popover = gtk::Popover::builder()
        .child(&content)
        .position(gtk::PositionType::Bottom)
        .pointing_to(area)
        .build();
    popover.set_parent(webview);
    let ids: Vec<RecordId> = records.iter().map(|record| record.id).collect();
    let closing = popover.downgrade();
    list.connect_row_activated(move |_, row| {
        if let Some(popover) = closing.upgrade() {
            popover.popdown();
        }
        if let Some(&id) = ids.get(row.index() as usize) {
            chosen(id);
        }
    });
    let page = webview.downgrade();
    popover.connect_closed(move |popover| {
        // Not while GTK is still closing it.
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
        // Back to the field that was clicked.
        if let Some(webview) = page.upgrade() {
            webview.grab_focus();
        }
    });
    popover.popup();
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
        first.grab_focus();
    }
}

/// Asks whether to save the password entered on `domain`, with the username
/// and password to change if need be, and calls `save` with them, which says
/// why if it couldn't.
pub fn ask_to_save(
    parent: &gtk::ApplicationWindow,
    domain: &str,
    username: &str,
    password: &str,
    save: impl Fn(&str, &str) -> Result<(), String> + 'static,
) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let question = gtk::Label::builder()
        .label(format!("Save a password for {domain}?"))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["heading"])
        .build();
    content.append(&question);

    let grid = gtk::Grid::builder()
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
    let username_entry = gtk::Entry::builder()
        .text(username)
        .hexpand(true)
        .activates_default(true)
        .build();
    let password_entry = gtk::PasswordEntry::builder()
        .text(password)
        .show_peek_icon(true)
        .hexpand(true)
        .activates_default(true)
        .build();
    grid.attach(&label("Username"), 0, 0, 1, 1);
    grid.attach(&username_entry, 1, 0, 1, 1);
    grid.attach(&label("Password"), 0, 1, 1, 1);
    grid.attach(&password_entry, 1, 1, 1, 1);
    content.append(&grid);

    let problem = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["error"])
        .visible(false)
        .build();
    content.append(&problem);

    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::builder()
        .label("OK")
        .css_classes(["suggested-action"])
        .build();
    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::End)
        .build();
    buttons.append(&cancel);
    buttons.append(&ok);
    content.append(&buttons);

    let window = gtk::Window::builder()
        .title("Save Password")
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(420)
        .child(&content)
        .default_widget(&ok)
        .build();
    closing::add_close_keys(&window);

    let closing = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = closing.upgrade() {
            window.close();
        }
    });
    let closing = window.downgrade();
    ok.connect_clicked(move |_| {
        let username = username_entry.text();
        let password = password_entry.text();
        if password.is_empty() {
            problem.set_text("The password can't be empty.");
            problem.set_visible(true);
            return;
        }
        match save(&username, &password) {
            Ok(()) => {
                if let Some(window) = closing.upgrade() {
                    window.close();
                }
            }
            Err(error) => {
                problem.set_text(&error);
                problem.set_visible(true);
            }
        }
    });
    window.present();
    ok.grab_focus();
}
