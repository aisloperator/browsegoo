#!/bin/sh
# Builds BrowseGoo's .deb for Debian 13 (amd64), from a release build, into
# target/debian/. Needs dpkg-dev, for dpkg-shlibdeps.
set -eu
cd "$(dirname "$0")/.."

# The fonts that pages are limited to (see src/page_fonts.rs), which must be
# installed for every BrowseGoo user to look the same to pages.
FONTS="fonts-dejavu-core, fonts-dejavu-mono, fonts-dejavu-extra, fonts-liberation, fonts-noto-color-emoji"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
arch=$(dpkg --print-architecture)
name="browsegoo_${version}-1_${arch}"
work=target/debian/work
root="$work/$name"

cargo build --release
rm -rf "$work"
install -d -m 755 "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/applications" \
    "$root/usr/share/doc/browsegoo" "$work/debian"
install -m 755 -s target/release/browsegoo "$root/usr/bin/browsegoo"
install -m 644 packaging/com.aisloperator.BrowseGoo.desktop "$root/usr/share/applications/"
install -m 644 README.md "$root/usr/share/doc/browsegoo/"

# dpkg-shlibdeps wants a debian/control to be there.
printf 'Source: browsegoo\n\nPackage: browsegoo\nArchitecture: %s\n' "$arch" > "$work/debian/control"
libraries=$(cd "$work" && dpkg-shlibdeps -O "$name/usr/bin/browsegoo" 2>/dev/null |
    sed 's/^shlibs:Depends=//')

cat > "$root/DEBIAN/control" <<CONTROL
Package: browsegoo
Version: ${version}-1
Section: web
Priority: optional
Architecture: ${arch}
Maintainer: AI Sloperator <noreply@aisloperator.com>
Homepage: https://www.aisloperator.com/
Depends: ${libraries}, ${FONTS}
Description: Barebones Web browser built around WebKit, using GTK4
 BrowseGoo is a barebones Web browser, assembled around the WebKit engine,
 with a GTK4 interface. Created by AI Sloperator with Claude Code.
 Portions Copyright. No license is given.
CONTROL

dpkg-deb --root-owner-group --build "$root" "target/debian/$name.deb"
echo "target/debian/$name.deb"
