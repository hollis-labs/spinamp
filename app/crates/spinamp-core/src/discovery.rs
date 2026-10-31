//! Discovery: score tracks you do *not* have against a taste profile learned from feedback.
//!
//! Provider-agnostic: candidates come from any adapter (`MusicProvider::candidates`),
//! and similarity uses only fields every source can fill (tags, tempo, energy, artist).

use crate::feedback::FeedbackConfig;
use crate::shuffle::weighted_index;
use crate::{Track, TrackId, TrackState, Ts};
use rand::Rng;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct TasteProfile {
    tag_weights: HashMap<String, f64>,
    artist_weights: HashMap<String, f64>,
    tempo: Option<(f64, f64)>,  // (mean, std) of liked tracks
    energy: Option<(f64, f64)>, // (mean, std)
}

impl TasteProfile {
    /// Learn from the library. Each track contributes in proportion to its feedback score;
    /// tracks with no votes contribute a small positive prior (you did add them).
    pub fn learn(library: &[TrackState], now: Ts, fb: &FeedbackConfig) -> Self {
        let mut p = TasteProfile::default();
        let (mut tempos, mut energies): (Vec<(f64, f64)>, Vec<(f64, f64)>) = (vec![], vec![]);
        for t in library {
            let w = t.votes.score(now, fb) + 0.25 + if t.favorite { 1.0 } else { 0.0 };
            for tag in &t.track.tags {
                *p.tag_weights.entry(tag.to_lowercase()).or_default() += w;
            }
            *p.artist_weights.entry(t.track.artist.clone()).or_default() += w;
            if w > 0.0 {
                if let Some(x) = t.track.tempo {
                    tempos.push((x as f64, w));
                }
                if let Some(x) = t.track.energy {
                    energies.push((x as f64, w));
                }
            }
        }
        p.tempo = weighted_mean_std(&tempos);
        p.energy = weighted_mean_std(&energies);
        p
    }

    /// 0.0 (hates it) .. 1.0 (perfect match); ~0.5 when we know nothing.
    pub fn similarity(&self, c: &Track) -> f64 {
        let mut score = 0.0;
        let mut weight_sum = 0.0;

        if !c.tags.is_empty() && !self.tag_weights.is_empty() {
            let raw: f64 = c
                .tags
                .iter()
                .map(|t| self.tag_weights.get(&t.to_lowercase()).copied().unwrap_or(0.0))
                .sum::<f64>()
                / (c.tags.len() as f64).sqrt();
            score += 4.0 * logistic(raw / 3.0);
            weight_sum += 4.0;
        }
        if let (Some((m, s)), Some(x)) = (self.tempo, c.tempo) {
            score += 1.0 * gauss(x as f64, m, s.max(12.0));
            weight_sum += 1.0;
        }
        if let (Some((m, s)), Some(x)) = (self.energy, c.energy) {
            score += 1.0 * gauss(x as f64, m, s.max(0.12));
            weight_sum += 1.0;
        }
        // Same artist as something you love is a strong, cheap signal.
        if let Some(a) = self.artist_weights.get(&c.artist) {
            score += 2.0 * logistic(*a);
            weight_sum += 2.0;
        }
        if weight_sum == 0.0 { 0.5 } else { score / weight_sum }
    }
}

fn logistic(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn gauss(x: f64, mean: f64, std: f64) -> f64 {
    (-((x - mean) / std).powi(2) / 2.0).exp()
}

fn weighted_mean_std(xs: &[(f64, f64)]) -> Option<(f64, f64)> {
    let w: f64 = xs.iter().map(|x| x.1).sum();
    if xs.len() < 3 || w <= 0.0 {
        return None;
    }
    let mean = xs.iter().map(|x| x.0 * x.1).sum::<f64>() / w;
    let var = xs.iter().map(|x| x.1 * (x.0 - mean).powi(2)).sum::<f64>() / w;
    Some((mean, var.sqrt()))
}

/// Pick a discovery track: drop anything already owned or recently suggested, then sample
/// by `similarity^sharpness` so good matches dominate but surprises still happen.
pub fn pick<R: Rng>(
    candidates: &[Track],
    owned: &HashSet<TrackId>,
    recently_suggested: &[TrackId],
    profile: &TasteProfile,
    sharpness: f64,
    rng: &mut R,
) -> Option<usize> {
    let weights: Vec<f64> = candidates
        .iter()
        .map(|c| {
            if owned.contains(&c.id) || recently_suggested.contains(&c.id) {
                0.0
            } else {
                profile.similarity(c).powf(sharpness)
            }
        })
        .collect();
    weighted_index(&weights, rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shuffle::tests::ts;
    use rand::{SeedableRng, rngs::StdRng};

    fn cand(n: u32, artist: &str, tags: &[&str], tempo: f32) -> Track {
        let mut t = ts(n + 1000, artist).track;
        t.tags = tags.iter().map(|s| s.to_string()).collect();
        t.tempo = Some(tempo);
        t
    }

    fn library() -> Vec<TrackState> {
        (0..6)
            .map(|i| {
                let mut s = ts(i, "Loved Band");
                s.track.tags = vec!["shoegaze".into(), "dream pop".into()];
                s.track.tempo = Some(100.0 + i as f32);
                s.votes.record(0, true);
                s
            })
            .collect()
    }

    #[test]
    fn similar_scores_higher_than_dissimilar() {
        let p = TasteProfile::learn(&library(), 86_400, &FeedbackConfig::default());
        let near = p.similarity(&cand(1, "Someone", &["shoegaze"], 102.0));
        let far = p.similarity(&cand(2, "Other", &["death metal"], 190.0));
        assert!(near > far + 0.2, "near={near} far={far}");
    }

    #[test]
    fn dislikes_push_a_tag_down() {
        let mut lib = library();
        for i in 10..16 {
            let mut s = ts(i, "Hated");
            s.track.tags = vec!["polka".into()];
            s.votes.record(0, false);
            s.votes.record(86_400, false);
            lib.push(s);
        }
        let p = TasteProfile::learn(&lib, 2 * 86_400, &FeedbackConfig::default());
        assert!(p.similarity(&cand(3, "x", &["polka"], 100.0)) < 0.4);
    }

    #[test]
    fn owned_tracks_are_never_suggested() {
        let p = TasteProfile::learn(&library(), 86_400, &FeedbackConfig::default());
        let cands = vec![cand(1, "a", &["shoegaze"], 100.0), cand(2, "b", &["shoegaze"], 100.0)];
        let owned: HashSet<_> = [cands[0].id.clone()].into();
        let mut rng = StdRng::seed_from_u64(5);
        for _ in 0..50 {
            assert_eq!(pick(&cands, &owned, &[], &p, 2.0, &mut rng), Some(1));
        }
    }
}
