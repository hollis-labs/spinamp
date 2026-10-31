mod analysis;
mod capture;
mod catalog;
mod credentials;
mod extras;
mod settings;

use rand::rngs::StdRng;
use rand::SeedableRng;
use serde::Serialize;
use spinamp_core::engine::{self, Context, EngineConfig, PlayMode, Pick};
use spinamp_core::feedback::FeedbackConfig;
use spinamp_core::sort::{self, SortKey};
use spinamp_core::{Track, TrackId, TrackState};
use spinamp_core::block::{Blocklist, playable};
use spinamp_core::listen::{Event, Observation, Tracker};
use spinamp_core::provider::MusicProvider;
use spinamp_core::stats::{self, StatsConfig};
use spinamp_spotify::SpotifyProvider;
use spinamp_store::{PlayLogRow, PlayStart, PlaylistInfo, Store};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tauri::{AppHandle, Manager, State};
use settings::Settings;

#[derive(Default)]
struct Enrich {
    running: AtomicBool,
    stop: AtomicBool,
    total: AtomicU32,
    done: AtomicU32,
    current: Mutex<String>,
}

struct App {
    blocklist: Blocklist,
    capture: Arc<capture::Capture>,
    analysis: Arc<analysis::Analysis>,
    settings: Settings,
    catalog: Arc<catalog::Catalog>,
    enrich: Arc<Enrich>,
    store: Store,
    spotify: Arc<SpotifyProvider>,
    library: Vec<TrackState>,
    /// Selected playlist id and its ordered members; None = all tracks.
    view: Option<(String, Vec<TrackId>)>,
    candidates: Vec<Track>,
    history: Vec<TrackId>,
    suggested: Vec<TrackId>,
    rng: StdRng,
    cfg: EngineConfig,
    tracker: Tracker,
    play_row: Option<i64>,
    ctx_saved: bool,
    now_track: Option<Track>,
    stats_cfg: StatsConfig,
}

type Shared = Mutex<App>;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Serialize)]
struct Row {
    #[serde(flatten)]
    track: Track,
    weight: f64,
    plays: u32,
    listens: u32,
    cooling: bool,
    rating: Option<u8>,
    favorite: bool,
    /// Muted until this unix time (only present while the mute is active).
    muted_until: Option<i64>,
    /// Matches a block on its song, artist or a tag/genre.
    blocked: bool,
}

#[derive(Serialize)]
struct Next {
    track: Track,
    discovery: bool,
}

/// The tracks currently in scope (selected playlist or whole library), in display order.
fn view_states(a: &App) -> Vec<TrackState> {
    match &a.view {
        None => a.library.clone(),
        Some((_, ids)) => {
            let by_id: std::collections::HashMap<_, _> = a.library.iter().map(|t| (&t.track.id, t)).collect();
            ids.iter().filter_map(|id| by_id.get(id).map(|t| (*t).clone())).collect()
        }
    }
}

fn rows(app: &App) -> Vec<Row> {
    let (n, fb) = (now(), &app.cfg.shuffle.feedback);
    // plays/listens come from the play log (quiet hours excluded), not the raw counter.
    let counts = stats::tally(&app.store.play_events().unwrap_or_default(), &app.stats_cfg);
    view_states(app)
        .iter()
        .map(|t| Row {
            track: t.track.clone(),
            weight: t.votes.score(n, fb),
            plays: counts.get(&t.track.id).map_or(0, |c| c.plays),
            listens: counts.get(&t.track.id).map_or(0, |c| c.listens),
            cooling: t.votes.cooldown_until(n, fb).is_some(),
            rating: t.rating,
            favorite: t.favorite,
            muted_until: t.muted_until.filter(|u| *u > n),
            blocked: app.blocklist.blocks(&t.track),
        })
        .collect()
}

#[tauri::command]
fn library(app: State<Shared>) -> Vec<Row> {
    rows(&app.lock().unwrap())
}

#[tauri::command]
fn sort_library(app: State<Shared>, key: SortKey, descending: bool) -> Vec<Row> {
    let mut a = app.lock().unwrap();
    let fb = FeedbackConfig::default();
    let mut v = view_states(&a);
    sort::sort(&mut v, key, descending, now(), &fb);
    match &mut a.view {
        Some((_, ids)) => *ids = v.iter().map(|t| t.track.id.clone()).collect(),
        None => a.library = v,
    }
    rows(&a)
}

#[tauri::command]
async fn next_track(app: State<'_, Shared>, handle: AppHandle, mode: PlayMode) -> Result<Option<Next>, String> {
    let (picked, spotify) = pick_next(&app, mode);
    if let Some(p) = &picked {
        if p.track.provider == "spotify" {
            play_or_wake(&spotify, &p.track).await?;
        }
        begin_play(&mut app.lock().unwrap(), &p.track, &format!("{mode:?}"), "auto", p.discovery);
        capture::reconcile(&handle);
        if p.discovery && pool_low(&handle) {
            catalog::maybe_refill(&handle, true);
        }
    }
    Ok(picked)
}

fn pick_next(app: &Shared, mode: PlayMode) -> (Option<Next>, Arc<SpotifyProvider>) {
    let mut guard = app.lock().unwrap();
    let a = &mut *guard;
    let spotify = a.spotify.clone();
    let picked = pick_inner(a, mode);
    (picked, spotify)
}

