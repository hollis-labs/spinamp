use rustfft::{FftPlanner, num_complex::Complex};
use spinamp_core::{AudioFeatures, VizPattern};

const FRAME: usize = 1024;
const HOP: usize = 256;
const BPM_MIN: f32 = 55.0;
const BPM_MAX: f32 = 200.0;
/// Calibrated on real previews (examples/probe.rs): library flux spans ~8 (sparse) to ~20 (busy bass music).
const FLUX_REF: f32 = 18.0;

fn percentile(sorted: &[f32], p: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * p).round() as usize]
}

/// Everything measured from one clip.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub features: AudioFeatures,
    /// Beat-aligned 4-band pattern; `None` for free-time music with no usable beat.
    pub viz: Option<VizPattern>,
}

/// Spectrum bands recorded for the visualiser (log-spaced).
const NB: usize = 16;
const SUBDIV: usize = 8;
const MAX_LOOP_BEATS: usize = 96;

fn lerp(v: &[f32], pos: f32) -> f32 {
    if pos < 0.0 {
        return 0.0;
    }
    let i = pos.floor() as usize;
    if i + 1 >= v.len() {
        return 0.0;
    }
    let f = pos - i as f32;
    v[i] * (1.0 - f) + v[i + 1] * f
}

/// Measure a mono clip. Needs >= 5 s of non-silent audio; previews are ~30 s.
pub fn analyze(mono: &[f32], sample_rate: u32) -> Option<Analysis> {
    // cheap decimation to ~22 kHz (average pairs): plenty for tempo and loudness
    let f = (sample_rate / 22_050).max(1) as usize;
    let x: Vec<f32> = mono.chunks(f).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    let sr = (sample_rate / f as u32) as f32;
    if x.len() < (sr * 5.0) as usize {
        return None;
    }

    /* ---- level statistics over 50 ms windows ---- */
    let win = (sr * 0.05) as usize;
    let db: Vec<f32> = x.chunks_exact(win).map(|w| 10.0 * (w.iter().map(|s| s * s).sum::<f32>() / win as f32 + 1e-12).log10()).collect();
    let mut active: Vec<f32> = db.iter().copied().filter(|d| *d > -70.0).collect();
    if active.len() < 40 {
        return None; // essentially silent
    }
    active.sort_by(|a, b| a.total_cmp(b));
    let loudness_db = 10.0 * (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32 + 1e-12).log10();
    let peak_db = 20.0 * (x.iter().fold(0f32, |m, s| m.max(s.abs())) + 1e-9).log10();
    let dynamic_range_db = percentile(&active, 0.95) - percentile(&active, 0.10);
    let spike_db = percentile(&active, 0.995) - percentile(&active, 0.50);

    /* ---- STFT: spectral flux (onsets) and centroid (brightness) ---- */
    let fft = FftPlanner::<f32>::new().plan_fft_forward(FRAME);
    let hann: Vec<f32> = (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos()).collect();
    let bin_hz = sr / FRAME as f32;
    let kmax = ((8_000.0 / bin_hz) as usize).min(FRAME / 2 - 1);

    let mut prev = vec![0f32; kmax];
    let (mut flux, mut cent_num, mut cent_den) = (Vec::<f32>::new(), 0f64, 0f64);
    let mut buf = vec![Complex::new(0f32, 0f32); FRAME];
    let mut start = 0;
    while start + FRAME <= x.len() {
        for i in 0..FRAME {
            buf[i] = Complex::new(x[start + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let (mut fsum, mut msum, mut wsum) = (0f32, 0f32, 0f32);
        for k in 1..kmax {
            let m = buf[k].norm();
            let lm = (1.0 + 40.0 * m / 256.0).ln();
            fsum += (lm - prev[k]).max(0.0);
            prev[k] = lm;
            msum += m;
            wsum += m * k as f32 * bin_hz;
        }
        flux.push(fsum);
        if msum > 1e-3 {
            cent_num += wsum as f64;
            cent_den += msum as f64;
        }
        start += HOP;
    }
    if flux.len() < 200 {
        return None;
    }
    let brightness_hz = if cent_den > 0.0 { (cent_num / cent_den) as f32 } else { 0.0 };
    let mean_flux = flux.iter().sum::<f32>() / flux.len() as f32;

    /* ---- tempo: autocorrelation of the onset envelope ---- */
    let fps = sr / HOP as f32;
    let half = (fps * 0.4) as usize; // ~0.8 s moving-average window
    let onset: Vec<f32> = (0..flux.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half).min(flux.len()));
            (flux[i] - flux[a..b].iter().sum::<f32>() / (b - a) as f32).max(0.0)
        })
        .collect();
    let n = onset.len();
    let acf = |lag: usize| -> f32 { (0..n - lag).map(|i| onset[i] * onset[i + lag]).sum::<f32>() / (n - lag) as f32 };
    let acf0 = acf(0).max(1e-9);
    let lag_hi = (fps * 60.0 / BPM_MIN).ceil() as usize;
    let max_lag = (lag_hi * 4 + 2).min(n - 1);
    let a: Vec<f32> = (0..=max_lag).map(acf).collect();
    // linear interpolation so fractional lags can be scored
    let at = |l: f32| -> f32 {
        let (i, fr) = (l.floor() as usize, l.fract());
        if i + 1 > max_lag { 0.0 } else { a[i] * (1.0 - fr) + a[(i + 1).min(max_lag)] * fr }
    };
    // Harmonic comb: a true beat period correlates at 1x, 2x, 3x and 4x its length, while a
    // wrong guess (e.g. the 3:2 relation) only matches some of them.
    let mut best: Option<(f32, f32)> = None; // (bpm, score)
    let mut scores: Vec<(f32, f32)> = Vec::new();
    let mut bpm = BPM_MIN;
    while bpm <= BPM_MAX {
        let lag = fps * 60.0 / bpm;
        let comb: f32 = (1..=4).map(|h| at(lag * h as f32).max(0.0) / (1.0 + 0.15 * (h - 1) as f32)).sum();
        let prior = (-0.5 * ((bpm / 125.0).log2() / 1.2).powi(2)).exp(); // gentle: people favour ~125
        let sc = comb * prior;
        scores.push((bpm, sc));
        if best.is_none_or(|b| sc > b.1) {
            best = Some((bpm, sc));
        }
        bpm += 0.25;
    }
    let (mut bpm, _) = best?;
    let best_lag = (fps * 60.0 / bpm).round() as usize;
    let beat_strength = (a[best_lag.min(max_lag)] / acf0).clamp(0.0, 1.0);
    // fold into the conventional 70..180 range (half/double time are the same groove)
    while bpm < 70.0 { bpm *= 2.0; }
    while bpm > 180.0 { bpm /= 2.0; }
    let tempo_bpm = (beat_strength >= 0.08).then_some(bpm);

    // alternative readings: distinct local maxima of the comb score, strongest first
    let top = scores.iter().map(|x| x.1).fold(0f32, f32::max).max(1e-9);
    let mut peaks: Vec<(f32, f32)> = (1..scores.len() - 1)
        .filter(|&i| scores[i].1 >= scores[i - 1].1 && scores[i].1 > scores[i + 1].1)
        .map(|i| (scores[i].0, scores[i].1 / top))
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut tempo_candidates: Vec<(f32, f32)> = vec![];
    for p in peaks {
        if tempo_candidates.iter().all(|c| (c.0 - p.0).abs() / c.0 > 0.05) && p.1 > 0.35 {
            tempo_candidates.push(p);
        }
        if tempo_candidates.len() == 4 {
            break;
        }
    }

    // beat grid: pick the phase whose beats collect the most onset energy
    let (viz, beats_per_bar) = match tempo_bpm {
        Some(t) => beat_pattern(&onset, &band_frames(&x, sr), fps, t),
        None => (None, None),
    };

    /* ---- composite energy ---- */
    let loud_n = ((loudness_db + 28.0) / 20.0).clamp(0.0, 1.0);
    let flux_n = (mean_flux / FLUX_REF).tanh();
    let flux = mean_flux;
    let bright_n = ((brightness_hz - 800.0) / 2800.0).clamp(0.0, 1.0);
    let energy = 0.40 * loud_n + 0.40 * flux_n + 0.20 * bright_n;

    let features = AudioFeatures { tempo_bpm, beat_strength, loudness_db, peak_db, dynamic_range_db, spike_db, brightness_hz, flux, energy, tempo_candidates, beats_per_bar, ..Default::default() };
    let mut features = features;
    let key = crate::key::estimate_key(&crate::key::chroma(&x, sr));
    features.key = key.as_ref().map(|k| k.name.clone());
    features.key_confidence = key.as_ref().map_or(0.0, |k| k.confidence);
    features.valence = crate::key::valence(key.as_ref(), brightness_hz, tempo_bpm);
    features.moods = crate::key::moods(energy, features.valence, beat_strength, tempo_bpm);
    features.danceability = crate::key::danceability(beat_strength, tempo_bpm);
    Some(Analysis { features, viz })
}

