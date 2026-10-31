//! Lyrics, artist profiles, and importing your Spotify history.
use crate::{Shared, now, reload};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use spinamp_core::stats;
use spinamp_core::{Track, TrackId, primary_artist};
use spinamp_services::lrclib::Lyrics;
use spinamp_services::lyrics as lyric_search;
use spinamp_services::musicbrainz::MusicBrainz;
use spinamp_services::{deezer, wikipedia};
use tauri::State;

const DAY: i64 = 86_400;

fn kv_get<T: DeserializeOwned>(app: &Shared, key: &str) -> Option<T> {
    let j = app.lock().unwrap().store.kv_get(key).ok().flatten()?;
    serde_json::from_str(&j).ok()
}

fn kv_put<T: Serialize>(app: &Shared, key: &str, v: &T) {
    let _ = app.lock().unwrap().store.kv_set(key, &serde_json::to_string(v).unwrap());
}

/* ---------------- lyrics ---------------- */

#[derive(Serialize, Deserialize)]
struct CachedLyrics {
    at: i64,
    lyrics: Option<Lyrics>,
}

/// Lyrics for a track: LRCLIB (synced when available), lyrics.ovh as backup. Hits are cached forever, misses for a week.
#[tauri::command]
pub async fn lyrics(app: State<'_, Shared>, track: Track) -> Result<Option<Lyrics>, String> {
    let key = format!("lyrics:{}", track.id.0);
    if let Some(c) = kv_get::<CachedLyrics>(&app, &key) {
        if c.lyrics.is_some() || now() - c.at < 7 * DAY {
            return Ok(c.lyrics);
        }
    }
    let artist = primary_artist(&track.artist).to_string();
    let found = lyric_search::find(&artist, &track.title, track.album.as_deref(), track.duration_ms).await?;
    kv_put(&app, &key, &CachedLyrics { at: now(), lyrics: found.clone() });
    Ok(found)
}

/* ---------------- artist profile ---------------- */