/// Play on Spotify; if no device is online, launch the desktop app (macOS) and retry for ~12s.
async fn play_or_wake(sp: &SpotifyProvider, track: &Track) -> Result<(), String> {
    let first = sp.play(track).await.map_err(|e| e.to_string())
        .err();
    let Some(err) = first else { return Ok(()) };
    if !err.contains("NO_DEVICE") || !cfg!(target_os = "macos") {
        return Err(err);
    }
    let _ = std::process::Command::new("open").args(["-a", "Spotify"]).status();
    for _ in 0..12 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if sp.play(track).await.is_ok() {
            return Ok(());
        }
    }
    Err("Spotify launched but no device became available. Open Spotify, play anything once, then retry.".into())
}

fn pool_low(handle: &AppHandle) -> bool {
    catalog::pool_size(handle) < catalog::POOL_LOW
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Persist what the listen tracker worked out.
fn apply(a: &mut App, events: Vec<Event>) {
    for ev in events {
        match ev {
            Event::Progress { listened_ms } => {
                if let Some(r) = a.play_row {
                    let _ = a.store.update_play(r, listened_ms, None);
                }
            }
            Event::Ended { listened_ms, position_ms, reason, .. } => {
                if let Some(r) = a.play_row.take() {
                    let _ = a.store.update_play(r, listened_ms, Some((reason.as_str(), position_ms)));
                }
            }
            Event::Restarted { track } => {
                let dur = a.library.iter().find(|t| t.track.id == track).map_or(0, |t| t.track.duration_ms);
                open_row(a, &track, dur, "repeat", "auto");
            }
            Event::External { track } => {
                // Changed in the Spotify app: adopt it only if it is something in our library.
                if let Some(t) = a.library.iter().find(|t| t.track.id == track).map(|t| t.track.clone()) {
                    let _ = a.tracker.begin(track.clone(), t.duration_ms, now_ms());
                    open_row(a, &track, t.duration_ms, "external", "external");
                    a.capture.track_started(&a.store, &t);
                    a.now_track = Some(t);
                    a.history.push(track);
                }
            }
        }
    }
}

fn open_row(a: &mut App, id: &TrackId, duration_ms: u64, mode: &str, trigger: &str) {
    let n = now();
    a.play_row = a
        .store
        .start_play(&PlayStart {
            track: Some(id.clone()),
            started_at: n,
            duration_ms,
            mode: mode.into(),
            trigger: trigger.into(),
            from_playlist: a.view.as_ref().map(|(p, _)| p.clone()),
        })
        .ok();
    a.ctx_saved = false;
    if let Some(t) = a.library.iter_mut().find(|t| &t.track.id == id) {
        t.play_count += 1;
        t.last_played = Some(n);
    }
}

/// Call once playback has actually started. Closes the previous session, opens a new one.
fn begin_play(a: &mut App, track: &Track, mode: &str, trigger: &str, discovery: bool) {
    if let Some(ended) = a.tracker.begin(track.id.clone(), track.duration_ms, now_ms()) {
        apply(a, vec![ended]);
    }
    open_row(a, &track.id, track.duration_ms, mode, trigger);
    a.capture.track_started(&a.store, track);
    a.now_track = Some(track.clone());
    if discovery {
        let _ = a.store.mark_candidate(&track.id, "played");
        a.candidates.retain(|c| c.id != track.id);
        a.suggested.push(track.id.clone());
    } else {
        a.history.push(track.id.clone());
    }
}

/// Play a specific track (clicked in the playlist).
#[tauri::command]
async fn play_track(app: State<'_, Shared>, handle: AppHandle, id: String) -> Result<Option<Next>, String> {
    let (track, spotify) = {
        let a = app.lock().unwrap();
        let Some(t) = a.library.iter().find(|t| t.track.id.0 == id).map(|t| t.track.clone()) else { return Ok(None) };
        (t, a.spotify.clone())
    };
    if track.provider == "spotify" {
        play_or_wake(&spotify, &track).await?;
    }
    begin_play(&mut app.lock().unwrap(), &track, "manual", "click", false);
    capture::reconcile(&handle);
    Ok(Some(Next { track, discovery: false }))
}

#[derive(Serialize)]
struct PollResult {
    track: Option<Track>,
    is_playing: bool,
    position_ms: u64,
    /// Spotify is playing the track we think is playing.
    matches: bool,
}

/// Called every couple of seconds by the UI: syncs the clock and records real listening.
#[tauri::command]
async fn poll_playback(app: State<'_, Shared>, handle: AppHandle) -> Result<PollResult, String> {
    let idle = PollResult { track: None, is_playing: false, position_ms: 0, matches: false };
    let sp = app.lock().unwrap().spotify.clone();
    if !sp.is_authenticated().await {
        return Ok(idle);
    }
    let Ok(pb) = sp.now_playing().await else { return Ok(idle) };
    let mut guard = app.lock().unwrap();
    let a = &mut *guard;
    let evs = a.tracker.observe(Observation {
        at_ms: now_ms(),
        track: pb.track.as_ref(),
        is_playing: pb.is_playing,
        position_ms: pb.position_ms,
    });
    apply(a, evs);
    let matches = a.tracker.current().is_some() && a.tracker.current() == pb.track.as_ref();
    a.capture.update_clock(pb.position_ms, pb.is_playing && matches);
    if matches && !a.ctx_saved {
        if let Some(r) = a.play_row {
            let _ = a.store.update_play_context(r, pb.device_name.as_deref(), pb.device_type.as_deref(), pb.volume_percent, pb.shuffle);
            a.ctx_saved = true;
        }
    }
    let result = PollResult { track: a.now_track.clone().filter(|_| matches), is_playing: pb.is_playing, position_ms: pb.position_ms, matches };
    drop(guard);
    capture::reconcile(&handle);
    Ok(result)
}

/// Full play log with context, for analysis and future algorithms.
#[tauri::command]
fn play_log(app: State<Shared>) -> Result<Vec<PlayLogRow>, String> {
    app.lock().unwrap().store.play_log().map_err(|e| e.to_string())
}

/// Local UTC offset from the webview (JS `getTimezoneOffset`, minutes west), for quiet hours.
#[tauri::command]
fn set_tz_offset(app: State<Shared>, minutes_west: i64) {
    app.lock().unwrap().stats_cfg.utc_offset_secs = -minutes_west * 60;
}

#[tauri::command]
async fn playback_pause(app: State<'_, Shared>, handle: AppHandle) -> Result<(), String> {
    let sp = app.lock().unwrap().spotify.clone();
    app.lock().unwrap().capture.set_playing(false);
    capture::reconcile(&handle);
    if sp.is_authenticated().await { sp.pause().await.map_err(|e| e.to_string())?; }
    Ok(())
}

#[tauri::command]
async fn playback_resume(app: State<'_, Shared>, handle: AppHandle) -> Result<(), String> {
    let sp = app.lock().unwrap().spotify.clone();
    app.lock().unwrap().capture.set_playing(true);
    capture::reconcile(&handle);
    if sp.is_authenticated().await { sp.resume().await.map_err(|e| e.to_string())?; }
    Ok(())
}


/// Vote on any track, including a discovery pick that is not in the library yet.
/// Voting adds it to the library so the opinion is remembered (and it will not be suggested again).
#[tauri::command]
fn vote_track(app: State<Shared>, track: Track, up: bool) -> Result<Vec<Row>, String> {
    let mut guard = app.lock().unwrap();
    let a = &mut *guard;
    let n = now();
    if !a.library.iter().any(|t| t.track.id == track.id) {
        a.store.add_track(&track, n).map_err(|e| e.to_string())?;
        reload(a)?;
        a.store.tag_from_artist_cache().map_err(|e| e.to_string())?;
        reload(a)?;
    }
    if let Some(t) = a.library.iter_mut().find(|t| t.track.id == track.id) {
        t.votes.record(n, up);
        t.updated_at = n;
        let _ = a.store.record_vote(&t.track.id, n, up);
    }
    Ok(rows(a))
}

#[derive(Serialize)]
struct EnrichStatus {
    running: bool,
    done: u32,
    total: u32,
    current: String,
    /// Artists in the library not yet looked up.
    missing: u32,
}

fn enrich_status_of(a: &App) -> EnrichStatus {
    EnrichStatus {
        running: a.enrich.running.load(Ordering::Relaxed),
        done: a.enrich.done.load(Ordering::Relaxed),
        total: a.enrich.total.load(Ordering::Relaxed),
        current: a.enrich.current.lock().unwrap().clone(),
        missing: a.store.artists_missing().map(|m| m.len() as u32).unwrap_or(0),
    }
}

#[tauri::command]
fn enrich_status(app: State<Shared>) -> EnrichStatus {
    enrich_status_of(&app.lock().unwrap())
}

#[tauri::command]
fn enrich_stop(app: State<Shared>) {
    app.lock().unwrap().enrich.stop.store(true, Ordering::Relaxed);
}

/// Look up tags for every artist we have not seen, one per second, in the background.
#[tauri::command]
fn enrich_start(app: State<Shared>, handle: AppHandle) -> bool {
    let (enrich, missing, contact) = {
        let a = app.lock().unwrap();
        if a.enrich.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        (a.enrich.clone(), a.store.artists_missing().unwrap_or_default(), a.settings.contact())
    };
    enrich.stop.store(false, Ordering::Relaxed);
    enrich.total.store(missing.len() as u32, Ordering::Relaxed);
    enrich.done.store(0, Ordering::Relaxed);
    tauri::async_runtime::spawn(async move {
        let mb = spinamp_services::musicbrainz::MusicBrainz::new(contact.as_deref());
        for name in missing {
            if enrich.stop.load(Ordering::Relaxed) {
                break;
            }
            *enrich.current.lock().unwrap() = name.clone();
            let mut result = mb.lookup_artist(&name).await;
            if result.is_err() {
                tokio::time::sleep(std::time::Duration::from_secs(6)).await; // rate limited? back off, retry once
                result = mb.lookup_artist(&name).await;
            }
            let state = handle.state::<Shared>();
            if let Ok(info) = result {
                let mut a = state.lock().unwrap();
                let _ = a.store.save_artist(&name, info.as_ref(), now());
                if enrich.done.load(Ordering::Relaxed) % 10 == 9 {
                    let _ = a.store.tag_from_artist_cache();
                    let _ = reload(&mut a);
                }
            }
            enrich.done.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        }
        {
            let state = handle.state::<Shared>();
            let mut a = state.lock().unwrap();
            let _ = a.store.tag_from_artist_cache();
            let _ = reload(&mut a);
        }
        *enrich.current.lock().unwrap() = String::new();
        enrich.running.store(false, Ordering::SeqCst);
    });
    true
}

#[derive(Serialize)]
struct Generation {
    intent: spinamp_core::generate::Intent,
    tracks: Vec<Track>,
    total_ms: u64,
    target_ms: Option<u64>,
    matched: usize,
    used_ai: bool,
    note: String,
    /// Library tracks that have tags at all; generation is only as good as this.
    tagged: usize,
    library: usize,
}

/// AI client if enabled in settings and a key exists (keychain first, env as fallback).
fn ai_client(settings: &Settings) -> Option<spinamp_services::ai::Ai> {
    if !settings.ai_enabled {
        return None;
    }
    let key = settings::secret_get("ai_key").or_else(spinamp_services::ai::Ai::env_key)?;
    Some(spinamp_services::ai::Ai::new(key, settings.ai_model.clone()))
}

#[tauri::command]
fn ai_enabled(app: State<Shared>) -> bool {
    ai_client(&app.lock().unwrap().settings).is_some()
}

#[derive(Serialize)]
struct SettingsView {
    settings: Settings,
    ai_key_set: bool,
    ai_key_from_env: bool,
}

fn settings_view(a: &App) -> SettingsView {
    SettingsView {
        settings: a.settings.clone(),
        ai_key_set: settings::secret_get("ai_key").is_some(),
        ai_key_from_env: settings::secret_get("ai_key").is_none() && spinamp_services::ai::Ai::env_key().is_some(),
    }
}

fn apply_settings(a: &mut App) {
    a.cfg.discovery_ratio = a.settings.discovery_ratio;
    a.cfg.discovery_sharpness = a.settings.discovery_sharpness;
    a.stats_cfg.listen_threshold = a.settings.listen_threshold;
    a.stats_cfg.quiet_hours = a.settings.quiet_enabled.then_some((a.settings.quiet_start, a.settings.quiet_end));
    a.spotify.set_client_id(&a.settings.client_id());
}

#[tauri::command]
fn get_settings(app: State<Shared>) -> SettingsView {
    settings_view(&app.lock().unwrap())
}

#[tauri::command]
fn save_settings(app: State<Shared>, settings: Settings) -> Result<SettingsView, String> {
    let mut a = app.lock().unwrap();
    a.settings = settings.sanitized();
    a.store.kv_set("settings", &serde_json::to_string(&a.settings).unwrap()).map_err(|e| e.to_string())?;
    apply_settings(&mut a);
    Ok(settings_view(&a))
}

/// Store the AI key in the OS keychain. The UI never gets it back.
#[tauri::command]
fn set_ai_key(app: State<Shared>, key: String) -> Result<SettingsView, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Key is empty.".into());
    }
    settings::secret_set("ai_key", key)?;
    Ok(settings_view(&app.lock().unwrap()))
}

