//! Prompt-to-playlist. Two stages so the AI part is optional and the result is checkable:
//!
//! 1. **Intent**: turn free text into a structured [`Intent`]. `parse_heuristic` does it
//!    offline (durations + a mood/tag keyword table); an LLM can fill the same struct.
//! 2. **Generate**: deterministic selection from the library: tag fit x taste weight,
//!    artist caps, artist spacing, then trimmed to land on the target length.

use crate::feedback::FeedbackConfig;
use crate::{TrackState, Ts, primary_artist};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Intent {
    pub name: String,
    /// Exact length to hit, if asked for.
    pub target_minutes: Option<f64>,
    /// Tracks matching any of these tags are favoured (empty = no tag preference).
    #[serde(default)]
    pub include_tags: Vec<String>,
    /// Tracks with any of these tags are excluded.
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    /// Max tracks per artist (default 2).
    #[serde(default)]
    pub max_per_artist: Option<usize>,
    /// Only tracks that match a tag (strict) vs. just prefer them (default false).
    #[serde(default)]
    pub strict: bool,
    /// BPM range. Tracks without measured tempo are neutral, not excluded.
    #[serde(default)]
    pub tempo_min: Option<f32>,
    #[serde(default)]
    pub tempo_max: Option<f32>,
    /// Energy range on the 0..1 scale (0.3 quiet, 0.5 moderate, 0.7+ intense).
    #[serde(default)]
    pub energy_min: Option<f32>,
    #[serde(default)]
    pub energy_max: Option<f32>,
    /// Preferred mood words (uplifting, energetic, aggressive, dark, calm, peaceful, melancholic,
    /// somber, dreamy, danceable, upbeat, moody, balanced). Matching tracks are favoured.
    #[serde(default)]
    pub moods: Vec<String>,
    /// Avoid tracks whose loudest moments jump more than this many dB above their typical level.
    #[serde(default)]
    pub max_spike_db: Option<f32>,
}

impl Intent {
    pub fn has_audio_constraints(&self) -> bool {
        self.tempo_min.is_some() || self.tempo_max.is_some() || self.energy_min.is_some() || self.energy_max.is_some() || self.max_spike_db.is_some() || !self.moods.is_empty()
    }
}

/// 1.0 inside the wanted range, decaying outside it (Gaussian in units of `scale`).
fn range_fit(v: f32, lo: Option<f32>, hi: Option<f32>, scale: f32) -> f64 {
    let viol = match (lo, hi) {
        (Some(l), _) if v < l => l - v,
        (_, Some(h)) if v > h => v - h,
        _ => 0.0,
    };
    (-((viol / scale) as f64).powi(2)).exp()
}

/// How well a track's *measured* audio fits the intent. Unmeasured tracks get 0.5 so that,
/// when constraints exist, analysed matches win but nothing is shut out.
pub fn audio_fit(t: &crate::Track, i: &Intent) -> f64 {
    if !i.has_audio_constraints() {
        return 1.0;
    }
    let mut fit = 1.0;
    let mut known = false;
    if let Some(bpm) = t.tempo {
        known = true;
        fit *= range_fit(bpm, i.tempo_min, i.tempo_max, 12.0);
    }
    if let Some(e) = t.energy {
        known = true;
        fit *= range_fit(e, i.energy_min, i.energy_max, 0.12);
    }
    if let (Some(max), Some(a)) = (i.max_spike_db, t.meta.audio.as_ref()) {
        known = true;
        fit *= range_fit(a.spike_db, None, Some(max), 4.0);
    }
    if !i.moods.is_empty() {
        if let Some(a) = t.meta.audio.as_ref().filter(|a| !a.moods.is_empty()) {
            known = true;
            fit *= if a.moods.iter().any(|m| i.moods.iter().any(|w| w.eq_ignore_ascii_case(m))) { 1.4 } else { 0.7 };
        }
    }
    if known { fit } else { 0.5 }
}