/// Log-spaced band energies (dB per bin) on a 2048-point FFT, one frame per HOP samples.
/// Frame `i` is centred 1024 samples in, i.e. 2 onset-frames after onset-frame `i`.
fn band_frames(x: &[f32], sr: f32) -> Vec<[f32; NB]> {
    const F2: usize = 2048;
    let fft = FftPlanner::<f32>::new().plan_fft_forward(F2);
    let hann: Vec<f32> = (0..F2).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / F2 as f32).cos()).collect();
    let bin_hz = sr / F2 as f32;
    let edge = |i: usize| 40.0 * (10_000.0f32 / 40.0).powf(i as f32 / NB as f32);
    let ranges: Vec<(usize, usize)> = (0..NB)
        .map(|b| {
            let lo = ((edge(b) / bin_hz).round() as usize).max(1);
            let hi = ((edge(b + 1) / bin_hz).round() as usize).max(lo + 1).min(F2 / 2);
            (lo, hi)
        })
        .collect();
    let mut out = Vec::new();
    let mut buf = vec![Complex::new(0f32, 0f32); F2];
    let mut start = 0;
    while start + F2 <= x.len() {
        for i in 0..F2 {
            buf[i] = Complex::new(x[start + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);
        let mut f = [0f32; NB];
        for (b, (lo, hi)) in ranges.iter().enumerate() {
            let p: f32 = buf[*lo..*hi].iter().map(|c| c.norm_sqr()).sum::<f32>() / (*hi - *lo) as f32;
            f[b] = 10.0 * (p + 1e-6).log10();
        }
        out.push(f);
        start += HOP;
    }
    out
}

/// Record the clip's actual spectrum on the beat grid (NOT averaged into a short loop, so the
/// builds, fills and texture changes of the excerpt survive), starting at the downbeat and
/// covering a whole number of bars.
fn beat_pattern(onset: &[f32], band_frames: &[[f32; NB]], fps: f32, bpm: f32) -> (Option<VizPattern>, Option<u8>) {
    let period = fps * 60.0 / bpm;
    // align band frames to onset frames (band frame centres sit 2 onset-frames later)
    let n = onset.len().min(band_frames.len() + 2);
    if period < 4.0 || (n as f32) < period * 12.0 {
        return (None, None);
    }
    // phase: beats at phase + k*period should land on onsets
    let mut best = (0.0f32, f32::MIN);
    let mut ph = 0.0;
    while ph < period {
        let (mut sum, mut k) = (0.0, 0);
        while ph + k as f32 * period < n as f32 - 1.0 {
            sum += lerp(onset, ph + k as f32 * period);
            k += 1;
        }
        let score = sum / k.max(1) as f32;
        if score > best.1 {
            best = (ph, score);
        }
        ph += 0.5;
    }
    let phase = best.0;
    let beats: Vec<f32> = (0..).map(|k| phase + k as f32 * period).take_while(|p| p + period < n as f32 - 1.0).collect();
    if beats.len() < 12 {
        return (None, None);
    }
    // meter: which beat in the bar carries the accent, for 3 vs 4 beats per bar
    let accent: Vec<f32> = beats.iter().map(|p| lerp(onset, *p).max(0.0)).collect();
    let judge = |bpb: usize| -> (f32, usize) {
        let mean_at = |d: usize| -> f32 {
            let v: Vec<f32> = accent.iter().enumerate().filter(|(k, _)| k % bpb == d).map(|(_, a)| *a).collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        let means: Vec<f32> = (0..bpb).map(mean_at).collect();
        let (d, top) = means.iter().cloned().enumerate().fold((0, f32::MIN), |b, (i, m)| if m > b.1 { (i, m) } else { b });
        let rest = (means.iter().sum::<f32>() - top) / (bpb - 1) as f32;
        (top / rest.max(1e-6), d)
    };
    let (r3, d3) = judge(3);
    let (r4, d4) = judge(4);
    let (bpb, down) = if r3 > 1.25 * r4 { (3usize, d3) } else { (4usize, d4) };

    // whole bars from the first downbeat onward
    let usable = beats.len() - down;
    let loop_beats = ((usable / bpb) * bpb).min(MAX_LOOP_BEATS / bpb * bpb);
    if loop_beats < 8 {
        return (None, Some(bpb as u8));
    }

    // per-band columns on the onset-frame timeline, normalised to the clip's own dynamic range
    let cols: Vec<Vec<f32>> = (0..NB)
        .map(|b| {
            let raw: Vec<f32> = (0..n).map(|j| band_frames[j.saturating_sub(2).min(band_frames.len() - 1)][b]).collect();
            let mut sorted = raw.clone();
            sorted.sort_by(|a, c| a.total_cmp(c));
            let (lo, hi) = (sorted[n / 10], sorted[n * 98 / 100].max(sorted[n / 10] + 1.0));
            raw.iter().map(|v| ((v - lo) / (hi - lo)).clamp(0.0, 1.0)).collect()
        })
        .collect();
    let mut data = Vec::with_capacity(loop_beats * SUBDIV * NB);
    for j in 0..loop_beats {
        let p = beats[down + j];
        for s in 0..SUBDIV {
            let pos = p + s as f32 * period / SUBDIV as f32;
            for col in &cols {
                data.push((lerp(col, pos).clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
    }
    (
        Some(VizPattern { beats: loop_beats as u8, subdiv: SUBDIV as u8, beats_per_bar: bpb as u8, bands: NB as u8, version: 2, bpm, data }),
        Some(bpb as u8),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 22_050;

    /// Decaying noise bursts at `bpm` over a quiet tone bed: a stand-in for a drum pattern.
    fn click_track(bpm: f32, secs: f32, amp: f32) -> Vec<f32> {
        let n = (SR as f32 * secs) as usize;
        let period = (SR as f32 * 60.0 / bpm) as usize;
        let mut seed = 12345u32;
        let mut rnd = || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed >> 8) as f32 / 8_388_608.0 - 1.0 };
        (0..n).map(|i| {
            let ph = i % period;
            let env = (-(ph as f32) / (SR as f32 * 0.03)).exp();
            amp * env * rnd() + 0.01 * (i as f32 * 0.05).sin()
        }).collect()
    }

    fn tone(hz: f32, secs: f32, amp: f32) -> Vec<f32> {
        (0..(SR as f32 * secs) as usize).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SR as f32).sin()).collect()
    }

    #[test]
    fn finds_tempo_of_click_tracks() {
        for bpm in [76.0, 90.0, 100.0, 120.0, 128.0, 140.0, 170.0] {
            let a = analyze(&click_track(bpm, 24.0, 0.6), SR).unwrap().features;
            let t = a.tempo_bpm.expect("tempo found");
            // allow the metrical octave (half/double) but not arbitrary values
            let ok = [t, t * 2.0, t / 2.0].iter().any(|c| (c - bpm).abs() <= 2.5);
            assert!(ok, "expected ~{bpm}, got {t} (strength {})", a.beat_strength);
            if bpm < 160.0 {
                assert!((t - bpm).abs() <= 2.5, "preferred octave wrong for {bpm}: got {t}");
            } // above ~160 the half-time reading is equally valid

            assert!(a.beat_strength > 0.2);
        }
    }

    #[test]
    fn steady_tone_has_no_beat_and_loudness_scales() {
        let quiet = analyze(&tone(220.0, 12.0, 0.05), SR).unwrap().features;
        let loud = analyze(&tone(220.0, 12.0, 0.5), SR).unwrap().features;
        assert!(quiet.tempo_bpm.is_none(), "{:?}", quiet.tempo_bpm);
        let diff = loud.loudness_db - quiet.loudness_db;
        assert!((diff - 20.0).abs() < 1.0, "10x amplitude = 20 dB, got {diff}");
        assert!(loud.energy > quiet.energy);
    }

    #[test]
    fn detects_a_sudden_loud_spike() {
        let mut steady = tone(220.0, 20.0, 0.1);
        let flat = analyze(&steady, SR).unwrap().features;
        for s in steady[(SR as usize * 10)..(SR as usize * 10 + SR as usize / 4)].iter_mut() {
            *s *= 12.0; // quarter-second burst, +21 dB
        }
        let spiky = analyze(&steady, SR).unwrap().features;
        assert!(flat.spike_db < 3.0, "{}", flat.spike_db);
        assert!(spiky.spike_db > 12.0 && spiky.peak_db > flat.peak_db + 15.0, "{} {}", spiky.spike_db, spiky.peak_db);
    }

    #[test]
    fn brightness_follows_frequency() {
        let dark = analyze(&tone(200.0, 10.0, 0.3), SR).unwrap().features;
        let bright = analyze(&tone(4000.0, 10.0, 0.3), SR).unwrap().features;
        assert!((dark.brightness_hz - 200.0).abs() < 100.0, "{}", dark.brightness_hz);
        assert!((bright.brightness_hz - 4000.0).abs() < 200.0, "{}", bright.brightness_hz);
    }

    /// Bursts every beat, with a much louder one on the first beat of each bar.
    fn accented(bpm: f32, per_bar: usize, secs: f32) -> Vec<f32> {
        let n = (SR as f32 * secs) as usize;
        let period = (SR as f32 * 60.0 / bpm) as usize;
        let mut seed = 99u32;
        let mut rnd = || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed >> 8) as f32 / 8_388_608.0 - 1.0 };
        (0..n).map(|i| {
            let beat = i / period;
            let ph = i % period;
            let amp = if beat % per_bar == 0 { 0.8 } else { 0.25 };
            amp * (-(ph as f32) / (SR as f32 * 0.03)).exp() * rnd() + 0.01 * (i as f32 * 0.05).sin()
        }).collect()
    }

    #[test]
    fn detects_time_signature_and_downbeat_pattern() {
        for per_bar in [4usize, 3] {
            let a = analyze(&accented(110.0, per_bar, 28.0), SR).unwrap();
            let v = a.viz.expect("pattern");
            assert_eq!(a.features.beats_per_bar, Some(per_bar as u8), "meter for {per_bar}/4");
            assert_eq!((v.subdiv, v.bands, v.version), (8, 16, 2));
            assert!(v.beats as usize % per_bar == 0 && v.beats >= 8, "whole bars: {} beats", v.beats);
            assert_eq!(v.data.len(), v.beats as usize * 8 * 16);
            // energy right after the downbeat beats the other beats of the bar
            let at = |b: usize| -> u32 { (0..3).flat_map(|s| (0..16).map(move |k| (b * 8 + s) * 16 + k)).map(|i| v.data[i] as u32).sum() };
            assert!(at(0) > at(1) && at(0) > at(per_bar - 1), "downbeat at slot 0 for {per_bar}/4: {} {} {}", at(0), at(1), at(per_bar - 1));
        }
    }

    #[test]
    fn pattern_keeps_the_excerpts_real_variation() {
        // a crescendo: same drum pattern getting steadily louder over 28 s. A short averaged loop
        // would flatten this; the recording must keep it.
        let n = (SR as f32 * 28.0) as usize;
        let mut x = accented(120.0, 4, 28.0);
        for (i, s) in x.iter_mut().enumerate().take(n) {
            *s *= 0.15 + 0.85 * i as f32 / n as f32;
        }
        let v = analyze(&x, SR).unwrap().viz.expect("pattern");
        let third = v.data.len() / 3;
        let mean = |r: std::ops::Range<usize>| r.map(|i| v.data[i] as f32).sum::<f32>() / third as f32;
        let (first, last) = (mean(0..third), mean(2 * third..3 * third));
        assert!(last > first * 1.25, "later bars should be louder: first {first:.0}, last {last:.0}");
    }

    #[test]
    fn offers_alternative_tempi_and_no_pattern_without_a_beat() {
        let a = analyze(&click_track(120.0, 24.0, 0.6), SR).unwrap().features;
        assert!(a.tempo_candidates.len() >= 2 && (a.tempo_candidates[0].1 - 1.0).abs() < 1e-6, "{:?}", a.tempo_candidates);
        assert!(analyze(&tone(220.0, 12.0, 0.1), SR).unwrap().viz.is_none());
    }

    #[test]
    fn rejects_silence_and_short_clips() {
        assert!(analyze(&vec![0.0; SR as usize * 20], SR).is_none());
        assert!(analyze(&tone(440.0, 2.0, 0.3), SR).is_none());
    }
}