#[tauri::command]
fn clear_ai_key(app: State<Shared>) -> Result<SettingsView, String> {
    settings::secret_clear("ai_key")?;
    Ok(settings_view(&app.lock().unwrap()))
}

#[tauri::command]
async fn test_ai(app: State<'_, Shared>) -> Result<String, String> {
    let ai = ai_client(&app.lock().unwrap().settings).ok_or("No AI key set (or AI is turned off).")?;
    ai.ping().await?;
    Ok("Key works.".into())
}

#[derive(Serialize)]
struct AnalysisStatus {
    running: bool,
    done: u32,
    total: u32,
    failed: u32,
    current: String,
    error: String,
    /// Tracks with measured audio / tracks that could have it.
    analyzed: u32,
    eligible: u32,
    /// Still to do (excludes recently failed).
    pending: u32,
    /// Tracks per batch, and seconds until the next automatic batch (0 = not waiting).
    batch: u32,
    next_in_secs: i64,
}

#[tauri::command]
fn analysis_status(app: State<Shared>) -> AnalysisStatus {
    let a = app.lock().unwrap();
    let an = &a.analysis;
    let (analyzed, eligible) = a.store.audio_counts().unwrap_or((0, 0));
    AnalysisStatus {
        running: an.running.load(Ordering::Relaxed),
        done: an.done.load(Ordering::Relaxed),
        total: an.total.load(Ordering::Relaxed),
        failed: an.failed.load(Ordering::Relaxed),
        current: an.current.lock().unwrap().clone(),
        error: an.error.lock().unwrap().clone(),
        analyzed,
        eligible,
        pending: a.store.audio_pending(now(), 30 * 24 * 3600).map(|j| j.len() as u32).unwrap_or(0),
        batch: a.settings.analyze_batch,
        next_in_secs: (an.next_at.load(Ordering::Relaxed) - now()).max(0),
    }
}

