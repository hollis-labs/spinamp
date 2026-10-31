use crate::auth::{self, Tokens};
use crate::map::{SpTrack, to_track, uri};
use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use spinamp_core::provider::{MusicProvider, Playback, ProviderError, RemotePlaylist, Result};
use spinamp_core::{Track, TrackId};
use tokio::sync::Mutex;

/// Spotify ISO-8601 ("2026-08-30T22:58:11Z", optional fractional seconds) -> unix seconds.
pub fn parse_iso(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.trim_end_matches('Z').split(':');
    let (hh, mm) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let ss = t.next().and_then(|x| x.split('.').next()).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// A track with the moment it happened (saved to library / played).
#[derive(Debug, Clone)]
pub struct Stamped {
    pub track: Track,
    pub at: i64,
}

const API: &str = "https://api.spotify.com/v1";

pub struct SpotifyProvider {
    client_id: std::sync::Mutex<String>,
    http: reqwest::Client,
    tokens: Mutex<Option<Tokens>>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn other(e: impl ToString) -> ProviderError {
    ProviderError::Other(e.to_string())
}

impl SpotifyProvider {
    pub fn new(client_id: impl Into<String>, tokens: Option<Tokens>) -> Self {
        Self { client_id: std::sync::Mutex::new(client_id.into()), http: reqwest::Client::new(), tokens: Mutex::new(tokens) }
    }

    pub fn set_client_id(&self, id: &str) {
        *self.client_id.lock().unwrap() = id.to_string();
    }

    pub async fn set_tokens(&self, t: Tokens) {
        *self.tokens.lock().await = Some(t);
    }

    /// Current tokens, e.g. to persist after a refresh.
    pub async fn tokens(&self) -> Option<Tokens> {
        self.tokens.lock().await.clone()
    }

    async fn access(&self) -> Result<String> {
        let mut g = self.tokens.lock().await;
        let t = g.as_ref().ok_or_else(|| ProviderError::Unauthenticated("spotify".into()))?;
        if t.expires_at <= now() {
            let cid = self.client_id.lock().unwrap().clone();
            let fresh = auth::refresh(&cid, t, now()).await.map_err(other)?;
            *g = Some(fresh);
        }
        Ok(g.as_ref().unwrap().access_token.clone())
    }

    /// Request with one refresh-and-retry on 401 and a polite wait on 429.
    async fn send(&self, m: Method, path: &str, query: &[(&str, String)], body: Option<Value>) -> Result<(StatusCode, Value)> {
        for attempt in 0..3 {
            let token = self.access().await?;
            let url = if path.starts_with("http") { path.to_string() } else { format!("{API}{path}") };
            let mut rq = self.http.request(m.clone(), url).bearer_auth(token).query(query);
            rq = match &body {
                Some(b) => rq.json(b),
                None => rq.header("Content-Length", "0"),
            };
            let r = rq.send().await.map_err(other)?;
            let status = r.status();
            match status {
                StatusCode::UNAUTHORIZED if attempt == 0 => {
                    if let Some(t) = self.tokens.lock().await.as_mut() {
                        t.expires_at = 0; // force refresh
                    }
                    continue;
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    let wait = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(2);
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    continue;
                }
                s if s.is_success() => {
                    let v = r.text().await.map_err(other)?;
                    return Ok((s, serde_json::from_str(&v).unwrap_or(Value::Null)));
                }
                s => return Err(other(format!("spotify {s}: {}", r.text().await.unwrap_or_default()))),
            }
        }
        Err(other("spotify request failed after retries"))
    }

    /// Start/resume playback. With no active device, Spotify returns NO_ACTIVE_DEVICE;
    /// we then pick an available device and target it explicitly (which also activates it).
    async fn player_play(&self, body: Option<Value>) -> Result<()> {
        match self.send(Method::PUT, "/me/player/play", &[], body.clone()).await {
            Err(e) if e.to_string().contains("NO_ACTIVE_DEVICE") => {
                let devices = self.get("/me/player/devices", &[]).await?;
                let id = choose_device(&devices).ok_or_else(|| {
                    ProviderError::Other("NO_DEVICE: no Spotify device is online. Open Spotify on this or another device.".into())
                })?;
                self.send(Method::PUT, "/me/player/play", &[("device_id", id)], body).await.map(|_| ())
            }
            other => other.map(|_| ()),
        }
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        Ok(self.send(Method::GET, path, query, None).await?.1)
    }
}

/// Prefer a desktop ("Computer") device, else any non-restricted one.
pub fn choose_device(v: &Value) -> Option<String> {
    let list: Vec<&Value> = v["devices"].as_array()?.iter().filter(|d| d["is_restricted"] != true).collect();
    let pick = list.iter().find(|d| d["type"] == "Computer").or(list.first())?;
    pick["id"].as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spotify_timestamps() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-08-30T22:58:11Z"), Some(1_788_130_691));
        assert_eq!(parse_iso("2024-02-29T12:00:00.123Z"), Some(1_709_208_000));
        assert_eq!(parse_iso("garbage"), None);
    }

    #[test]
    fn prefers_computer_and_skips_restricted() {
        let v = json!({"devices":[
            {"id":"p","type":"Smartphone","is_restricted":false},
            {"id":"r","type":"Computer","is_restricted":true},
            {"id":"c","type":"Computer","is_restricted":false}]});
        assert_eq!(choose_device(&v).as_deref(), Some("c"));
        assert_eq!(choose_device(&json!({"devices":[]})), None);
    }
}

#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
    next: Option<String>,
}

