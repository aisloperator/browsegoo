//! BrowseGoo: a barebones Web browser built around the Servo engine, using GTK4.

mod bookmarks;
mod bookmarks_editor;
mod browser;
mod completion;
mod fetch;
mod filter;
mod filter_lists;
mod input;
mod location;
mod location_bar;
mod preferences;
mod web_area;

use std::path::PathBuf;
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

    // Servo does TLS itself, using rustls. It only needs us to pick the crypto provider.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("nothing else should have installed a TLS crypto provider yet");

    let config_dir = match create_config_dir() {
        Ok(config_dir) => config_dir,
        Err(error) => {
            eprintln!("Could not create the BrowseGoo configuration directory: {error}");
            return ExitCode::FAILURE;
        }
    };

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| browser::open(app, config_dir.clone(), start_url.clone()));

    // Our arguments were handled above, so that GTK doesn't try to open them as files.
    match app.run_with_args::<&str>(&[]) {
        glib::ExitCode::SUCCESS => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

/// Creates the directory for bookmarks, and for Servo's cookies. Those and the
/// HTTP authentication that Servo also saves there are private to the user.
fn create_config_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let config_dir = glib::user_config_dir().join("browsegoo");
    fs::create_dir_all(&config_dir)?;
    fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o700))?;
    Ok(config_dir)
}
