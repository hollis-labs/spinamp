//! Last.fm: scrobble history, loved tracks and top artists in, similar artists out (catalog.rs).
//! Read-only and keyed by the user's own API key (OS keychain) plus their public username.

use crate::{Shared, reload, settings};
use serde::Serialize;
use spinamp_core::merge::match_key;
use spinamp_core::{TrackId, primary_artist};
use spinamp_services::lastfm::LastFm;
use std::collections::HashMap;
use tauri::State;

const KEY: &str = "lastfm_key";
/// 200 scrobbles a page; this stops a runaway at 100k scrobbles.
const MAX_PAGES: u32 = 500;

pub fn client() -> Option<LastFm> {
    settings::secret_get(KEY).filter(|k| !k.trim().is_empty()).map(|k| LastFm::new(&k))
}

#[derive(Serialize)]
pub struct LfStatus {
    key_set: bool,
    user: String,
}

#[tauri::command]
pub fn lastfm_status(app: State<Shared>) -> LfStatus {
    LfStatus { key_set: settings::secret_get(KEY).is_some(), user: app.lock().unwrap().settings.lastfm_user.clone() }
}

#[tauri::command]
pub fn set_lastfm_key(key: String) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Key is empty.".into());
    }
    settings::secret_set(KEY, key)
}

#[tauri::command]
pub fn clear_lastfm_key() -> Result<(), String> {
    settings::secret_clear(KEY)
}

fn need(user: &str) -> Result<(LastFm, String), String> {
    let user = user.trim();
    if user.is_empty() {
        return Err("Enter your Last.fm username first.".into());
    }
    Ok((client().ok_or("Save your Last.fm API key first.")?, user.to_string()))
}

#[tauri::command]
pub async fn lastfm_test(user: String) -> Result<String, String> {
    let (lf, user) = need(&user)?;
    let (name, plays) = lf.user_info(&user).await?;
    Ok(format!("Connected as {name} ({plays} scrobbles)"))
}

/// kind: scrobbles | loved | artists | all. `votes`: count loved tracks as a +1.
#[tauri::command]
pub async fn lastfm_import(app: State<'_, Shared>, user: String, kind: String, votes: bool) -> Result<String, String> {
    let (lf, user) = need(&user)?;
    let kinds: Vec<&str> = if kind == "all" { vec!["scrobbles", "loved", "artists"] } else { vec![kind.as_str()] };
    let mut notes = vec![];
    for k in kinds {
        notes.push(match k {
            "scrobbles" => import_scrobbles(&app, &lf, &user).await?,
            "loved" => import_loved(&app, &lf, &user, votes).await?,
            "artists" => import_artists(&app, &lf, &user).await?,
            other => return Err(format!("unknown import kind {other}")),
        });
    }
    reload(&mut app.lock().unwrap())?;
    Ok(notes.join("; "))
}

/// Every copy of every library song keyed for matching, pointing at the primary id + length.
fn index(app: &Shared) -> HashMap<String, (TrackId, u64)> {
    let a = app.lock().unwrap();
    let mut m = HashMap::new();
    for s in &a.library {
        for v in s.variants() {
            m.entry(match_key(&v.title, &v.artist)).or_insert((s.track.id.clone(), s.track.duration_ms));
        }
    }
    m
}

async fn import_scrobbles(app: &Shared, lf: &LastFm, user: &str) -> Result<String, String> {
    let idx = index(app);
    let (mut matched, mut other, mut page, mut total_pages) = (vec![], 0usize, 1u32, 1u32);
    while page <= total_pages && page <= MAX_PAGES {
        let p = lf.recent_tracks(user, page, None).await?;
        total_pages = p.total_pages.max(1);
        for s in p.items {
            match idx.get(&match_key(&s.title, &s.artist)) {
                Some((id, dur)) => matched.push((id.clone(), s.at, *dur)),
                None => other += 1,
            }
        }
        page += 1;
        tokio::time::sleep(std::time::Duration::from_millis(220)).await; // Last.fm asks for <= ~5 requests/s
    }
    let added = app.lock().unwrap().store.add_scrobbles(&matched).map_err(|e| e.to_string())?;
    let cut = if total_pages > MAX_PAGES { format!(" (stopped at {MAX_PAGES} pages; the oldest history was not read)") } else { String::new() };
    Ok(format!("{} scrobbles matched your library, {added} new plays added, {other} were songs you don't have{cut}", matched.len()))
}

async fn import_loved(app: &Shared, lf: &LastFm, user: &str, votes: bool) -> Result<String, String> {
    let idx = index(app);
    let (mut seen, mut matched, mut voted, mut page, mut total_pages) = (0usize, 0usize, 0usize, 1u32, 1u32);
    while page <= total_pages && page <= MAX_PAGES {
        let p = lf.loved_tracks(user, page).await?;
        total_pages = p.total_pages.max(1);
        for s in p.items {
            seen += 1;
            if let Some((id, _)) = idx.get(&match_key(&s.title, &s.artist)) {
                matched += 1;
                if votes {
                    let a = app.lock().unwrap();
                    if !a.store.has_votes(id).unwrap_or(true) {
                        let _ = a.store.record_vote(id, s.at.max(1), true);
                        voted += 1;
                    }
                }
            }
        }
        page += 1;
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
    }
    Ok(format!("{seen} loved tracks, {matched} in your library ({voted} new +1)"))
}

async fn import_artists(app: &Shared, lf: &LastFm, user: &str) -> Result<String, String> {
    let names = lf.top_artists(user, 100).await?;
    let a = app.lock().unwrap();
    let mut all: Vec<String> = a.store.kv_get("top_artists").ok().flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    let before = all.len();
    for n in names {
        if !all.iter().any(|x| primary_artist(x).eq_ignore_ascii_case(primary_artist(&n))) {
            all.push(n);
        }
    }
    let _ = a.store.kv_set("top_artists", &serde_json::to_string(&all).unwrap());
    Ok(format!("{} new top artists saved as discovery seeds", all.len() - before))
}
