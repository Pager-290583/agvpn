#!/bin/sh
# Runs cargo inside the GNOME SDK (no Rust toolchain or -devel packages needed on the host).
#   ./dev.sh build | test | run | clippy | clean ...
# Build artifacts go to ~/.cache/agvpn-rs, so the project folder (on Yandex Disk) stays small.
set -eu
cd "$(dirname "$0")"
CACHE="${AGVPN_BUILD_CACHE:-$HOME/.cache/agvpn-rs}"
mkdir -p "$CACHE"
exec flatpak run --share=network --share=ipc --socket=wayland --socket=fallback-x11 --device=dri \
    --talk-name=org.freedesktop.Flatpak --talk-name=org.kde.StatusNotifierWatcher --talk-name=org.freedesktop.Notifications --own-name=io.github.agvpn.AGVPN \
    --filesystem="$PWD" --filesystem="$CACHE" \
    --env=CARGO_HOME="$CACHE/cargo-home" --env=CARGO_TARGET_DIR="$CACHE/target" \
    --command=sh org.gnome.Sdk//50 \
    -c '. /usr/lib/sdk/rust-stable/enable.sh; exec cargo "$@"' cargo "$@"
