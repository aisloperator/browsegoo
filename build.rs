//! Compiles the files in `resources`, such as icons, into the program, with
//! GLib's `glib-compile-resources`, which comes with GTK's development files.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"))
        .join("resources.gresource");
    println!("cargo::rerun-if-changed=resources");
    let status = Command::new("glib-compile-resources")
        .arg("--sourcedir=resources")
        .arg("--target")
        .arg(&output)
        .arg("resources/resources.gresource.xml")
        .status()
        .expect("glib-compile-resources should be installed, with GTK's development files");
    assert!(status.success(), "glib-compile-resources failed");
}