impl SpotifyProvider {
    /// Liked Songs, newest first, up to `max`.
    pub async fn saved_tracks(&self, max: usize) -> Result<Vec<Stamped>> {
        let mut out = vec![];
        let mut next = Some(format!("{API}/me/tracks?limit=50"));
        while let Some(url) = next {
            if out.len() >= max {
                break;
            }
            let page: Page<Value> = serde_json::from_value(self.get(&url, &[]).await?).map_err(other)?;
            for it in page.items {
                if let Ok(t) = serde_json::from_value::<SpTrack>(it["track"].clone()) {
                    if let Some(track) = to_track(t) {
                        out.push(Stamped { track, at: it["added_at"].as_str().and_then(parse_iso).unwrap_or(0) });
                    }
                }
            }
            next = page.next;
        }
        Ok(out)
    }

    /// `range`: short_term (4 weeks), medium_term (6 months), long_term (all time).
    pub async fn top_tracks(&self, range: &str, limit: u32) -> Result<Vec<Track>> {
        let v = self.get("/me/top/tracks", &[("time_range", range.into()), ("limit", limit.min(50).to_string())]).await?;
        let items: Vec<Value> = serde_json::from_value(v["items"].clone()).map_err(other)?;
        Ok(items.into_iter().filter_map(|i| serde_json::from_value::<SpTrack>(i).ok().and_then(to_track)).collect())
    }

    pub async fn top_artists(&self, range: &str, limit: u32) -> Result<Vec<String>> {
        let v = self.get("/me/top/artists", &[("time_range", range.into()), ("limit", limit.min(50).to_string())]).await?;
        Ok(v["items"].as_array().map(|a| a.iter().filter_map(|x| x["name"].as_str().map(String::from)).collect()).unwrap_or_default())
    }

    /// The last (up to 50) plays Spotify remembers, newest first.
    pub async fn recently_played(&self, limit: u32) -> Result<Vec<Stamped>> {
        let v = self.get("/me/player/recently-played", &[("limit", limit.min(50).to_string())]).await?;
        let items: Vec<Value> = serde_json::from_value(v["items"].clone()).map_err(other)?;
        Ok(items
            .into_iter()
            .filter_map(|i| {
                let t = to_track(serde_json::from_value::<SpTrack>(i["track"].clone()).ok()?)?;
                Some(Stamped { track: t, at: i["played_at"].as_str().and_then(parse_iso)? })
            })
            .collect())
    }
}