#[derive(Serialize, Deserialize, Default, Clone)]
struct ArtistStatic {
    name: String,
    mbid: Option<String>,
    description: Option<String>,
    extract: Option<String>,
    wiki_url: Option<String>,
    image: Option<String>,
    country: Option<String>,
    area: Option<String>,
    kind: Option<String>,
    begin_year: Option<i32>,
    tags: Vec<String>,
    links: Vec<(String, String)>,
    members: Vec<String>,
    fans: Option<u64>,
    deezer_url: Option<String>,
    top_tracks: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct CachedProfile {
    at: i64,
    data: ArtistStatic,
}

#[derive(Serialize)]
pub struct ArtistProfile {
    #[serde(flatten)]
    info: ArtistStatic,
    /// Your own numbers for this artist.
    tracks: usize,
    plays: u32,
    listens: u32,
}

async fn fetch_artist_static(app: &Shared, name: &str) -> ArtistStatic {
    let (cached_info, contact) = {
        let a = app.lock().unwrap();
        (a.store.get_artist(name).ok().flatten(), a.settings.contact())
    };
    let mb = MusicBrainz::new(contact.as_deref());
    let mut info = cached_info;
    if info.is_none() {
        if let Ok(found) = mb.lookup_artist(name).await {
            let _ = app.lock().unwrap().store.save_artist(name, found.as_ref(), now());
            info = found;
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        }
    }
    let mut st = ArtistStatic { name: name.to_string(), ..Default::default() };
    if let Some(i) = &info {
        st.mbid = i.mbid.clone();
        st.country = i.country.clone();
        st.kind = i.kind.clone();
        st.begin_year = i.begin_year;
        st.tags = i.tags.iter().take(8).map(|t| t.0.clone()).collect();
    }
    let mut wiki_title = None;
    if let Some(mbid) = &st.mbid {
        if let Ok(d) = mb.artist_details(mbid).await {
            wiki_title = d.links.iter().find(|l| l.0 == "wikipedia").and_then(|l| wikipedia::title_from_url(&l.1));
            st.area = d.area;
            st.members = d.members;
            st.links = d.links;
        }
    }
    if let Ok(Some(w)) = wikipedia::artist_summary(name, wiki_title.as_deref()).await {
        st.description = w.description;
        st.extract = Some(w.extract);
        st.wiki_url = w.url;
        st.image = w.thumbnail;
    }
    if let Ok(Some(d)) = deezer::artist(name).await {
        st.fans = Some(d.fans);
        st.deezer_url = d.link;
        st.top_tracks = d.top_tracks;
        if st.image.is_none() {
            st.image = d.picture;
        }
    }
    st
}

#[tauri::command]
pub async fn artist_profile(app: State<'_, Shared>, name: String) -> Result<ArtistProfile, String> {
    let name = primary_artist(&name).to_string();
    let key = format!("profile:{}", name.to_lowercase());
    let info = match kv_get::<CachedProfile>(&app, &key) {
        Some(c) if now() - c.at < 30 * DAY => c.data,
        _ => {
            let st = fetch_artist_static(&app, &name).await;
            let found_something = st.mbid.is_some() || st.extract.is_some() || st.fans.is_some();
            if found_something {
                kv_put(&app, &key, &CachedProfile { at: now(), data: st.clone() });
            }
            st
        }
    };
    let a = app.lock().unwrap();
    let mine: Vec<&TrackId> =
        a.library.iter().filter(|t| primary_artist(&t.track.artist).eq_ignore_ascii_case(&name)).map(|t| &t.track.id).collect();
    let counts = stats::tally(&a.store.play_events().unwrap_or_default(), &a.stats_cfg);
    let (plays, listens) = mine.iter().filter_map(|id| counts.get(*id)).fold((0, 0), |(p, l), c| (p + c.plays, l + c.listens));
    Ok(ArtistProfile { info, tracks: mine.len(), plays, listens })
}

/* ---------------- spotify history import ---------------- */

fn friendly(e: impl ToString) -> String {
    let e = e.to_string();
    if e.contains("Insufficient client scope") {
        "Spotify needs new permissions. Open the SPOTIFY tab, press RECONNECT, approve, then try again.".into()
    } else {
        e
    }
}

/// Merge-import tracks and playlist (add only). Returns how many entries were new to the playlist.
fn save_as_playlist(a: &mut crate::App, name: &str, remote: &str, tracks: &[Track]) -> Result<usize, String> {
    let n = now();
    a.store.add_tracks(tracks, n).map_err(|e| e.to_string())?;
    let ids: Vec<TrackId> = tracks.iter().map(|t| t.id.clone()).collect();
    let saved = a.store.save_playlist(name, Some(remote), &ids, n).map_err(|e| e.to_string())?;
    Ok(saved.added)
}

/// kind: liked | top | recent | artists | all.  `votes`: count Liked Songs as a +1 (dated when you saved them).
#[tauri::command]
pub async fn spotify_import_history(app: State<'_, Shared>, kind: String, votes: bool) -> Result<String, String> {
    let sp = app.lock().unwrap().spotify.clone();
    let kinds: Vec<&str> = if kind == "all" { vec!["liked", "top", "recent", "artists"] } else { vec![kind.as_str()] };
    let mut notes: Vec<String> = vec![];

    for k in kinds {
        match k {
            "liked" => {
                let items = sp.saved_tracks(5000).await.map_err(friendly)?;
                let tracks: Vec<Track> = items.iter().map(|i| i.track.clone()).collect();
                let mut a = app.lock().unwrap();
                save_as_playlist(&mut a, "Liked Songs", "saved", &tracks)?;
                let mut voted = 0;
                if votes {
                    for it in &items {
                        if !a.store.has_votes(&it.track.id).unwrap_or(true) {
                            let _ = a.store.record_vote(&it.track.id, it.at.max(1), true);
                            voted += 1;
                        }
                    }
                }
                notes.push(format!("{} liked songs ({voted} new +1)", tracks.len()));
            }
            "top" => {
                let mut total = 0;
                for (range, label) in [("short_term", "4 Weeks"), ("medium_term", "6 Months"), ("long_term", "All Time")] {
                    let tracks = sp.top_tracks(range, 50).await.map_err(friendly)?;
                    total += tracks.len();
                    save_as_playlist(&mut app.lock().unwrap(), &format!("Top Tracks · {label}"), &format!("top_{range}"), &tracks)?;
                }
                notes.push(format!("{total} top tracks in 3 playlists"));
            }
            "recent" => {
                let items = sp.recently_played(50).await.map_err(friendly)?;
                let tracks: Vec<Track> = items.iter().map(|i| i.track.clone()).collect();
                let mut a = app.lock().unwrap();
                save_as_playlist(&mut a, "Recently Played", "recent", &tracks)?;
                let plays: Vec<_> = items.iter().map(|i| (i.track.id.clone(), i.at, i.track.duration_ms)).collect();
                let added = a.store.add_history_plays(&plays).map_err(|e| e.to_string())?;
                notes.push(format!("{} recent tracks ({added} plays backfilled)", tracks.len()));
            }
            "artists" => {
                let mut names: Vec<String> = vec![];
                for range in ["long_term", "medium_term", "short_term"] {
                    for n in sp.top_artists(range, 50).await.map_err(friendly)? {
                        if !names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                            names.push(n);
                        }
                    }
                }
                kv_put(&app, "top_artists", &names);
                notes.push(format!("{} top artists saved as discovery seeds", names.len()));
            }
            _ => return Err(format!("unknown import kind {k}")),
        }
    }
    crate::persist_tokens(&app, &*sp).await;
    let mut a = app.lock().unwrap();
    let _ = a.store.tag_from_artist_cache();
    reload(&mut a)?;
    Ok(notes.join("; "))
}
