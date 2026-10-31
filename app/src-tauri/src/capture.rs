//! Live capture: hear what Spotify is actually playing, so the visualiser can show the real
//! spectrum and we can record a time-aligned fingerprint for later listens.
//!
//! macOS: ScreenCaptureKit stream filtered to the Spotify desktop app's audio only (needs the
//! Screen Recording permission). Audio is reduced to 16 band levels every 50 ms and the raw audio
//! is discarded immediately; only those numbers are kept.
//!
//! Pipeline: SCK callback (tiny) -> channel -> processing thread: SpectrumStream -> position
//! lookup -> Recording (saved to sqlite, merged across listens) and a live event to the UI.

use crate::{Shared, now};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::Serialize;
use spinamp_audio::BANDS;
use spinamp_core::{Track, TrackId};
use spinamp_store::{CaptureRow, Store, capture_key};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

pub const FRAME_MS: u32 = 50;
const DB_MIN: f32 = -60.0;
const DB_MAX: f32 = 60.0;
const FULL_COVERAGE: f32 = 0.95;
const SAVE_EVERY: Duration = Duration::from_secs(20);

fn db_to_u8(db: f32) -> u8 {
    (((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0) * 254.0) as u8 + 1
}

struct PosClock {
    pos_ms: u64,
    at: Instant,
    playing: bool,
}

impl PosClock {
    fn pos(&self) -> Option<u64> {
        self.playing.then(|| self.pos_ms + self.at.elapsed().as_millis() as u64)
    }
}

struct Recording {
    track: TrackId,
    /// Where the fingerprint is stored/shared: ISRC-based, see `capture_key`.
    key: String,
    frames: Vec<u8>,
    present: Vec<u8>,
    duration_ms: u64,
    coverage: f32,
    dirty: bool,
    last_save: Instant,
    /// Running per-band range for normalising the live display.
    lo: [f32; BANDS],
    hi: [f32; BANDS],
}

impl Recording {
    fn new(track: TrackId, key: String, duration_ms: u64, existing: Option<CaptureRow>) -> Self {
        let n = (duration_ms / FRAME_MS as u64) as usize + 2;
        let (mut frames, mut present) = (vec![0u8; n * BANDS], vec![0u8; n]);
        let mut coverage = 0.0;
        if let Some(c) = existing.filter(|c| c.frame_ms == FRAME_MS && c.bands as usize == BANDS) {
            coverage = c.coverage();
            let m = c.present.len().min(n);
            frames[..m * BANDS].copy_from_slice(&c.frames[..m * BANDS]);
            present[..m].copy_from_slice(&c.present[..m]);
        }
        Self { track, key, frames, present, duration_ms, coverage, dirty: false, last_save: Instant::now(), lo: [0.0; BANDS], hi: [0.0; BANDS] }
    }

    fn row(&self) -> CaptureRow {
        CaptureRow { frame_ms: FRAME_MS, bands: BANDS as u8, frames: self.frames.clone(), present: self.present.clone(), duration_ms: self.duration_ms }
    }
}

pub struct Capture {
    pub running: AtomicBool,
    stop: AtomicBool,
    pub error: Mutex<String>,
    clock: Mutex<PosClock>,
    rec: Mutex<Option<Recording>>,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            error: Mutex::new(String::new()),
            clock: Mutex::new(PosClock { pos_ms: 0, at: Instant::now(), playing: false }),
            rec: Mutex::new(None),
        }
    }
}

impl Capture {
    /// A new track started playing: save the previous recording, load any existing fingerprint.
    pub fn track_started(&self, store: &Store, track: &Track) {
        let mut rec = self.rec.lock().unwrap();
        if let Some(old) = rec.as_ref() {
            if old.dirty {
                let _ = store.save_capture(&old.key, &old.row(), now());
            }
        }
        let key = capture_key(track);
        let existing = store.get_capture(&key).ok().flatten();
        *rec = Some(Recording::new(track.id.clone(), key, track.duration_ms, existing));
        *self.clock.lock().unwrap() = PosClock { pos_ms: 0, at: Instant::now(), playing: true };
    }

    pub fn update_clock(&self, pos_ms: u64, playing: bool) {
        *self.clock.lock().unwrap() = PosClock { pos_ms, at: Instant::now(), playing };
    }

    pub fn set_playing(&self, playing: bool) {
        let mut c = self.clock.lock().unwrap();
        let pos = c.pos().unwrap_or(c.pos_ms);
        *c = PosClock { pos_ms: pos, at: Instant::now(), playing };
    }

    /// Should audio be captured right now?
    fn wanted(&self) -> bool {
        let playing = self.clock.lock().unwrap().playing;
        let incomplete = self.rec.lock().unwrap().as_ref().is_some_and(|r| r.coverage < FULL_COVERAGE);
        playing && incomplete
    }
}

