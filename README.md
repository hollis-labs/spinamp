# SPINAMP

**Try it:** https://spinamp.fm (macOS: `brew tap hollis-labs/tap && brew trust hollis-labs/tap && brew install --cask spinamp` or `curl -fsSL https://spinamp.fm/install.sh | sh`; Windows: `irm https://spinamp.fm/install.ps1 | iex`). Releases and signing: see [RELEASING.md](RELEASING.md).

Playlist-first music player. Tauri 2 + React/TS shell, Rust core, provider adapters (Spotify first).

```
app/crates/spinamp-core   pure Rust: feedback, shuffle, discovery, sort, stats, provider trait (30 tests)
app/crates/spinamp-spotify  OAuth PKCE (loopback :8888), Web API, Connect playback
app/crates/spinamp-audio    tempo / energy / loudness / loud-spike measurement from 30s previews (symphonia + rustfft)
app/crates/spinamp-services MusicBrainz artist tags, optional AI prompt parsing
app/crates/spinamp-store    SQLite: library, votes, play events (~/Library/Application Support/Spinamp)
app/src-tauri             Tauri shell + commands (demo library until the Spotify adapter lands)
app/src                   React UI (Lumon / dot-matrix style, see inspiration/ui-style)
```

Run: `cd app && pnpm install && pnpm tauri dev`   Test core: `cd app/crates/spinamp-core && cargo test`

## Decisions
- Playback (MVP): Spotify Connect remote control via Web API (Premium). Web Playback SDK needs Widevine, which WKWebView lacks.
- Spotify withdrew audio-features / recommendations / related-artists for new apps. Tempo/energy/tags must come from other sources (ReccoBeats, Last.fm, MusicBrainz/ListenBrainz); core treats them as optional.
- Thumbs: escalate on repeats (cap 3x), damp double-taps (<5s), 90-day half-life, thumbs-down cooldown doubles per consecutive down.
- Stats: play = started, listen = >=80% played; starts during 23:00-07:00 local are excluded from both.

## Spotify setup
Create an app at developer.spotify.com, add redirect `http://127.0.0.1:43821/callback`, run `pnpm tauri dev`, then paste the Client ID in Settings (gear icon). Env vars (`SPINAMP_SPOTIFY_CLIENT_ID`, `SPINAMP_AI_KEY`, `SPINAMP_CONTACT`) still work as fallbacks. Open Spotify on a device first (Connect needs an active device).

## Data captured (for algorithms)
- Per track: ISRC, explicit, release date, album art, album/artist ids, disc, uri (Spotify no longer returns genres, popularity or audio features).
- Per play (`play_events`): started_at, listened_ms, how it ended (finished/skipped/stopped), end position, engine mode, trigger (auto/click/external), source playlist, device, device type, volume, shuffle state. Listening is measured by polling Spotify every 2s (`spinamp-core::listen`).
- Re-import a playlist to backfill the new per-track fields on older imports.

## Tags and prompt-to-playlist
- Tags: MusicBrainz artist lookup (1 req/s, background, cached in `artists`; no match is cached too). Tracks inherit their primary artist's top tags. Set `SPINAMP_CONTACT` to add a contact string to the User-Agent (default sends none).
- Generate: prompt -> Intent (offline keyword table, or Claude if `SPINAMP_AI_KEY`/`ANTHROPIC_API_KEY` is set; it sees the prompt and tag names only) -> weighted pick from the library with artist caps, then a local search to land within seconds of the requested length.

## Settings and secrets
Prefs are stored in sqlite (`kv.settings`). The AI key goes in the OS keychain (macOS Keychain; Credential Manager / Secret Service elsewhere) and is never returned to the UI. Spotify tokens are still in sqlite; moving them to the keychain is open.

## Discovery catalog
seeds = library artists you like (weighted by feedback) -> similar artists from Last.fm (by name, with your key) and ListenBrainz (public, by MusicBrainz id) -> drop artists you own or already queued -> Spotify search for their tracks -> MusicBrainz tags -> `candidates` table. Tops up automatically when the pool drops under 20; voting on a discovery track adds it to the library.

## Lyrics, artist panel, Spotify history
- Lyrics: LRCLIB (free, keyless), synced when available, cached (hits forever, misses 7 days).
- Artist panel: Wikipedia summary + MusicBrainz links/members + Deezer fans/photo; cached 30 days.
- Spotify import (SPOTIFY tab): Liked Songs (optionally +1, dated when you saved them), Top Tracks (3 ranges), Recently Played (also backfills plays), Top Artists (discovery seeds, never recommended back). Needs the extra scopes: press RECONNECT once.
- Not available from Spotify any more: audio features, recommendations, related artists, artist top tracks. ReccoBeats (audio features by Spotify id) was down when tried; revisit for tempo/energy.

## Apple Music, merging, Last.fm
- Apple Music: bring your own MusicKit key (Settings > Apple Music: path to the `.p8`, Key ID, Team ID), then CONNECT in the SOURCES tab. Playback controls the Music app (macOS) and only plays songs that are in your library.
- A song on several services (same ISRC) is one row; Settings > Playback chooses which service plays it.
- Discogs (read-only, your own personal access token): the CREDITS button shows the release's label, catalogue number, country, format, genres/styles and per-song credits; the ARTIST panel adds real name, aliases and members. Cached 90 days.
- Last.fm (read-only, your own API key + username): similar artists feed Discovery, and Settings can import scrobbles (matched to songs you own, backfilled with real dates), loved tracks and top artists.

