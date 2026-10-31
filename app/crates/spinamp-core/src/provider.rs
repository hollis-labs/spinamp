//! Provider adapters. Spotify first; any service implementing [`MusicProvider`] plugs in,
//! and playlists can freely mix tracks from several providers.

use crate::{Track, TrackId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider `{0}` is not registered")]
    Unknown(String),
    #[error("not authenticated with `{0}`")]
    Unauthenticated(String),
    #[error("`{provider}` does not support {feature}")]
    Unsupported { provider: String, feature: &'static str },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ProviderError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePlaylist {
    pub id: String,
    pub name: String,
    pub track_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playback {
    pub track: Option<TrackId>,
    pub is_playing: bool,
    pub position_ms: u64,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub device_type: Option<String>,
    #[serde(default)]
    pub volume_percent: Option<u32>,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[async_trait]
pub trait MusicProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;

    async fn is_authenticated(&self) -> bool;

    /// Opaque, serialisable credentials (tokens) for the app to keep in the OS keychain.
    /// Each provider decides what is in the string; the app never looks inside.
    async fn credentials(&self) -> Option<String> {
        None
    }
    /// Restore credentials previously returned by [`credentials`].
    async fn restore_credentials(&self, _blob: &str) -> Result<()> {
        Ok(())
    }
    /// Forget the session (log out locally).
    async fn disconnect(&self) -> Result<()> {
        Ok(())
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Track>>;
    async fn playlists(&self) -> Result<Vec<RemotePlaylist>>;
    async fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>>;

    /// Tracks outside the user's library that resemble `seeds`. Feeds Discovery.
    async fn candidates(&self, _seeds: &[Track], _limit: u32) -> Result<Vec<Track>> {
        Err(ProviderError::Unsupported { provider: self.id().into(), feature: "candidates" })
    }

    async fn play(&self, track: &Track) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn seek(&self, position_ms: u64) -> Result<()>;
    async fn now_playing(&self) -> Result<Playback>;
}

#[derive(Default, Clone)]
pub struct ProviderRegistry {
    providers: HashMap<&'static str, Arc<dyn MusicProvider>>,
}

impl ProviderRegistry {
    pub fn register(&mut self, p: Arc<dyn MusicProvider>) {
        self.providers.insert(p.id(), p);
    }

    pub fn get(&self, id: &str) -> Result<&Arc<dyn MusicProvider>> {
        self.providers.get(id).ok_or_else(|| ProviderError::Unknown(id.into()))
    }

    /// Route playback to whichever provider owns the track.
    pub async fn play(&self, track: &Track) -> Result<()> {
        self.get(&track.provider)?.play(track).await
    }

    /// Search every authenticated provider and concatenate results.
    pub async fn search_all(&self, query: &str, limit: u32) -> Vec<Track> {
        let mut out = vec![];
        for p in self.providers.values() {
            if p.is_authenticated().await {
                if let Ok(mut r) = p.search(query, limit).await {
                    out.append(&mut r);
                }
            }
        }
        out
    }
}