#[derive(Serialize, Clone)]
struct LivePayload {
    track_id: String,
    bands: Vec<u8>,
}

/// Handle one analysed frame: record it at the current playback position and stream it live.
fn on_frame(handle: &AppHandle, cap: &Capture, f: [f32; BANDS]) {
    let Some(pos) = cap.clock.lock().unwrap().pos() else { return };
    let mut guard = cap.rec.lock().unwrap();
    let Some(rec) = guard.as_mut() else { return };
    let heard = f.iter().any(|d| *d > DB_MIN + 5.0);

    if heard {
        let idx = (pos / FRAME_MS as u64) as usize;
        if idx < rec.present.len() && rec.present[idx] == 0 {
            for b in 0..BANDS {
                rec.frames[idx * BANDS + b] = db_to_u8(f[b]);
            }
            rec.present[idx] = 1;
            rec.dirty = true;
            let have = rec.present.iter().filter(|p| **p != 0).count();
            rec.coverage = have as f32 / rec.present.len() as f32;
        }
    }

    // live display: adaptive per-band range so quiet and loud tracks both fill the chart
    let mut bands = vec![0u8; BANDS];
    for b in 0..BANDS {
        let v = f[b];
        rec.hi[b] = if v > rec.hi[b] { v } else { (rec.hi[b] - 0.12).max(v) };
        rec.lo[b] = if rec.lo[b] == 0.0 || v < rec.lo[b] { v } else { (rec.lo[b] + 0.05).min(v) };
        let range = (rec.hi[b] - rec.lo[b]).max(14.0);
        bands[b] = (((v - rec.lo[b]) / range).clamp(0.0, 1.0) * 255.0) as u8;
    }
    let payload = LivePayload { track_id: rec.track.0.clone(), bands };

    if rec.dirty && rec.last_save.elapsed() >= SAVE_EVERY {
        rec.last_save = Instant::now();
        rec.dirty = false;
        let (key, row) = (rec.key.clone(), rec.row());
        drop(guard);
        let state = handle.state::<Shared>();
        let _ = state.lock().unwrap().store.save_capture(&key, &row, now());
    } else {
        drop(guard);
    }
    let _ = handle.emit("viz-live", payload);
}

/// Save whatever has been recorded so far (call when stopping).
fn flush(handle: &AppHandle, cap: &Capture) {
    let (key, row) = {
        let mut g = cap.rec.lock().unwrap();
        let Some(rec) = g.as_mut().filter(|r| r.dirty) else { return };
        rec.dirty = false;
        (rec.key.clone(), rec.row())
    };
    let state = handle.state::<Shared>();
    let _ = state.lock().unwrap().store.save_capture(&key, &row, now());
}

