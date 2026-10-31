//! What must never (or, for now, not yet) be played automatically: blocked songs, artists and
//! tags/genres (permanent) and muted tracks (until a date).

use crate::{Track, TrackState, Ts};
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct Blocklist {
    /// Track keys: the provider id, or `isrc:<ISRC>` (so a blocked recording stays blocked on any provider).
    pub songs: HashSet<String>,
    /// Lowercase artist names; matches any credited artist.
    pub artists: HashSet<String>,
    /// Lowercase tags, which includes genres.
    pub tags: HashSet<String>,
}

impl Blocklist {
    pub fn add(&mut self, kind: &str, value: &str) {
        let v = value.trim();
        match kind {
            "song" => self.songs.insert(v.to_string()),
            "artist" => self.artists.insert(v.to_lowercase()),
            "tag" | "genre" => self.tags.insert(v.to_lowercase()),
            _ => false,
        };
    }

    pub fn from_rows(rows: &[(String, String)]) -> Self {
        let mut b = Self::default();
        for (k, v) in rows {
            b.add(k, v);
        }
        b
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty() && self.artists.is_empty() && self.tags.is_empty()
    }

    pub fn blocks(&self, t: &Track) -> bool {
        if self.songs.contains(&t.id.0) {
            return true;
        }
        if let Some(i) = t.meta.isrc.as_deref().filter(|i| !i.trim().is_empty()) {
            if self.songs.contains(&format!("isrc:{}", i.trim().to_uppercase())) {
                return true;
            }
        }
        t.artist.split(", ").any(|a| self.artists.contains(&a.trim().to_lowercase())) || t.tags.iter().any(|x| self.tags.contains(&x.to_lowercase()))
    }
}

/// May the engine pick this track on its own right now?
pub fn playable(t: &TrackState, blocks: &Blocklist, now: Ts) -> bool {
    !blocks.blocks(&t.track) && t.muted_until.is_none_or(|u| u <= now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TrackId, TrackMeta};

    fn t(artist: &str, tags: &[&str], isrc: Option<&str>) -> TrackState {
        TrackState::new(
            Track {
                id: TrackId::new("spotify", "1"), provider: "spotify".into(), title: "x".into(), artist: artist.into(), album: None, track_number: None,
                duration_ms: 1, tempo: None, energy: None, tags: tags.iter().map(|s| s.to_string()).collect(),
                meta: TrackMeta { isrc: isrc.map(String::from), ..Default::default() },
            },
            0,
        )
    }

    #[test]
    fn blocks_by_song_artist_and_tag_case_insensitively() {
        let mut b = Blocklist::default();
        assert!(playable(&t("A", &["rock"], None), &b, 0));
        b.add("artist", "Slowdive");
        b.add("tag", "Polka");
        b.add("song", "isrc:USX123");
        assert!(!playable(&t("Slowdive, Guest", &[], None), &b, 0), "any credited artist");
        assert!(!playable(&t("A", &["polka", "rock"], None), &b, 0), "genre/tag");
        assert!(!playable(&t("A", &[], Some("usx123")), &b, 0), "same recording via ISRC");
        assert!(playable(&t("A", &["rock"], Some("OTHER")), &b, 0));
    }

    #[test]
    fn mute_expires() {
        let mut s = t("A", &[], None);
        s.muted_until = Some(100);
        let b = Blocklist::default();
        assert!(!playable(&s, &b, 99));
        assert!(playable(&s, &b, 100), "mute is over at the deadline");
    }
}
