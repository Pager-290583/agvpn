#!/bin/bash
# Publish a complete installer release after the Pages deployment succeeds.
set -euo pipefail
: "${GH_REPO:?}" "${GH_TOKEN:?}" "${GITHUB_SHA:?}"
TAG=$1
TITLE=$2
DIR=$3
assets=("$DIR/agvpn.flatpakref" "$DIR/agvpn.flatpakrepo" "$DIR/INSTALL.txt" "$DIR/build-info.json" "$DIR/SHA256SUMS")
for file in "${assets[@]}" "$DIR/release-notes.txt"; do
    test -s "$file"
done
(cd "$DIR" && sha256sum -c SHA256SUMS)

# A rerun of the same workflow uses the same tag and refreshes its assets.
if gh release view "$TAG" --repo "$GH_REPO" >/dev/null 2>&1; then
    gh release upload "$TAG" "${assets[@]}" --repo "$GH_REPO" --clobber
else
    gh release create "$TAG" "${assets[@]}" --repo "$GH_REPO" \
        --target "$GITHUB_SHA" --title "$TITLE" --notes-file "$DIR/release-notes.txt" --draft
fi
# A newly created release is invisible until all files have uploaded successfully.
gh release edit "$TAG" --repo "$GH_REPO" --draft=false --latest \
    --title "$TITLE" --notes-file "$DIR/release-notes.txt"
