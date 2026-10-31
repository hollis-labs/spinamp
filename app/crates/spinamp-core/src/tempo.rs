//! Tempo reconciliation. The analyser often cannot tell 85 from 170 (half/double time) or 108 from
//! 162 (3:2). It reports its alternatives (`AudioFeatures::tempo_candidates`); this module picks
//! between them using evidence the analyser does not have:
//!
//! - **learned genre priors**: typical tempi per tag, learned from the library's own confident
//!   measurements (dnb tracks cluster near 172, house near 125, ...), improving as more is analysed;
//! - an **external BPM** (Deezer's, when it has one);
//! and it only overrides the measurement when the evidence is clearly stronger.

use crate::{AudioFeatures, TrackState};
use std::collections::HashMap;

const MIN_SAMPLES: u32 = 8;
const OVERRIDE_MARGIN: f32 = 1.4;

#[derive(Debug, Default, Clone)]
pub struct TempoPriors {
    /// tag -> (mean bpm, std, samples)
    by_tag: HashMap<String, (f32, f32, u32)>,
}

fn confident(f: &AudioFeatures) -> Option<f32> {
    let t = f.tempo_bpm?;
    let clear = f.tempo_candidates.get(1).is_none_or(|c| c.1 < 0.8);
    (f.beat_strength >= 0.45 && clear).then_some(t)
}

impl TempoPriors {
    /// Learn from tracks whose tempo was measured confidently (strong beat, no close rival).
    pub fn learn(tracks: &[TrackState]) -> Self {
        let mut samples: HashMap<String, Vec<f32>> = HashMap::new();
        for t in tracks {
            if let Some(bpm) = t.track.meta.audio.as_ref().and_then(confident) {
                for tag in &t.track.tags {
                    samples.entry(tag.to_lowercase()).or_default().push(bpm);
                }
            }
        }
        let by_tag = samples
            .into_iter()
            .filter(|(_, v)| v.len() as u32 >= MIN_SAMPLES)
            .map(|(tag, v)| {
                let n = v.len() as f32;
                let mean = v.iter().sum::<f32>() / n;
                let std = (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n).sqrt().max(6.0);
                (tag, (mean, std, v.len() as u32))
            })
            .collect();
        Self { by_tag }
    }

    /// Combined expectation for a track's tags: (mean, std). Tags with tighter, better-sampled
    /// clusters count for more.
    pub fn prior_for(&self, tags: &[String]) -> Option<(f32, f32)> {
        let hits: Vec<&(f32, f32, u32)> = tags.iter().filter_map(|t| self.by_tag.get(&t.to_lowercase())).collect();
        if hits.is_empty() {
            return None;
        }
        let w: Vec<f32> = hits.iter().map(|h| h.2 as f32 / h.1).collect();
        let total: f32 = w.iter().sum();
        let mean = hits.iter().zip(&w).map(|(h, w)| h.0 * w).sum::<f32>() / total;
        let std = hits.iter().zip(&w).map(|(h, w)| h.1 * w).sum::<f32>() / total;
        Some((mean, std))
    }

    pub fn len(&self) -> usize {
        self.by_tag.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tag.is_empty()
    }
}

