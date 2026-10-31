#!/bin/sh
# cargo "runner": signs the freshly built dev binary with a stable identity before running it.
#
# Why: the macOS Keychain ties "Always Allow" to the app's code signature. An unsigned dev build
# gets a new identity on every rebuild, so macOS asks for your password again after each one.
# Signed with the same certificate every time, the identity is stable and "Always Allow" sticks.
#
# Identity: $SPINAMP_SIGN_IDENTITY if set (any code-signing identity in your keychain), else
# "Spinamp Dev" (created by setup-dev-signing.sh). With neither, this just runs the binary.
BIN="$1"
IDENT="${SPINAMP_SIGN_IDENTITY:-Spinamp Dev}"
if security find-identity -v -p codesigning 2>/dev/null | grep -q "$IDENT"; then
  codesign --force --sign "$IDENT" --identifier fm.spinamp.player "$BIN" >/dev/null 2>&1
fi
exec "$@"
