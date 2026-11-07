//! Translates GTK input to Servo's input events.

use gtk::gdk;
use servo::{
    Code, Cursor, InputEvent, Key, KeyState, KeyboardEvent, Location, Modifiers, MouseButton,
    NamedKey,
};

pub fn keyboard_event(state: KeyState, key: gdk::Key, modifiers: gdk::ModifierType) -> InputEvent {
    InputEvent::Keyboard(KeyboardEvent::new_without_event(
        state,
        translate_key(key),
        Code::Unidentified,
        translate_location(key),
        translate_modifiers(modifiers),
        false,
        false,
    ))
}

fn translate_key(key: gdk::Key) -> Key {
    match named_key(key) {
        Some(named) => Key::Named(named),
        None => match key.to_unicode() {
            Some(character) if !character.is_control() => Key::Character(character.to_string()),
            _ => Key::Named(NamedKey::Unidentified),
        },
    }
}

fn named_key(key: gdk::Key) -> Option<NamedKey> {
    use gdk::Key as K;

    Some(match key {
        K::Return | K::KP_Enter | K::ISO_Enter => NamedKey::Enter,
        K::Tab | K::KP_Tab | K::ISO_Left_Tab => NamedKey::Tab,
        K::BackSpace => NamedKey::Backspace,
        K::Delete | K::KP_Delete => NamedKey::Delete,
        K::Insert | K::KP_Insert => NamedKey::Insert,
        K::Escape => NamedKey::Escape,
        K::Left | K::KP_Left => NamedKey::ArrowLeft,
        K::Right | K::KP_Right => NamedKey::ArrowRight,
        K::Up | K::KP_Up => NamedKey::ArrowUp,
        K::Down | K::KP_Down => NamedKey::ArrowDown,
        K::Home | K::KP_Home => NamedKey::Home,
        K::End | K::KP_End => NamedKey::End,
        K::Page_Up | K::KP_Page_Up => NamedKey::PageUp,
        K::Page_Down | K::KP_Page_Down => NamedKey::PageDown,
        K::Shift_L | K::Shift_R => NamedKey::Shift,
        K::Control_L | K::Control_R => NamedKey::Control,
        K::Alt_L | K::Alt_R => NamedKey::Alt,
        K::Super_L | K::Super_R | K::Meta_L | K::Meta_R => NamedKey::Meta,
        K::Caps_Lock => NamedKey::CapsLock,
        K::F1 => NamedKey::F1,
        K::F2 => NamedKey::F2,
        K::F3 => NamedKey::F3,
        K::F4 => NamedKey::F4,
        K::F5 => NamedKey::F5,
        K::F6 => NamedKey::F6,
        K::F7 => NamedKey::F7,
        K::F8 => NamedKey::F8,
        K::F9 => NamedKey::F9,
        K::F10 => NamedKey::F10,
        K::F11 => NamedKey::F11,
        K::F12 => NamedKey::F12,
        _ => return None,
    })
}

fn translate_location(key: gdk::Key) -> Location {
    use gdk::Key as K;

    match key {
        K::Shift_L | K::Control_L | K::Alt_L | K::Super_L | K::Meta_L => Location::Left,
        K::Shift_R | K::Control_R | K::Alt_R | K::Super_R | K::Meta_R => Location::Right,
        K::KP_Enter | K::KP_Left | K::KP_Right | K::KP_Up | K::KP_Down => Location::Numpad,
        _ => Location::Standard,
    }
}

fn translate_modifiers(state: gdk::ModifierType) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    modifiers.set(
        Modifiers::SHIFT,
        state.contains(gdk::ModifierType::SHIFT_MASK),
    );
    modifiers.set(
        Modifiers::CONTROL,
        state.contains(gdk::ModifierType::CONTROL_MASK),
    );
    modifiers.set(Modifiers::ALT, state.contains(gdk::ModifierType::ALT_MASK));
    modifiers.set(
        Modifiers::META,
        state.contains(gdk::ModifierType::META_MASK),
    );
    modifiers
}

pub fn mouse_button(gtk_button: u32) -> MouseButton {
    match gtk_button {
        gdk::BUTTON_PRIMARY => MouseButton::Primary,
        gdk::BUTTON_MIDDLE => MouseButton::Auxiliary,
        gdk::BUTTON_SECONDARY => MouseButton::Secondary,
        8 => MouseButton::Back,
        9 => MouseButton::Forward,
        other => MouseButton::Other(other as u16),
    }
}

/// The CSS cursor name for a Servo cursor, which is also what GTK uses.
pub fn cursor_name(cursor: Cursor) -> &'static str {
    match cursor {
        Cursor::None => "none",
        Cursor::Default => "default",
        Cursor::Pointer => "pointer",
        Cursor::ContextMenu => "context-menu",
        Cursor::Help => "help",
        Cursor::Progress => "progress",
        Cursor::Wait => "wait",
        Cursor::Cell => "cell",
        Cursor::Crosshair => "crosshair",
        Cursor::Text => "text",
        Cursor::VerticalText => "vertical-text",
        Cursor::Alias => "alias",
        Cursor::Copy => "copy",
        Cursor::Move => "move",
        Cursor::NoDrop => "no-drop",
        Cursor::NotAllowed => "not-allowed",
        Cursor::Grab => "grab",
        Cursor::Grabbing => "grabbing",
        Cursor::EResize => "e-resize",
        Cursor::NResize => "n-resize",
        Cursor::NeResize => "ne-resize",
        Cursor::NwResize => "nw-resize",
        Cursor::SResize => "s-resize",
        Cursor::SeResize => "se-resize",
        Cursor::SwResize => "sw-resize",
        Cursor::WResize => "w-resize",
        Cursor::EwResize => "ew-resize",
        Cursor::NsResize => "ns-resize",
        Cursor::NeswResize => "nesw-resize",
        Cursor::NwseResize => "nwse-resize",
        Cursor::ColResize => "col-resize",
        Cursor::RowResize => "row-resize",
        Cursor::AllScroll => "all-scroll",
        Cursor::ZoomIn => "zoom-in",
        Cursor::ZoomOut => "zoom-out",
    }
}
