//! Limits the fonts that pages can use to a fixed set, so that pages can't
//! tell which fonts are installed, which would help to tell one user from
//! another. Fonts that pages provide themselves still work.
//!
//! WebKit's web processes find fonts with fontconfig, and are given a
//! fontconfig file of BrowseGoo's own, through `FONTCONFIG_FILE`, which they
//! inherit. It only lets them see the fonts in the packages that BrowseGoo's
//! `.deb` depends on, with the same rendering settings for everyone, so that
//! BrowseGoo users on the same Debian release look the same to pages.
//!
//! BrowseGoo's own window keeps the desktop's fonts, as fontconfig is started
//! in this process before `FONTCONFIG_FILE` is set.

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::bookmarks::write_atomically;

/// The fonts that pages can use, as fontconfig globs of their files, and the
/// Debian packages they're in.
const ALLOWED: [(&str, &str); 5] = [
    (
        "/usr/share/fonts/truetype/dejavu/DejaVu*.ttf",
        "fonts-dejavu-core, -mono and -extra",
    ),
    (
        "/usr/share/fonts/truetype/liberation/LiberationSans-*.ttf",
        "fonts-liberation",
    ),
    (
        "/usr/share/fonts/truetype/liberation/LiberationSerif-*.ttf",
        "fonts-liberation",
    ),
    (
        "/usr/share/fonts/truetype/liberation/LiberationMono-*.ttf",
        "fonts-liberation",
    ),
    (
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "fonts-noto-color-emoji",
    ),
];

/// A file from each package, to check that it's installed. Without DejaVu
/// Sans, pages would have no font for most text, so fonts aren't limited.
const REQUIRED: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
const EXPECTED: [(&str, &str); 5] = [
    (
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
        "fonts-dejavu-extra",
    ),
    (
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "fonts-dejavu-mono",
    ),
    (
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "fonts-liberation",
    ),
    (
        "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
        "fonts-liberation",
    ),
    (
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "fonts-noto-color-emoji",
    ),
];

/// Fonts that pages often ask for, and the allowed ones with the same widths
/// of text, so that pages lay out as they're meant to.
const SAME_WIDTHS: [(&str, &str); 9] = [
    ("Arial", "Liberation Sans"),
    ("Helvetica", "Liberation Sans"),
    ("Arimo", "Liberation Sans"),
    ("Times New Roman", "Liberation Serif"),
    ("Times", "Liberation Serif"),
    ("Tinos", "Liberation Serif"),
    ("Courier New", "Liberation Mono"),
    ("Courier", "Liberation Mono"),
    ("Cousine", "Liberation Mono"),
];

const EMOJI: &str = "Noto Color Emoji";

/// Whether fonts are limited for pages.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Limited {
        /// Packages whose fonts are missing, which pages can't use.
        missing: Vec<&'static str>,
    },
    /// The user chose to let pages use all of the installed fonts.
    NotLimited,
    /// They would be, but couldn't be, for this reason.
    Unavailable(String),
}

impl State {
    /// What to say about it in the preferences, if anything.
    pub fn note(&self) -> Option<String> {
        match self {
            State::Limited { missing } if missing.is_empty() => None,
            State::Limited { missing } => Some(format!(
                "Some of the fonts aren't installed, so pages could look different \
                 to other BrowseGoo users': {}.",
                missing.join(", ")
            )),
            State::NotLimited => None,
            State::Unavailable(reason) => Some(format!("Fonts aren't limited now: {reason}")),
        }
    }
}

/// Where the user's choice is saved: `all` if pages can use all fonts.
fn choice_path(config_dir: &Path) -> PathBuf {
    config_dir.join("page_fonts.txt")
}

/// Whether the user wants fonts limited, which they are unless chosen not to be.
pub fn is_chosen(config_dir: &Path) -> bool {
    fs::read_to_string(choice_path(config_dir)).map_or(true, |text| text.trim() != "all")
}

/// Saves whether to limit fonts, from when BrowseGoo next starts.
pub fn choose(config_dir: &Path, limited: bool) -> io::Result<()> {
    let text = if limited { "limited\n" } else { "all\n" };
    write_atomically(&choice_path(config_dir), text)
}

/// The folder with BrowseGoo's fontconfig file for pages, which WebKit's
/// sandbox must let them read.
pub fn config_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("page-fonts")
}

#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcInit() -> std::ffi::c_int;
}

/// Limits the fonts that WebKit's web processes can use, unless the user chose
/// not to. This must be done before any other thread starts, as it changes the
/// environment, and before WebKit starts any web process.
pub fn apply(config_dir_path: &Path, cache_dir: &Path) -> State {
    if !is_chosen(config_dir_path) {
        return State::NotLimited;
    }
    if !Path::new(REQUIRED).exists() {
        return State::Unavailable("DejaVu Sans, from fonts-dejavu-core, isn't installed.".into());
    }
    let directory = config_dir(cache_dir);
    let file = directory.join("fonts.conf");
    let written = fs::create_dir_all(&directory).and_then(|()| write_atomically(&file, &config()));
    if let Err(error) = written {
        return State::Unavailable(format!(
            "the font settings for pages couldn't be saved: {error}"
        ));
    }
    // This process's fontconfig reads its configuration now, as usual, so
    // that BrowseGoo's own window keeps the desktop's fonts.
    // SAFETY: FcInit is safe to call at any time.
    if unsafe { FcInit() } == 0 {
        return State::Unavailable("fontconfig couldn't be started.".into());
    }
    // SAFETY: no other thread has started yet to read the environment, as
    // `main` calls this before GLib or GTK starts.
    unsafe { std::env::set_var("FONTCONFIG_FILE", &file) };

    let mut missing: Vec<&'static str> = EXPECTED
        .iter()
        .filter(|(path, _)| !Path::new(path).exists())
        .map(|(_, package)| *package)
        .collect();
    missing.dedup();
    State::Limited { missing }
}

