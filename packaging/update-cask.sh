#!/bin/sh
# Usage: packaging/update-cask.sh 0.1.0
# Downloads the release's Apple Silicon disk image, computes its SHA-256 and rewrites the cask.
set -eu
V="${1:?usage: update-cask.sh <version, e.g. 0.1.0>}"
CASK="$(dirname "$0")/homebrew/spinamp.rb"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
curl -fL -o "$TMP/a.dmg" "https://github.com/hollis-labs/spinamp/releases/download/v$V/Spinamp_${V}_aarch64.dmg"
SUM="$(shasum -a 256 "$TMP/a.dmg" | awk '{print $1}')"
sed -i '' -E "s/^  version \".*\"/  version \"$V\"/; s/^  sha256 \".*\"/  sha256 \"$SUM\"/" "$CASK"
echo "Updated $CASK to $V"; grep -E '^  (version|sha256)' "$CASK"
echo "Copy it to the tap repo (Casks/spinamp.rb), commit and push."