/// Start or stop the capture stream to match what is wanted right now. Cheap; call freely.
pub fn reconcile(handle: &AppHandle) {
    let state = handle.state::<Shared>();
    let (cap, enabled) = {
        let a = state.lock().unwrap();
        (a.capture.clone(), a.settings.capture_enabled)
    };
    let wanted = enabled && cap.wanted();
    let running = cap.running.load(Ordering::SeqCst);
    if wanted && !running {
        if cap.running.swap(true, Ordering::SeqCst) {
            return;
        }
        cap.stop.store(false, Ordering::SeqCst);
        cap.error.lock().unwrap().clear();
        let (handle, cap) = (handle.clone(), cap.clone());
        std::thread::spawn(move || {
            if let Err(e) = run_stream(&handle, &cap) {
                *cap.error.lock().unwrap() = e;
            }
            flush(&handle, &cap);
            cap.running.store(false, Ordering::SeqCst);
        });
    } else if !wanted && running {
        cap.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(target_os = "macos")]
mod sck {
    use screencapturekit::prelude::*;
    use std::sync::mpsc::Sender;

    pub const SPOTIFY_BUNDLE: &str = "com.spotify.client";

    pub struct AudioHandler {
        pub tx: Sender<(Vec<f32>, u32)>,
    }

    impl SCStreamOutputTrait for AudioHandler {
        fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
            if !matches!(of_type, SCStreamOutputType::Audio) {
                return;
            }
            let rate = sample.format_description().and_then(|f| f.audio_sample_rate()).unwrap_or(48_000.0) as u32;
            let Ok(list) = sample.audio_buffer_list() else { return };
            let f32s = |bytes: &[u8]| -> Vec<f32> { bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect() };
            let mut mono: Vec<f32> = vec![];
            let n = list.num_buffers();
            if n >= 2 {
                // planar: one buffer per channel
                let chans: Vec<Vec<f32>> = list.iter().map(|b| f32s(b.data())).collect();
                let len = chans.iter().map(|c| c.len()).min().unwrap_or(0);
                mono = (0..len).map(|i| chans.iter().map(|c| c[i]).sum::<f32>() / n as f32).collect();
            } else if let Some(b) = list.get(0) {
                // interleaved
                let ch = (b.number_channels as usize).max(1);
                let all = f32s(b.data());
                mono = all.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
            }
            if !mono.is_empty() {
                let _ = self.tx.send((mono, rate));
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn run_stream(handle: &AppHandle, cap: &Arc<Capture>) -> Result<(), String> {
    use screencapturekit::prelude::*;
    use spinamp_audio::SpectrumStream;

    let content = SCShareableContent::get().map_err(|e| format!("Screen Recording permission is needed: {e}"))?;
    let apps = content.applications();
    let spotify = apps
        .iter()
        .find(|a| a.bundle_identifier() == sck::SPOTIFY_BUNDLE)
        .ok_or("The Spotify desktop app is not running, so there is nothing to capture.")?;
    let display = content.displays().into_iter().next().ok_or("No display found")?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_including_applications(&[spotify], &[])
        .build()
        .map_err(|e| e.to_string())?;
    let config = SCStreamConfiguration::new()
        .with_width(16)
        .with_height(16)
        .with_captures_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(2);

    let (tx, rx) = std::sync::mpsc::channel::<(Vec<f32>, u32)>();
    let mut stream = SCStream::new(&filter, &config).map_err(|e| e.to_string())?;
    stream.add_output_handler(sck::AudioHandler { tx }, SCStreamOutputType::Audio).map_err(|e| e.to_string())?;
    stream.start_capture().map_err(|e| e.to_string())?;

    let (mut spec, mut rate) = (SpectrumStream::new(48_000), 48_000u32);
    while !cap.stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok((mono, sr)) => {
                if sr != rate {
                    spec = SpectrumStream::new(sr);
                    rate = sr;
                }
                for frame in spec.push(&mono) {
                    on_frame(handle, cap, frame);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = stream.stop_capture();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn run_stream(_handle: &AppHandle, _cap: &Arc<Capture>) -> Result<(), String> {
    Err("Live capture is only available on macOS so far.".into())
}

/// Ask for the permission (macOS shows its prompt the first time) and check Spotify is visible.
#[tauri::command]
pub fn capture_check() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        use screencapturekit::prelude::*;
        let content = SCShareableContent::get().map_err(|e| {
            format!("Not allowed yet ({e}). Open System Settings > Privacy & Security > Screen & System Audio Recording, enable the app that launched Spinamp (Terminal, iTerm or Spinamp), then quit and reopen it.")
        })?;
        let found = content.applications().iter().any(|a| a.bundle_identifier() == sck::SPOTIFY_BUNDLE);
        return Ok(if found { "Permission granted and Spotify is running. Capture is ready.".into() } else { "Permission granted. Open the Spotify desktop app to capture its audio.".into() });
    }
    #[allow(unreachable_code)]
    Err("Live capture is only available on macOS so far.".into())
}

#[derive(Serialize)]
pub struct CaptureStatus {
    supported: bool,
    enabled: bool,
    running: bool,
    error: String,
    /// Tracks with any recording / tracks fully recorded.
    recorded: u32,
    complete: u32,
}

#[tauri::command]
pub fn capture_status(app: State<Shared>) -> CaptureStatus {
    let a = app.lock().unwrap();
    let (recorded, complete) = a.store.capture_counts().unwrap_or((0, 0));
    CaptureStatus {
        supported: cfg!(target_os = "macos"),
        enabled: a.settings.capture_enabled,
        running: a.capture.running.load(Ordering::SeqCst),
        error: a.capture.error.lock().unwrap().clone(),
        recorded,
        complete,
    }
}

/// Per-frame loudness of each layer (bass, low, mid, high) in dB, stretched across the song's own
/// range to 0..=255 (5th..98th percentile), so quiet passages read low and loud ones high. This is
/// the song's "shape". Missing frames are 0. (Averaging per-band-normalised values instead would
/// flatten everything toward the middle.)
pub fn shape_groups(frames: &[u8], present: &[u8], bands: usize) -> Vec<u8> {
    let n = present.len();
    let g = (bands / 4).max(1);
    let to_db = |v: u8| DB_MIN + (v.saturating_sub(1) as f32 / 254.0) * (DB_MAX - DB_MIN);
    let mut db = vec![[f32::NAN; 4]; n];
    for i in (0..n).filter(|i| present[*i] != 0) {
        for k in 0..4 {
            let power: f32 = (k * g..((k + 1) * g).min(bands)).map(|b| 10f32.powf(to_db(frames[i * bands + b]) / 10.0) * 1.41f32.powi(b as i32)).sum();
            db[i][k] = 10.0 * (power + 1e-12).log10();
        }
    }
    let mut out = vec![0u8; n * 4];
    for k in 0..4 {
        let mut vals: Vec<f32> = db.iter().map(|d| d[k]).filter(|v| !v.is_nan()).collect();
        if vals.len() < 4 {
            continue;
        }
        vals.sort_by(|a, b| a.total_cmp(b));
        let lo = vals[vals.len() * 5 / 100];
        let hi = vals[(vals.len() * 98 / 100).min(vals.len() - 1)].max(lo + 3.0);
        for i in 0..n {
            if !db[i][k].is_nan() {
                out[i * 4 + k] = (((db[i][k] - lo) / (hi - lo)).clamp(0.0, 1.0) * 254.0) as u8 + 1;
            }
        }
    }
    out
}

/// A recorded fingerprint, normalised per band to 0..=255 for display.
#[derive(Serialize)]
pub struct CaptureView {
    frame_ms: u32,
    bands: u8,
    coverage: f32,
    /// base64, `frames * bands` bytes
    data: String,
    /// base64, one byte per frame (1 = heard)
    present: String,
    /// base64, `frames * 4` bytes: per-frame loudness of bass/low/mid/high across the song's range
    shape: String,
}

#[tauri::command]
pub fn viz_capture(app: State<Shared>, track_id: String) -> Option<CaptureView> {
    let a = app.lock().unwrap();
    let id = TrackId(track_id);
    let key = a.store.capture_key_for(&id).ok()?;
    // prefer the recording in progress (it is ahead of what has been saved)
    let live = a.capture.rec.lock().unwrap().as_ref().filter(|r| r.key == key).map(|r| r.row());
    let row = live.or_else(|| a.store.get_capture(&key).ok().flatten())?;
    let b = row.bands as usize;
    let n = row.present.len();
    let mut data = vec![0u8; row.frames.len()];
    for band in 0..b {
        let mut vals: Vec<u8> = (0..n).filter(|i| row.present[*i] != 0).map(|i| row.frames[i * b + band]).collect();
        if vals.len() < 10 {
            continue;
        }
        vals.sort_unstable();
        let (lo, hi) = (vals[vals.len() / 10] as f32, (vals[vals.len() * 98 / 100] as f32).max(vals[vals.len() / 10] as f32 + 6.0));
        for i in 0..n {
            if row.present[i] != 0 {
                data[i * b + band] = (((row.frames[i * b + band] as f32 - lo) / (hi - lo)).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
    let shape = shape_groups(&row.frames, &row.present, b);
    Some(CaptureView {
        frame_ms: row.frame_ms,
        bands: row.bands,
        coverage: row.coverage(),
        data: B64.encode(data),
        present: B64.encode(&row.present),
        shape: B64.encode(shape),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_mapping_is_monotonic_and_never_zero() {
        assert_eq!(db_to_u8(-100.0), 1, "zero is reserved for 'missing'");
        assert!(db_to_u8(-20.0) < db_to_u8(20.0));
        assert_eq!(db_to_u8(500.0), 255);
    }

    #[test]
    fn recording_resumes_from_an_existing_capture() {
        let existing = CaptureRow { frame_ms: FRAME_MS, bands: BANDS as u8, frames: vec![9; 10 * BANDS], present: vec![1, 1, 1, 1, 1, 1, 1, 1, 0, 0], duration_ms: 500 };
        let r = Recording::new(TrackId::new("spotify", "x"), "isrc:X".into(), 500, Some(existing));
        assert!(r.coverage > 0.6 && r.coverage < 0.9, "{}", r.coverage);
        assert_eq!(r.frames[0], 9);
        assert!(r.present.len() >= 10);
    }

    #[test]
    fn shape_reflects_loudness_changes_instead_of_flattening() {
        // 40 frames, 16 bands: quiet first half, loud second half, bass group louder than the rest
        let bands = 16;
        let mut frames = vec![0u8; 40 * bands];
        let present = vec![1u8; 40];
        for i in 0..40 {
            for b in 0..bands {
                let base = if i < 20 { 90 } else { 170 };
                frames[i * bands + b] = if b < 4 { base + 20 } else { base };
            }
        }
        let shape = shape_groups(&frames, &present, bands);
        let at = |i: usize, k: usize| shape[i * 4 + k] as i32;
        assert!(at(30, 0) > at(5, 0) + 150, "loud section reads high: {} vs {}", at(30, 0), at(5, 0));
        assert!(at(30, 3) > at(5, 3) + 150);
        // missing frames stay 0
        let mut p2 = present.clone();
        p2[10] = 0;
        assert_eq!(shape_groups(&frames, &p2, bands)[10 * 4], 0);
    }

    #[test]
    fn position_clock_only_runs_while_playing() {
        let c = Capture::default();
        c.update_clock(1000, true);
        assert!(c.clock.lock().unwrap().pos().unwrap() >= 1000);
        c.set_playing(false);
        assert!(c.clock.lock().unwrap().pos().is_none());
    }
}
