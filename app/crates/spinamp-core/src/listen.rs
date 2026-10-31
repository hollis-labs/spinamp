//! Turns periodic playback observations into listening sessions.
//!
//! The provider only tells us "track X, playing, at position P". This state machine
//! works out how long you actually listened and how each session ended:
//! `Finished` (played through), `Skipped` (left early), `Replaced` (we/you started
//! something else), or `Stopped`. Pure and clock-free: callers pass timestamps.

use crate::TrackId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Finished,
    Skipped,
    Stopped,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            EndReason::Finished => "finished",
            EndReason::Skipped => "skipped",
            EndReason::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    pub at_ms: i64,
    pub track: Option<&'a TrackId>,
    pub is_playing: bool,
    pub position_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Session still running; total listened so far.
    Progress { listened_ms: u64 },
    /// Session ended.
    Ended { track: TrackId, listened_ms: u64, position_ms: u64, reason: EndReason },
    /// Same track started over (repeat): caller should open a new play row.
    Restarted { track: TrackId },
    /// Playback shows a track we did not start (changed in the Spotify app).
    External { track: TrackId },
}

#[derive(Debug, Clone)]
struct Session {
    track: TrackId,
    duration_ms: u64,
    listened_ms: u64,
    last_at: i64,
    last_pos: u64,
    last_playing: bool,
}

#[derive(Debug, Default)]
pub struct Tracker {
    s: Option<Session>,
}

/// Longest gap we credit as listening between two observations (laptop sleep etc.).
const MAX_CREDIT_MS: i64 = 5_000;
const FINISH_FRACTION: f64 = 0.9;

impl Tracker {
    pub fn current(&self) -> Option<&TrackId> {
        self.s.as_ref().map(|s| &s.track)
    }

    /// We commanded playback of `track`. Closes any session in progress.
    pub fn begin(&mut self, track: TrackId, duration_ms: u64, at_ms: i64) -> Option<Event> {
        let ended = self.close(None);
        self.s = Some(Session { track, duration_ms, listened_ms: 0, last_at: at_ms, last_pos: 0, last_playing: true });
        ended
    }

    fn close(&mut self, forced: Option<EndReason>) -> Option<Event> {
        let s = self.s.take()?;
        let reason = forced.unwrap_or(if s.duration_ms > 0 && s.last_pos as f64 >= FINISH_FRACTION * s.duration_ms as f64 {
            EndReason::Finished
        } else {
            EndReason::Skipped
        });
        Some(Event::Ended { track: s.track, listened_ms: s.listened_ms, position_ms: s.last_pos, reason })
    }

    pub fn observe(&mut self, o: Observation<'_>) -> Vec<Event> {
        let mut out = vec![];
        let Some(s) = self.s.as_mut() else {
            if let (Some(t), true) = (o.track, o.is_playing) {
                out.push(Event::External { track: t.clone() });
            }
            return out;
        };

        if o.track != Some(&s.track) {
            // Gave time to start? A just-begun session has not appeared in playback yet.
            let fresh = s.listened_ms == 0 && s.last_pos == 0 && o.at_ms - s.last_at < 8_000;
            if fresh {
                return out;
            }
            out.extend(self.close(if o.track.is_none() { Some(EndReason::Stopped) } else { None }));
            if let (Some(t), true) = (o.track, o.is_playing) {
                out.push(Event::External { track: t.clone() });
            }
            return out;
        }

        // Same track: repeat detected by a big jump back after nearly finishing.
        if s.duration_ms > 0
            && s.last_pos as f64 >= 0.8 * s.duration_ms as f64
            && o.position_ms + 3_000 < s.last_pos
        {
            let t = s.track.clone();
            let dur = s.duration_ms;
            out.extend(self.close(Some(EndReason::Finished)));
            self.s = Some(Session { track: t.clone(), duration_ms: dur, listened_ms: 0, last_at: o.at_ms, last_pos: o.position_ms, last_playing: o.is_playing });
            out.push(Event::Restarted { track: t });
            return out;
        }

        let dt = (o.at_ms - s.last_at).clamp(0, MAX_CREDIT_MS) as u64;
        if s.last_playing && o.is_playing {
            s.listened_ms += dt;
        }
        s.last_at = o.at_ms;
        s.last_pos = o.position_ms;
        s.last_playing = o.is_playing;

        // Playback ran off the end and went idle at/near position 0 or the end.
        let near_end = s.duration_ms > 0 && s.last_pos as f64 >= FINISH_FRACTION * s.duration_ms as f64;
        if !o.is_playing && near_end {
            out.extend(self.close(Some(EndReason::Finished)));
        } else {
            out.push(Event::Progress { listened_ms: s.listened_ms });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> TrackId {
        TrackId::new("t", n)
    }
    fn obs<'a>(at: i64, t: Option<&'a TrackId>, playing: bool, pos: u64) -> Observation<'a> {
        Observation { at_ms: at, track: t, is_playing: playing, position_ms: pos }
    }

