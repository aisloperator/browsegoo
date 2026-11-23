//! Following the desktop's choice of a dark or light style, when it starts
//! and whenever it changes.
//!
//! Desktops publish that choice through the XDG desktop portal, as the
//! `color-scheme` setting in the `org.freedesktop.appearance` namespace. GTK
//! before 4.20 doesn't read it itself, so it's passed on to GTK as its
//! `gtk-application-prefer-dark-theme` setting, which picks the dark or light
//! variant of the GTK theme, and which WebKit follows for pages'
//! `prefers-color-scheme`.

use gtk::gio;
use gtk::glib::Variant;
use gtk::prelude::*;

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS_INTERFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

thread_local! {
    /// The portal's proxy, kept for its `SettingChanged` signals, which stop
    /// when it's dropped.
    static PROXY: std::cell::OnceCell<gio::DBusProxy> = const { std::cell::OnceCell::new() };
}

/// The desktop's choice, as the portal gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorScheme {
    NoPreference,
    Dark,
    Light,
}

impl ColorScheme {
    /// From the portal's number for it. Unknown numbers are taken as no
    /// preference, as the portal's documentation says to.
    fn from_portal(value: u32) -> Self {
        match value {
            1 => Self::Dark,
            2 => Self::Light,
            _ => Self::NoPreference,
        }
    }

    /// From the setting's value, which `Read` wraps in a further variant.
    fn from_variant(value: &Variant) -> Option<Self> {
        let mut value = value.clone();
        while value.is::<Variant>() {
            value = value.as_variant()?;
        }
        value.get::<u32>().map(Self::from_portal)
    }

    /// Whether GTK should prefer its theme's dark variant, given what it
    /// preferred before BrowseGoo changed it, which is kept when the desktop
    /// has no preference.
    fn prefers_dark(self, otherwise: bool) -> bool {
        match self {
            Self::Dark => true,
            Self::Light => false,
            Self::NoPreference => otherwise,
        }
    }
}

/// Starts following the desktop's choice. Without a portal, or with a desktop
/// that doesn't publish the setting, GTK's own settings are left as they are.
pub fn follow_desktop() {
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    let initial = settings.is_gtk_application_prefer_dark_theme();
    gio::DBusProxy::for_bus(
        gio::BusType::Session,
        gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
        None,
        PORTAL_NAME,
        PORTAL_PATH,
        SETTINGS_INTERFACE,
        gio::Cancellable::NONE,
        move |proxy| {
            let Ok(proxy) = proxy else {
                return;
            };
            let apply = move |scheme: ColorScheme| {
                settings.set_gtk_application_prefer_dark_theme(scheme.prefers_dark(initial));
            };

            let on_change = apply.clone();
            // Arguments: the proxy, the sender, the signal's name and its
            // parameters.
            proxy.connect_local("g-signal", false, move |arguments| {
                let signal = arguments[2].get::<String>().ok()?;
                let parameters = arguments[3].get::<Variant>().ok()?;
                if signal == "SettingChanged"
                    && let Some((namespace, key, value)) =
                        parameters.get::<(String, String, Variant)>()
                    && namespace == NAMESPACE
                    && key == KEY
                    && let Some(scheme) = ColorScheme::from_variant(&value)
                {
                    on_change(scheme);
                }
                None
            });

            PROXY.with(|kept| {
                let _ = kept.set(proxy.clone());
            });
            read(&proxy, "ReadOne", move |proxy, result| match result {
                Some(scheme) => apply(scheme),
                // Portals before version 2 have only the deprecated `Read`.
                None => read(proxy, "Read", move |_, result| {
                    if let Some(scheme) = result {
                        apply(scheme);
                    }
                }),
            });
        },
    );
}

/// Reads the setting with the portal's `method`.
fn read(
    proxy: &gio::DBusProxy,
    method: &str,
    done: impl FnOnce(&gio::DBusProxy, Option<ColorScheme>) + 'static,
) {
    let proxy_for_done = proxy.clone();
    proxy.call(
        method,
        Some(&(NAMESPACE, KEY).to_variant()),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        move |result| {
            let scheme = result
                .ok()
                .and_then(|reply| reply.try_child_value(0))
                .and_then(|value| ColorScheme::from_variant(&value));
            done(&proxy_for_done, scheme);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_numbers() {
        assert_eq!(ColorScheme::from_portal(0), ColorScheme::NoPreference);
        assert_eq!(ColorScheme::from_portal(1), ColorScheme::Dark);
        assert_eq!(ColorScheme::from_portal(2), ColorScheme::Light);
        assert_eq!(ColorScheme::from_portal(7), ColorScheme::NoPreference);
    }

    #[test]
    fn values_from_read_one_and_read() {
        // `ReadOne` gives the value in one variant, `Read` in two.
        let read_one = 1u32.to_variant().to_variant();
        let read = 2u32.to_variant().to_variant().to_variant();
        assert_eq!(
            ColorScheme::from_variant(&read_one),
            Some(ColorScheme::Dark)
        );
        assert_eq!(ColorScheme::from_variant(&read), Some(ColorScheme::Light));
        assert_eq!(ColorScheme::from_variant(&"dark".to_variant()), None);
    }

    #[test]
    fn no_preference_keeps_what_gtk_preferred() {
        assert!(ColorScheme::Dark.prefers_dark(false));
        assert!(!ColorScheme::Light.prefers_dark(true));
        assert!(ColorScheme::NoPreference.prefers_dark(true));
        assert!(!ColorScheme::NoPreference.prefers_dark(false));
    }
}
