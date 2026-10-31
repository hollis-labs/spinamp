//! Background audio analysis: for each track with an ISRC, fetch a 30 s preview (Deezer, then
//! iTunes), measure tempo / loudness / energy / loud spikes and a beat-aligned visual pattern
//! locally, and store only the numbers. The audio itself is held in memory for a moment and
//! dropped; nothing is kept on disk.
//!
//! Runs in batches so the ~0.5 MB-per-track download is spread out: a manual run does one batch,
//! "automatic" mode does a batch, waits, and continues until everything is measured.

use crate::{Shared, now};
use spinamp_core::{AudioFeatures, VizPattern, primary_artist};
use spinamp_services::{deezer, itunes};
use spinamp_store::AudioJob;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use tauri::{AppHandle, Manager};
use tokio::task::JoinSet;

#[derive(Default)]
pub struct Analysis {
    pub running: AtomicBool,
    pub stop: AtomicBool,
    pub total: AtomicU32,
    pub done: AtomicU32,
    pub failed: AtomicU32,
    /// Unix seconds when the next automatic batch starts (0 = not waiting).
    pub next_at: AtomicI64,
    pub current: Mutex<String>,
    pub error: Mutex<String>,
}

const RETRY_SECS: i64 = 30 * 24 * 3600;
const MAX_BYTES: usize = 3_000_000;
const CONCURRENCY: usize = 3;
const MAX_CONSECUTIVE_ERRORS: u32 = 8;

type Measured = (AudioFeatures, Option<VizPattern>);

/// `Ok(None)` = no usable preview or unreadable audio (recorded, retried in 30 days).
/// `Err` = network/quota trouble (not recorded; picked up next run).
async fn analyze_one(job: &AudioJob) -> Result<Option<Measured>, String> {
    let dz = deezer::track_by_isrc(&job.isrc).await?;
    let deezer_bpm = dz.as_ref().and_then(|d| d.bpm);
    let (mut url, mut source, gain, rank) = match &dz {
        Some(d) => (d.preview.clone(), "preview:deezer", d.gain, d.rank),
        None => (None, "", None, None),
    };
    if url.is_none() {
        url = itunes::preview(primary_artist(&job.artist), &job.title).await.unwrap_or(None);
        source = "preview:itunes";
    }
    let Some(url) = url else { return Ok(None) };
    let bytes = deezer::download(&url, MAX_BYTES).await?;
    let analysed = tokio::task::spawn_blocking(move || {
        spinamp_audio::decode(bytes).ok().and_then(|(pcm, sr)| spinamp_audio::analyze(&pcm, sr))
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(analysed.map(|a| {
        let mut f = a.features;
        f.source = source.into();
        f.deezer_gain = gain;
        f.deezer_rank = rank;
        f.deezer_bpm = deezer_bpm;
        (f, a.viz)
    }))
}

/// Start analysing. `all` = everything now; otherwise one batch (and, in automatic mode, keep
/// going batch after batch with a pause between).
pub fn start(handle: &AppHandle, all: bool) -> bool {
    let state = handle.state::<Shared>();
    let an = {
        let a = state.lock().unwrap();
        if a.analysis.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        a.analysis.clone()
    };
    an.stop.store(false, Ordering::Relaxed);
    an.error.lock().unwrap().clear();
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<Shared>();
        loop {
            let (jobs, rolling, gap_secs) = {
                let a = state.lock().unwrap();
                let s = &a.settings;
                let mut jobs = a.store.audio_pending(now(), RETRY_SECS).unwrap_or_default();
                if !all && s.analyze_batch > 0 {
                    jobs.truncate(s.analyze_batch as usize);
                }
                (jobs, s.analyze_auto && !all, s.analyze_gap_minutes as i64 * 60)
            };
            if jobs.is_empty() {
                break;
            }
            run_batch(&handle, &an, jobs).await;
            if !rolling || an.stop.load(Ordering::Relaxed) {
                break;
            }
            let more = state.lock().unwrap().store.audio_pending(now(), RETRY_SECS).map(|j| !j.is_empty()).unwrap_or(false);
            if !more {
                break;
            }
            // pause between batches (checks for a stop request every second)
            an.next_at.store(now() + gap_secs, Ordering::Relaxed);
            while now() < an.next_at.load(Ordering::Relaxed) && !an.stop.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            an.next_at.store(0, Ordering::Relaxed);
            if an.stop.load(Ordering::Relaxed) {
                break;
            }
        }
        an.next_at.store(0, Ordering::Relaxed);
        an.current.lock().unwrap().clear();
        an.running.store(false, Ordering::SeqCst);
    });
    true
}

async fn run_batch(handle: &AppHandle, an: &std::sync::Arc<Analysis>, jobs: Vec<AudioJob>) {
    let state = handle.state::<Shared>();
    an.total.store(jobs.len() as u32, Ordering::Relaxed);
    an.done.store(0, Ordering::Relaxed);
    an.failed.store(0, Ordering::Relaxed);
    let mut jobs = jobs.into_iter();
    let mut set: JoinSet<(AudioJob, Result<Option<Measured>, String>)> = JoinSet::new();
    let mut consecutive_errors = 0;
    loop {
        while set.len() < CONCURRENCY && !an.stop.load(Ordering::Relaxed) {
            let Some(job) = jobs.next() else { break };
            *an.current.lock().unwrap() = format!("{} - {}", job.artist, job.title);
            set.spawn(async move {
                let r = analyze_one(&job).await;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await; // be polite
                (job, r)
            });
        }
        let Some(res) = set.join_next().await else { break };
        let Ok((job, r)) = res else { continue };
        match r {
            Ok(m) => {
                consecutive_errors = 0;
                let (f, v) = match &m {
                    Some((f, v)) => (Some(f), v.as_ref()),
                    None => (None, None),
                };
                let _ = state.lock().unwrap().store.save_audio(&job.id, f, v, now());
                let done = an.done.fetch_add(1, Ordering::Relaxed) + 1;
                if done % 25 == 0 {
                    apply(&state);
                }
            }
            Err(e) => {
                an.failed.fetch_add(1, Ordering::Relaxed);
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    *an.error.lock().unwrap() = format!("Stopped after repeated errors: {e}");
                    an.stop.store(true, Ordering::Relaxed);
                }
            }
        }
    }
    apply(&state);
}

/// Copy new measurements onto the tracks and refresh the in-memory library and discovery pool.
fn apply(state: &Shared) {
    let mut a = state.lock().unwrap();
    let _ = a.store.apply_audio_features();
    let _ = crate::reconcile_tempos(&mut a);
    let _ = crate::reload(&mut a);
    a.candidates = a.store.load_candidates().unwrap_or_default();
}
