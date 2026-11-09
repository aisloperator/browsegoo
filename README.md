# BrowseGoo

BrowseGoo is a barebones Web browser, assembled around the
[WebKit](https://webkitgtk.org/) engine, with a [GTK4](https://www.gtk.org/) interface.

Created by AI Sloperator with Claude Code.

Web site: <https://www.aisloperator.com/>

No license is given.

## Features

- A browser window with a location field, and back, forward and reload.
- Completion in the location field from the bookmarks, like Firefox's. As an
  address is typed, the rest of a bookmarked one is filled in, which Tab accepts,
  and matching bookmarks are listed below the field, to choose with the arrow
  keys and Enter, or a click.
- Tabs, which can be reordered by dragging. A link or script that would open a
  new window opens a new tab instead. Middle-clicking or Ctrl+clicking a link
  opens it in a new tab in the background, or with Shift held, switches to it.
  Tabs opened from a page go after its tab, in the order they were opened.
- HTTP and HTTPS. TLS is done by WebKit, with GnuTLS through GLib, and
  certificates are checked with the system's trusted roots.
- Simple bookmarks: the star button adds or removes the current page, and the
  button next to it lists them. They can also be added, renamed, changed,
  reordered and deleted in the bookmarks editor, from the main menu (the button
  with three lines), which also has the preferences and About.
- Filtering of ad and tracking requests, with lists like EasyList, in the
  Adblock Plus format that uBlock Origin also uses. BrowseGoo converts the lists
  to WebKit's content blocker rules, which WebKit compiles and applies. The
  shield button's menu turns filtering off or on for the current tab, from its
  next page load, and opens the preferences, where lists are enabled and
  downloaded.
- Whole pages are blocked too, as with uBlock Origin's strict blocking, by rules
  that are just a host name, such as `||ads.example.com^`, and by rules with the
  `$document` or `$all` option. Other rules only block what pages load. A
  blocked page is replaced by one that says so.
- Your own filter rules, in the same format, used along with the lists. They're
  edited in the Your Filter Rules window, from the main menu or the shield
  button's menu, and its Apply button saves them and starts using them, from
  each tab's next page load. The window says how many rules are used, and which
  lines aren't applied, such as element hiding rules.
- Cookies, and what sites store, such as with `localStorage`, persist between
  runs.
- A Cookies and Site Data window, from the main menu, which lists each cookie,
  and each kind of other data that a site has stored, such as local storage or
  IndexedDB. A field above the list filters it by site, or by name, and
  checkboxes for each type of data filter it by type. Selecting one
  item shows more about it below, such as a cookie's whole value and when it
  expires, and the selected items can be deleted. Cookies are listed again as
  they change, and other data whenever the window is switched to, or with its
  Refresh button, as WebKit doesn't say when that changes. WebKit only deletes
  a kind of data other than cookies for a whole site at once, such as all of a
  site's local storage, so that's one item.
- Page content runs in separate, sandboxed processes. See
  [Process isolation](#process-isolation).
- The User-Agent is `BrowseGoo/` followed by the version.

| Shortcut                    | Action               |
|-----------------------------|----------------------|
| Ctrl+L                      | Go to location field |
| Ctrl+T                      | New tab              |
| Ctrl+W                      | Close tab            |
| Ctrl+Tab, Ctrl+PageDown     | Next tab             |
| Ctrl+Shift+Tab, Ctrl+PageUp | Previous tab         |
| Ctrl+D                      | Toggle bookmark      |
| Ctrl+Shift+O                | Edit bookmarks       |
| Ctrl+R, F5                  | Reload               |
| Alt+Left, Alt+Right         | Back, forward        |
| Ctrl+Q                      | Quit                 |

A tab can also be closed by middle-clicking it. Closing the last tab quits.

The Bookmarks, Preferences and About windows close with Ctrl+W or Escape, as
well as with their close buttons. Closing the bookmarks editor keeps a change
being typed, as leaving its field would, unless it isn't valid.

Bookmarks are in `~/.config/browsegoo/bookmarks.tsv`. Cookies are in
`~/.local/share/browsegoo/cookies.sqlite`, with what sites store beside them, and
WebKit's cache is in `~/.cache/browsegoo/web/`.

Which filter lists are enabled is in `~/.config/browsegoo/filter_lists.tsv`, and
the downloaded lists are in `~/.cache/browsegoo/`, with WebKit's compiled
versions in `compiled/` there. Lists are only downloaded with the Update Lists
button in the preferences, so filtering does nothing until then. Compiling the
four lists offered took about 15 seconds, and is only done again when a list
changes. The compiled lists are large: about 90 MB for all four.

Your own rules are in `~/.config/browsegoo/user_filters.txt`. WebKit applies each
list on its own, and an exception in one doesn't stop another from blocking, so
your exceptions (rules starting with `@@`) are also added to each enabled list.
Changing them compiles the lists again, which takes a while, but changing your
other rules doesn't. Your blocking rules block even what a list makes an
exception for.

## Building

You need stable Rust from [rustup](https://rustup.rs/), a C compiler, and the
development files for GTK4 and WebKitGTK 6.0, version 2.42 or later. On Debian 13 or Ubuntu 24.04 and
later, that is:

    sudo apt install build-essential pkg-config libgtk-4-dev libwebkitgtk-6.0-dev

The build also uses GLib's `glib-compile-resources`, which comes with those, to
build BrowseGoo's icons in `resources` into the program.

Then:

    cargo build --release
    ./target/release/browsegoo https://www.aisloperator.com/

The first release build took about 2 minutes on 2 CPU cores. WebKit itself is
the system's shared library, so it isn't compiled.

## Process isolation

BrowseGoo's own code is Rust, but WebKit, GTK and the libraries they use are C
and C++, where a memory error can corrupt or take over the process it happens
in. WebKitGTK already splits that work into separate processes:

- Each page's HTML, CSS, JavaScript, images and media are handled in a web
  process, `WebKitWebProcess`. These run in a
  [bubblewrap](https://github.com/containers/bubblewrap) sandbox, which WebKitGTK
  6.0 always uses: they see only a limited, mostly read-only view of the file
  system, and reach D-Bus only through a filtering proxy.
- Networking, including TLS and the cookie store, is in the network process,
  `WebKitNetworkProcess`.

So a memory error triggered by a page can only crash or take over its sandboxed
web process, not the browser. When a web process crashes, BrowseGoo shows "This
page crashed" in the tabs it served, and Reload loads the page again.

What stays in the browser's process is GTK, the side of WebKit that drives those
processes and displays their output, and BrowseGoo's own code. Downloaded filter
lists are parsed and converted there, in Rust, and WebKit then compiles the
converted rules there too, in C++, but only rules that BrowseGoo wrote, with
their patterns escaped. Isolating any more of it would mean moving GTK's own
drawing and input out of the process that has the window, which isn't
practical.

## Known limitations

This is a starting point, and these are not done yet:

- There's only one window.
- There's no interface for downloads, or for permission requests such as for
  location or notifications, which are denied.
- The shield button can't say how many requests were blocked, because WebKit
  doesn't tell the browser about requests that its content blockers block.
- Filtering only blocks requests by URL. It doesn't hide page elements, so blocked
  ads can leave empty space. Rules it can't apply correctly are skipped, such as
  regular expressions, redirects and scriptlets, and the preferences say how many.
