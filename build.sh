#!/bin/sh
# Builds the Flatpak and installs it for the current user.
#
#   ./build.sh            build + install
#   ./build.sh bundle     additionally create agvpn-<arch>.flatpak in this folder
#
# Build state (sources cache, build dir, local repo) lives in ~/.cache/agvpn-rs, so the
# project folder (on Yandex Disk) stays small.
set -eu
cd "$(dirname "$0")"

MANIFEST=flatpak/io.github.agvpn.AGVPN.yml
APP_ID=io.github.agvpn.AGVPN
ARCH=$(flatpak --default-arch)
CACHE="${AGVPN_BUILD_CACHE:-$HOME/.cache/agvpn-rs}"
REPO="$CACHE/repo"
mkdir -p "$CACHE"

# Keep the offline crate list in sync with Cargo.lock.
if [ ! -f flatpak/cargo-sources.json ] || [ Cargo.lock -nt flatpak/cargo-sources.json ]; then
    python3 -I flatpak/cargo-sources.py Cargo.lock flatpak/cargo-sources.json
fi

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
if ! command -v flatpak-builder >/dev/null 2>&1; then
    flatpak install --user -y flathub org.flatpak.Builder
    BUILDER="flatpak run org.flatpak.Builder"
else
    BUILDER=flatpak-builder
fi

$BUILDER --user --install-deps-from=flathub --force-clean --state-dir="$CACHE/flatpak-builder" \
    --repo="$REPO" "$CACHE/build-dir" "$MANIFEST"

# Install with the host's flatpak: apply_extra (unpacking the CLI) needs bwrap, which cannot
# start a nested sandbox inside the org.flatpak.Builder sandbox.
if flatpak remotes --user --columns=name | grep -qx agvpn-rs; then
    flatpak remote-modify --user --url="file://$REPO" agvpn-rs
else
    flatpak remote-add --user --no-gpg-verify agvpn-rs "file://$REPO"
fi
flatpak install --user -y --reinstall agvpn-rs "$APP_ID"

if [ "${1:-}" = "bundle" ]; then
    flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
        "$REPO" "agvpn-$ARCH.flatpak" "$APP_ID"
    echo "Bundle: agvpn-$ARCH.flatpak"
fi
echo "Run: flatpak run $APP_ID"