/// Mood/situation words -> tags likely to appear in MusicBrainz data.
const VIBES: &[(&str, &[&str])] = &[
    ("chill", &["ambient", "downtempo", "trip hop", "dream pop", "chillout", "lo-fi", "ethereal"]),
    ("relax", &["ambient", "downtempo", "chillout", "new age", "dream pop", "ethereal"]),
    ("focus", &["ambient", "idm", "electronica", "post-rock", "minimal"]),
    ("study", &["ambient", "lo-fi", "idm", "post-rock", "classical"]),
    ("sleep", &["ambient", "drone", "new age", "ethereal"]),
    ("workout", &["electronic", "hip hop", "techno", "trance", "metal", "drum and bass", "punk"]),
    ("energy", &["electronic", "rock", "punk", "techno", "drum and bass", "hard rock"]),
    ("party", &["dance", "electronic", "house", "hip hop", "pop", "disco"]),
    ("sad", &["shoegaze", "indie folk", "slowcore", "singer-songwriter", "dream pop"]),
    ("melancholy", &["shoegaze", "indie folk", "slowcore", "dream pop", "post-rock"]),
    ("happy", &["pop", "indie pop", "funk", "disco", "synthpop"]),
    ("dark", &["industrial", "darkwave", "post-punk", "gothic rock", "dark ambient", "metal"]),
    ("dreamy", &["dream pop", "shoegaze", "ethereal", "ambient"]),
    ("drive", &["rock", "indie rock", "electronic", "synthpop", "alternative rock"]),
    ("commute", &["indie rock", "electronic", "synthpop", "alternative rock", "dream pop", "trip hop"]),
    ("road trip", &["rock", "indie rock", "country", "folk", "alternative rock"]),
    ("dinner", &["jazz", "soul", "bossa nova", "downtempo", "acoustic"]),
    ("acoustic", &["acoustic", "folk", "indie folk", "singer-songwriter"]),
    ("night", &["synthwave", "trip hop", "downtempo", "electronica", "ambient"]),
    ("morning", &["indie pop", "folk", "pop", "acoustic", "indie folk"]),
];

/// Read "45 minutes", "1h", "1.5 hours", "90 min" out of a prompt.
pub fn parse_duration_minutes(text: &str) -> Option<f64> {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split(|c: char| !(c.is_alphanumeric() || c == '.')).filter(|w| !w.is_empty()).collect();
    for (i, w) in words.iter().enumerate() {
        // "45min" / "2h" glued
        let digits: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if digits.is_empty() || !digits.chars().next().unwrap().is_ascii_digit() {
            continue;
        }
        let Ok(n) = digits.parse::<f64>() else { continue };
        let unit = if w.len() > digits.len() { &w[digits.len()..] } else { words.get(i + 1).copied().unwrap_or("") };
        if unit.starts_with("h") {
            return Some(n * 60.0);
        }
        if unit.starts_with("min") || unit == "m" {
            return Some(n);
        }
    }
    None
}

/// (keyword, tempo_min, tempo_max, energy_min, energy_max)
const AUDIO_VIBES: &[(&str, Option<f32>, Option<f32>, Option<f32>, Option<f32>)] = &[
    ("chill", None, Some(115.0), None, Some(0.5)),
    ("relax", None, Some(110.0), None, Some(0.45)),
    ("sleep", None, Some(100.0), None, Some(0.35)),
    ("calm", None, Some(110.0), None, Some(0.4)),
    ("mellow", None, Some(115.0), None, Some(0.45)),
    ("slow", None, Some(95.0), None, None),
    ("focus", None, None, Some(0.2), Some(0.6)),
    ("study", None, None, Some(0.15), Some(0.55)),
    ("workout", Some(120.0), Some(185.0), Some(0.6), None),
    ("run", Some(135.0), Some(185.0), Some(0.55), None),
    ("party", Some(110.0), Some(140.0), Some(0.6), None),
    ("upbeat", Some(110.0), None, Some(0.55), None),
    ("hype", Some(120.0), None, Some(0.65), None),
    ("intense", None, None, Some(0.65), None),
    ("fast", Some(125.0), None, None, None),
];

