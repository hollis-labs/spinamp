# Third-party software and assets

Spinamp is built on open-source software. The main pieces, and how they are licensed:

| Component | Use | License |
|---|---|---|
| [Tauri](https://tauri.app) | desktop shell | MIT or Apache-2.0 |
| [React](https://react.dev), Vite | user interface | MIT |
| [symphonia](https://github.com/pdeljanov/Symphonia) | decoding audio previews | MPL-2.0 (unmodified) |
| [rustfft](https://github.com/ejmahler/RustFFT) | spectrum analysis | MIT or Apache-2.0 |
| [rusqlite](https://github.com/rusqlite/rusqlite) / SQLite | local database | MIT / public domain |
| [reqwest](https://github.com/seanmonstar/reqwest), tokio | networking | MIT or Apache-2.0 |
| [keyring](https://github.com/hwchen/keyring-rs) | system keychain access | MIT or Apache-2.0 |
| [screencapturekit](https://github.com/doom-fish/screencapturekit-rs) | macOS live capture | MIT or Apache-2.0 |

Dependency licenses can be listed with `cargo license` (Rust) and `pnpm licenses list` (JavaScript).

## Fonts (SIL Open Font License 1.1)
Doto, Chakra Petch, Share Tech Mono and JetBrains Mono, bundled via [Fontsource](https://fontsource.org).
The OFL permits bundling and redistribution; the fonts are not sold on their own.

## Data sources
Spinamp queries these public services at your request: Spotify Web API (your own account),
MusicBrainz, ListenBrainz, LRCLIB, lyrics.ovh, Deezer, iTunes Search and Wikipedia. Their content
belongs to its respective owners and is subject to their terms. Spinamp stores only derived numbers
from audio previews (tempo, key, loudness, spectrum fingerprints), never the audio itself.
