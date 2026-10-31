//! Plays vs listens.
//!
//! - **play**: the track was started.
//! - **listen**: it was played to at least `listen_threshold` of its length.
//! - Events that start inside quiet hours (default 23:00-07:00 local, for overnight
//!   listening) are excluded.

use crate::{TrackId, Ts};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayEvent {
    pub track: TrackId,
    pub started_at: Ts,
    pub listened_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct StatsConfig {
    pub listen_threshold: f64,
    /// Local hours, start inclusive, end exclusive; may wrap midnight.
    pub quiet_hours: Option<(u32, u32)>,
    pub utc_offset_secs: i64,
    pub quiet_applies_to_plays: bool,
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self { listen_threshold: 0.8, quiet_hours: Some((23, 7)), utc_offset_secs: 0, quiet_applies_to_plays: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub plays: u32,
    pub listens: u32,
}

impl StatsConfig {
    pub fn is_quiet(&self, at: Ts) -> bool {
        let Some((start, end)) = self.quiet_hours else { return false };
        let hour = ((at + self.utc_offset_secs).rem_euclid(86_400) / 3600) as u32;
        if start <= end { (start..end).contains(&hour) } else { hour >= start || hour < end }
    }
}

pub fn tally(events: &[PlayEvent], cfg: &StatsConfig) -> HashMap<TrackId, Counts> {
    let mut out: HashMap<TrackId, Counts> = HashMap::new();
    for e in events {
        let quiet = cfg.is_quiet(e.started_at);
        let c = out.entry(e.track.clone()).or_default();
        if !quiet || !cfg.quiet_applies_to_plays {
            c.plays += 1;
        }
        let finished = e.duration_ms > 0 && e.listened_ms as f64 / e.duration_ms as f64 >= cfg.listen_threshold;
        if finished && !quiet {
            c.listens += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(hour: i64, listened: u64) -> PlayEvent {
        PlayEvent { track: TrackId::new("t", "1"), started_at: hour * 3600, listened_ms: listened, duration_ms: 100 }
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        let c = StatsConfig::default();
        assert!(c.is_quiet(23 * 3600));
        assert!(c.is_quiet(3 * 3600));
        assert!(!c.is_quiet(7 * 3600));
        assert!(!c.is_quiet(12 * 3600));
    }

    #[test]
    fn plays_vs_listens_and_overnight_exclusion() {
        let events = [ev(12, 100), ev(13, 30), ev(2, 100)];
        let got = tally(&events, &StatsConfig::default())[&TrackId::new("t", "1")];
        assert_eq!(got, Counts { plays: 2, listens: 1 });
    }

    #[test]
    fn timezone_offset_applies() {
        let c = StatsConfig { utc_offset_secs: -8 * 3600, ..Default::default() };
        assert!(c.is_quiet(8 * 3600)); // 08:00 UTC = 00:00 local
    }
}