/// Pull "120 bpm" / "around 90 bpm" out of a prompt.
pub fn parse_bpm(text: &str) -> Option<f32> {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split(|c: char| !(c.is_alphanumeric() || c == '.')).filter(|w| !w.is_empty()).collect();
    for (i, w) in words.iter().enumerate() {
        if let Some(n) = w.strip_suffix("bpm").and_then(|n| n.parse::<f32>().ok()) {
            return Some(n);
        }
        if words.get(i + 1) == Some(&"bpm") {
            if let Ok(n) = w.parse::<f32>() {
                return Some(n);
            }
        }
    }
    None
}

pub fn parse_heuristic(prompt: &str) -> Intent {
    let t = prompt.to_lowercase();
    let mut tags: Vec<String> = vec![];
    for (word, ts) in VIBES {
        if t.contains(word) {
            for x in *ts {
                if !tags.iter().any(|y| y == x) {
                    tags.push((*x).into());
                }
            }
        }
    }
    let name = {
        let cleaned: String = prompt.trim().chars().take(40).collect();
        if cleaned.is_empty() { "Generated".into() } else { cleaned }
    };
    let mut intent = Intent { name, target_minutes: parse_duration_minutes(prompt), include_tags: tags, ..Default::default() };
    for (word, tmin, tmax, emin, emax) in AUDIO_VIBES {
        if t.split(|c: char| !c.is_alphanumeric()).any(|w| w == *word) {
            intent.tempo_min = match (intent.tempo_min, *tmin) { (Some(a), Some(b)) => Some(a.max(b)), (a, b) => a.or(b) };
            intent.tempo_max = match (intent.tempo_max, *tmax) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
            intent.energy_min = match (intent.energy_min, *emin) { (Some(a), Some(b)) => Some(a.max(b)), (a, b) => a.or(b) };
            intent.energy_max = match (intent.energy_max, *emax) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
        }
    }
    // mood words -> the mood labels the analyser produces
    for (word, moods) in [
        ("happy", &["uplifting", "upbeat"][..]), ("sad", &["melancholic", "somber"]), ("calm", &["calm", "peaceful"]),
        ("relax", &["calm", "peaceful"]), ("dark", &["dark", "aggressive", "moody"]), ("angry", &["aggressive"]),
        ("dance", &["danceable", "energetic"]), ("party", &["danceable", "energetic", "uplifting"]), ("dreamy", &["dreamy"]),
        ("melancholy", &["melancholic", "somber"]), ("moody", &["moody", "somber"]),
    ] {
        if t.split(|c: char| !c.is_alphanumeric()).any(|w| w == word) {
            for m in moods {
                if !intent.moods.iter().any(|x| x == m) {
                    intent.moods.push((*m).into());
                }
            }
        }
    }
    if let Some(bpm) = parse_bpm(prompt) {
        intent.tempo_min = Some(bpm - 8.0);
        intent.tempo_max = Some(bpm + 8.0);
    }
    if ["no spikes", "no loud", "steady volume", "consistent volume", "even volume", "no sudden", "without spikes"].iter().any(|k| t.contains(k)) {
        intent.max_spike_db = Some(6.0);
    }
    intent
}

fn tag_fit(track_tags: &[String], want: &[String]) -> f64 {
    if want.is_empty() {
        return 1.0;
    }
    let hits = track_tags.iter().filter(|t| want.iter().any(|w| w.eq_ignore_ascii_case(t))).count();
    hits as f64
}

#[derive(Debug, Clone, Serialize)]
pub struct Generated {
    /// Indices into the pool, in play order.
    pub picks: Vec<usize>,
    pub total_ms: u64,
    pub target_ms: Option<u64>,
    /// Tracks that matched the requested tags.
    pub matched: usize,
}