    #[test]
    fn accumulates_only_while_playing() {
        let a = id("a");
        let mut t = Tracker::default();
        t.begin(a.clone(), 200_000, 0);
        t.observe(obs(2_000, Some(&a), true, 2_000));
        t.observe(obs(4_000, Some(&a), true, 4_000));
        t.observe(obs(6_000, Some(&a), false, 4_000)); // paused
        let ev = t.observe(obs(8_000, Some(&a), false, 4_000));
        assert_eq!(ev, [Event::Progress { listened_ms: 4_000 }]);
        t.observe(obs(10_000, Some(&a), true, 4_000));
        let ev = t.observe(obs(12_000, Some(&a), true, 6_000));
        assert_eq!(ev, [Event::Progress { listened_ms: 6_000 }]);
    }

    #[test]
    fn early_replace_is_skipped_and_late_is_finished() {
        let (a, b) = (id("a"), id("b"));
        let mut t = Tracker::default();
        t.begin(a.clone(), 200_000, 0);
        t.observe(obs(2_000, Some(&a), true, 2_000));
        match t.begin(b.clone(), 100_000, 3_000) {
            Some(Event::Ended { reason, listened_ms, .. }) => assert_eq!((reason, listened_ms), (EndReason::Skipped, 2_000)),
            e => panic!("{e:?}"),
        }
        t.observe(obs(60_000, Some(&b), true, 95_000));
        match t.begin(a.clone(), 200_000, 61_000) {
            Some(Event::Ended { reason, .. }) => assert_eq!(reason, EndReason::Finished),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn idle_at_end_finishes() {
        let a = id("a");
        let mut t = Tracker::default();
        t.begin(a.clone(), 10_000, 0);
        t.observe(obs(5_000, Some(&a), true, 5_000));
        t.observe(obs(9_500, Some(&a), true, 9_500));
        let ev = t.observe(obs(11_000, Some(&a), false, 9_900));
        assert!(matches!(ev[0], Event::Ended { reason: EndReason::Finished, .. }), "{ev:?}");
        assert!(t.current().is_none());
    }

    #[test]
    fn external_change_ends_session_and_is_reported() {
        let (a, x) = (id("a"), id("x"));
        let mut t = Tracker::default();
        t.begin(a.clone(), 200_000, 0);
        t.observe(obs(3_000, Some(&a), true, 3_000));
        let ev = t.observe(obs(20_000, Some(&x), true, 500));
        assert!(matches!(ev[0], Event::Ended { reason: EndReason::Skipped, .. }));
        assert_eq!(ev[1], Event::External { track: x });
    }

    #[test]
    fn startup_lag_is_not_a_skip() {
        let (a, old) = (id("a"), id("old"));
        let mut t = Tracker::default();
        t.begin(a.clone(), 200_000, 0);
        assert!(t.observe(obs(1_000, Some(&old), true, 90_000)).is_empty());
        assert_eq!(t.current(), Some(&a));
    }

    #[test]
    fn repeat_opens_new_session() {
        let a = id("a");
        let mut t = Tracker::default();
        t.begin(a.clone(), 10_000, 0);
        t.observe(obs(9_000, Some(&a), true, 9_000));
        let ev = t.observe(obs(11_000, Some(&a), true, 500));
        assert!(matches!(ev[0], Event::Ended { reason: EndReason::Finished, .. }));
        assert_eq!(ev[1], Event::Restarted { track: a });
    }
}
