# BrowseGoo

## Preparing a release

- Update the Chrome and Firefox User-Agent strings, `CHROME` and `FIREFOX` in
  `src/user_agent.rs`, to those of the currently most popular variants of the
  latest stable Chrome and Firefox on 64-bit Windows 11 on x86. Look up the
  current stable versions rather than relying on memory. Also update the
  versions that the tests in that file and the release notes mention, if they
  changed.
