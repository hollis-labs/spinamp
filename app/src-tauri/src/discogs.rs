//! Discogs: artist profile and release credits for the Artist panel and the Credits screen.
//! Read-only, your own personal access token (OS keychain). Results are cached for 90 days;
//! a miss is cached for a week so a playlist full of obscure tracks does not hammer the API.

use crate::{Shared, now, settings};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use spinamp_core::{Track, primary_artist};
use spinamp_services::discogs::{self, ArtistInfo, Discogs, ReleaseInfo, TrackInfo};
use tauri::State;

const KEY: &str = "discogs_token";
const DAY: i64 = 86_400;

fn client() -> Option<Discogs> {
    settings::secret_get(KEY).filter(|k| !k.trim().is_empty()).map(|k| Discogs::new(&k))
}

#[tauri::command]
pub fn discogs_status() -> bool {
    settings::secret_get(KEY).is_some()
}

#[tauri::command]
pub fn set_discogs_token(token: String) -> Result<(), String> {
    let t = token.trim();
    if t.is_empty() {
        return Err("Token is empty.".into());
    }
    settings::secret_set(KEY, t)
}

#[tauri::command]
pub fn clear_discogs_token() -> Result<(), String> {
    settings::secret_clear(KEY)
}

#[tauri::command]
pub async fn discogs_test() -> Result<String, String> {
    let d = client().ok_or("Save your Discogs token first.")?;
    Ok(format!("Connected as {}", d.identity().await?))
}

#[derive(Serialize, Deserialize)]
struct Cached<T> {
    at: i64,
    data: Option<T>,
}

fn cached<T: DeserializeOwned>(app: &Shared, key: &str) -> Option<Cached<T>> {
    let j = app.lock().unwrap().store.kv_get(key).ok().flatten()?;
    let c: Cached<T> = serde_json::from_str(&j).ok()?;
    let ttl = if c.data.is_some() { 90 * DAY } else { 7 * DAY };
    (now() - c.at < ttl).then_some(c)
}

fn put<T: Serialize>(app: &Shared, key: &str, data: &Option<T>) {
    let c = Cached { at: now(), data: data.as_ref() };
    let _ = app.lock().unwrap().store.kv_set(key, &serde_json::to_string(&c).unwrap());
}

/// None when no token is saved (the UI then shows how to add one).
#[tauri::command]
pub async fn discogs_artist(app: State<'_, Shared>, name: String) -> Result<Option<ArtistInfo>, String> {
    let name = primary_artist(&name).to_string();
    let key = format!("dg:artist:{}", name.to_lowercase());
    if let Some(c) = cached::<ArtistInfo>(&app, &key) {
        return Ok(c.data);
    }
    let Some(d) = client() else { return Ok(None) };
    let found = d.artist(&name).await?;
    put(&app, &key, &found);
    Ok(found)
}

#[derive(Serialize)]
pub struct Credits {
    release: ReleaseInfo,
    /// The playing song's entry on that release, with its own credits.
    track: Option<TrackInfo>,
}

/// The release this track is on (by artist + album) and the track's credits on it.
#[tauri::command]
pub async fn discogs_credits(app: State<'_, Shared>, track: Track) -> Result<Option<Credits>, String> {
    let Some(album) = track.album.clone().filter(|a| !a.trim().is_empty()) else { return Ok(None) };
    let artist = primary_artist(&track.artist).to_string();
    let key = format!("dg:rel:{}|{}", artist.to_lowercase(), album.to_lowercase());
    let release = match cached::<ReleaseInfo>(&app, &key) {
        Some(c) => c.data,
        None => {
            let Some(d) = client() else { return Ok(None) };
            let found = d.release(&artist, &album).await?;
            put(&app, &key, &found);
            found
        }
    };
    Ok(release.map(|r| {
        let t = discogs::find_track(&r, &track.title).cloned();
        Credits { release: r, track: t }
    }))
}