/// Measure tempo/energy/loudness from 30 s previews, in the background.
#[tauri::command]
fn analysis_start(handle: AppHandle, all: bool) -> bool {
    analysis::start(&handle, all)
}

/// Beat-aligned 4-band pattern for the visualiser (None until the track is analysed).
#[tauri::command]
fn viz_pattern(app: State<Shared>, track_id: String) -> Option<spinamp_core::VizPattern> {
    app.lock().unwrap().store.get_viz(&TrackId(track_id)).ok().flatten()
}

/// Forget all measured audio (tempo, energy, patterns) so analysis starts over. Recorded
/// captures and everything else are kept.
#[tauri::command]
fn analysis_reset(app: State<Shared>) -> Result<(), String> {
    let mut a = app.lock().unwrap();
    if a.analysis.running.load(Ordering::Relaxed) {
        return Err("Stop the running analysis first.".into());
    }
    a.store.reset_audio().map_err(|e| e.to_string())?;
    reload(&mut a)?;
    a.candidates = a.store.load_candidates().unwrap_or_default();
    Ok(())
}

/// Pick the most plausible tempo for every analysed track (genre priors learned from the library,
/// Deezer's BPM when it has one), keep the raw measurement, and let your own overrides win.
/// Returns how many tracks changed.
pub(crate) fn reconcile_tempos(a: &mut App) -> Result<usize, String> {
    a.library = a.store.load_library().map_err(|e| e.to_string())?;
    let priors = spinamp_core::tempo::TempoPriors::learn(&a.library);
    let user = a.store.user_tempos().map_err(|e| e.to_string())?;
    let mut changed = 0;
    for t in &a.library {
        let want = match user.get(&t.track.id.0) {
            Some(b) => Some(*b),
            None => t.track.meta.audio.as_ref().and_then(|f| spinamp_core::tempo::reconcile(f, &t.track.tags, &priors)),
        };
        if let Some(w) = want {
            if t.track.tempo.is_none_or(|cur| (cur - w).abs() > 0.05) {
                a.store.update_track_tempo(&t.track.id, w).map_err(|e| e.to_string())?;
                changed += 1;
            }
        }
    }
    Ok(changed)
}

