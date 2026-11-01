//! Discovery catalog: finds playable tracks by artists you do not own.
//!
//! seeds (library artists you like, weighted by feedback)
//!   -> similar artists: Last.fm (by name, needs your API key) and ListenBrainz (public, keyed by
//!      the MusicBrainz id from tag enrichment); both are used when available
//!   -> drop artists you already own or already queued
//!   -> Spotify search for each new artist's tracks
//!   -> MusicBrainz tags for them, so taste matching has signal
//!   -> `candidates` table (the discovery pool)

use crate::{Shared, now};
use rand::{Rng, SeedableRng, rngs::StdRng};
use spinamp_core::provider::MusicProvider;
use spinamp_core::{Track, primary_artist};
use spinamp_services::{listenbrainz, musicbrainz::MusicBrainz};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct Catalog {
    pub running: AtomicBool,
    pub error: Mutex<String>,
}

pub const POOL_LOW: usize = 20;
pub const POOL_FULL: usize = 60;
const SEEDS_PER_RUN: usize = 6;
const ARTISTS_PER_RUN: usize = 8;
const CACHE_SECS: i64 = 30 * 24 * 3600;

/// Pick `n` distinct keys, weighted. Pure, so it is tested.
pub fn weighted_sample<R: Rng>(items: &[(String, f64)], n: usize, rng: &mut R) -> Vec<String> {
    let mut pool: Vec<(String, f64)> = items.iter().filter(|(_, w)| *w > 0.0).cloned().collect();
    let mut out = vec![];
    while out.len() < n && !pool.is_empty() {
        let total: f64 = pool.iter().map(|p| p.1).sum();
        let mut x = rng.random_range(0.0..total);
        let mut k = pool.len() - 1;
        for (i, (_, w)) in pool.iter().enumerate() {
            if x < *w {
                k = i;
                break;
            }
            x -= w;
        }
        out.push(pool.swap_remove(k).0);
    }
    out
}

/// Combine "similar to" lists from several seeds into one score per candidate artist.
pub fn aggregate(seeds: &[(f64, Vec<(String, f64)>)], exclude: &HashSet<String>) -> Vec<(String, f64)> {
    let mut score: HashMap<String, (String, f64)> = HashMap::new();
    for (weight, similar) in seeds {
        let max = similar.iter().map(|s| s.1).fold(0.0, f64::max).max(1e-9);
        for (name, s) in similar {
            let key = name.to_lowercase();
            if exclude.contains(&key) {
                continue;
            }
            score.entry(key).or_insert_with(|| (name.clone(), 0.0)).1 += weight * (s / max);
        }
    }
    let mut v: Vec<_> = score.into_values().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

pub fn pool_size(handle: &AppHandle) -> usize {
    handle.state::<Shared>().lock().unwrap().candidates.len()
}

/// Start a refill in the background unless one is running or the pool is healthy.
pub fn maybe_refill(handle: &AppHandle, force: bool) -> bool {
    let (cat, size) = {
        let a = handle.state::<Shared>();
        let a = a.lock().unwrap();
        (a.catalog.clone(), a.candidates.len())
    };
    if (!force && size >= POOL_FULL) || cat.running.swap(true, Ordering::SeqCst) {
        return false;
    }
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let res = refill(&handle).await;
        *cat.error.lock().unwrap() = res.err().unwrap_or_default();
        cat.running.store(false, Ordering::SeqCst);
    });
    true
}

