//! Ways to close BrowseGoo's other windows, besides the window manager's.

use gtk::prelude::*;

/// Lets Ctrl+W and Escape close `window`.
pub fn add_close_keys(window: &impl IsA<gtk::Window>) {
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>w|Escape"),
        Some(gtk::CallbackAction::new(|widget, _| {
            if let Some(window) = widget.downcast_ref::<gtk::Window>() {
                window.close();
            }
            gtk::glib::Propagation::Stop
        })),
    ));
    window.as_ref().add_controller(shortcuts);
}

/// A Close button for `window`.
pub fn close_button(window: &impl IsA<gtk::Window>) -> gtk::Button {
    let button = gtk::Button::with_label("Close");
    let window = window.as_ref().downgrade();
    button.connect_clicked(move |_| {
        if let Some(window) = window.upgrade() {
            window.close();
        }
    });
    button
}
