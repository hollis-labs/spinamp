# Releasing Spinamp

## One-time setup
1. **Repo:** `hollis-labs/spinamp` (public). `.gitignore` already excludes build output, local databases and `inspiration/` (third-party reference images). Licensed MIT (`LICENSE`), with the brand protected by `TRADEMARK.md`.
2. **Site:** Settings > Pages > Source: *GitHub Actions*; custom domain `spinamp.fm` (the `site/CNAME` file is there). DNS for the apex domain: four A records to GitHub Pages (`185.199.108.153`, `.109.153`, `.110.153`, `.111.153`) and `www` CNAME to `hollis-labs.github.io`. Pushes to `main` that touch `site/` redeploy it.
3. **Homebrew tap:** `hollis-labs/homebrew-tap` exists (it has a `Formula/` folder). Cask files go in a new `Casks/` folder: put `packaging/homebrew/spinamp.rb` at `Casks/spinamp.rb`. Users then run `brew tap hollis-labs/tap && brew trust hollis-labs/tap && brew install --cask spinamp` (Homebrew 7 requires trusting non-official taps).

## Cut a release
1. Bump the version in `app/src-tauri/tauri.conf.json`, `app/src-tauri/Cargo.toml` and `app/package.json` (keep it plain `x.y.z`: the Windows installer cannot take beta suffixes).
2. `git tag v0.1.0 && git push --tags`. The `release` workflow builds macOS (Apple Silicon) and Windows installers, publishes them as a **prerelease**, and adds `SHA256SUMS`.
3. `packaging/update-cask.sh 0.1.0` (Apple Silicon only), then copy the updated cask into the tap repo and push.
4. The install scripts on spinamp.fm always read the newest release (prereleases included), so nothing else to update.

## Signing and notarization (macOS)
Apple Developer Program membership is in place, so releases are signed with a *Developer ID Application*
certificate and notarized, which means a downloaded app opens with no Gatekeeper warning and the Keychain
"Always Allow" prompt sticks (stable signature).

**Locally, from a Mac that has the certificate and a `notarytool` keychain profile** (profile name `spinamp`;
create it once with `xcrun notarytool store-credentials spinamp`):

```sh
cd app
export APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)"
pnpm tauri build --bundles app,dmg          # signs the app with the hardened runtime
DMG=src-tauri/target/release/bundle/dmg/Spinamp_<version>_aarch64.dmg
codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG"
xcrun notarytool submit "$DMG" --keychain-profile spinamp --wait
xcrun stapler staple "$DMG"
spctl -a -t open --context context:primary-signature -v "$DMG"   # expect: accepted, Notarized Developer ID
```
Then upload the DMG to the release (`gh release upload vX.Y.Z "$DMG" --clobber`), regenerate `SHA256SUMS`,
and run `packaging/update-cask.sh`.

**In CI (not set up yet).** A `notarytool` keychain profile lives in one Mac's keychain and cannot be copied to
GitHub, so CI needs its own credentials. Add these repository secrets and the `release` workflow signs and
notarizes automatically (no code change):

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | the Developer ID Application cert exported as `.p12`, base64 (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH` | an App Store Connect API key (preferred), or instead |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | your Apple ID, an app-specific password, and team ID |

The cask no longer strips the quarantine flag: every published macOS DMG must be signed and notarized. A tag
pushed to CI without the secrets above publishes an UNSIGNED DMG, so sign locally (above) and replace it, or
add the secrets first.

**Windows.** Unsigned installers show a SmartScreen notice (More info > Run anyway). Removing it needs a
code-signing certificate (an OV/EV certificate from a CA, roughly $200-600/year, hardware-token based) or
Azure Trusted Signing (about $10/month, with eligibility rules). Not needed for an early beta.

## Notes
- **Intel Macs are not built.** The ScreenCaptureKit Swift bridge (`apple-metal`) needs the macOS 26 SDK, which the Intel runner images lack, so the first Intel job failed. A cross-compile of the Intel target on the Apple Silicon runner was staged but not tried; re-add the matrix entry (`os: macos-latest`, `--target x86_64-apple-darwin`) to attempt it.
- Linux is not built yet (needs the webkit/libsecret toolchain in CI and a capture story).
- Windows has not been built or run by us yet: the first CI run is the real test, expect to fix a thing or two.
- Live capture is macOS-only (ScreenCaptureKit); on Windows the app runs without it.