async fn refill(handle: &AppHandle) -> Result<usize, String> {
    let state = handle.state::<Shared>();

    // 1. snapshot what we need, then release the lock before any network call
    let lf = crate::lastfm::client();
    let (sp, mut seeds, mut owned, have, contact, top, blocks) = {
        let a = state.lock().unwrap();
        let (n, fb) = (now(), &a.cfg.shuffle.feedback);
        let mut weight: HashMap<String, f64> = HashMap::new();
        let mut owned: HashSet<String> = HashSet::new();
        for t in &a.library {
            let name = primary_artist(&t.track.artist).to_string();
            owned.insert(name.to_lowercase());
            *weight.entry(name).or_default() += t.votes.score(n, fb) + 0.25;
        }
        // Last.fm looks artists up by name, so every liked artist can seed; ListenBrainz needs an mbid.
        let seeds: Vec<(String, f64, Option<String>)> = weight
            .into_iter()
            .filter(|(_, w)| *w > 0.0)
            .filter_map(|(name, w)| {
                let m = a.store.artist_mbid(&name).ok().flatten();
                (m.is_some() || lf.is_some()).then_some((name, w, m))
            })
            .collect();
        let top: Vec<String> = a.store.kv_get("top_artists").ok().flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        (a.spotify.clone(), seeds, owned, a.store.candidate_artists().unwrap_or_default(), a.settings.contact(), top, a.blocklist.clone())
    };
    if !sp.is_authenticated().await {
        return Err("Connect Spotify first.".into());
    }
    // Your Spotify top artists: strong seeds, and never recommended back to you.
    let mb = MusicBrainz::new(contact.as_deref());
    let mut looked_up = 0;
    for (rank, name) in top.iter().enumerate() {
        owned.insert(name.to_lowercase());
        let mbid = {
            let a = state.lock().unwrap();
            a.store.artist_mbid(name).ok().flatten()
        };
        let mbid = match mbid {
            Some(m) => Some(m),
            None if looked_up < 3 && state.lock().unwrap().store.get_artist(name).ok().flatten().is_none() => {
                looked_up += 1;
                let found = mb.lookup_artist(name).await.ok().flatten();
                let _ = state.lock().unwrap().store.save_artist(name, found.as_ref(), now());
                tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
                found.and_then(|f| f.mbid)
            }
            None => None,
        };
        if (mbid.is_some() || lf.is_some()) && !seeds.iter().any(|s| s.0.eq_ignore_ascii_case(name)) {
            seeds.push((name.clone(), 2.0 * (1.0 - 0.6 * rank as f64 / top.len().max(1) as f64), mbid));
        }
    }
    if seeds.is_empty() {
        return Err("No tagged artists yet. Let tag enrichment finish (or add a Last.fm key in Settings), then try again.".into());
    }

    // 2. choose seeds (weighted, different every run)
    let mut rng = StdRng::from_os_rng();
    let by_name: HashMap<&str, (&f64, &Option<String>)> = seeds.iter().map(|(n, w, m)| (n.as_str(), (w, m))).collect();
    let chosen = weighted_sample(&seeds.iter().map(|(n, w, _)| (n.clone(), *w)).collect::<Vec<_>>(), SEEDS_PER_RUN, &mut rng);

    // 3. similar artists per seed (cached 30 days)
    let mut lists: Vec<(f64, Vec<(String, f64)>)> = vec![];
    for name in &chosen {
        let (w, mbid) = by_name[name.as_str()];
        if let Some(mbid) = mbid {
            let key = format!("lb:{mbid}");
            let cached: Option<(i64, Vec<(String, f64)>)> =
                state.lock().unwrap().store.kv_get(&key).ok().flatten().and_then(|j| serde_json::from_str(&j).ok());
            let list = match cached {
                Some((at, l)) if now() - at < CACHE_SECS => Some(l),
                _ => match listenbrainz::similar_artists(mbid).await {
                    Ok(s) => {
                        let l: Vec<(String, f64)> = s.into_iter().map(|x| (x.name, x.score)).collect();
                        let _ = state.lock().unwrap().store.kv_set(&key, &serde_json::to_string(&(now(), &l)).unwrap());
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        Some(l)
                    }
                    Err(_) => None,
                },
            };
            if let Some(l) = list {
                lists.push((*w, l));
            }
        }
        if let Some(lf) = &lf {
            let key = format!("lf:{}", name.to_lowercase());
            let cached: Option<(i64, Vec<(String, f64)>)> =
                state.lock().unwrap().store.kv_get(&key).ok().flatten().and_then(|j| serde_json::from_str(&j).ok());
            let list = match cached {
                Some((at, l)) if now() - at < CACHE_SECS => Some(l),
                _ => match lf.similar_artists(name, 50).await {
                    Ok(l) => {
                        let _ = state.lock().unwrap().store.kv_set(&key, &serde_json::to_string(&(now(), &l)).unwrap());
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        Some(l)
                    }
                    Err(_) => None,
                },
            };
            if let Some(l) = list {
                lists.push((*w, l));
            }
        }
    }

    // 4. rank artists we do not own and have not queued, then sample a handful
    let mut exclude = owned;
    exclude.extend(have);
    exclude.extend(blocks.artists.iter().cloned()); // never suggest a blocked artist
    let ranked = aggregate(&lists, &exclude);
    let picks = weighted_sample(&ranked.iter().take(40).cloned().collect::<Vec<_>>(), ARTISTS_PER_RUN, &mut rng);

    // 5. find playable tracks + tags for each
    let mut added = 0;
    for name in picks {
        let miss_key = format!("cand_miss:{}", name.to_lowercase());
        if state.lock().unwrap().store.kv_get(&miss_key).ok().flatten().is_some() {
            continue;
        }
        let found = sp.search(&format!("artist:\"{}\"", name.replace('"', "")), 10).await.unwrap_or_default();
        let mut seen_titles = HashSet::new();
        let mut tracks: Vec<Track> = found
            .into_iter()
            .filter(|t| primary_artist(&t.artist).eq_ignore_ascii_case(&name))
            .filter(|t| seen_titles.insert(t.title.to_lowercase()))
            .take(3)
            .collect();
        if tracks.is_empty() {
            let _ = state.lock().unwrap().store.kv_set(&miss_key, "1");
            continue;
        }
        let info = mb.lookup_artist(&name).await.ok().flatten();
        if let Some(i) = &info {
            let tags: Vec<String> = i.tags.iter().take(6).map(|t| t.0.clone()).collect();
            for t in &mut tracks {
                t.tags = tags.clone();
            }
        }
        tracks.retain(|t| !blocks.blocks(t)); // blocked tags/genres/songs
        if tracks.is_empty() {
            continue;
        }
        {
            let a = state.lock().unwrap();
            let _ = a.store.save_artist(&name, info.as_ref(), now());
            let seed = chosen.first().cloned().unwrap_or_default();
            added += tracks.len();
            let _ = a.store.add_candidates(&tracks.into_iter().map(|t| (t, seed.clone())).collect::<Vec<_>>(), now());
        }
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }

    let mut a = state.lock().unwrap();
    a.candidates = a.store.load_candidates().map_err(|e| e.to_string())?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_weights_and_excludes_owned() {
        let seeds = vec![
            (2.0, vec![("Beach House".to_string(), 100.0), ("Owned".to_string(), 90.0)]),
            (1.0, vec![("beach house".to_string(), 50.0), ("Other".to_string(), 80.0)]),
        ];
        let ex: HashSet<String> = ["owned".to_string()].into();
        let r = aggregate(&seeds, &ex);
        assert_eq!(r[0].0, "Beach House");
        assert!((r[0].1 - 2.625).abs() < 1e-9, "got {}", r[0].1);
        assert!(r.iter().all(|x| x.0 != "Owned"));
    }

    #[test]
    fn weighted_sample_is_distinct_and_skips_zero() {
        let items = vec![("a".to_string(), 5.0), ("b".to_string(), 0.0), ("c".to_string(), 1.0)];
        let mut rng = StdRng::seed_from_u64(4);
        let s = weighted_sample(&items, 5, &mut rng);
        assert_eq!(s.len(), 2);
        assert!(!s.contains(&"b".to_string()));
    }
}