/// Re-run tempo reconciliation now (e.g. after more tracks were analysed).
#[tauri::command]
fn tempo_reconcile(app: State<Shared>) -> Result<usize, String> {
    let mut a = app.lock().unwrap();
    let n = reconcile_tempos(&mut a)?;
    reload(&mut a)?;
    Ok(n)
}

/// Set (or clear, with `None`) a track's tempo by hand. Your value always wins.
#[tauri::command]
fn set_tempo(app: State<Shared>, track_id: String, bpm: Option<f32>) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    let id = TrackId(track_id);
    if let Some(b) = bpm {
        if !(30.0..=300.0).contains(&b) {
            return Err("Tempo must be between 30 and 300 BPM.".into());
        }
    }
    a.store.set_user_tempo(&id, bpm, now()).map_err(|e| e.to_string())?;
    match bpm {
        Some(b) => a.store.update_track_tempo(&id, b).map_err(|e| e.to_string())?,
        None => { reconcile_tempos(&mut a)?; }
    }
    reload(&mut a)?;
    Ok(rows(&a))
}

#[tauri::command]
fn analysis_stop(app: State<Shared>) {
    app.lock().unwrap().analysis.stop.store(true, Ordering::Relaxed);
}

#[derive(Serialize)]
struct CatalogStatus {
    running: bool,
    pool: usize,
    error: String,
}

#[tauri::command]
fn catalog_status(app: State<Shared>) -> CatalogStatus {
    let a = app.lock().unwrap();
    CatalogStatus { running: a.catalog.running.load(Ordering::Relaxed), pool: a.candidates.len(), error: a.catalog.error.lock().unwrap().clone() }
}

/// Find more discovery tracks now (otherwise it tops up automatically when the pool runs low).
#[tauri::command]
fn catalog_refill(handle: AppHandle, force: bool) -> bool {
    catalog::maybe_refill(&handle, force)
}

/// Turn "chill 45 minute commute" into a playlist drawn from the library.
#[tauri::command]
async fn generate_playlist(app: State<'_, Shared>, prompt: String, minutes: Option<f64>) -> Result<Generation, String> {
    let vocab: Vec<String> = {
        let a = app.lock().unwrap();
        let mut freq: std::collections::HashMap<&str, usize> = Default::default();
        for t in &a.library {
            for tag in &t.track.tags {
                *freq.entry(tag.as_str()).or_default() += 1;
            }
        }
        let mut v: Vec<_> = freq.into_iter().collect();
        v.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(y.0)));
        v.into_iter().take(150).map(|(t, _)| t.to_string()).collect()
    };
    let heuristic = spinamp_core::generate::parse_heuristic(&prompt);
    let mut note = String::new();
    let mut used_ai = false;
    let mut intent = heuristic.clone();
    let ai = ai_client(&app.lock().unwrap().settings);
    if let Some(ai) = ai {
        match ai.interpret(&prompt, &vocab).await {
            Ok(mut i) => {
                let keep = |tags: Vec<String>| -> Vec<String> {
                    tags.into_iter().map(|t| t.to_lowercase()).filter(|t| vocab.contains(t)).collect()
                };
                i.include_tags = keep(i.include_tags);
                i.exclude_tags = keep(i.exclude_tags);
                if i.include_tags.is_empty() {
                    i.include_tags = heuristic.include_tags.clone();
                }
                if i.name.trim().is_empty() {
                    i.name = heuristic.name.clone();
                }
                intent = i;
                used_ai = true;
            }
            Err(e) => note = format!("AI unavailable ({e}); used keyword matching"),
        }
    }
    if let Some(m) = minutes {
        intent.target_minutes = Some(m);
    }

    let mut guard = app.lock().unwrap();
    let a = &mut *guard;
    let tagged = a.library.iter().filter(|t| !t.track.tags.is_empty()).count();
    let n = now();
    let pool: Vec<TrackState> = a.library.iter().filter(|t| playable(t, &a.blocklist, n)).cloned().collect();
    let g = spinamp_core::generate::generate(&pool, &intent, n, &a.cfg.shuffle.feedback, &mut a.rng);
    if tagged * 2 < a.library.len() && !intent.include_tags.is_empty() {
        note = format!("{note} Only {tagged} of {} tracks have tags yet; tag enrichment is still running, so matches may be loose.", a.library.len()).trim().to_string();
    }
    if intent.has_audio_constraints() {
        let measured = a.library.iter().filter(|t| t.track.tempo.is_some() || t.track.energy.is_some()).count();
        if measured * 2 < a.library.len() {
            note = format!("{note} Only {measured} of {} tracks have measured tempo/energy (run ANALYZE in the top bar), so tempo and energy matching is partial.", a.library.len()).trim().to_string();
        }
    }
    Ok(Generation {
        tracks: g.picks.iter().map(|i| pool[*i].track.clone()).collect(),
        total_ms: g.total_ms,
        target_ms: g.target_ms,
        matched: g.matched,
        used_ai,
        note,
        tagged,
        library: a.library.len(),
        intent,
    })
}

