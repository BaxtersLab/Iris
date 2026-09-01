#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Copyright 2026 Baxters Lab
# baxters-iris — build the .deb from a release build of this tree.
#
# The guard style here is deliberate and is the estate's hard-won one: assert
# what LANDED IN THE PAYLOAD, never what the build reported. GGUF Chatbox's
# packager once produced a package with no frontend at all and exited 0,
# because `[ -e ] && cp` silently skips a missing source. Every required item
# below is asserted after copying.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"
HERE="$PWD"
PKG=baxters-iris
VERSION="$(sed -n 's/^Version: //p' packaging/DEBIAN/control)"
[ -n "$VERSION" ] || { echo "FATAL: no Version in packaging/DEBIAN/control" >&2; exit 1; }

# The package version and the application's own version must agree.
#
# They silently drifted: the .deb stayed 0.1.1 across a session that added the
# icon, the settings panel, the adaptive window and two capture fixes, so dpkg
# reported "Unpacking (0.1.1) over (0.1.1)". Installing the FILE directly still
# works, which is how it went unnoticed — but apt sees an identical version and
# skips the upgrade entirely, so a published build would simply never reach
# anyone who already had one.
CRATE_VERSION="$(sed -n '0,/^version = /s/^version = "\(.*\)"/\1/p' crates/iris-ui/Cargo.toml)"
if [ "$VERSION" != "$CRATE_VERSION" ]; then
    echo "FATAL: package version $VERSION != iris-ui crate version $CRATE_VERSION" >&2
    echo "       bump both, or a published upgrade will be skipped as already-installed" >&2
    exit 1
fi

# Honour CARGO_TARGET_DIR, as run.sh does — builds on this box use a space-free
# target directory outside the tree.
TARGET_DIR="${CARGO_TARGET_DIR:-$HERE/target}"
BIN="$TARGET_DIR/release/iris-ui"
[ -x "$BIN" ] || {
    echo "FATAL: $BIN not built. Run: cargo build --workspace --release" >&2; exit 1; }

STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
DEST="$STAGE/opt/baxters/iris"
mkdir -p "$DEST/target/release" "$STAGE/usr/share/applications"

cp -a packaging/DEBIAN "$STAGE/DEBIAN"
cp -a packaging/usr/. "$STAGE/usr/"

# Stage the theme icons FROM the generator's output rather than from a second
# committed copy of the same files.
#
# They were committed twice — packaging/icons/iris_NxN.png and
# packaging/usr/share/icons/hicolor/NxN/apps/baxters-iris.png — byte-identical,
# with nothing keeping them so. Regenerating the icon would have updated one set
# and left the other, and the package ships the stale one.
for icon in packaging/icons/iris_*x*.png; do
    [ -e "$icon" ] || { echo "FATAL: no generated icons in packaging/icons/" >&2; exit 1; }
    dim=$(basename "$icon" .png); dim=${dim#iris_}
    dest="$STAGE/usr/share/icons/hicolor/$dim/apps"
    mkdir -p "$dest"
    cp -a "$icon" "$dest/baxters-iris.png"
done
cp -a "$BIN" "$DEST/target/release/iris-ui"

# Required payload items. Asserted individually so a missing one names itself.
for item in run.sh README.md LICENSE ROADMAP.md; do
    [ -e "$item" ] || { echo "FATAL: required item missing from source: $item" >&2; exit 1; }
    cp -a "$item" "$DEST/"
done

# Normalise modes. Files inherit the build user's umask, which on this box is
# 002 — so everything arrived group-writable (0664/0775) and dpkg would install
# it that way. Debian wants 0644 for data and 0755 for programs and directories.
find "$STAGE" -type f -exec chmod 0644 {} +
find "$STAGE" -type d -exec chmod 0755 {} +
chmod 0755 "$DEST/run.sh" "$DEST/target/release/iris-ui" \
           "$STAGE/DEBIAN/postinst" "$STAGE/DEBIAN/postrm"

# --- assertions on the STAGED tree, not on the source ---------------------
[ -x "$DEST/target/release/iris-ui" ] || { echo "FATAL: binary not in payload" >&2; exit 1; }
[ -x "$DEST/run.sh" ]                 || { echo "FATAL: run.sh not executable in payload" >&2; exit 1; }

DESKTOP="$STAGE/usr/share/applications/$PKG.desktop"
[ -f "$DESKTOP" ] || { echo "FATAL: desktop entry not in payload" >&2; exit 1; }

# The Exec= target must exist in the payload. A desktop entry pointing at a
# path that was never packaged gives a launcher that silently does nothing.
EXEC_PATH=$(sed -n 's/^Exec=//p' "$DESKTOP" | awk '{print $1}')
[ -n "$EXEC_PATH" ] || { echo "FATAL: desktop entry has no Exec=" >&2; exit 1; }
[ -x "$STAGE$EXEC_PATH" ] || {
    echo "FATAL: Exec=$EXEC_PATH is not an executable in the payload" >&2; exit 1; }

# Icon= is a NAMED freedesktop icon here, not a path. If someone later points it
# at a file, that file must be packaged — the same check GGUF Chatbox needs.
ICON=$(sed -n 's/^Icon=//p' "$DESKTOP")
case "$ICON" in
    /*) [ -f "$STAGE$ICON" ] || { echo "FATAL: Icon=$ICON not in payload" >&2; exit 1; } ;;
    "") echo "FATAL: desktop entry has no Icon=" >&2; exit 1 ;;
    *)
        # A NAMED icon still has to be installed, or the launcher shows a blank
        # tile with no error anywhere. This branch used to be `:` — a check that
        # passed by not looking, which is the shape of guard this build script
        # exists to avoid.
        icon_count=$(find "$STAGE/usr/share/icons/hicolor" -name "$ICON.png" 2>/dev/null | wc -l)
        [ "$icon_count" -gt 0 ] || {
            echo "FATAL: Icon=$ICON is a theme name but no hicolor/*/apps/$ICON.png is in the payload" >&2
            exit 1; }
        echo "  icon: $icon_count size(s) of $ICON in hicolor"
        ;;
esac

[ -f "$STAGE/usr/share/doc/$PKG/copyright" ] || {
    echo "FATAL: copyright file not in payload" >&2; exit 1; }

# A group-writable file in a package is a lintian error and a real one: any
# member of the installing group could then edit an installed program.
if find "$STAGE" \( -type f -o -type d \) -perm -g+w -print -quit | grep -q .; then
    echo "FATAL: group-writable entries in the payload:" >&2
    find "$STAGE" \( -type f -o -type d \) -perm -g+w >&2
    exit 1
fi

# The desktop entry must satisfy the spec, not merely exist.
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$DESKTOP" || {
        echo "FATAL: desktop entry failed validation" >&2; exit 1; }
else
    echo "  note: desktop-file-validate not installed, entry not validated"
fi

mkdir -p "$HERE/dist"
OUT="$HERE/dist/${PKG}_${VERSION}_amd64.deb"
# --root-owner-group: without it dpkg-deb records the build user's uid/gid, so
# the package installs files owned by a uid that means nothing on the target.
dpkg-deb --root-owner-group -Zzstd --build "$STAGE" "$OUT" >/dev/null

echo "built $OUT"
dpkg-deb -I "$OUT" | sed -n '/Package:/,/^$/p'
echo "payload:"
dpkg-deb -c "$OUT" | awk '{print "  " $6, $3}' | grep -vE "/$" | head -20