/// BrowseGoo's fontconfig file for pages. It doesn't include the system's or
/// the user's fontconfig settings, which would differ from one computer to
/// another.
fn config() -> String {
    let mut text = String::from(
        "<?xml version=\"1.0\"?>\n\
         <!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n\
         <!-- Written by BrowseGoo, for the fonts that pages can use. -->\n\
         <fontconfig>\n",
    );
    let mut directories: Vec<&str> = ALLOWED
        .iter()
        .filter_map(|(glob, _)| glob.rsplit_once('/').map(|(directory, _)| directory))
        .collect();
    directories.dedup();
    for directory in directories {
        text.push_str(&format!("  <dir>{directory}</dir>\n"));
    }
    text.push_str(
        "  <cachedir>/var/cache/fontconfig</cachedir>\n\
         \x20 <cachedir prefix=\"xdg\">fontconfig</cachedir>\n\
         \x20 <!-- Only the allowed font files. A glob's leading star doesn't\n\
         \x20      match a slash, so rejecting everything starts with one. -->\n\
         \x20 <selectfont>\n\
         \x20   <rejectfont><glob>/*</glob></rejectfont>\n\
         \x20   <acceptfont>\n",
    );
    for (glob, _) in ALLOWED {
        text.push_str(&format!("      <glob>{glob}</glob>\n"));
    }
    text.push_str("    </acceptfont>\n  </selectfont>\n");

    for (generic, font) in [
        ("sans-serif", "DejaVu Sans"),
        ("serif", "DejaVu Serif"),
        ("monospace", "DejaVu Sans Mono"),
        ("system-ui", "DejaVu Sans"),
        ("emoji", EMOJI),
    ] {
        text.push_str(&format!(
            "  <alias><family>{generic}</family><prefer><family>{font}</family>\
             <family>{EMOJI}</family></prefer></alias>\n"
        ));
    }
    for (asked, font) in SAME_WIDTHS {
        text.push_str(&format!(
            "  <alias binding=\"same\"><family>{asked}</family>\
             <accept><family>{font}</family></accept></alias>\n"
        ));
    }
    text.push_str(
        "  <!-- Any other font falls back to sans-serif. -->\n\
         \x20 <match target=\"pattern\">\n\
         \x20   <test qual=\"all\" name=\"family\" compare=\"not_eq\"><string>sans-serif</string></test>\n\
         \x20   <test qual=\"all\" name=\"family\" compare=\"not_eq\"><string>serif</string></test>\n\
         \x20   <test qual=\"all\" name=\"family\" compare=\"not_eq\"><string>monospace</string></test>\n\
         \x20   <edit name=\"family\" mode=\"append_last\"><string>sans-serif</string></edit>\n\
         \x20 </match>\n\
         \x20 <!-- The same rendering for everyone. -->\n\
         \x20 <match target=\"font\">\n\
         \x20   <edit name=\"antialias\" mode=\"assign\"><bool>true</bool></edit>\n\
         \x20   <edit name=\"hinting\" mode=\"assign\"><bool>true</bool></edit>\n\
         \x20   <edit name=\"hintstyle\" mode=\"assign\"><const>hintslight</const></edit>\n\
         \x20   <edit name=\"autohint\" mode=\"assign\"><bool>false</bool></edit>\n\
         \x20   <edit name=\"rgba\" mode=\"assign\"><const>none</const></edit>\n\
         \x20   <edit name=\"lcdfilter\" mode=\"assign\"><const>lcddefault</const></edit>\n\
         \x20   <edit name=\"embeddedbitmap\" mode=\"assign\"><bool>false</bool></edit>\n\
         \x20 </match>\n\
         </fontconfig>\n",
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_persists_and_defaults_to_limited() {
        let directory =
            std::env::temp_dir().join(format!("browsegoo-test-{}-page-fonts", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        assert!(is_chosen(&directory));
        choose(&directory, false).unwrap();
        assert!(!is_chosen(&directory));
        choose(&directory, true).unwrap();
        assert!(is_chosen(&directory));
    }

    #[test]
    fn config_lists_only_the_allowed_fonts() {
        let config = config();
        assert!(config.contains("<rejectfont><glob>/*</glob></rejectfont>"));
        for (glob, _) in ALLOWED {
            assert!(config.contains(&format!("<glob>{glob}</glob>")));
        }
        assert_eq!(config.matches("<dir>").count(), 3);
        // Neither the system's nor the user's settings.
        assert!(!config.contains("<include"));
        assert!(config.contains("<family>Arial</family><accept><family>Liberation Sans"));
    }
}
