# BrowseGoo

BrowseGoo is a barebones Web browser, assembled around the
[Servo](https://servo.org/) engine, with a [GTK4](https://www.gtk.org/) interface.

Created by AI Sloperator with Claude Code.

Web site: <https://www.aisloperator.com/>

No license is given.

## Features

- A browser window with a location field, and back, forward and reload.
- Tabs, which can be reordered by dragging. A link or script that would open a
  new window opens a new tab instead.
- HTTP and HTTPS. TLS is done by Servo, with `rustls`, and certificates are
  checked with the system's trusted roots.
- Simple bookmarks: the star button adds or removes the current page, and the
  button next to it lists them.
- Cookies that persist between runs, in Servo's own cookie store.
- The User-Agent is `BrowseGoo/` followed by the version.

| Shortcut                    | Action               |
|-----------------------------|----------------------|
| Ctrl+L                      | Go to location field |
| Ctrl+T                      | New tab              |
| Ctrl+W                      | Close tab            |
| Ctrl+Tab, Ctrl+PageDown     | Next tab             |
| Ctrl+Shift+Tab, Ctrl+PageUp | Previous tab         |
| Ctrl+D                      | Toggle bookmark      |
| Ctrl+R, F5                  | Reload               |
| Alt+Left, Alt+Right         | Back, forward        |
| Ctrl+Q                      | Quit                 |

A tab can also be closed by middle-clicking it. Closing the last tab quits.

Bookmarks are in `~/.config/browsegoo/bookmarks.tsv`, with Servo's data, such as
`cookie_jar.json`, beside them. Cookies are saved when the window is closed, so
changes made since the last clean exit are lost in a crash.

## Building

You need stable Rust from [rustup](https://rustup.rs/), a C and C++ compiler, and
the development files for GTK4. On Debian or Ubuntu, that is:

    sudo apt install build-essential pkg-config libgtk-4-dev

Then:

    cargo build --release
    ./target/release/browsegoo https://servo.org/

The first build compiles about 550 crates, which took about 18 minutes on 2 CPU
cores. It also downloads a prebuilt SpiderMonkey library, the JavaScript engine
used by Servo.

Compiling Servo takes a lot of memory. A release build used about 5 GB in total,
and worked with 8 GB of RAM. With only 4 GB, use the default profile and one job
at a time, with `cargo build -j1`, which is set up to use less. If `rustc` gets
killed while compiling `servo-script`, it ran out of memory.

At run time, Servo needs a software OpenGL driver through EGL. On Debian and
Ubuntu, that's in `libegl1` and `libgl1-mesa-dri`, which most desktops already have.

## Known limitations

This is a starting point, and these are not done yet:

- Pages display through a software renderer, with each frame copied to GTK, so
  it's slower than a GPU-accelerated browser.
- Servo's features for WebGL, WebGPU, media playback and developer tools aren't
  compiled in.
- There's only one window.
- There's no interface yet for `<select>` drop-downs, file pickers, color pickers,
  context menus, HTTP authentication, or the `alert()`, `confirm()` and `prompt()`
  dialogs.
- Keyboard input doesn't go through an input method, so composed text doesn't work.
