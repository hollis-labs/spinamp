//! Spotify adapter: OAuth (PKCE, loopback redirect), Web API reads, Connect playback control.
pub mod auth;
pub mod client;
pub mod map;

pub use auth::{Session, Tokens};
pub use client::SpotifyProvider;
