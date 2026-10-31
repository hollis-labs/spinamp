# Releasing Spinamp

## One-time setup
1. **Repo:** `hollis-labs/spinamp` (public). `.gitignore` already excludes build output, local databases and `inspiration/` (third-party reference images). Licensed MIT (`LICENSE`), with the brand protected by `TRADEMARK.md`.
2. **Site:** Settings > Pages > Source: *GitHub Actions*; custom domain `spinamp.fm` (the `site/CNAME` file is there). DNS for the apex domain: four A records to GitHub Pages (`185.199.108.153`, `.109.153`, `.110.153`, `.111.153`) and `www` CNAME to `hollis-labs.github.io`. Pushes to `main` that touch `site/` redeploy it.
3. **Homebrew tap:** `hollis-labs/homebrew-tap` exists (it has a `Formula/` folder). Cask files go in a new `Casks/` folder: put `packaging/homebrew/spinamp.rb` at `Casks/spinamp.rb`. Users then run `brew install --cask hollis-labs/tap/spinamp`.

## Cut a release
1. Bump the version in `app/src-tauri/tauri.conf.json`, `app/src-tauri/Cargo.toml` and `app/package.json` (keep it plain `x.y.z`: the Windows installer cannot take beta suffixes).
2. `git tag v0.1.0 && git push --tags`. The `release` workflow builds macOS (Apple Silicon + Intel) and Windows installers, publishes them as a **prerelease**, and adds `SHA256SUMS`.
3. `packaging/update-cask.sh 0.1.0`, then copy the updated cask into the tap repo and push.
4. The install scripts on spinamp.fm always read the newest release (prereleases included), so nothing else to update.

## Signing: what it takes
**macOS (recommended before a wider audience).** Apple Developer Program, **$99/year**. It gives you a *Developer ID Application* certificate and notarization, so a downloaded app opens with no Gatekeeper warning, and the Keychain "Always Allow" prompt sticks (a stable signature). Add these repository secrets and the workflow signs and notarizes automatically (no code change):

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | the Developer ID Application cert exported as `.p12`, base64 (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | your Apple ID email |
| `APPLE_PASSWORD` | an app-specific password (appleid.apple.com) |
| `APPLE_TEAM_ID` | your 10-character team ID |

Once releases are notarized, remove the `postflight` quarantine block from the cask.

**Without it (fine for the early release).** Files fetched by `curl` (the install script) and by the tap cask (which strips the quarantine flag) open normally. Someone who double-clicks a `.dmg` downloaded in a browser will see "Apple cannot check it": they right-click > Open, or System Settings > Privacy & Security > Open Anyway. Note: Homebrew's *official* cask repo requires notarized apps from September 2026, so unsigned builds can only live in your own tap.

**Windows.** Unsigned installers show a SmartScreen notice (More info > Run anyway). Removing it needs a code-signing certificate (an OV/EV certificate from a CA, roughly $200-600/year, hardware-token based) or Azure Trusted Signing (about $10/month, with eligibility rules). Not needed for an early beta.

## Notes
- Linux is not built yet (needs the webkit/libsecret toolchain in CI and a capture story).
- Windows has not been built or run by us yet: the first CI run is the real test, expect to fix a thing or two.
- Live capture is macOS-only (ScreenCaptureKit); on Windows the app runs without it.