#[async_trait]
impl MusicProvider for SpotifyProvider {
    fn id(&self) -> &'static str {
        "spotify"
    }
    fn display_name(&self) -> &'static str {
        "Spotify"
    }
    async fn is_authenticated(&self) -> bool {
        self.tokens.lock().await.is_some()
    }
    async fn credentials(&self) -> Option<String> {
        self.tokens.lock().await.as_ref().and_then(|t| serde_json::to_string(t).ok())
    }
    async fn restore_credentials(&self, blob: &str) -> Result<()> {
        let t: Tokens = serde_json::from_str(blob).map_err(other)?;
        *self.tokens.lock().await = Some(t);
        Ok(())
    }
    async fn disconnect(&self) -> Result<()> {
        *self.tokens.lock().await = None;
        Ok(())
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<Track>> {
        let v = self.get("/search", &[("q", query.into()), ("type", "track".into()), ("limit", limit.clamp(1, 10).to_string())]).await?;
        let items: Vec<SpTrack> = serde_json::from_value(v["tracks"]["items"].clone()).map_err(other)?;
        Ok(items.into_iter().filter_map(to_track).collect())
    }

    async fn playlists(&self) -> Result<Vec<RemotePlaylist>> {
        let mut out = vec![];
        let mut next = Some(format!("{API}/me/playlists?limit=50"));
        while let Some(url) = next {
            let v = self.get(&url, &[]).await?;
            let page: Page<Value> = serde_json::from_value(v).map_err(other)?;
            for p in page.items {
                out.push(RemotePlaylist {
                    id: p["id"].as_str().unwrap_or_default().into(),
                    name: p["name"].as_str().unwrap_or_default().into(),
                    track_count: p["items"]["total"].as_u64().or(p["tracks"]["total"].as_u64()).unwrap_or(0) as u32,
                });
            }
            next = page.next;
        }
        Ok(out)
    }

    async fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>> {
        let mut out = vec![];
        let mut next = Some(format!("{API}/playlists/{playlist_id}/items?limit=100"));
        while let Some(url) = next {
            let v = self.get(&url, &[]).await?;
            let page: Page<Value> = serde_json::from_value(v).map_err(other)?;
            for it in page.items {
                // Feb 2026 API renamed `track` to `item`. It is null for removed entries
                // and may be an episode (no artists), which fails to parse and is skipped.
                let raw = if it["item"].is_null() { it["track"].clone() } else { it["item"].clone() };
                if let Ok(t) = serde_json::from_value::<SpTrack>(raw) {
                    out.extend(to_track(t));
                }
            }
            next = page.next;
        }
        Ok(out)
    }

    async fn play(&self, track: &Track) -> Result<()> {
        self.player_play(Some(json!({ "uris": [uri(&track.id)] }))).await
    }
    async fn pause(&self) -> Result<()> {
        self.send(Method::PUT, "/me/player/pause", &[], None).await.map(|_| ())
    }
    async fn resume(&self) -> Result<()> {
        self.player_play(Some(json!({}))).await
    }
    async fn seek(&self, position_ms: u64) -> Result<()> {
        self.send(Method::PUT, "/me/player/seek", &[("position_ms", position_ms.to_string())], None).await.map(|_| ())
    }
    async fn now_playing(&self) -> Result<Playback> {
        let (s, v) = self.send(Method::GET, "/me/player", &[], None).await?;
        if s == StatusCode::NO_CONTENT || v.is_null() {
            return Ok(Playback { track: None, is_playing: false, position_ms: 0, device_name: None, device_type: None, volume_percent: None, shuffle: false, duration_ms: None });
        }
        Ok(Playback {
            track: v["item"]["id"].as_str().map(|id| TrackId::new("spotify", id)),
            is_playing: v["is_playing"].as_bool().unwrap_or(false),
            position_ms: v["progress_ms"].as_u64().unwrap_or(0),
            device_name: v["device"]["name"].as_str().map(String::from),
            device_type: v["device"]["type"].as_str().map(String::from),
            volume_percent: v["device"]["volume_percent"].as_u64().map(|x| x as u32),
            shuffle: v["shuffle_state"].as_bool().unwrap_or(false),
            duration_ms: v["item"]["duration_ms"].as_u64(),
        })
    }
    // `candidates` intentionally unimplemented: Spotify recommendations are closed to new apps.
}
