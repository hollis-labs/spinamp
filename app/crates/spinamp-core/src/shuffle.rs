//! Weighted shuffle. Not uniform random: it respects what you like, what you just heard,
//! and who you just heard.

use crate::feedback::FeedbackConfig;
use crate::{TrackId, TrackState, Ts};
use rand::Rng;

#[derive(Debug, Clone)]
pub struct ShuffleConfig {
    pub feedback: FeedbackConfig,
    /// Tracks played within the last N picks are excluded outright.
    pub no_repeat_window: usize,
    /// Seconds for the "recently played" penalty to fade to ~63%.
    pub recency_tau_secs: f64,
    /// Multiplier when the candidate's artist matches the previous pick.
    pub same_artist_penalty: f64,
    /// Multiplier when the artist appears in the last 3 picks.
    pub recent_artist_penalty: f64,
    /// Boost for rarely-played tracks (0 disables). Keeps the long tail alive.
    pub novelty_strength: f64,
}

impl Default for ShuffleConfig {
    fn default() -> Self {
        Self {
            feedback: FeedbackConfig::default(),
            no_repeat_window: 20,
            recency_tau_secs: 6.0 * 3600.0,
            same_artist_penalty: 0.02,
            recent_artist_penalty: 0.4,
            novelty_strength: 0.3,
        }
    }
}

/// Multiplier on a starred track's chance of being picked.
pub const FAVORITE_HEAT: f64 = 1.8;

/// Weight of one track for the next pick. `None` = ineligible (cooldown / just played).
pub fn weight(
    t: &TrackState,
    history: &[TrackId],
    recent_artists: &[&str],
    now: Ts,
    cfg: &ShuffleConfig,
) -> Option<f64> {
    let skip = history.len().saturating_sub(cfg.no_repeat_window);
    if history[skip..].contains(&t.track.id) {
        return None;
    }
    if t.votes.cooldown_until(now, &cfg.feedback).is_some() || t.muted_until.is_some_and(|u| u > now) {
        return None;
    }
    let mut w = t.votes.play_weight(now, &cfg.feedback);
    if t.favorite {
        w *= FAVORITE_HEAT; // starred: permanently hotter, on top of any votes
    }

    if let Some(last) = t.last_played {
        let age = (now - last).max(0) as f64;
        w *= 1.0 - (-age / cfg.recency_tau_secs).exp() + 0.02;
    }
    if recent_artists.first() == Some(&t.track.artist.as_str()) {
        w *= cfg.same_artist_penalty;
    } else if recent_artists.contains(&t.track.artist.as_str()) {
        w *= cfg.recent_artist_penalty;
    }
    w *= 1.0 / (1.0 + cfg.novelty_strength * (1.0 + t.play_count as f64).ln());
    // Rating nudges: 3 stars neutral.
    if let Some(r) = t.rating {
        w *= 1.0 + 0.15 * (r as f64 - 3.0);
    }
    Some(w.max(0.0))
}

/// Pick an index from `pool` by weight. `history` is oldest -> newest played ids.
pub fn pick<R: Rng>(
    pool: &[TrackState],
    history: &[TrackId],
    now: Ts,
    cfg: &ShuffleConfig,
    rng: &mut R,
) -> Option<usize> {
    let recent_artists: Vec<&str> = history
        .iter()
        .rev()
        .take(3)
        .filter_map(|id| pool.iter().find(|t| &t.track.id == id))
        .map(|t| t.track.artist.as_str())
        .collect();
    let weights: Vec<f64> = pool
        .iter()
        .map(|t| weight(t, history, &recent_artists, now, cfg).unwrap_or(0.0))
        .collect();
    weighted_index(&weights, rng)
}

pub(crate) fn weighted_index<R: Rng>(weights: &[f64], rng: &mut R) -> Option<usize> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut x = rng.random_range(0.0..total);
    for (i, w) in weights.iter().enumerate() {
        if x < *w {
            return Some(i);
        }
        x -= w;
    }
    weights.iter().rposition(|w| *w > 0.0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::Track;
    use rand::{SeedableRng, rngs::StdRng};

    pub fn ts(n: u32, artist: &str) -> TrackState {
        TrackState::new(
            Track {
                id: TrackId::new("test", &n.to_string()),
                provider: "test".into(),
                title: format!("t{n}"),
                artist: artist.into(),
                album: None,
                track_number: Some(n),
                duration_ms: 200_000,
                tempo: Some(120.0),
                energy: Some(0.5),
                tags: vec![],
                meta: Default::default(),
            },
            0,
        )
    }

    #[test]
    fn liked_beats_disliked_beats_nothing() {
        let now = 100 * 86_400;
        let cfg = ShuffleConfig::default();
        let mut liked = ts(1, "a");
        let mut meh = ts(2, "b");
        let mut disliked = ts(3, "c");
        for d in 0..3 {
            liked.votes.record(now - (d + 10) * 86_400, true);
        }
        meh.play_count = 0;
        disliked.votes.record(now - 3600, false);
        let pool = vec![liked, meh, disliked];
        let mut rng = StdRng::seed_from_u64(7);
        let mut counts = [0usize; 3];
        for _ in 0..3000 {
            counts[pick(&pool, &[], now, &cfg, &mut rng).unwrap()] += 1;
        }
        assert!(counts[0] > counts[1], "{counts:?}");
        assert_eq!(counts[2], 0, "cooling-down track must not play: {counts:?}");
    }

    #[test]
    fn favorites_run_hotter_and_muted_tracks_never_play() {
        let now = 1_000_000;
        let cfg = ShuffleConfig::default();
        let mut fav = ts(1, "a");
        fav.favorite = true;
        let plain = ts(2, "b");
        let mut muted = ts(3, "c");
        muted.muted_until = Some(now + 3600);
        let pool = vec![fav, plain, muted];
        let mut rng = StdRng::seed_from_u64(11);
        let mut counts = [0usize; 3];
        for _ in 0..3000 {
            counts[pick(&pool, &[], now, &cfg, &mut rng).unwrap()] += 1;
        }
        assert!(counts[0] as f64 > counts[1] as f64 * 1.4, "favorite should be picked clearly more: {counts:?}");
        assert_eq!(counts[2], 0, "muted never picked: {counts:?}");
        // mute over: eligible again
        let mut later = pool.clone();
        later[2].muted_until = Some(now - 1);
        assert!(weight(&later[2], &[], &[], now, &cfg).is_some());
    }

    #[test]
    fn avoids_same_artist_back_to_back() {
        let now = 1_000_000;
        let cfg = ShuffleConfig::default();
        let pool = vec![ts(1, "x"), ts(2, "x"), ts(3, "y")];
        let history = vec![pool[0].track.id.clone()];
        let mut rng = StdRng::seed_from_u64(1);
        let mut y = 0;
        for _ in 0..500 {
            if pick(&pool, &history, now, &cfg, &mut rng) == Some(2) {
                y += 1;
            }
        }
        assert!(y > 450, "expected y almost always, got {y}/500");
    }

    #[test]
    fn never_repeats_within_window() {
        let now = 1_000_000;
        let cfg = ShuffleConfig { no_repeat_window: 5, ..Default::default() };
        let pool: Vec<_> = (0..6).map(|i| ts(i, &format!("a{i}"))).collect();
        let mut history = vec![];
        let mut rng = StdRng::seed_from_u64(3);
        for _ in 0..30 {
            let i = pick(&pool, &history, now, &cfg, &mut rng).unwrap();
            let id = pool[i].track.id.clone();
            let skip = history.len().saturating_sub(5);
            assert!(!history[skip..].contains(&id));
            history.push(id);
        }
    }
}
