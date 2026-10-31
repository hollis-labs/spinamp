//! Mode selection: Standard, Shuffle, Discovery, Shuffle+Discovery.

use crate::discovery::{self, TasteProfile};
use crate::shuffle::{self, ShuffleConfig};
use crate::{Track, TrackId, TrackState, Ts};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
    /// Playlist order.
    Standard,
    /// Weighted shuffle over the playlist.
    Shuffle,
    /// Only tracks you do not have, matched to your taste.
    Discovery,
    /// Shuffle with discovery tracks mixed in at `discovery_ratio`.
    ShuffleDiscovery,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub shuffle: ShuffleConfig,
    /// Share of picks that are discovery in `ShuffleDiscovery` (0..=1).
    pub discovery_ratio: f64,
    /// Higher = discovery sticks closer to your taste; lower = more surprise.
    pub discovery_sharpness: f64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { shuffle: ShuffleConfig::default(), discovery_ratio: 0.25, discovery_sharpness: 2.0 }
    }
}

#[derive(Debug, Clone)]
pub enum Pick<'a> {
    Library(&'a TrackState),
    Discovery(&'a Track),
}

pub struct Context<'a> {
    pub now: Ts,
    /// Played ids, oldest -> newest.
    pub history: &'a [TrackId],
    /// Index in `pool` of what is playing now (Standard mode).
    pub current: Option<usize>,
    pub recently_suggested: &'a [TrackId],
    /// The WHOLE library: used to learn taste and to exclude owned tracks from discovery,
    /// even when `pool` is just one playlist.
    pub library: &'a [TrackState],
}

pub fn next<'a, R: Rng>(
    mode: PlayMode,
    pool: &'a [TrackState],
    candidates: &'a [Track],
    ctx: &Context<'_>,
    cfg: &EngineConfig,
    rng: &mut R,
) -> Option<Pick<'a>> {
    let discover = |rng: &mut R| {
        let profile = TasteProfile::learn(ctx.library, ctx.now, &cfg.shuffle.feedback);
        let owned: HashSet<TrackId> = ctx.library.iter().map(|t| t.track.id.clone()).collect();
        discovery::pick(candidates, &owned, ctx.recently_suggested, &profile, cfg.discovery_sharpness, rng)
            .map(|i| Pick::Discovery(&candidates[i]))
    };
    let shuffled = |rng: &mut R| {
        shuffle::pick(pool, ctx.history, ctx.now, &cfg.shuffle, rng).map(|i| Pick::Library(&pool[i]))
    };

    match mode {
        PlayMode::Standard => {
            if pool.is_empty() {
                return None;
            }
            let i = ctx.current.map_or(0, |c| (c + 1) % pool.len());
            Some(Pick::Library(&pool[i]))
        }
        PlayMode::Shuffle => shuffled(rng),
        PlayMode::Discovery => discover(rng).or_else(|| shuffled(rng)),
        PlayMode::ShuffleDiscovery => {
            if rng.random_bool(cfg.discovery_ratio.clamp(0.0, 1.0)) {
                discover(rng).or_else(|| shuffled(rng))
            } else {
                shuffled(rng).or_else(|| discover(rng))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shuffle::tests::ts;
    use rand::{SeedableRng, rngs::StdRng};

    fn ctx<'a>(history: &'a [TrackId], current: Option<usize>) -> Context<'a> {
        Context { now: 1_000_000, history, current, recently_suggested: &[], library: &[] }
    }

    #[test]
    fn standard_walks_in_order_and_wraps() {
        let pool: Vec<_> = (0..3).map(|i| ts(i, "a")).collect();
        let cfg = EngineConfig::default();
        let mut rng = StdRng::seed_from_u64(0);
        let mut at = |c| match next(PlayMode::Standard, &pool, &[], &ctx(&[], c), &cfg, &mut rng) {
            Some(Pick::Library(t)) => t.track.title.clone(),
            _ => panic!(),
        };
        assert_eq!(at(None), "t0");
        assert_eq!(at(Some(0)), "t1");
        assert_eq!(at(Some(2)), "t0");
    }

    #[test]
    fn mix_honours_ratio() {
        let pool: Vec<_> = (0..10).map(|i| ts(i, &format!("a{i}"))).collect();
        let cands: Vec<_> = (0..10).map(|i| ts(100 + i, "new").track).collect();
        let cfg = EngineConfig { discovery_ratio: 0.3, ..Default::default() };
        let mut rng = StdRng::seed_from_u64(9);
        let mut disc = 0;
        for _ in 0..1000 {
            if matches!(
                next(PlayMode::ShuffleDiscovery, &pool, &cands, &ctx(&[], None), &cfg, &mut rng),
                Some(Pick::Discovery(_))
            ) {
                disc += 1;
            }
        }
        assert!((240..360).contains(&disc), "~30% expected, got {disc}/1000");
    }

    #[test]
    fn discovery_falls_back_when_no_candidates() {
        let pool: Vec<_> = (0..3).map(|i| ts(i, &format!("a{i}"))).collect();
        let mut rng = StdRng::seed_from_u64(2);
        let p = next(PlayMode::Discovery, &pool, &[], &ctx(&[], None), &EngineConfig::default(), &mut rng);
        assert!(matches!(p, Some(Pick::Library(_))));
    }
}
