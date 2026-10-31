//! Streaming spectrum analyser for live capture. Push mono PCM at any chunk size; get one frame
//! of 16 log-spaced band levels (dB, mean power per bin) every 50 ms. Same band layout as the
//! preview analysis, so recordings and previews are interchangeable in the visualiser.

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

pub const BANDS: usize = 16;
const SIZE: usize = 4096;

pub struct SpectrumStream {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    ranges: Vec<(usize, usize)>,
    buf: Vec<f32>,
    hop: usize,
    scratch: Vec<Complex<f32>>,
}

impl SpectrumStream {
    /// A frame is produced every `sample_rate / 20` samples (50 ms).
    pub fn new(sample_rate: u32) -> Self {
        let bin_hz = sample_rate as f32 / SIZE as f32;
        let edge = |i: usize| 40.0 * (10_000.0f32 / 40.0).powf(i as f32 / BANDS as f32);
        let ranges = (0..BANDS)
            .map(|b| {
                let lo = ((edge(b) / bin_hz).round() as usize).max(1);
                let hi = ((edge(b + 1) / bin_hz).round() as usize).max(lo + 1).min(SIZE / 2);
                (lo, hi)
            })
            .collect();
        Self {
            fft: FftPlanner::<f32>::new().plan_fft_forward(SIZE),
            window: (0..SIZE).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / SIZE as f32).cos()).collect(),
            ranges,
            buf: Vec::with_capacity(SIZE * 2),
            hop: (sample_rate / 20) as usize,
            scratch: vec![Complex::new(0.0, 0.0); SIZE],
        }
    }

    pub fn hop_ms(&self) -> u32 {
        50
    }

    /// Feed samples; returns every frame that became complete. Frames cover the most recent
    /// 4096 samples, so a frame's time is "now" for the last sample consumed.
    pub fn push(&mut self, mono: &[f32]) -> Vec<[f32; BANDS]> {
        self.buf.extend_from_slice(mono);
        let mut out = Vec::new();
        while self.buf.len() >= SIZE {
            for i in 0..SIZE {
                self.scratch[i] = Complex::new(self.buf[i] * self.window[i], 0.0);
            }
            self.fft.process(&mut self.scratch);
            let mut f = [0f32; BANDS];
            for (b, (lo, hi)) in self.ranges.iter().enumerate() {
                let p: f32 = self.scratch[*lo..*hi].iter().map(|c| c.norm_sqr()).sum::<f32>() / (*hi - *lo) as f32;
                f[b] = 10.0 * (p + 1e-6).log10();
            }
            out.push(f);
            self.buf.drain(..self.hop.min(self.buf.len()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, secs: f32, sr: u32) -> Vec<f32> {
        (0..(sr as f32 * secs) as usize).map(|i| 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / sr as f32).sin()).collect()
    }
    fn argmax(f: &[f32; BANDS]) -> usize {
        f.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0
    }

    #[test]
    fn tone_lands_in_the_right_band() {
        for (hz, band) in [(100.0, 2), (1000.0, 9), (6000.0, 14)] {
            let mut s = SpectrumStream::new(48_000);
            let frames = s.push(&sine(hz, 1.0, 48_000));
            assert!(frames.len() >= 10, "{} frames", frames.len());
            let last = frames.last().unwrap();
            assert_eq!(argmax(last), band, "{hz} Hz -> band {band}, got {}: {last:?}", argmax(last));
        }
    }

    #[test]
    fn chunking_does_not_change_the_output() {
        let x = sine(440.0, 1.0, 48_000);
        let whole = SpectrumStream::new(48_000).push(&x);
        let mut s = SpectrumStream::new(48_000);
        let mut chunked = vec![];
        for c in x.chunks(777) {
            chunked.extend(s.push(c));
        }
        assert_eq!(whole.len(), chunked.len());
        assert!(whole.iter().zip(&chunked).all(|(a, b)| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)));
    }

    #[test]
    fn frame_rate_is_twenty_per_second() {
        let frames = SpectrumStream::new(44_100).push(&sine(300.0, 3.0, 44_100));
        assert!((frames.len() as i32 - 57).abs() <= 2, "{}", frames.len()); // (3 s - window) * 20/s
    }
}
