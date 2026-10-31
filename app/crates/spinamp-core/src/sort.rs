use crate::feedback::FeedbackConfig;
use crate::{TrackState, Ts};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Artist,
    Title,
    Album,
    TrackNumber,
    Duration,
    Added,
    Updated,
    /// Personal weight from thumbs feedback.
    Weight,
    /// Explicit star rating.
    Rating,
    Plays,
    Tempo,
    Energy,
    Loudness,
}

fn cmp_f(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Stable sort; ties fall back to artist, album, track number so output is deterministic.
pub fn sort(tracks: &mut [TrackState], key: SortKey, descending: bool, now: Ts, fb: &FeedbackConfig) {
    let lc = |s: &str| s.to_lowercase();
    tracks.sort_by(|a, b| {
        let primary = match key {
            SortKey::Artist => lc(&a.track.artist).cmp(&lc(&b.track.artist)),
            SortKey::Title => lc(&a.track.title).cmp(&lc(&b.track.title)),
            SortKey::Album => a.track.album.as_deref().map(lc).cmp(&b.track.album.as_deref().map(lc)),
            SortKey::TrackNumber => a.track.track_number.cmp(&b.track.track_number),
            SortKey::Duration => a.track.duration_ms.cmp(&b.track.duration_ms),
            SortKey::Added => a.added_at.cmp(&b.added_at),
            SortKey::Updated => a.updated_at.cmp(&b.updated_at),
            SortKey::Weight => {
                let w = |t: &TrackState| t.votes.score(now, fb) + if t.favorite { 2.0 } else { 0.0 };
                cmp_f(w(a), w(b))
            }
            SortKey::Rating => a.rating.cmp(&b.rating),
            SortKey::Plays => a.play_count.cmp(&b.play_count),
            SortKey::Tempo => a.track.tempo.map(|x| x as i64).cmp(&b.track.tempo.map(|x| x as i64)),
            SortKey::Energy => cmp_f(a.track.energy.unwrap_or(-1.0) as f64, b.track.energy.unwrap_or(-1.0) as f64),
            SortKey::Loudness => cmp_f(
                a.track.meta.audio.as_ref().map_or(-99.0, |x| x.loudness_db as f64),
                b.track.meta.audio.as_ref().map_or(-99.0, |x| x.loudness_db as f64),
            ),
        };
        let primary = if descending { primary.reverse() } else { primary };
        primary
            .then_with(|| lc(&a.track.artist).cmp(&lc(&b.track.artist)))
            .then_with(|| a.track.album.as_deref().map(lc).cmp(&b.track.album.as_deref().map(lc)))
            .then_with(|| a.track.track_number.cmp(&b.track.track_number))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shuffle::tests::ts;

    #[test]
    fn sorts_by_weight_desc() {
        let fb = FeedbackConfig::default();
        let mut a = ts(1, "a");
        let mut b = ts(2, "b");
        let c = ts(3, "c");
        a.votes.record(0, false);
        b.votes.record(0, true);
        let mut v = vec![a, c, b];
        sort(&mut v, SortKey::Weight, true, 10, &fb);
        let names: Vec<_> = v.iter().map(|t| t.track.artist.as_str()).collect();
        assert_eq!(names, ["b", "c", "a"]);
    }

    #[test]
    fn artist_sort_is_case_insensitive_with_track_tiebreak() {
        let fb = FeedbackConfig::default();
        let mut v = vec![ts(2, "beta"), ts(9, "Alpha"), ts(1, "alpha")];
        sort(&mut v, SortKey::Artist, false, 0, &fb);
        let ids: Vec<_> = v.iter().map(|t| t.track.track_number.unwrap()).collect();
        assert_eq!(ids, [1, 9, 2]);
    }
}
