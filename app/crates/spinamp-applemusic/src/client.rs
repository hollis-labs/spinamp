use crate::map::{self, Page, PROVIDER};
use crate::{player, token};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use spinamp_core::provider::{MusicProvider, Playback, ProviderError, RemotePlaylist, Result};
use spinamp_core::{Track, TrackId};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::RwLock;

const API: &str = "https://api.music.apple.com";

/// Everything needed to talk to Apple Music; stored as one opaque blob in the OS keychain.
/// The `.p8` is the user's own MusicKit key (bring-your-own-key).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Creds {
    pub key_pem: String,
    pub key_id: String,
    pub team_id: String,
    #[serde(default)]
    pub user_token: Option<String>,
    #[serde(default)]
    pub storefront: Option<String>,
}

pub struct AppleMusicProvider {
    creds: RwLock<Option<Creds>>,
    dev: Mutex<Option<(String, i64)>>,
    http: reqwest::Client,
    /// Tracks seen via search / playlists, so what the Music app reports can be mapped back to ids.
    seen: Mutex<HashMap<String, TrackId>>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn key_of(title: &str, artist: &str) -> String {
    let first = artist.split(", ").next().unwrap_or(artist);
    format!("{}|{}", norm(title), norm(first))
}

fn err(e: impl ToString) -> ProviderError {
    ProviderError::Other(e.to_string())
}

impl AppleMusicProvider {
    pub fn new(saved: Option<Creds>) -> Self {
        Self { creds: RwLock::new(saved), dev: Mutex::new(None), http: reqwest::Client::new(), seen: Mutex::new(HashMap::new()) }
    }

    /// Install (or replace) the user's MusicKit key. Signing in again is needed after a change.
    pub async fn set_key(&self, key_pem: &str, key_id: &str, team_id: &str) -> std::result::Result<(), String> {
        token::developer_token(key_pem, key_id, team_id, now())?; // validate before keeping it
        *self.creds.write().await =
            Some(Creds { key_pem: key_pem.into(), key_id: key_id.into(), team_id: team_id.into(), user_token: None, storefront: None });
        *self.dev.lock().unwrap() = None;
        Ok(())
    }

    pub async fn has_key(&self) -> bool {
        self.creds.read().await.as_ref().is_some_and(|c| !c.key_pem.is_empty())
    }

    pub async fn set_user_token(&self, t: String) {
        if let Some(c) = self.creds.write().await.as_mut() {
            c.user_token = Some(t);
        }
    }

    /// Cached developer token, renewed a day before it expires.
    pub async fn developer_token(&self) -> std::result::Result<String, String> {
        let n = now();
        if let Some((t, exp)) = self.dev.lock().unwrap().as_ref() {
            if *exp - n > 86_400 {
                return Ok(t.clone());
            }
        }
        let c = self.creds.read().await.clone().ok_or("Add your Apple MusicKit key in Settings first.")?;
        let t = token::developer_token(&c.key_pem, &c.key_id, &c.team_id, n)?;
        *self.dev.lock().unwrap() = Some((t.clone(), n + token::LIFETIME_SECS));
        Ok(t)
    }

    async fn user_token(&self) -> Result<String> {
        self.creds
            .read()
            .await
            .as_ref()
            .and_then(|c| c.user_token.clone())
            .ok_or_else(|| ProviderError::Unauthenticated(PROVIDER.into()))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let dev = self.developer_token().await.map_err(err)?;
        let url = if path.starts_with("http") { path.to_string() } else { format!("{API}{path}") };
        let r = self
            .http
            .get(url)
            .bearer_auth(dev)
            .header("Music-User-Token", self.user_token().await?)
            .query(query)
            .send()
            .await
            .map_err(err)?;
        crate::log(&format!("GET {} -> {}", path.split('?').next().unwrap_or(path), r.status()));
        match r.status().as_u16() {
            200..=299 => {
                let body = r.text().await.map_err(err)?;
                serde_json::from_str::<T>(&body).map_err(|e| {
                    crate::log(&format!("could not read Apple's reply: {e}; starts: {}", body.chars().take(300).collect::<String>()));
                    err(format!("could not read Apple's reply: {e}"))
                })
            }
            401 => Err(err("Apple rejected the developer token. Check the Key ID, Team ID and .p8 in Settings.")),
            403 => Err(err("Apple Music access expired or was denied. Reconnect Apple Music (needs an active subscription).")),
            429 => Err(err("Apple Music rate limit hit; try again in a moment.")),
            s => Err(err(format!("apple music {s}: {}", r.text().await.unwrap_or_default()))),
        }
    }

    async fn storefront(&self) -> Result<String> {
        if let Some(s) = self.creds.read().await.as_ref().and_then(|c| c.storefront.clone()) {
            return Ok(s);
        }
        let p: Page = self.get("/v1/me/storefront", &[]).await?;
        let sf = p.data.into_iter().next().map(|d| d.id).unwrap_or_else(|| "us".into());
        if let Some(c) = self.creds.write().await.as_mut() {
            c.storefront = Some(sf.clone());
        }
        Ok(sf)
    }

