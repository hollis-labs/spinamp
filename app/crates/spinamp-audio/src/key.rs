//! Musical key (Krumhansl-Schmuckler on a chroma profile) and mood words.
//!
//! Key: average the clip's energy into the 12 pitch classes, correlate with the 24 major/minor
//! key profiles, take the best. Reported only when it clearly beats the runner-up; percussive,
//! atonal or noisy music gets no key rather than a wrong one.
//!
//! Mood: a transparent heuristic from arousal (energy), valence (mode, brightness, tempo) and beat
//! steadiness. It is a proxy for how music tends to feel, not a measurement of any listener.

use rustfft::{FftPlanner, num_complex::Complex};

const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

/// Energy per pitch class (C = 0), from 110 Hz to 2 kHz, ignoring quiet frames.
pub fn chroma(x: &[f32], sr: f32) -> [f32; 12] {
    const N: usize = 4096;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
    let hann: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
    let bin_hz = sr / N as f32;
    let (lo, hi) = ((110.0 / bin_hz).ceil() as usize, ((2000.0 / bin_hz) as usize).min(N / 2 - 1));
    let pc_of: Vec<usize> = (lo..=hi).map(|k| (((69.0 + 12.0 * (k as f32 * bin_hz / 440.0).log2()).round() as i32).rem_euclid(12)) as usize).collect();

    let mut frames: Vec<([f32; 12], f32)> = vec![];
    let mut buf = vec![Complex::new(0f32, 0f32); N];
    let mut start = 0;
    while start + N <= x.len() {
        for i in 0..N {
            buf[i] = Complex::new(x[start + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let mut c = [0f32; 12];
        let mut total = 0.0;
        for (j, k) in (lo..=hi).enumerate() {
            let m = buf[k].norm().sqrt(); // compress so a few loud partials do not dominate
            c[pc_of[j]] += m;
            total += m;
        }
        frames.push((c, total));
        start += N / 2;
    }
    if frames.is_empty() {
        return [0.0; 12];
    }
    let mut totals: Vec<f32> = frames.iter().map(|f| f.1).collect();
    totals.sort_by(|a, b| a.total_cmp(b));
    let floor = totals[totals.len() / 3];
    let mut out = [0f32; 12];
    for (c, t) in frames.iter().filter(|f| f.1 >= floor && f.1 > 0.0) {
        for i in 0..12 {
            out[i] += c[i] / t; // each frame counts equally, so a loud bar does not outvote the rest
        }
    }
    let s: f32 = out.iter().sum();
    if s > 0.0 {
        out.iter_mut().for_each(|v| *v /= s);
    }
    out
}

fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let (ma, mb) = (a.iter().sum::<f32>() / 12.0, b.iter().sum::<f32>() / 12.0);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da <= 0.0 || db <= 0.0 { 0.0 } else { num / (da.sqrt() * db.sqrt()) }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub name: String,
    pub major: bool,
    pub confidence: f32,
}

/// Best-matching key, or None when nothing clearly stands out.
pub fn estimate_key(chroma: &[f32; 12]) -> Option<Key> {
    let mut scored: Vec<(f32, usize, bool)> = vec![];
    for tonic in 0..12 {
        for (profile, major) in [(&MAJOR, true), (&MINOR, false)] {
            let mut rot = [0f32; 12];
            for i in 0..12 {
                rot[(i + tonic) % 12] = profile[i];
            }
            scored.push((pearson(chroma, &rot), tonic, major));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, second) = (scored[0], scored[1]);
    // the relative major/minor (and neighbouring keys) are legitimately close; require a clear lead
    // over the runner-up unless the correlation itself is strong
    if best.0 < 0.5 || (best.0 - second.0 < 0.03 && best.0 < 0.8) {
        return None;
    }
    Some(Key { name: format!("{} {}", NOTES[best.1], if best.2 { "major" } else { "minor" }), major: best.2, confidence: best.0.clamp(0.0, 1.0) })
}

/// Valence proxy 0..1.
pub fn valence(key: Option<&Key>, brightness_hz: f32, tempo: Option<f32>) -> f32 {
    let mode = key.map_or(0.0, |k| if k.major { 0.18 } else { -0.18 } * ((k.confidence - 0.5) / 0.3).clamp(0.3, 1.0));
    let bright = ((brightness_hz - 1800.0) / 6000.0).clamp(-0.12, 0.12);
    let pace = tempo.map_or(0.0, |t| ((t - 105.0) / 300.0).clamp(-0.08, 0.1));
    (0.5 + mode + bright + pace).clamp(0.0, 1.0)
}

/// Mood words from arousal (energy 0..1), valence, beat steadiness and tempo.
pub fn moods(energy: f32, valence: f32, beat_strength: f32, tempo: Option<f32>) -> Vec<String> {
    let mut m: Vec<&str> = if energy >= 0.62 {
        if valence >= 0.55 { vec!["uplifting", "energetic"] } else if valence <= 0.42 { vec!["aggressive", "dark"] } else { vec!["energetic"] }
    } else if energy <= 0.42 {
        if valence >= 0.55 { vec!["calm", "peaceful"] } else if valence <= 0.42 { vec!["melancholic", "somber"] } else { vec!["calm"] }
    } else if valence >= 0.58 {
        vec!["upbeat"]
    } else if valence <= 0.4 {
        vec!["moody"]
    } else {
        vec!["balanced"]
    };
    if beat_strength < 0.3 && energy < 0.5 {
        m.push("dreamy");
    }
    if beat_strength >= 0.6 && energy >= 0.45 && tempo.is_some_and(|t| (95.0..=140.0).contains(&t)) {
        m.push("danceable");
    }
    m.into_iter().map(String::from).collect()
}

/// 0..1: a steady beat near dancing tempo.
pub fn danceability(beat_strength: f32, tempo: Option<f32>) -> f32 {
    let fit = tempo.map_or(0.4, |t| (-0.5 * ((t - 120.0) / 30.0).powi(2)).exp());
    (beat_strength.clamp(0.0, 1.0) * (0.35 + 0.65 * fit)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 22_050.0;

    /// Alternating notes (Hz) with their weights, 0.5 s each, for `secs`.
    fn melody(notes: &[(f32, f32)], secs: f32) -> Vec<f32> {
        let per = (SR * 0.5) as usize;
        (0..(SR * secs) as usize)
            .map(|i| {
                let (hz, w) = notes[(i / per) % notes.len()];
                let t = i as f32 / SR;
                w * 0.3 * ((2.0 * std::f32::consts::PI * hz * t).sin() + 0.4 * (2.0 * std::f32::consts::PI * 2.0 * hz * t).sin())
            })
            .collect()
    }

    // equal-tempered frequencies
    const C4: f32 = 261.63; const D4: f32 = 293.66; const E4: f32 = 329.63; const F4: f32 = 349.23; const G4: f32 = 392.00;
    const A3: f32 = 220.00; const A4: f32 = 440.00; const B4: f32 = 493.88; const C5: f32 = 523.25; const E5: f32 = 659.25;

    #[test]
    fn finds_c_major() {
        // tonic and fifth emphasised, with the rest of the scale
        let k = estimate_key(&chroma(&melody(&[(C4, 3.0), (E4, 1.5), (G4, 2.0), (C4, 3.0), (D4, 0.8), (F4, 0.8), (G4, 2.0), (C5, 2.0)], 16.0), SR)).expect("a key");
        assert_eq!(k.name, "C major", "{k:?}");
    }

    #[test]
    fn finds_a_minor() {
        let k = estimate_key(&chroma(&melody(&[(A3, 3.0), (C4, 1.8), (E4, 2.0), (A4, 3.0), (B4, 0.8), (C5, 1.5), (E5, 1.5), (A3, 3.0)], 16.0), SR)).expect("a key");
        assert_eq!(k.name, "A minor", "{k:?}");
        assert!(!k.major);
    }

    #[test]
    fn noise_has_no_key() {
        let mut seed = 5u32;
        let noise: Vec<f32> = (0..(SR * 10.0) as usize).map(|_| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * 0.3 }).collect();
        assert_eq!(estimate_key(&chroma(&noise, SR)), None);
    }

    #[test]
    fn mood_words_follow_arousal_and_valence() {
        assert_eq!(moods(0.8, 0.7, 0.7, Some(124.0)), ["uplifting", "energetic", "danceable"]);
        assert_eq!(moods(0.8, 0.3, 0.5, Some(150.0)), ["aggressive", "dark"]);
        assert_eq!(moods(0.3, 0.7, 0.5, Some(80.0)), ["calm", "peaceful"]);
        assert_eq!(moods(0.3, 0.3, 0.2, None), ["melancholic", "somber", "dreamy"]);
        assert_eq!(moods(0.5, 0.5, 0.5, None), ["balanced"]);
    }

    #[test]
    fn valence_rises_with_major_mode_and_brightness() {
        let major = Key { name: "C major".into(), major: true, confidence: 0.85 };
        let minor = Key { name: "A minor".into(), major: false, confidence: 0.85 };
        assert!(valence(Some(&major), 2500.0, Some(124.0)) > valence(Some(&minor), 1500.0, Some(124.0)) + 0.3);
        assert!((valence(None, 1800.0, Some(105.0)) - 0.5).abs() < 0.01);
    }

    #[test]
    fn danceability_peaks_near_120_with_a_steady_beat() {
        assert!(danceability(0.8, Some(120.0)) > danceability(0.8, Some(170.0)));
        assert!(danceability(0.8, Some(120.0)) > danceability(0.2, Some(120.0)));
    }
}
