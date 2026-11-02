#!/bin/sh
# Tauri `bundle.windows.signCommand` hook for cross-builds on build-1: signs one file with Azure Artifact Signing via hl-sign.
# Tauri calls it for app.exe, the NSIS uninstaller and the installer (and for NSIS's own plugin DLLs, which we skip).
set -eu
f="$1"
case "$f" in */nsis/*/Plugins/*) exit 0 ;; esac
exec "$HOME/.local/bin/hl-sign" win spinamp "$f"