## Provider credentials
Tokens are stored in the OS keychain under `cred:<provider>` (opaque blob per provider; the adapter decides the contents), with a sqlite fallback if the keychain refuses. The `MusicProvider` trait has `credentials()`, `restore_credentials()` and `disconnect()`, so Apple Music / YouTube / Amazon adapters plug in without app changes. Old sqlite tokens are migrated automatically.

## Audio analysis (tempo, energy, loudness)
Spotify removed audio features and ReccoBeats is unreliable, so Spinamp measures its own. Per track with an ISRC: preview from Deezer (iTunes as fallback) -> decode -> tempo (onset autocorrelation + harmonic comb, folded to 70-180 BPM), loudness (dBFS), dynamic range, loud-spike size (p99.5 minus median of 50ms windows), brightness, and a composite energy score. Only numbers are stored. Opt-in (about 0.5 MB per track; whole library about 1 GB). Known limits: tempo can land on a 3:2 or half/double-time reading; energy is relative to typical music, calibrated on bass-heavy electronic, so re-check on quiet genres. `cargo run --release --example probe -- <isrc>...` in `spinamp-audio` prints measurements for calibration.

## Imports: merge, never overwrite; soft delete only
- Imports are create-or-update (idempotent). Provider fields refresh when the provider has a value; tags are unioned; measured tempo/energy/audio, votes, ratings, play history and added-at are never touched; playlists only gain tracks (nothing removed or reordered, names left alone).
- Deleting a playlist, track or playlist entry only sets `deleted_at`. Imports skip tombstoned items (reported as "skipped"); explicit actions (voting, adding to a playlist, re-importing a deleted playlist) restore them. Settings > Data has RESTORE ALL.

## Visualiser
Spotify audio is DRM-protected, so what the visualiser shows depends on the best source available:
1. **Live** (opt-in, macOS): ScreenCaptureKit stream filtered to the Spotify desktop app's audio only (needs Screen & System Audio Recording permission). 16 log-spaced bands (40 Hz-10 kHz) every 50 ms; audio is discarded immediately. Streamed to the UI as `viz-live` events.
2. **Recorded**: the same frames, saved per track (`captures` table) indexed by playback time, merged across listens (first-recorded frames win, gaps fill), replayed on later listens with no grid assumptions.
3. **Preview**: the analysed 30 s of the track as a beat-quantised spectrum recording (unaveraged), replayed locked to tempo, plus a proxy arrangement (intro build, 8-bar sections, breakdowns, fills, fade-out) for the rest of the song.
4. **Synth**: genre-shaped drum/bass/pad pattern from tempo, tags, energy and measured brightness.
Rendering: EQ bars with fast-attack/slow-decay physics, falling peak caps and treble-weighted jitter. Four layers painted back to front: dark red (highs, rough detail) behind, orange, amber and yellow (bass, smooth) in front. Optional LED segmentation. Capture is off by default; Settings > Live capture.
Dev note: building needs the Swift compatibility libs on the link path (handled in `src-tauri/build.rs` for Command Line Tools installs).

## Keychain
All secrets live in ONE keychain item (`vault`, a JSON map) read once per launch. Dev builds are ad-hoc signed, so their identity changes on every rebuild and macOS re-asks for your password even after "Always Allow". Fix: sign dev builds with a stable identity. `src-tauri/scripts/setup-dev-signing.sh` creates a self-signed "Spinamp Dev" certificate (one-time, you run it); `.cargo/config.toml` signs each build through `scripts/sign-and-run.sh` (also honours `SPINAMP_SIGN_IDENTITY`). Release builds signed with a real identity do not have this problem.

## Tempo reconciliation
The analyser reports its alternative readings (`tempo_candidates`) and keeps the raw measurement in `meta.audio.tempo_bpm`. `spinamp-core::tempo` picks the tempo shown/used (`track.tempo`): learned per-tag priors from the library's own confident measurements (needs 8+ samples per tag, improves as more is analysed) and Deezer's BPM when it has one, overriding the measurement only when the evidence is clearly stronger (1.4x). Your own value (double-click a BPM cell; empty = automatic) always wins. A pattern keeps the grid tempo it was built on (`VizPattern.bpm`) so reconciling never changes visualiser speed. Re-runs after each analysis batch.

## Key, mood, and curation controls
- **Key / mood** (analysis format 3): chroma -> Krumhansl-Schmuckler key (reported only when it clearly wins; percussive/atonal music gets none), valence proxy (mode, brightness, tempo), mood words from energy x valence (uplifting, energetic, aggressive, dark, calm, peaceful, melancholic, somber, dreamy, danceable, ...), danceability. Heuristics, documented in `spinamp-audio/src/key.rs`. Prompt-to-playlist understands mood words.
- **Mute** (1/7/30 days), **favorite** (x1.8 shuffle heat, +2 weight in sort, extra taste learning), **remove** (this playlist / whole library, both soft), **block** songs (by ISRC, so any provider), artists, tags/genres. Muted and blocked tracks are skipped by shuffle, discovery and generated playlists, and discovery never suggests blocked artists/tags; they stay listed and marked. Manage blocks in Settings.

## Next
1. spinamp.fm landing page + install commands; shared fingerprint server
2. Generate: include new (non-library) tracks from the discovery pool
3. Prompt-to-playlist (exact duration, mood), AI optional
4. Lyrics, video embeds, loudness/tempo filters, sharing

## License
Code: [MIT](LICENSE). The Spinamp name and logo are trademarks of Hollis Labs: see [TRADEMARK.md](TRADEMARK.md). Third-party notices: [THIRD_PARTY.md](THIRD_PARTY.md).
