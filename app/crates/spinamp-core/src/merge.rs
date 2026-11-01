//! One row per recording. The same song on Spotify and Apple Music has different ids but the
//! same ISRC, so rows sharing an ISRC are folded into one: the earliest-added copy is primary,
//! the rest become `alts`. Nothing is deleted or rewritten in the store; this is a view, so
//! imports stay idempotent and every provider's own data is kept.
use crate::{Track, TrackId, TrackState};
use std::collections::HashMap;

/// ISRC with case and hyphens normalised; None when the track has none.
pub fn isrc_of(t: &Track) -> Option<String> {
    let i: String = t.meta.isrc.as_deref()?.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    (!i.is_empty()).then_some(i)
}

/// Fold same-ISRC rows together. Order follows each recording's first appearance.
pub fn merge_by_isrc(states: Vec<TrackState>) -> Vec<TrackState> {
    let mut out: Vec<TrackState> = Vec::with_capacity(states.len());
    let mut by_isrc: HashMap<String, usize> = HashMap::new();
    for s in states {
        let Some(key) = isrc_of(&s.track) else {
            out.push(s);
            continue;
        };
        match by_isrc.get(&key) {
            None => {
                by_isrc.insert(key, out.len());
                out.push(s);
            }
            Some(&i) => fold(&mut out[i], s),
        }
    }
    out
}

fn fold(into: &mut TrackState, other: TrackState) {
    into.alts.push(other.track.clone());
    into.alts.extend(other.alts);
    into.play_count += other.play_count;
    into.last_played = into.last_played.max(other.last_played);
    into.updated_at = into.updated_at.max(other.updated_at);
    into.rating = into.rating.max(other.rating);
    into.muted_until = into.muted_until.max(other.muted_until);
    into.favorite |= other.favorite;
    for v in other.votes.votes() {
        into.votes.record(v.at, v.up);
    }
    // Fill gaps in the primary from the other copy.
    let t = &mut into.track;
    let o = &other.track;
    t.tempo = t.tempo.or(o.tempo);
    t.energy = t.energy.or(o.energy);
    t.meta.audio = t.meta.audio.take().or_else(|| o.meta.audio.clone());
    t.meta.image_url = t.meta.image_url.take().or_else(|| o.meta.image_url.clone());
    t.album = t.album.take().or_else(|| o.album.clone());
    for tag in &o.tags {
        if !t.tags.contains(tag) {
            t.tags.push(tag.clone());
        }
    }
}

/// Maps every non-primary copy's id to its primary's id.
pub fn alias_map(states: &[TrackState]) -> HashMap<TrackId, TrackId> {
    let mut m = HashMap::new();
    for s in states {
        for a in &s.alts {
            m.insert(a.id.clone(), s.track.id.clone());
        }
    }
    m
}

/// Order in which to try a recording's copies: the chosen provider first, then the one already
/// playing (avoids a switch), then the rest as given. Only providers in `usable` are kept.
pub fn playback_order<'a>(variants: impl Iterator<Item = &'a Track>, preferred: &str, active: Option<&str>, usable: &[String]) -> Vec<Track> {
    let mut v: Vec<Track> = variants.filter(|t| usable.contains(&t.provider)).cloned().collect();
    let rank = |t: &Track| -> u8 {
        if !preferred.is_empty() && t.provider == preferred {
            0
        } else if active == Some(t.provider.as_str()) {
            1
        } else {
            2
        }
    };
    v.sort_by_key(rank); // stable: ties keep primary-first order
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackMeta;

    fn st(provider: &str, native: &str, isrc: Option<&str>, added: i64) -> TrackState {
        let t = Track {
            id: TrackId::new(provider, native),
            provider: provider.into(),
            title: "Alison".into(),
            artist: "Slowdive".into(),
            album: None,
            track_number: None,
            duration_ms: 231_000,
            tempo: None,
            energy: None,
            tags: vec![],
            meta: TrackMeta { isrc: isrc.map(String::from), ..Default::default() },
        };
        TrackState::new(t, added)
    }

    #[test]
    fn same_isrc_folds_into_earliest_and_keeps_order() {
        let merged = merge_by_isrc(vec![
            st("spotify", "a", Some("GBBQY0400035"), 1),
            st("spotify", "z", Some("OTHER"), 2),
            st("applemusic", "b", Some("gb-bqy-04-00035"), 3),
            st("applemusic", "n", None, 4),
        ]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].track.id.0, "spotify:a");
        assert_eq!(merged[0].alts.iter().map(|t| t.id.0.as_str()).collect::<Vec<_>>(), vec!["applemusic:b"]);
        assert_eq!(merged[0].sources(), vec!["spotify", "applemusic"]);
        assert_eq!(merged[2].track.id.0, "applemusic:n", "no ISRC: never merged");
    }

    #[test]
    fn stats_votes_and_flags_combine() {
        let mut a = st("spotify", "a", Some("X1"), 1);
        a.play_count = 2;
        a.votes.record(100, true);
        a.last_played = Some(50);
        let mut b = st("applemusic", "b", Some("X1"), 2);
        b.play_count = 3;
        b.votes.record(200, true);
        b.favorite = true;
        b.rating = Some(4);
        b.muted_until = Some(999);
        b.last_played = Some(80);
        b.track.tempo = Some(120.0);
        b.track.tags = vec!["shoegaze".into()];
        let m = merge_by_isrc(vec![a, b]).remove(0);
        assert_eq!((m.play_count, m.votes.votes().len(), m.favorite, m.rating, m.muted_until, m.last_played), (5, 2, true, Some(4), Some(999), Some(80)));
        assert_eq!((m.track.tempo, m.track.tags.as_slice()), (Some(120.0), &["shoegaze".to_string()][..]));
    }

    #[test]
    fn alias_maps_alternates_to_primary() {
        let m = merge_by_isrc(vec![st("spotify", "a", Some("X"), 1), st("applemusic", "b", Some("X"), 2), st("deezer", "c", Some("X"), 3)]);
        let al = alias_map(&m);
        assert_eq!(al.get(&TrackId::new("applemusic", "b")), Some(&TrackId::new("spotify", "a")));
        assert_eq!(al.get(&TrackId::new("deezer", "c")), Some(&TrackId::new("spotify", "a")));
        assert!(al.get(&TrackId::new("spotify", "a")).is_none());
    }

    #[test]
    fn playback_order_prefers_choice_then_active_and_drops_unusable() {
        let m = merge_by_isrc(vec![st("spotify", "a", Some("X"), 1), st("applemusic", "b", Some("X"), 2)]).remove(0);
        let usable = vec!["spotify".to_string(), "applemusic".to_string()];
        let ids = |v: Vec<Track>| v.into_iter().map(|t| t.provider).collect::<Vec<_>>();
        assert_eq!(ids(playback_order(m.variants(), "applemusic", None, &usable)), vec!["applemusic", "spotify"]);
        assert_eq!(ids(playback_order(m.variants(), "", Some("applemusic"), &usable)), vec!["applemusic", "spotify"]);
        assert_eq!(ids(playback_order(m.variants(), "", None, &usable)), vec!["spotify", "applemusic"]);
        assert_eq!(ids(playback_order(m.variants(), "applemusic", None, &["spotify".to_string()])), vec!["spotify"]);
    }
}
