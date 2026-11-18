//! Scripts that BrowseGoo adds to every page for privacy, which run in a
//! JavaScript world of their own, so that the page's scripts can't change or
//! undo what they do.

/// The JavaScript world that the scripts run in, apart from the page's.
const WORLD: &str = "browsegoo-privacy";

/// Stops "ping" hyperlink auditing, by removing links' `ping` attributes.
/// WebKitGTK's `enable-hyperlink-auditing` setting is deprecated, and no
/// longer stops it.
const NO_PING: &str = include_str!("no_ping.js");

/// Adds the scripts to a tab's pages, in every frame.
pub fn add_scripts(content: &webkit::UserContentManager) {
    content.add_script(&webkit::UserScript::for_world(
        NO_PING,
        webkit::UserContentInjectedFrames::AllFrames,
        webkit::UserScriptInjectionTime::Start,
        WORLD,
        &[],
        &[],
    ));
}
