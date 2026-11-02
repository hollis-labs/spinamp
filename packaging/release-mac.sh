#!/bin/sh
# Build, sign and notarize the Apple Silicon DMG on the home-laptop runner (or any Mac with the Developer ID certificate and a
# `notarytool` keychain profile named "spinamp"; see RELEASING.md). Prints the DMG path on the last line.
# usage: packaging/release-mac.sh        env: NOTARY_PROFILE (default spinamp), SIGN_ID (default: first Developer ID Application)
set -eu
cd "$(dirname "$0")/../app"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)}"
[ -n "$SIGN_ID" ] || { echo "no Developer ID Application identity in the keychain" >&2; exit 1; }
VER=$(sed -n 's/.*"version": "\(.*\)",/\1/p' src-tauri/tauri.conf.json | head -1)
pnpm install --frozen-lockfile
APPLE_SIGNING_IDENTITY="$SIGN_ID" pnpm tauri build --bundles app,dmg
DMG="${CARGO_TARGET_DIR:-src-tauri/target}/release/bundle/dmg/Spinamp_${VER}_aarch64.dmg"
codesign --force --timestamp --sign "$SIGN_ID" "$DMG"
xcrun notarytool submit "$DMG" --keychain-profile "${NOTARY_PROFILE:-spinamp}" --wait
# stapling fetches the ticket from Apple CloudKit, which drops connections now and then: retry
for i in 1 2 3 4 5; do xcrun stapler staple "$DMG" && break; [ "$i" = 5 ] && exit 1; sleep $((i * 10)); done
spctl -a -t open --context context:primary-signature -v "$DMG"
echo "$DMG"
