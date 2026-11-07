# BrowseGoo

BrowseGoo is a barebones Web browser, assembled around the
[Servo](https://servo.org/) engine, with a [GTK4](https://www.gtk.org/) interface.

Created by AI Sloperator with Claude Code.  
Web site: <https://www.aisloperator.com/>

No license is given.

## Features

- A browser window with a location field, and back, forward and reload.
- HTTP and HTTPS. TLS is done by Servo, with `rustls`, and certificates are
  checked with the system's trusted roots.
- Simple bookmarks: the star button adds or removes the current page, and the
  button next to it lists them.
- Cookies that persist between runs, in Servo's own cookie store.
- The User-Agent is `BrowseGoo/` followed by the version.

| Shortcut               | Action                |
|------------------------|-----------------------|
| Ctrl+L                 | Go to location field  |
| Ctrl+D                 | Toggle bookmark       |
| Ctrl+R, F5             | Reload                |
| Alt+Left, Alt+Right    | Back, forward         |
| Ctrl+Q                 | Quit                  |

Bookmarks are in `~/.config/browsegoo/bookmarks.tsv`, with Servo's data, such as
`cookie_jar.json`, beside them. Cookies are saved when the window is closed, so
changes made since the last clean exit are lost in a crash.

## Building

You need stable Rust from [rustup](https://rustup.rs/), a C and C++ compiler, and
the development files for GTK4. On Debian or Ubuntu, that is:

    sudo apt install build-essential pkg-config libgtk-4-dev

Then:

    cargo run -- https://servo.org/

The first build compiles about 550 crates, and takes a while. It also downloads
a prebuilt SpiderMonkey library, the JavaScript engine used by Servo.

Compiling Servo takes a lot of memory. The default profile in `Cargo.toml` is set
up to be practical, and has been built on 2 CPU cores and 4 GB of RAM, with no
swap, by using `cargo build -j1` for the last part of the build. If `rustc` gets
killed while compiling `servo-script`, that's why. A `--release` build should
run faster, but needs considerably more memory, and hasn't been tried yet.

At run time, Servo needs a software OpenGL driver through EGL. On Debian and
Ubuntu, that's in `libegl1` and `libgl1-mesa-dri`, which most desktops already have.

## Known limitations

This is a starting point, and these are not done yet:

- Pages display through a software renderer, with each frame copied to GTK, so
  it's slower than a GPU-accelerated browser.
- Servo's features for WebGL, WebGPU, media playback and developer tools aren't
  compiled in.
- There's one page per window, and only one window. A link that would open a new
  window opens in the current one, replacing its back and forward history.
- There's no interface yet for `<select>` drop-downs, file pickers, color pickers,
  context menus, HTTP authentication, or the `alert()`, `confirm()` and `prompt()`
  dialogs.
- Keyboard input doesn't go through an input method, so composed text doesn't work.
