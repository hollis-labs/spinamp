//! Spinamp core: no UI, no network, no Tauri. Everything here is pure and testable.
//!
//! - [`provider`]  adapter trait + registry (Spotify first, others later)
//! - [`feedback`]  thumbs up/down with escalation, burst damping and cooldowns
//! - [`shuffle`]   weighted, artist-spacing, recency-aware shuffle
//! - [`block`]     blocked songs/artists/tags and muted tracks
//! - [`discovery`] taste profile + candidate scoring for tracks you do not own
//! - [`engine`]    Standard / Shuffle / Discovery / Shuffle+Discovery selection
//! - [`sort`]      playlist sorting (artist, added, weight, rating, ...)
//! - [`generate`]  prompt -> intent -> playlist of an exact length from your library
//! - [`listen`]    turns playback polls into listening sessions (listened ms, how they ended)
//! - [`tempo`]    reconcile ambiguous tempo readings with learned genre priors and external hints
//! - [`stats`]     plays vs listens, with overnight (quiet hours) exclusion
pub mod block;
pub mod discovery;
pub mod engine;
pub mod feedback;
pub mod generate;
pub mod listen;
pub mod model;
pub mod provider;
pub mod shuffle;
pub mod sort;
pub mod stats;
pub mod tempo;

pub use model::*;
