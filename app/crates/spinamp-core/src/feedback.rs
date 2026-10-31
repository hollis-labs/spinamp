//! Thumbs up / down with memory.
//!
//! Rules:
//! - Repeating the same vote **escalates** it: the Nth consecutive same-direction vote
//!   weighs `1 + streak_gain * (N-1)`, capped at `streak_cap`.
//! - **Velocity**: a vote landing within `burst_window_secs` of the previous one is
//!   treated as a double-tap and counts only `burst_damp`, with no escalation.
//! - Votes **decay** with a half-life, so old opinions fade.
//! - A thumbs-down starts a **cooldown** that doubles with each consecutive down.

use crate::Ts;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vote {
    pub at: Ts,
    pub up: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackConfig {
    pub half_life_days: f64,
    pub streak_gain: f64,
    pub streak_cap: f64,
    pub burst_window_secs: i64,
    pub burst_damp: f64,
    pub down_cooldown_base_secs: i64,
    pub down_cooldown_max_secs: i64,
    /// Steepness of score -> play-weight multiplier.
    pub weight_slope: f64,
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        Self {
            half_life_days: 90.0,
            streak_gain: 0.5,
            streak_cap: 3.0,
            burst_window_secs: 5,
            burst_damp: 0.2,
            down_cooldown_base_secs: 24 * 3600,
            down_cooldown_max_secs: 90 * 24 * 3600,
            weight_slope: 0.5,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedbackLedger {
    votes: Vec<Vote>,
}

impl FeedbackLedger {
    pub fn record(&mut self, at: Ts, up: bool) {
        self.votes.push(Vote { at, up });
        self.votes.sort_by_key(|v| v.at);
    }

    pub fn votes(&self) -> &[Vote] {
        &self.votes
    }

    /// Signed opinion strength at `now`. Positive = liked, negative = disliked.
    pub fn score(&self, now: Ts, cfg: &FeedbackConfig) -> f64 {
        let mut total = 0.0;
        let mut streak = 0u32;
        let mut prev: Option<Vote> = None;
        for v in &self.votes {
            let burst = prev.is_some_and(|p| v.at - p.at <= cfg.burst_window_secs);
            let same_dir = prev.is_some_and(|p| p.up == v.up);
            streak = if same_dir { streak + 1 } else { 1 };
            let weight = if burst {
                cfg.burst_damp
            } else {
                (1.0 + cfg.streak_gain * (streak as f64 - 1.0)).min(cfg.streak_cap)
            };
            let age_days = (now - v.at).max(0) as f64 / 86_400.0;
            let decay = 0.5f64.powf(age_days / cfg.half_life_days);
            total += if v.up { weight } else { -weight } * decay;
            prev = Some(*v);
        }
        total
    }

    /// Multiplier applied to a track's chance of being picked. 1.0 = neutral.
    pub fn play_weight(&self, now: Ts, cfg: &FeedbackConfig) -> f64 {
        (cfg.weight_slope * self.score(now, cfg)).exp().clamp(0.05, 4.0)
    }

    /// If the track is currently cooling down after thumbs-down, when it becomes eligible.
    pub fn cooldown_until(&self, now: Ts, cfg: &FeedbackConfig) -> Option<Ts> {
        let last = self.votes.last()?;
        if last.up {
            return None;
        }
        let downs = self.votes.iter().rev().take_while(|v| !v.up).count() as u32;
        let secs = cfg
            .down_cooldown_base_secs
            .saturating_mul(1i64 << (downs - 1).min(20))
            .min(cfg.down_cooldown_max_secs);
        let until = last.at + secs;
        (until > now).then_some(until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger(votes: &[(Ts, bool)]) -> FeedbackLedger {
        let mut l = FeedbackLedger::default();
        for &(t, up) in votes {
            l.record(t, up);
        }
        l
    }

    #[test]
    fn repeated_votes_escalate_then_cap() {
        let c = FeedbackConfig::default();
        let day = 86_400;
        let one = ledger(&[(0, true)]).score(0, &c);
        let two = ledger(&[(0, true), (day, true)]).score(day, &c);
        let many: Vec<_> = (0..10).map(|i| (i * day, true)).collect();
        let ten = ledger(&many).score(9 * day, &c);
        assert!(two > one * 2.0 - 0.01, "second vote counts more than the first");
        assert!(ten < 10.0 * c.streak_cap, "weight per vote is capped");
        assert!(ten > two);
    }

    #[test]
    fn burst_taps_are_damped() {
        let c = FeedbackConfig::default();
        let spam = ledger(&[(0, true), (1, true), (2, true)]).score(2, &c);
        assert!(spam < 1.5, "triple tap within seconds ~ one vote, got {spam}");
    }

    #[test]
    fn opposite_vote_resets_streak() {
        let c = FeedbackConfig::default();
        let day = 86_400;
        let l = ledger(&[(0, true), (day, true), (2 * day, false)]);
        assert!(l.score(2 * day, &c) < l.score(day, &c));
    }

    #[test]
    fn votes_decay() {
        let c = FeedbackConfig::default();
        let l = ledger(&[(0, true)]);
        let later = (c.half_life_days as i64) * 86_400;
        assert!((l.score(later, &c) - 0.5).abs() < 0.01);
    }

    #[test]
    fn down_cooldown_doubles_and_up_clears() {
        let c = FeedbackConfig::default();
        let day = 86_400;
        let one = ledger(&[(0, false)]);
        assert_eq!(one.cooldown_until(10, &c), Some(day));
        let two = ledger(&[(0, false), (day * 2, false)]);
        assert_eq!(two.cooldown_until(day * 2 + 10, &c), Some(day * 2 + 2 * day));
        let cleared = ledger(&[(0, false), (10, true)]);
        assert_eq!(cleared.cooldown_until(11, &c), None);
        assert_eq!(one.cooldown_until(day + 1, &c), None);
    }
}
