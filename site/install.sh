#!/bin/sh
# Spinamp installer for macOS:   curl -fsSL https://spinamp.fm/install.sh | sh
#
# What it does (read it, it is short): finds the newest release of hollis-labs/spinamp on GitHub,
# downloads the .dmg for your Mac, checks it against the release's SHA256SUMS, copies Spinamp.app
# into /Applications and unmounts. Nothing else is installed or changed.
set -eu

REPO="hollis-labs/spinamp"
API="https://api.github.com/repos/$REPO/releases?per_page=1"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "this installer is for macOS. Windows: see https://spinamp.fm"
case "$(uname -m)" in
  arm64) ARCH="aarch64" ;;
  x86_64) ARCH="x64" ;;
  *) die "unsupported CPU: $(uname -m)" ;;
esac

say "Looking for the latest Spinamp release..."
JSON="$(curl -fsSL "$API")" || die "could not reach GitHub"
URLS="$(printf '%s' "$JSON" | grep -o '"browser_download_url": *"[^"]*"' | sed 's/.*: *"//; s/"$//')"
DMG_URL="$(printf '%s\n' "$URLS" | grep "_${ARCH}\.dmg$" | head -n 1 || true)"
SUMS_URL="$(printf '%s\n' "$URLS" | grep "/SHA256SUMS$" | head -n 1 || true)"
[ -n "$DMG_URL" ] || die "no macOS ($ARCH) download found in the latest release"

TMP="$(mktemp -d)"
MNT="$TMP/mnt"
cleanup() { hdiutil detach "$MNT" -quiet >/dev/null 2>&1 || true; rm -rf "$TMP"; }
trap cleanup EXIT INT TERM

DMG="$TMP/$(basename "$DMG_URL")"
say "Downloading $(basename "$DMG_URL")..."
curl -fL --progress-bar -o "$DMG" "$DMG_URL" || die "download failed"

if [ -n "$SUMS_URL" ]; then
  EXPECT="$(curl -fsSL "$SUMS_URL" | grep " $(basename "$DMG")$" | awk '{print $1}' | head -n 1 || true)"
  ACTUAL="$(shasum -a 256 "$DMG" | awk '{print $1}')"
  [ -n "$EXPECT" ] && [ "$EXPECT" = "$ACTUAL" ] || die "checksum mismatch, not installing"
  say "Checksum OK."
else
  say "Note: this release has no SHA256SUMS; skipping the checksum check."
fi

mkdir -p "$MNT"
hdiutil attach "$DMG" -nobrowse -readonly -quiet -mountpoint "$MNT" || die "could not open the disk image"
APP="$(find "$MNT" -maxdepth 1 -name '*.app' | head -n 1)"
[ -n "$APP" ] || die "no app found in the disk image"

DEST="/Applications/$(basename "$APP")"
if [ -w /Applications ]; then
  rm -rf "$DEST" && cp -R "$APP" "$DEST"
else
  say "Administrator permission is needed to write to /Applications."
  sudo rm -rf "$DEST" && sudo cp -R "$APP" "$DEST"
fi

say ""
say "Installed: $DEST"
say "Open it from Launchpad or:  open -a Spinamp"
say "Spinamp controls the Spotify desktop app, so you need Spotify Premium and the Spotify app installed."
