#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Iris — Linux launcher.
#
# Iris renders through winit/eframe on native Wayland (EGL + OpenGL), which the
# 2026-08-01 Ubuntu 26.04 intake proved works on this stack. So unlike the GTK
# apps in the suite this launcher does NOT force GDK_BACKEND=x11 — there is no
# GTK client-side-decoration hit-offset to work around here, and XWayland would
# only add a translation layer to a path already proven native.
#
# What it DOES exist for is contamination. A terminal inside the VS Code snap
# exports variables that follow every child process into the snap's own runtime:
#
#   LOCPATH                 snap locales built against a different glibc
#   GTK_PATH / GTK_EXE_PREFIX / GTK_IM_MODULE_FILE
#   GIO_MODULE_DIR, GDK_PIXBUF_MODULEDIR, GDK_PIXBUF_MODULE_FILE
#   XDG_DATA_HOME, XDG_DATA_DIRS, SNAP, SNAP_*
#
# Measured from a VS Code terminal on this box on 2026-08-31: EIGHTEEN variables
# resolve into /snap/. LOCPATH is the fatal one — a host binary that picks up
# /snap/core20's glibc dies with
#
#   symbol lookup error: .../libpthread.so.0: undefined symbol:
#   __libc_pthread_init, version GLIBC_PRIVATE
#
# and the rest fail quietly, which is worse. Unsetting a hand-picked list is
# whack-a-mole because the set changes with the snap, so scrub by VALUE.
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"

# ELECTRON_RUN_AS_NODE leaks out of the VS Code Electron host and forces any
# Electron child into bare-Node mode. Iris is not Electron, but it is inherited
# by anything Iris spawns, so it goes.
unset ELECTRON_RUN_AS_NODE

# PATH and XDG_DATA_DIRS are colon lists that legitimately hold non-snap entries
# too, so FILTER their components rather than dropping the variable whole —
# dropping XDG_DATA_DIRS outright loses /usr/share and friends.
_strip_snap_list() {
    local IFS=':' out=() part
    for part in $1; do
        [[ "$part" == */snap/* ]] || out+=("$part")
    done
    local joined; printf -v joined '%s:' "${out[@]}"
    printf '%s' "${joined%:}"
}
[[ "${PATH:-}" == */snap/* ]] && PATH="$(_strip_snap_list "$PATH")" && export PATH
[[ "${XDG_DATA_DIRS:-}" == */snap/* ]] \
    && XDG_DATA_DIRS="$(_strip_snap_list "$XDG_DATA_DIRS")" && export XDG_DATA_DIRS

# Everything else resolving into the snap is a module path, a locale path or a
# data root — none of which a host binary should read. Drop those whole.
while IFS='=' read -r _name _value; do
    case "$_name" in
        PATH|XDG_DATA_DIRS) continue ;;   # filtered above
        *) [[ "$_value" == */snap/* ]] && unset "$_name" ;;
    esac
done < <(env)

# Find the binary. CARGO_TARGET_DIR is consulted FIRST, but only counts if it
# actually holds an iris-ui; otherwise we fall back to a build beside this
# script, which is the installed layout (/opt/baxters/iris/target/release).
#
# Both halves of that rule were learned the hard way on 2026-08-31.
#
#   Checking CARGO_TARGET_DIR as a *single* directory with no fallback broke the
#   INSTALLED package: a developer shell on this box exports it, so the packaged
#   app reported "no binary found under /nonexistent" while its own binary sat
#   right next to the launcher.
#
#   Checking beside-the-script FIRST was worse, and quieter: this tree also has
#   an old ./target/debug/iris-ui from 2026-08-18, so run.sh launched a binary
#   thirteen days stale and every fix made since appeared not to work. That is
#   the estate's "stale builds lie" trap, and it cost a debugging round here.
#
# Hence: explicit build directory wins when it has something to offer, the
# script's own directory otherwise, and a warning when the two disagree.
APP=""
CANDIDATES=()
[[ -n "${CARGO_TARGET_DIR:-}" ]] && CANDIDATES+=("$CARGO_TARGET_DIR/release/iris-ui" "$CARGO_TARGET_DIR/debug/iris-ui")
CANDIDATES+=("$PWD/target/release/iris-ui" "$PWD/target/debug/iris-ui")
for candidate in "${CANDIDATES[@]}"; do
    if [[ -x "$candidate" ]]; then APP="$candidate"; break; fi
done
if [[ -z "$APP" ]]; then
    echo "[iris] no iris-ui binary found. Looked in:"
    printf '[iris]   %s\n' "${CANDIDATES[@]}"
    echo "[iris] build first:  cargo build --workspace --release"
    exit 1
fi

# A binary was chosen, but another candidate may be NEWER — the case where a
# stale build silently masquerades as the current one. Say so rather than let
# it mislead.
for candidate in "${CANDIDATES[@]}"; do
    [[ "$candidate" == "$APP" ]] && continue
    [[ -x "$candidate" ]] || continue
    if [[ "$candidate" -nt "$APP" ]]; then
        echo "[iris] WARNING: $candidate is NEWER than the one being launched."
        echo "[iris]          launching $APP — rebuild or remove the other to be sure."
    fi
done

# Report which iris.toml will actually be used. The search order in
# IrisConfig::config_search_paths is: the directory holding the executable
# first (so an existing side-by-side config keeps winning), then
# $XDG_CONFIG_HOME/iris/iris.toml, then ~/.config/iris/iris.toml.
#
# Saying only "no config beside the binary" was actively misleading once the
# XDG lookup landed: the app would load ~/.config/iris/iris.toml and the
# launcher would announce built-in defaults.
_xdg="${XDG_CONFIG_HOME:-$HOME/.config}"
[[ "$_xdg" = /* ]] || _xdg="$HOME/.config"   # XDG spec: ignore a relative value
CFG=""
for candidate in "$(dirname "$APP")/iris.toml" "$_xdg/iris/iris.toml"; do
    if [[ -f "$candidate" ]]; then CFG="$candidate"; break; fi
done
if [[ -n "$CFG" ]]; then
    echo "[iris] config: $CFG"
else
    echo "[iris] no iris.toml found — using built-in defaults (3840x2160 @30)"
    echo "[iris] searched: $(dirname "$APP")/iris.toml, $_xdg/iris/iris.toml"
fi

echo "[iris] launching $APP"
exec "$APP" "$@"
