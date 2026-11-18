//! BrowseGoo: a Web browser.

mod block_sites;
mod bookmarks;
mod bookmarks_editor;
mod browser;
mod closing;
mod completion;
mod decorations;
mod fetch;
mod filter;
mod filter_lists;
mod find_bar;
mod location;
mod local_network;
mod location_bar;
mod page_fonts;
mod password_manager;
mod password_prompts;
mod passwords;
mod preferences;
mod preserved;
mod privacy;
mod site_data;
mod socks_proxy;
mod user_agent;
mod user_filters;

use std::process::ExitCode;
use std::{env, fs};

use gtk::prelude::*;
use gtk::{gio, glib};
use url::Url;

const APP_ID: &str = "com.aisloperator.BrowseGoo";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "Usage: browsegoo [URL]";

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let first = arguments.next();
    if arguments.next().is_some() {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }

    let start_url = match first.as_deref() {
        None => Url::parse("about:blank").expect("about:blank is a valid URL"),
        Some("--help" | "-h") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("--version" | "-V") => {
            println!("BrowseGoo {VERSION}");
            return ExitCode::SUCCESS;
        }
        Some(text) => match location::parse_location(text) {
            Some(url) => url,
            None => {
                eprintln!("Not a URL BrowseGoo can open: {text}");
                return ExitCode::FAILURE;
            }
        },
    };

    // WebKit does TLS for pages itself, and rustls is only for downloading filter
    // lists, but it needs its crypto provider picked first.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("nothing else should have installed a TLS crypto provider yet");

    let dirs = match create_dirs() {
        Ok(dirs) => dirs,
        Err(error) => {
            eprintln!("Could not create BrowseGoo's directories: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Before GLib or GTK starts any thread, as it sets an environment variable.
    let page_fonts = page_fonts::apply(&dirs.config, &dirs.cache);
    if let Some(note) = page_fonts.note() {
        eprintln!("{note}");
    }

    // Icons in it are found by name, as the application adds its icons
    // directory, under the resource path its ID implies, to the icon theme.
    let resources = gio::Resource::from_data(&glib::Bytes::from_static(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/resources.gresource"
    ))))
    .expect("the compiled resources should be valid");
    gio::resources_register(&resources);

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        browser::open(app, dirs.clone(), start_url.clone(), page_fonts.clone())
    });

    // Our arguments were handled above, so that GTK doesn't try to open them as files.
    match app.run_with_args::<&str>(&[]) {
        glib::ExitCode::SUCCESS => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

/// Creates BrowseGoo's directories. They're private to the user, as they have
/// cookies, saved passwords for sites, and browsing history in caches.
fn create_dirs() -> std::io::Result<browser::Dirs> {
    use std::os::unix::fs::PermissionsExt;

    let dirs = browser::Dirs {
        config: glib::user_config_dir().join("browsegoo"),
        data: glib::user_data_dir().join("browsegoo"),
        cache: glib::user_cache_dir().join("browsegoo"),
    };
    for dir in [&dirs.config, &dirs.data, &dirs.cache] {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dirs)
}