/// Jump to a position in the playing track (click on the song-shape bar).
#[tauri::command]
async fn playback_seek(app: State<'_, Shared>, handle: AppHandle, position_ms: u64) -> Result<(), String> {
    let sp = app.lock().unwrap().spotify.clone();
    if sp.is_authenticated().await {
        sp.seek(position_ms).await.map_err(|e| e.to_string())?;
    }
    app.lock().unwrap().capture.update_clock(position_ms, true);
    capture::reconcile(&handle);
    Ok(())
}

/// Answer the first-run question: let Spinamp listen to Spotify to learn each song's shape?
#[tauri::command]
fn capture_answer(app: State<Shared>, handle: AppHandle, enable: bool, share: bool) -> Result<SettingsView, String> {
    let mut a = app.lock().unwrap();
    a.settings.capture_enabled = enable;
    a.settings.share_fingerprints = share;
    a.settings.capture_prompted = true;
    a.store.kv_set("settings", &serde_json::to_string(&a.settings).unwrap()).map_err(|e| e.to_string())?;
    drop(a);
    capture::reconcile(&handle);
    Ok(settings_view(&app.lock().unwrap()))
}

/// Mute a track for 1, 7 or 30 days (or any number of days); `None` unmutes. Stays in every list.
#[tauri::command]
fn mute_track(app: State<Shared>, track_id: String, days: Option<u32>) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    let until = days.map(|d| now() + d as i64 * 86_400);
    a.store.set_mute(&TrackId(track_id), until).map_err(|e| e.to_string())?;
    reload(&mut a)?;
    Ok(rows(&a))
}

/// Star a track: permanently hotter in shuffle, sorting and taste learning.
#[tauri::command]
fn set_favorite(app: State<Shared>, track_id: String, favorite: bool) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    a.store.set_favorite(&TrackId(track_id), favorite, now()).map_err(|e| e.to_string())?;
    reload(&mut a)?;
    Ok(rows(&a))
}

/// Remove a track from the whole library (all playlists). Soft delete: votes and history stay,
/// imports will not bring it back, Settings > Data can restore it.
#[tauri::command]
fn remove_track_everywhere(app: State<Shared>, track_id: String) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    a.store.remove_track(&TrackId(track_id), now()).map_err(|e| e.to_string())?;
    reload(&mut a)?;
    refresh_view(&mut a)?;
    Ok(rows(&a))
}

#[derive(Serialize)]
struct BlockRow {
    kind: String,
    value: String,
}

fn block_rows(a: &App) -> Result<Vec<BlockRow>, String> {
    Ok(a.store.list_blocks().map_err(|e| e.to_string())?.into_iter().map(|(kind, value)| BlockRow { kind, value }).collect())
}

#[tauri::command]
fn list_blocks(app: State<Shared>) -> Result<Vec<BlockRow>, String> {
    block_rows(&app.lock().unwrap())
}

/// kind: "song" (value = track id; stored by ISRC when known), "artist", "tag" (includes genres).
#[tauri::command]
fn add_block(app: State<Shared>, kind: String, value: String) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    let stored = if kind == "song" {
        let id = TrackId(value);
        match a.store.find_track(&id).map_err(|e| e.to_string())? {
            Some(t) => spinamp_store::capture_key(&t),
            None => id.0,
        }
    } else {
        value
    };
    a.store.add_block(&kind, &stored, now()).map_err(|e| e.to_string())?;
    reload(&mut a)?;
    Ok(rows(&a))
}

#[tauri::command]
fn remove_block(app: State<Shared>, kind: String, value: String) -> Result<Vec<BlockRow>, String> {
    let mut a = app.lock().unwrap();
    a.store.remove_block(&kind, &value).map_err(|e| e.to_string())?;
    reload(&mut a)?;
    block_rows(&a)
}

#[tauri::command]
fn library_count(app: State<Shared>) -> usize {
    app.lock().unwrap().library.len()
}

fn pick_inner(a: &mut App, mode: PlayMode) -> Option<Next> {
    // only tracks that are allowed to play on their own: not blocked, not muted
    let n = now();
    let pool: Vec<TrackState> = view_states(a).into_iter().filter(|t| playable(t, &a.blocklist, n)).collect();
    let candidates: Vec<Track> = a.candidates.iter().filter(|c| !a.blocklist.blocks(c)).cloned().collect();
    let current = a.history.last().and_then(|id| pool.iter().position(|t| &t.track.id == id));
    let ctx = Context { now: n, history: &a.history, current, recently_suggested: &a.suggested, library: &a.library };
    let picked = match engine::next(mode, &pool, &candidates, &ctx, &a.cfg, &mut a.rng)? {
        Pick::Library(t) => Next { track: t.track.clone(), discovery: false },
        Pick::Discovery(t) => Next { track: t.clone(), discovery: true },
    };
    Some(picked)
}

#[tauri::command]
async fn spotify_connect(app: State<'_, Shared>, handle: AppHandle) -> Result<(), String> {
    let (cid, port) = {
        let a = app.lock().unwrap();
        (a.settings.client_id(), a.settings.callback_port)
    };
    if cid.is_empty() {
        return Err("Set your Spotify Client ID in Settings first.".into());
    }
    app.lock().unwrap().spotify.set_client_id(&cid);
    let session = spinamp_spotify::Session::begin(&cid, port).await.map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_url(session.authorize_url.clone(), None::<&str>).map_err(|e| e.to_string())?;
    let _ = handle;
    let tokens = session.finish(now()).await?;
    let sp = app.lock().unwrap().spotify.clone();
    sp.set_tokens(tokens).await;
    persist_tokens(&app, &sp).await;
    Ok(())
}