    fn remember(&self, tracks: &[Track]) {
        let mut seen = self.seen.lock().unwrap();
        for t in tracks {
            seen.insert(key_of(&t.title, &t.artist), t.id.clone());
        }
    }
}

#[async_trait]
impl MusicProvider for AppleMusicProvider {
    fn id(&self) -> &'static str {
        PROVIDER
    }
    fn display_name(&self) -> &'static str {
        "Apple Music"
    }

    async fn is_authenticated(&self) -> bool {
        self.creds.read().await.as_ref().is_some_and(|c| !c.key_pem.is_empty() && c.user_token.is_some())
    }

    async fn credentials(&self) -> Option<String> {
        self.creds.read().await.as_ref().and_then(|c| serde_json::to_string(c).ok())
    }

    async fn restore_credentials(&self, blob: &str) -> Result<()> {
        *self.creds.write().await = Some(serde_json::from_str(blob).map_err(err)?);
        Ok(())
    }

    async fn disconnect(&self) -> Result<()> {
        *self.creds.write().await = None;
        *self.dev.lock().unwrap() = None;
        Ok(())
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Track>> {
        #[derive(Deserialize)]
        struct R {
            results: Rs,
        }
        #[derive(Deserialize)]
        struct Rs {
            songs: Option<Page>,
        }
        let sf = self.storefront().await?;
        let r: R = self
            .get(&format!("/v1/catalog/{sf}/search"), &[("term", query.into()), ("types", "songs".into()), ("limit", limit.min(25).to_string())])
            .await?;
        let tracks: Vec<Track> = r.results.songs.map(|p| p.data).unwrap_or_default().into_iter().filter_map(map::to_track).collect();
        self.remember(&tracks);
        Ok(tracks)
    }

    async fn playlists(&self) -> Result<Vec<RemotePlaylist>> {
        let mut out: Vec<(String, String)> = vec![];
        let mut offset = 0;
        loop {
            let p: Page = self.get("/v1/me/library/playlists", &[("limit", "100".into()), ("offset", offset.to_string())]).await?;
            out.extend(p.data.into_iter().map(|d| (d.id, d.attributes.name.unwrap_or_else(|| "Untitled".into()))));
            if p.next.is_none() {
                break;
            }
            offset += 100;
        }
        // The list has no track counts; ask for one track of each to read `meta.total`.
        let counts = futures_util::future::join_all(out.iter().map(|(id, _)| async move {
            self.get::<Page>(&format!("/v1/me/library/playlists/{id}/tracks"), &[("limit", "1".into())])
                .await
                .ok()
                .and_then(|p| p.meta.and_then(|m| m.total))
                .unwrap_or(0)
        }))
        .await;
        Ok(out.into_iter().zip(counts).map(|((id, name), n)| RemotePlaylist { id, name, track_count: n }).collect())
    }

    async fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>> {
        let mut tracks = vec![];
        let mut offset = 0;
        let path = if playlist_id.starts_with("p.") {
            format!("/v1/me/library/playlists/{playlist_id}/tracks")
        } else {
            let sf = self.storefront().await?;
            format!("/v1/catalog/{sf}/playlists/{playlist_id}/tracks")
        };
        loop {
            let p: Page = self.get(&path, &[("include", "catalog".into()), ("limit", "100".into()), ("offset", offset.to_string())]).await?;
            tracks.extend(p.data.into_iter().filter_map(map::to_track));
            if p.next.is_none() {
                break;
            }
            offset += 100;
        }
        self.remember(&tracks);
        Ok(tracks)
    }

    async fn play(&self, track: &Track) -> Result<()> {
        self.remember(std::slice::from_ref(track));
        player::play(track).await.map_err(err)
    }
    async fn pause(&self) -> Result<()> {
        player::pause().await.map_err(err)
    }
    async fn resume(&self) -> Result<()> {
        player::resume().await.map_err(err)
    }
    async fn seek(&self, position_ms: u64) -> Result<()> {
        player::seek(position_ms).await.map_err(err)
    }

    async fn now_playing(&self) -> Result<Playback> {
        let snap = player::snapshot().await.map_err(err)?;
        Ok(match snap {
            None => Playback { track: None, is_playing: false, position_ms: 0, device_name: None, device_type: None, volume_percent: None, shuffle: false, duration_ms: None },
            Some(s) => Playback {
                track: self.seen.lock().unwrap().get(&key_of(&s.title, &s.artist)).cloned(),
                is_playing: s.playing,
                position_ms: s.position_ms,
                device_name: Some("Music".into()),
                device_type: Some("Computer".into()),
                volume_percent: s.volume,
                shuffle: s.shuffle,
                duration_ms: Some(s.duration_ms),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_case_punctuation_and_featured_artists() {
        assert_eq!(key_of("Alison!", "Slowdive, Guest"), key_of("alison", "SLOWDIVE"));
    }

    #[tokio::test]
    async fn not_authenticated_until_key_and_user_token() {
        let p = AppleMusicProvider::new(None);
        assert!(!p.is_authenticated().await);
        p.set_key(&crate::testkey::pair().0, "K", "T").await.unwrap();
        assert!(p.has_key().await && !p.is_authenticated().await);
        p.set_user_token("u".into()).await;
        assert!(p.is_authenticated().await);
        let blob = p.credentials().await.unwrap();
        let q = AppleMusicProvider::new(None);
        q.restore_credentials(&blob).await.unwrap();
        assert!(q.is_authenticated().await);
        q.disconnect().await.unwrap();
        assert!(!q.is_authenticated().await);
    }

    #[tokio::test]
    async fn bad_key_is_rejected_and_not_kept() {
        let p = AppleMusicProvider::new(None);
        assert!(p.set_key("junk", "K", "T").await.is_err());
        assert!(!p.has_key().await);
    }
}
