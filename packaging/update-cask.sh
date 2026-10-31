#!/bin/sh
# Usage: packaging/update-cask.sh 0.1.0
# Downloads the release's two macOS disk images, computes their SHA-256 and rewrites the cask.
set -eu
V="${1:?usage: update-cask.sh <version, e.g. 0.1.0>}"
CASK="$(dirname "$0")/homebrew/spinamp.rb"
BASE="https://github.com/hollis-labs/spinamp/releases/download/v$V"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
for a in aarch64 x64; do curl -fL -o "$TMP/$a.dmg" "$BASE/Spinamp_${V}_$a.dmg"; done
ARM="$(shasum -a 256 "$TMP/aarch64.dmg" | awk '{print $1}')"
X64="$(shasum -a 256 "$TMP/x64.dmg" | awk '{print $1}')"
sed -i '' -E "s/^  version \".*\"/  version \"$V\"/; s/arm:   \"[^\"]*\"/arm:   \"$ARM\"/; s/intel: \"[^\"]*\"/intel: \"$X64\"/" "$CASK"
echo "Updated $CASK to $V"; grep -E 'version|arm:|intel:' "$CASK" | head -4
echo "Copy it to the tap repo (Casks/spinamp.rb), commit and push."