#[derive(Serialize)]
struct ProviderStatus {
    connected: bool,
    storage: credentials::Storage,
}

/// Connection state and where the credentials are kept (keychain / sqlite fallback).
#[tauri::command]
async fn spotify_status(app: State<'_, Shared>) -> Result<ProviderStatus, String> {
    let sp = app.lock().unwrap().spotify.clone();
    let connected = sp.is_authenticated().await;
    let storage = credentials::storage_of(&credentials::Keychain, &app.lock().unwrap().store, "spotify");
    Ok(ProviderStatus { connected, storage })
}

/// Forget the Spotify session everywhere (memory, keychain, legacy sqlite copy).
#[tauri::command]
async fn spotify_disconnect(app: State<'_, Shared>) -> Result<(), String> {
    let sp = app.lock().unwrap().spotify.clone();
    sp.disconnect().await.map_err(|e| e.to_string())?;
    credentials::clear(&credentials::Keychain, &app.lock().unwrap().store, "spotify");
    Ok(())
}

#[tauri::command]
async fn spotify_playlists(app: State<'_, Shared>) -> Result<Vec<spinamp_core::provider::RemotePlaylist>, String> {
    let sp = app.lock().unwrap().spotify.clone();
    let r = sp.playlists().await.map_err(|e| e.to_string());
    persist_tokens(&app, &sp).await;
    r
}

/// Fetch a remote playlist's tracks WITHOUT saving anything.
#[tauri::command]
async fn spotify_preview(app: State<'_, Shared>, playlist_id: String) -> Result<Vec<Track>, String> {
    let sp = app.lock().unwrap().spotify.clone();
    let tracks = sp.playlist_tracks(&playlist_id).await.map_err(|e| e.to_string())?;
    persist_tokens(&app, &sp).await;
    Ok(tracks)
}

fn reload(a: &mut App) -> Result<(), String> {
    a.library = a.store.load_library().map_err(|e| e.to_string())?;
    a.blocklist = Blocklist::from_rows(&a.store.list_blocks().map_err(|e| e.to_string())?);
    Ok(())
}

/// Add tracks to the local library only (no playlist): create-or-update, merging with what exists.
#[tauri::command]
fn import_tracks(app: State<Shared>, tracks: Vec<Track>) -> Result<spinamp_store::ImportCounts, String> {
    let mut a = app.lock().unwrap();
    let c = a.store.add_tracks(&tracks, now()).map_err(|e| e.to_string())?;
    let _ = a.store.tag_from_artist_cache();
    let _ = a.store.apply_audio_features();
    reload(&mut a)?;
    Ok(c)
}

#[derive(Serialize)]
struct ImportResult {
    id: String,
    tracks: spinamp_store::ImportCounts,
    playlist: spinamp_store::PlaylistSave,
}

/// Create-or-update a local playlist from tracks and MERGE (add only; never remove, reorder or rename).
#[tauri::command]
fn import_playlist(app: State<Shared>, name: String, remote_id: Option<String>, tracks: Vec<Track>) -> Result<ImportResult, String> {
    let mut a = app.lock().unwrap();
    let n = now();
    let counts = a.store.add_tracks(&tracks, n).map_err(|e| e.to_string())?;
    let _ = a.store.tag_from_artist_cache();
    let _ = a.store.apply_audio_features();
    let ids: Vec<TrackId> = tracks.iter().map(|t| t.id.clone()).collect();
    let saved = a.store.save_playlist(&name, remote_id.as_deref(), &ids, n).map_err(|e| e.to_string())?;
    reload(&mut a)?;
    Ok(ImportResult { id: saved.id.clone(), tracks: counts, playlist: saved })
}

#[tauri::command]
fn create_playlist(app: State<Shared>, name: String) -> Result<String, String> {
    let a = app.lock().unwrap();
    a.store.save_playlist(name.trim(), None, &[], now()).map(|r| r.id).map_err(|e| e.to_string())
}

/// Add tracks (e.g. search results) to a playlist, adding them to the library first.
/// This is an explicit user action, so it also restores anything you had deleted.
#[tauri::command]
fn add_to_playlist(app: State<Shared>, playlist_id: String, tracks: Vec<Track>) -> Result<(), String> {
    let mut a = app.lock().unwrap();
    for t in &tracks {
        a.store.add_track(t, now()).map_err(|e| e.to_string())?;
    }
    let _ = a.store.tag_from_artist_cache();
    let _ = a.store.apply_audio_features();
    let ids: Vec<TrackId> = tracks.iter().map(|t| t.id.clone()).collect();
    a.store.append_to_playlist(&playlist_id, &ids).map_err(|e| e.to_string())?;
    reload(&mut a)?;
    refresh_view(&mut a)
}

/// Soft-removes the entry: it stays in your library and a later import will not re-add it.
#[tauri::command]
fn remove_from_playlist(app: State<Shared>, playlist_id: String, track_id: String) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    a.store.remove_from_playlist(&playlist_id, &TrackId(track_id), now()).map_err(|e| e.to_string())?;
    refresh_view(&mut a)?;
    Ok(rows(&a))
}

/// Re-read the selected playlist's membership after it changed.
fn refresh_view(a: &mut App) -> Result<(), String> {
    if let Some((id, _)) = a.view.clone() {
        let ids = a.store.playlist_track_ids(&id).map_err(|e| e.to_string())?;
        a.view = Some((id, ids));
    }
    Ok(())
}

