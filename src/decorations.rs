//! Whether the window draws its own title bar, with the tabs and the buttons
//! for closing, maximizing and minimizing it, or leaves that to the window
//! manager.
//!
//! On X11, the window manager provides the title bar and its own ways to
//! manage windows, which may be keys rather than buttons, so the window leaves
//! it to the window manager, as other X11 programs do. Wayland compositors
//! leave that to each GTK window, so there the window draws its own.

use gtk::gdk;
use gtk::prelude::*;

/// What the window system is, as far as title bars are concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowSystem {
    X11,
    Other,
}

/// Whether the window should draw its own title bar, on the window system in
/// use. Setting `GTK_CSD` to 1 asks for that on X11 too, as it does for GTK's
/// own windows.
pub fn draw_own_title_bar() -> bool {
    let system = match gdk::Display::default() {
        Some(display) if display.type_().name() == "GdkX11Display" => WindowSystem::X11,
        _ => WindowSystem::Other,
    };
    draws_own_title_bar(system, std::env::var("GTK_CSD").ok().as_deref())
}

fn draws_own_title_bar(system: WindowSystem, gtk_csd: Option<&str>) -> bool {
    match system {
        WindowSystem::X11 => gtk_csd == Some("1"),
        WindowSystem::Other => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_leaves_the_title_bar_to_the_window_manager() {
        assert!(!draws_own_title_bar(WindowSystem::X11, None));
        assert!(!draws_own_title_bar(WindowSystem::X11, Some("0")));
        assert!(draws_own_title_bar(WindowSystem::X11, Some("1")));
    }

    #[test]
    fn other_window_systems_draw_their_own() {
        assert!(draws_own_title_bar(WindowSystem::Other, None));
        assert!(draws_own_title_bar(WindowSystem::Other, Some("0")));
    }
}