/// The tempo to use for a track: the analyser's reading unless the evidence clearly favours one of
/// its alternatives (or a half/double/3:2 variant of them).
pub fn reconcile(f: &AudioFeatures, tags: &[String], priors: &TempoPriors) -> Option<f32> {
    let measured = f.tempo_bpm?;
    let prior = priors.prior_for(tags);
    let ext = f.deezer_bpm;
    if prior.is_none() && ext.is_none() {
        return Some(measured);
    }

    // candidate readings: the measurement, the analyser's alternatives, and the usual confusions
    let mut cands: Vec<(f32, f32)> = vec![(measured, 1.0)];
    cands.extend(f.tempo_candidates.iter().copied());
    let mut expanded: Vec<(f32, f32)> = vec![];
    for (bpm, w) in cands {
        for (m, penalty) in [(1.0, 1.0), (2.0, 0.6), (0.5, 0.6), (1.5, 0.55), (2.0 / 3.0, 0.55)] {
            let b = bpm * m;
            if (60.0..=200.0).contains(&b) {
                match expanded.iter_mut().find(|e| (e.0 - b).abs() / b < 0.02) {
                    Some(e) => e.1 = e.1.max(w * penalty),
                    None => expanded.push((b, w * penalty)),
                }
            }
        }
    }
    let score = |bpm: f32, base: f32| -> f32 {
        let prior_fit = prior.map_or(1.0, |(mean, std)| 0.35 + 0.65 * (-0.5 * ((bpm - mean) / std.max(10.0)).powi(2)).exp());
        let ext_fit = ext.map_or(1.0, |d| {
            let near = |x: f32| (bpm - x).abs() / x < 0.03;
            if near(d) { 3.0 } else if near(d * 2.0) || near(d / 2.0) { 1.3 } else { 0.7 }
        });
        base * prior_fit * ext_fit
    };
    let own = expanded.iter().find(|e| (e.0 - measured).abs() / measured < 0.02).map_or(1.0, |e| score(e.0, e.1));
    let best = expanded.iter().map(|e| (e.0, score(e.0, e.1))).max_by(|a, b| a.1.total_cmp(&b.1))?;
    // only overrule the measurement when the evidence is clearly stronger
    Some(if (best.0 - measured).abs() / measured < 0.02 || best.1 < own * OVERRIDE_MARGIN { measured } else { best.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Track, TrackId, TrackMeta};

    fn feat(bpm: f32, strength: f32, cands: &[(f32, f32)]) -> AudioFeatures {
        AudioFeatures { tempo_bpm: Some(bpm), beat_strength: strength, tempo_candidates: cands.to_vec(), ..Default::default() }
    }

    fn lib(tag: &str, bpm: f32, n: usize) -> Vec<TrackState> {
        (0..n)
            .map(|i| {
                TrackState::new(
                    Track {
                        id: TrackId::new("t", &format!("{tag}{i}")), provider: "t".into(), title: "t".into(), artist: format!("a{i}"), album: None,
                        track_number: None, duration_ms: 1, tempo: Some(bpm), energy: None, tags: vec![tag.into()],
                        meta: TrackMeta { audio: Some(feat(bpm + (i % 3) as f32 - 1.0, 0.8, &[(bpm, 1.0)])), ..Default::default() },
                    },
                    0,
                )
            })
            .collect()
    }

    #[test]
    fn learned_genre_prior_resolves_half_time() {
        let priors = TempoPriors::learn(&lib("drum and bass", 172.0, 12));
        assert_eq!(priors.len(), 1);
        let ambiguous = feat(86.0, 0.6, &[(86.0, 1.0), (172.0, 0.95)]);
        assert_eq!(reconcile(&ambiguous, &["drum and bass".into()], &priors), Some(172.0));
        // no tag evidence: leave the measurement alone
        assert_eq!(reconcile(&ambiguous, &["folk".into()], &priors), Some(86.0));
    }

    #[test]
    fn external_bpm_resolves_three_to_two() {
        let mut f = feat(108.0, 0.67, &[(108.0, 1.0), (162.0, 0.99), (81.0, 0.9)]);
        f.deezer_bpm = Some(162.13);
        assert_eq!(reconcile(&f, &[], &TempoPriors::default()), Some(162.0));
    }

    #[test]
    fn does_not_flap_on_weak_evidence() {
        let priors = TempoPriors::learn(&lib("house", 126.0, 10));
        // clear 120 reading; prior says 126, but a nudge is not enough to change it
        let f = feat(120.0, 0.8, &[(120.0, 1.0), (60.0, 0.5)]);
        assert_eq!(reconcile(&f, &["house".into()], &priors), Some(120.0));
    }

    #[test]
    fn priors_ignore_unconfident_measurements_and_small_samples() {
        let mut tracks = lib("techno", 130.0, 12);
        for t in &mut tracks {
            if let Some(a) = t.track.meta.audio.as_mut() {
                a.beat_strength = 0.2; // weak beat: not trusted for learning
            }
        }
        assert!(TempoPriors::learn(&tracks).is_empty());
        assert!(TempoPriors::learn(&lib("ska", 150.0, 5)).is_empty(), "needs {MIN_SAMPLES}+ samples");
        assert_eq!(reconcile(&feat(100.0, 0.7, &[]), &[], &TempoPriors::default()), Some(100.0));
        assert_eq!(reconcile(&AudioFeatures::default(), &[], &TempoPriors::default()), None);
    }
}