#[tauri::command]
fn local_playlists(app: State<Shared>) -> Result<Vec<PlaylistInfo>, String> {
    app.lock().unwrap().store.list_playlists().map_err(|e| e.to_string())
}

#[tauri::command]
fn select_playlist(app: State<Shared>, id: Option<String>) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    a.view = match id {
        Some(id) => {
            let ids = a.store.playlist_track_ids(&id).map_err(|e| e.to_string())?;
            Some((id, ids))
        }
        None => None,
    };
    Ok(rows(&a))
}

#[tauri::command]
fn deleted_counts(app: State<Shared>) -> Result<spinamp_store::DeletedCounts, String> {
    app.lock().unwrap().store.deleted_counts().map_err(|e| e.to_string())
}

/// Bring back everything you soft-deleted (playlists, tracks, playlist entries).
#[tauri::command]
fn restore_deleted(app: State<Shared>) -> Result<spinamp_store::DeletedCounts, String> {
    let mut a = app.lock().unwrap();
    let c = a.store.restore_deleted().map_err(|e| e.to_string())?;
    reload(&mut a)?;
    Ok(c)
}

#[tauri::command]
fn delete_playlist(app: State<Shared>, id: String) -> Result<(), String> {
    let mut a = app.lock().unwrap();
    a.store.delete_playlist(&id, now()).map_err(|e| e.to_string())?;
    if a.view.as_ref().is_some_and(|(v, _)| *v == id) {
        a.view = None;
    }
    Ok(())
}

#[tauri::command]
async fn search(app: State<'_, Shared>, query: String) -> Result<Vec<Track>, String> {
    let sp = app.lock().unwrap().spotify.clone();
    if !sp.is_authenticated().await {
        return Ok(vec![]);
    }
    sp.search(&query, 20).await.map_err(|e| e.to_string())
}

/// Add a remote search result to the local library.
#[tauri::command]
fn add_to_library(app: State<Shared>, track: Track) -> Result<Vec<Row>, String> {
    let mut a = app.lock().unwrap();
    a.store.add_track(&track, now()).map_err(|e| e.to_string())?;
    a.library = a.store.load_library().map_err(|e| e.to_string())?;
    Ok(rows(&a))
}

/// Save a provider's current credentials (call after sign-in and after any API call that may
/// have refreshed the token).
async fn persist_tokens(app: &Shared, sp: &SpotifyProvider) {
    if let Some(blob) = sp.credentials().await {
        credentials::save(&credentials::Keychain, &app.lock().unwrap().store, sp.id(), &blob);
    }
}

#[tauri::command]
fn vote(app: State<Shared>, id: String, up: bool) -> Vec<Row> {
    let mut guard = app.lock().unwrap();
    let a = &mut *guard;
    let n = now();
    if let Some(t) = a.library.iter_mut().find(|t| t.track.id.0 == id) {
        t.votes.record(n, up);
        t.updated_at = n;
        let _ = a.store.record_vote(&t.track.id, n, up);
    }
    rows(&a)
}

fn dirs_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = format!("{home}/Library/Application Support/Spinamp");
    let _ = std::fs::create_dir_all(&dir);
    format!("{dir}/spinamp.db")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let dir = dirs_path();
    let store = Store::open(&dir).expect("open db");
    let settings: Settings = store.kv_get("settings").ok().flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    let settings = settings.sanitized();
    let candidates = store.load_candidates().unwrap_or_default();
    let store_blocks = store.list_blocks().unwrap_or_default();
    let _ = store.tag_from_artist_cache();
    let tracks = store.load_library().unwrap_or_default();
    let (blob, _) = credentials::load_or_migrate(&credentials::Keychain, &store, "spotify");
    let saved = blob.and_then(|j| serde_json::from_str(&j).ok());
    let spotify = Arc::new(SpotifyProvider::new(settings.client_id(), saved));
    let mut state = App {
        settings,
        blocklist: Blocklist::from_rows(&store_blocks),
        capture: Arc::new(capture::Capture::default()),
        analysis: Arc::new(analysis::Analysis::default()),
        catalog: Arc::new(catalog::Catalog::default()),
        store,
        spotify,
        library: tracks,
        view: None,
        candidates,
        history: vec![],
        suggested: vec![],
        rng: StdRng::from_os_rng(),
        cfg: EngineConfig::default(),
        enrich: Arc::new(Enrich::default()),
        tracker: Tracker::default(),
        play_row: None,
        ctx_saved: false,
        now_track: None,
        stats_cfg: StatsConfig::default(),
    };
    apply_settings(&mut state);
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(state))
        .invoke_handler(tauri::generate_handler![mute_track, set_favorite, remove_track_everywhere, list_blocks, add_block, remove_block, tempo_reconcile, set_tempo, playback_seek, capture_answer, capture::capture_check, capture::capture_status, capture::viz_capture, analysis_reset, library, sort_library, next_track, vote, spotify_connect, analysis_status, analysis_start, analysis_stop, viz_pattern, spotify_status, spotify_disconnect, extras::lyrics, extras::artist_profile, extras::spotify_import_history, get_settings, save_settings, set_ai_key, clear_ai_key, test_ai, catalog_status, catalog_refill, vote_track, enrich_start, enrich_stop, enrich_status, ai_enabled, generate_playlist, play_track, poll_playback, play_log, set_tz_offset, playback_pause, playback_resume, library_count, spotify_playlists, spotify_preview, import_tracks, import_playlist, local_playlists, deleted_counts, restore_deleted, create_playlist, add_to_playlist, remove_from_playlist, select_playlist, delete_playlist, search, add_to_library])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