pub fn generate<R: Rng>(pool: &[TrackState], intent: &Intent, now: Ts, fb: &FeedbackConfig, rng: &mut R) -> Generated {
    let mut cap = intent.max_per_artist.unwrap_or(2).max(1);
    let target_ms = intent.target_minutes.map(|m| (m * 60_000.0) as u64);

    // 1. score every eligible track
    let mut scored: Vec<(usize, f64, bool)> = vec![];
    for (i, t) in pool.iter().enumerate() {
        if t.votes.cooldown_until(now, fb).is_some() || t.votes.score(now, fb) < -1.0 {
            continue;
        }
        if t.track.tags.iter().any(|x| intent.exclude_tags.iter().any(|e| e.eq_ignore_ascii_case(x))) {
            continue;
        }
        let fit = tag_fit(&t.track.tags, &intent.include_tags);
        let matched = intent.include_tags.is_empty() || fit > 0.0;
        if intent.strict && !matched {
            continue;
        }
        let audio = audio_fit(&t.track, intent);
        if intent.strict && audio < 0.05 {
            continue;
        }
        // matched tracks dominate, but unmatched stay possible so short libraries still fill
        let base = if matched { 1.0 + fit } else { 0.15 };
        scored.push((i, base * audio * t.votes.play_weight(now, fb), matched));
    }

    // 2. weighted draw without replacement, honouring the per-artist cap and spacing
    let mut remaining = scored.clone();
    let mut picks: Vec<usize> = vec![];
    let mut per_artist: HashMap<&str, usize> = HashMap::new();
    let mut total = 0u64;
    let limit = target_ms.unwrap_or(u64::MAX);
    let mut last_artist: Option<&str> = None;
    let mut skipped: Vec<(usize, f64, bool)> = vec![];
    while !remaining.is_empty() && (target_ms.is_some() || picks.len() < 25) {
        let weights: Vec<f64> = remaining
            .iter()
            .map(|(i, w, _)| {
                let a = primary_artist(&pool[*i].track.artist);
                if per_artist.get(a).copied().unwrap_or(0) >= cap {
                    0.0
                } else if Some(a) == last_artist {
                    w * 0.05
                } else {
                    *w
                }
            })
            .collect();
        let sum: f64 = weights.iter().sum();
        if sum <= 0.0 {
            // Artist caps are exhausted. If we are still short of the target, loosen the cap
            // rather than return a playlist that is too short.
            if target_ms.is_some() && total + 20_000 < limit && !remaining.is_empty() {
                cap += 1;
                continue;
            }
            break;
        }
        let mut x = rng.random_range(0.0..sum);
        let mut k = weights.len() - 1;
        for (j, w) in weights.iter().enumerate() {
            if x < *w {
                k = j;
                break;
            }
            x -= w;
        }
        let (i, w, m) = remaining.swap_remove(k);
        let dur = pool[i].track.duration_ms;
        if total + dur > limit + 20_000 {
            skipped.push((i, w, m)); // does not fit now; stays available for the final fit
            continue;
        }
        let a = primary_artist(&pool[i].track.artist);
        *per_artist.entry(a).or_default() += 1;
        last_artist = Some(a);
        total += dur;
        picks.push(i);
        if let Some(t) = target_ms {
            if total + 20_000 >= t {
                break;
            }
        }
    }

    remaining.extend(skipped);

    // 3. land on the target: local search over replace / add / drop moves until no move gets closer
    if let Some(t) = target_ms {
        let dur = |i: usize| pool[i].track.duration_ms;
        let artist_of = |i: usize| primary_artist(&pool[i].track.artist);
        for _ in 0..60 {
            let cur = total.abs_diff(t);
            if cur <= 5_000 {
                break;
            }
            // (new_total, move)
            let mut best: Option<(u64, usize, Option<usize>, Option<usize>)> = None; // (diff, pick_idx_or_MAX, cand, _)
            let mut consider = |diff: u64, slot: usize, cand: Option<usize>| {
                if diff < best.map_or(cur, |b| b.0) {
                    best = Some((diff, slot, cand, None));
                }
            };
            for (slot, &p) in picks.iter().enumerate() {
                if total > t {
                    consider((total - dur(p)).abs_diff(t), slot, None); // drop
                }
                for &(c, _, _) in &remaining {
                    let count = per_artist.get(artist_of(c)).copied().unwrap_or(0) - (artist_of(c) == artist_of(p)) as usize;
                    if count < cap {
                        consider((total - dur(p) + dur(c)).abs_diff(t), slot, Some(c)); // replace
                    }
                }
            }
            if total < t {
                for &(c, _, _) in &remaining {
                    if per_artist.get(artist_of(c)).copied().unwrap_or(0) < cap {
                        consider((total + dur(c)).abs_diff(t), usize::MAX, Some(c)); // add
                    }
                }
            }
            let Some((_, slot, cand, _)) = best else { break };
            match (slot, cand) {
                (usize::MAX, Some(c)) => {
                    picks.push(c);
                    total += dur(c);
                    *per_artist.entry(artist_of(c)).or_default() += 1;
                    remaining.retain(|r| r.0 != c);
                }
                (slot, Some(c)) => {
                    let old = picks[slot];
                    total = total - dur(old) + dur(c);
                    *per_artist.entry(artist_of(old)).or_default() -= 1;
                    *per_artist.entry(artist_of(c)).or_default() += 1;
                    picks[slot] = c;
                    remaining.retain(|r| r.0 != c);
                    remaining.push((old, 0.0, false));
                }
                (slot, None) => {
                    let old = picks.remove(slot);
                    total -= dur(old);
                    *per_artist.entry(artist_of(old)).or_default() -= 1;
                    remaining.push((old, 0.0, false));
                }
            }
        }
    }

    let matched = picks.iter().filter(|i| tag_fit(&pool[**i].track.tags, &intent.include_tags) > 0.0).count();
    Generated { picks, total_ms: total, target_ms, matched }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioFeatures, Track, TrackId, TrackMeta};
    use rand::{SeedableRng, rngs::StdRng};

    fn t(n: u32, artist: &str, tags: &[&str], secs: u64) -> TrackState {
        TrackState::new(
            Track {
                id: TrackId::new("t", &n.to_string()), provider: "t".into(), title: format!("t{n}"), artist: artist.into(),
                album: None, track_number: None, duration_ms: secs * 1000, tempo: None, energy: None,
                tags: tags.iter().map(|s| s.to_string()).collect(), meta: TrackMeta::default(),
            },
            0,
        )
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration_minutes("a 45 minute commute"), Some(45.0));
        assert_eq!(parse_duration_minutes("chill 1.5 hours"), Some(90.0));
        assert_eq!(parse_duration_minutes("2h run"), Some(120.0));
        assert_eq!(parse_duration_minutes("30min focus"), Some(30.0));
        assert_eq!(parse_duration_minutes("just vibes"), None);
    }

    #[test]
    fn heuristic_maps_vibes_to_tags() {
        let i = parse_heuristic("chill 45 minute commute");
        assert_eq!(i.target_minutes, Some(45.0));
        assert!(i.include_tags.iter().any(|x| x == "downtempo") && i.include_tags.iter().any(|x| x == "synthpop"));
    }

    fn library() -> Vec<TrackState> {
        (0..120)
            .map(|i| {
                let (tag, artist) = if i % 2 == 0 { ("ambient", format!("amb{}", i % 20)) } else { ("metal", format!("met{}", i % 20)) };
                t(i, &artist, &[tag], 150 + (i as u64 * 13) % 120)
            })
            .collect()
    }

    #[test]
    fn mood_words_steer_toward_matching_tracks() {
        assert!(parse_heuristic("a sad rainy drive").moods.contains(&"melancholic".to_string()));
        let mut lib = vec![];
        for i in 0..30u32 {
            let mut s = t(i, &format!("a{i}"), &[], 200);
            s.track.meta.audio = Some(AudioFeatures { moods: vec![if i < 15 { "melancholic".into() } else { "energetic".into() }], ..Default::default() });
            lib.push(s);
        }
        let intent = Intent { target_minutes: Some(20.0), moods: vec!["melancholic".into()], ..Default::default() };
        let (mut sad, mut other) = (0, 0);
        for seed in 0..20 {
            for i in generate(&lib, &intent, 0, &FeedbackConfig::default(), &mut StdRng::seed_from_u64(seed)).picks {
                if i < 15 { sad += 1 } else { other += 1 }
            }
        }
        assert!(sad as f64 > other as f64 * 1.6, "sad={sad} other={other}");
    }

    #[test]
    fn heuristic_reads_audio_constraints() {
        let i = parse_heuristic("chill 45 minute commute, no loud spikes");
        assert_eq!((i.tempo_max, i.energy_max, i.max_spike_db), (Some(115.0), Some(0.5), Some(6.0)));
        let w = parse_heuristic("workout around 150 bpm");
        assert_eq!((w.tempo_min, w.tempo_max, w.energy_min), (Some(142.0), Some(158.0), Some(0.6)));
        assert_eq!(parse_bpm("120bpm please"), Some(120.0));
        assert!(!parse_heuristic("just vibes").has_audio_constraints());
    }

    #[test]
    fn measured_audio_steers_selection_and_unmeasured_is_not_shut_out() {
        let mut lib = vec![];
        for i in 0..40u32 {
            let mut s = t(i, &format!("a{i}"), &[], 200);
            if i < 20 {
                s.track.tempo = Some(90.0);
                s.track.energy = Some(0.3);
            } else if i < 35 {
                s.track.tempo = Some(170.0);
                s.track.energy = Some(0.9);
            } // 35..40 unmeasured
            lib.push(s);
        }
        let intent = Intent { target_minutes: Some(30.0), tempo_max: Some(110.0), energy_max: Some(0.5), ..Default::default() };
        let mut slow = 0;
        let mut fast = 0;
        for seed in 0..30 {
            let g = generate(&lib, &intent, 0, &FeedbackConfig::default(), &mut StdRng::seed_from_u64(seed));
            for i in &g.picks {
                if *i < 20 { slow += 1 } else if *i < 35 { fast += 1 }
            }
        }
        assert!(slow > fast * 20, "slow={slow} fast={fast}");
        let strict = Intent { strict: true, ..intent.clone() };
        let g = generate(&lib, &strict, 0, &FeedbackConfig::default(), &mut StdRng::seed_from_u64(1));
        assert!(g.picks.iter().all(|i| *i < 20 || *i >= 35), "strict drops measured mismatches");
    }

    #[test]
    fn hits_target_length_within_a_minute() {
        let lib = library();
        let intent = Intent { target_minutes: Some(30.0), include_tags: vec!["ambient".into()], strict: true, ..Default::default() };
        for seed in 0..20 {
            let g = generate(&lib, &intent, 0, &FeedbackConfig::default(), &mut StdRng::seed_from_u64(seed));
            let diff = g.total_ms.abs_diff(30 * 60_000);
            assert!(diff <= 60_000, "seed {seed}: off by {diff}ms with {} tracks", g.picks.len());
            assert!(g.picks.iter().all(|i| lib[*i].track.tags[0] == "ambient"));
        }
    }

    #[test]
    fn respects_artist_cap_exclusions_and_dislikes() {
        let mut lib = library();
        lib[0].votes.record(0, false);
        lib[0].votes.record(100, false);
        let intent = Intent { target_minutes: Some(60.0), exclude_tags: vec!["metal".into()], max_per_artist: Some(2), ..Default::default() };
        let g = generate(&lib, &intent, 200, &FeedbackConfig::default(), &mut StdRng::seed_from_u64(1));
        let mut per: HashMap<&str, usize> = HashMap::new();
        for i in &g.picks {
            assert_ne!(lib[*i].track.tags[0], "metal");
            assert_ne!(*i, 0, "disliked track excluded");
            *per.entry(lib[*i].track.artist.as_str()).or_default() += 1;
        }
        assert!(per.values().all(|c| *c <= 2), "{per:?}");
    }

    #[test]
    fn non_strict_falls_back_when_few_matches() {
        let lib = vec![t(1, "a", &["ambient"], 200), t(2, "b", &["rock"], 200), t(3, "c", &["rock"], 200)];
        let intent = Intent { target_minutes: Some(10.0), include_tags: vec!["ambient".into()], ..Default::default() };
        let g = generate(&lib, &intent, 0, &FeedbackConfig::default(), &mut StdRng::seed_from_u64(2));
        assert_eq!(g.picks.len(), 3, "fills with non-matching tracks when there are not enough matches");
        assert_eq!(g.matched, 1);
    }
}
