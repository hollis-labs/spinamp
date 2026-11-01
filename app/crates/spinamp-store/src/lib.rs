//! SQLite persistence: library, votes, play events, and a small kv table.
//! Tracks are stored as JSON so provider-specific fields can evolve without migrations.
use rusqlite::{Connection, OptionalExtension, params};
use spinamp_core::stats::PlayEvent;
use spinamp_core::{ArtistInfo, AudioFeatures, VizPattern, merge_track, Track, TrackId, TrackState, Ts, primary_artist};

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlaylistInfo {
    pub id: String,
    pub name: String,
    /// Remote id (e.g. Spotify playlist id) when imported from a provider.
    pub remote: Option<String>,
    pub count: u32,
}

/// Context captured when a track starts playing; the raw material for future algorithms.
#[derive(Debug, Clone, Default)]
pub struct PlayStart {
    pub track: Option<TrackId>,
    pub started_at: Ts,
    pub duration_ms: u64,
    /// Engine mode that chose it (Standard/Shuffle/Discovery/ShuffleDiscovery) or "manual".
    pub mode: String,
    /// "auto" (engine pick), "click" (user chose it), "external" (changed in the Spotify app).
    pub trigger: String,
    pub from_playlist: Option<String>,
}

/// One row per play, with everything we learned about how it went.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlayLogRow {
    pub id: i64,
    pub track_id: String,
    pub started_at: Ts,
    pub listened_ms: u64,
    pub duration_ms: u64,
    pub ended_by: Option<String>,
    pub end_position_ms: Option<u64>,
    pub mode: Option<String>,
    pub trigger: Option<String>,
    pub from_playlist: Option<String>,
    pub device: Option<String>,
    pub device_type: Option<String>,
    pub volume: Option<u32>,
    pub shuffle: Option<bool>,
}

/// A track that still needs audio analysis.
#[derive(Debug, Clone)]
pub struct AudioJob {
    pub id: TrackId,
    pub isrc: String,
    pub artist: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    Added,
    Updated,
    /// Existed but had been deleted; brought back by an explicit action.
    Restored,
    /// Deleted by the user earlier; an import does not bring it back.
    SkippedDeleted,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImportCounts {
    pub added: usize,
    pub updated: usize,
    pub skipped_deleted: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistSave {
    pub id: String,
    pub added: usize,
    pub existing: usize,
    /// Tracks the user removed from this playlist earlier; not re-added.
    pub skipped_removed: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DeletedCounts {
    pub playlists: u32,
    pub tracks: u32,
    pub entries: u32,
}

/// Bump when the analysis output format changes; older rows are re-analysed.
pub const AUDIO_VERSION: i64 = 3; // 3: adds key, mood, valence, danceability

/// A recorded spectrum fingerprint of a whole track, captured while you listened.
/// Frames are `frame_ms` apart from position 0; `frames` is `n * bands` bytes (dB mapped
/// to 1..=255), `present[i]` says whether frame `i` was actually heard. Listens fill gaps.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureRow {
    pub frame_ms: u32,
    pub bands: u8,
    pub frames: Vec<u8>,
    pub present: Vec<u8>,
    pub duration_ms: u64,
}

impl CaptureRow {
    pub fn coverage(&self) -> f32 {
        if self.present.is_empty() { 0.0 } else { self.present.iter().filter(|p| **p != 0).count() as f32 / self.present.len() as f32 }
    }
}

/// Key under which a track's fingerprint is stored and (later) shared: its ISRC, so the same
/// recording matches across users and providers. Tracks without an ISRC fall back to their id.
pub fn capture_key(t: &Track) -> String {
    match &t.meta.isrc {
        Some(i) if !i.trim().is_empty() => format!("isrc:{}", i.trim().to_uppercase()),
        _ => t.id.0.clone(),
    }
}

pub type Result<T> = rusqlite::Result<T>;

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS library(
               id TEXT PRIMARY KEY, track TEXT NOT NULL,
               added_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
               last_played INTEGER, play_count INTEGER NOT NULL DEFAULT 0, rating INTEGER);
             CREATE TABLE IF NOT EXISTS votes(
               id INTEGER PRIMARY KEY, track_id TEXT NOT NULL REFERENCES library(id) ON DELETE CASCADE,
               at INTEGER NOT NULL, up INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS votes_track ON votes(track_id);
             CREATE TABLE IF NOT EXISTS play_events(
               id INTEGER PRIMARY KEY, track_id TEXT NOT NULL, started_at INTEGER NOT NULL,
               listened_ms INTEGER NOT NULL, duration_ms INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS playlists(
               id TEXT PRIMARY KEY, name TEXT NOT NULL, remote TEXT, created_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS playlist_tracks(
               playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
               track_id TEXT NOT NULL REFERENCES library(id) ON DELETE CASCADE,
               position INTEGER NOT NULL, PRIMARY KEY(playlist_id, track_id));
             CREATE TABLE IF NOT EXISTS artists(
               key TEXT PRIMARY KEY, info TEXT, fetched_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS candidates(
               id TEXT PRIMARY KEY, track TEXT NOT NULL, artist TEXT NOT NULL, seed TEXT,
               status TEXT NOT NULL DEFAULT 'new', added_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS audio_features(
               track_id TEXT PRIMARY KEY, json TEXT, at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS captures(
               track_id TEXT PRIMARY KEY, frame_ms INTEGER NOT NULL, bands INTEGER NOT NULL,
               frames BLOB NOT NULL, present BLOB NOT NULL, duration_ms INTEGER NOT NULL, updated_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS user_tempo(track_id TEXT PRIMARY KEY, bpm REAL NOT NULL, at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS mutes(track_id TEXT PRIMARY KEY, until INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS favorites(track_id TEXT PRIMARY KEY, at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS blocks(kind TEXT NOT NULL, value TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY(kind, value));
             CREATE TABLE IF NOT EXISTS kv(k TEXT PRIMARY KEY, v TEXT NOT NULL);",
        )?;
        let s = Self { db };
        for (table, col) in [("library", "deleted_at"), ("playlists", "deleted_at"), ("playlist_tracks", "deleted_at")] {
            s.ensure_column(table, col, "INTEGER")?;
        }
        s.ensure_column("audio_features", "viz", "TEXT")?;
        s.ensure_column("audio_features", "v", "INTEGER DEFAULT 1")?;
        let _ = s.migrate_capture_keys();
        for (col, ty) in [
            ("ended_by", "TEXT"), ("end_position_ms", "INTEGER"), ("mode", "TEXT"), ("trigger", "TEXT"),
            ("from_playlist", "TEXT"), ("device", "TEXT"), ("device_type", "TEXT"), ("volume", "INTEGER"), ("shuffle", "INTEGER"),
        ] {
            s.ensure_column("play_events", col, ty)?;
        }
        Ok(s)
    }

    fn ensure_column(&self, table: &str, col: &str, ty: &str) -> Result<()> {
        let mut q = self.db.prepare(&format!("PRAGMA table_info({table})"))?;
        let have: Vec<String> = q.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_>>()?;
        if !have.iter().any(|c| c == col) {
            self.db.execute(&format!("ALTER TABLE {table} ADD COLUMN {col} {ty}"), [])?;
        }
        Ok(())
    }

    pub fn memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Create-or-update a track by id, merging with what we hold (see [`merge_track`]).
    /// A track the user deleted stays deleted unless `restore` (an explicit user action such as
    /// voting or adding it to a playlist) says otherwise.
    pub fn upsert_track(&self, t: &Track, now: Ts, restore: bool) -> Result<Upsert> {
        let existing: Option<(String, Option<Ts>)> = self
            .db
            .query_row("SELECT track, deleted_at FROM library WHERE id = ?1", [&t.id.0], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match existing {
            None => {
                self.db.execute(
                    "INSERT INTO library(id, track, added_at, updated_at) VALUES(?1, ?2, ?3, ?3)",
                    params![t.id.0, serde_json::to_string(t).unwrap(), now],
                )?;
                Ok(Upsert::Added)
            }
            Some((_, Some(_))) if !restore => Ok(Upsert::SkippedDeleted),
            Some((json, deleted)) => {
                let merged = match serde_json::from_str::<Track>(&json) {
                    Ok(old) => merge_track(&old, t),
                    Err(_) => t.clone(),
                };
                self.db.execute(
                    "UPDATE library SET track = ?2, deleted_at = NULL WHERE id = ?1",
                    params![t.id.0, serde_json::to_string(&merged).unwrap()],
                )?;
                Ok(if deleted.is_some() { Upsert::Restored } else { Upsert::Updated })
            }
        }
    }

    /// Explicit single add (user action): restores a deleted track.
    pub fn add_track(&self, t: &Track, now: Ts) -> Result<()> {
        self.upsert_track(t, now, true).map(|_| ())
    }

    /// Bulk import in one transaction. Deleted tracks are skipped, not resurrected.
    pub fn add_tracks(&self, tracks: &[Track], now: Ts) -> Result<ImportCounts> {
        let tx = self.db.unchecked_transaction()?;
        let mut c = ImportCounts::default();
        for t in tracks {
            match self.upsert_track(t, now, false)? {
                Upsert::Added | Upsert::Restored => c.added += 1,
                Upsert::Updated => c.updated += 1,
                Upsert::SkippedDeleted => c.skipped_deleted += 1,
            }
        }
        tx.commit()?;
        Ok(c)
    }

    /// Create-or-update a playlist (matched by `remote` when given) and MERGE the tracks into it:
    /// new tracks are appended, nothing is removed or reordered, the name is left alone, and
    /// entries the user removed stay removed. Importing a playlist you deleted brings it back.
    pub fn save_playlist(&self, name: &str, remote: Option<&str>, ids: &[TrackId], now: Ts) -> Result<PlaylistSave> {
        let id = match remote {
            Some(r) => format!("spotify:{r}"),
            None => format!("local:{now}:{name}"),
        };
        let tx = self.db.unchecked_transaction()?;
        self.db.execute(
            "INSERT INTO playlists(id, name, remote, created_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET deleted_at = NULL",
            params![id, name, remote, now],
        )?;
        let mut pos: i64 =
            self.db.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks WHERE playlist_id = ?1", [&id], |r| r.get(0))?;
        let mut out = PlaylistSave { id: id.clone(), ..Default::default() };
        for t in ids {
            let seen: Option<Option<Ts>> = self
                .db
                .query_row("SELECT deleted_at FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2", params![id, t.0], |r| r.get(0))
                .optional()?;
            match seen {
                None => {
                    self.db.execute("INSERT INTO playlist_tracks(playlist_id, track_id, position) VALUES(?1, ?2, ?3)", params![id, t.0, pos])?;
                    pos += 1;
                    out.added += 1;
                }
                Some(None) => out.existing += 1,
                Some(Some(_)) => out.skipped_removed += 1,
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Explicit add of tracks (already in the library) to the end of a playlist.
    /// Restores an entry the user had removed earlier.
    pub fn append_to_playlist(&self, id: &str, ids: &[TrackId]) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let mut pos: i64 =
            self.db.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks WHERE playlist_id = ?1", [id], |r| r.get(0))?;
        for t in ids {
            let n = self.db.execute(
                "INSERT INTO playlist_tracks(playlist_id, track_id, position) VALUES(?1, ?2, ?3)
                 ON CONFLICT(playlist_id, track_id) DO UPDATE SET deleted_at = NULL WHERE deleted_at IS NOT NULL",
                params![id, t.0, pos],
            )?;
            if n > 0 {
                pos += 1;
            }
        }
        tx.commit()
    }

    /// Soft delete: the entry is hidden but remembered, so a later import will not re-add it.
    pub fn remove_from_playlist(&self, id: &str, track: &TrackId, now: Ts) -> Result<()> {
        self.db.execute(
            "UPDATE playlist_tracks SET deleted_at = ?3 WHERE playlist_id = ?1 AND track_id = ?2",
            params![id, track.0, now],
        )?;
        Ok(())
    }

    pub fn list_playlists(&self) -> Result<Vec<PlaylistInfo>> {
        let mut q = self.db.prepare(
            "SELECT p.id, p.name, p.remote,
                    (SELECT COUNT(*) FROM playlist_tracks pt JOIN library l ON l.id = pt.track_id
                      WHERE pt.playlist_id = p.id AND pt.deleted_at IS NULL AND l.deleted_at IS NULL)
             FROM playlists p WHERE p.deleted_at IS NULL ORDER BY p.created_at, p.rowid",
        )?;
        q.query_map([], |r| Ok(PlaylistInfo { id: r.get(0)?, name: r.get(1)?, remote: r.get(2)?, count: r.get(3)? }))?
            .collect()
    }

    pub fn playlist_track_ids(&self, id: &str) -> Result<Vec<TrackId>> {
        let mut q = self.db.prepare(
            "SELECT pt.track_id FROM playlist_tracks pt JOIN library l ON l.id = pt.track_id
              WHERE pt.playlist_id = ?1 AND pt.deleted_at IS NULL AND l.deleted_at IS NULL ORDER BY pt.position",
        )?;
        q.query_map([id], |r| Ok(TrackId(r.get(0)?)))?.collect()
    }

    /// Soft delete. Memberships, votes and play history are kept.
    pub fn delete_playlist(&self, id: &str, now: Ts) -> Result<()> {
        self.db.execute("UPDATE playlists SET deleted_at = ?2 WHERE id = ?1", params![id, now])?;
        Ok(())
    }

    /// Soft delete: hidden everywhere, but votes and play history stay.
    pub fn remove_track(&self, id: &TrackId, now: Ts) -> Result<()> {
        self.db.execute("UPDATE library SET deleted_at = ?2 WHERE id = ?1", params![id.0, now])?;
        Ok(())
    }

    pub fn deleted_counts(&self) -> Result<DeletedCounts> {
        let n = |sql: &str| -> Result<u32> { self.db.query_row(sql, [], |r| r.get(0)) };
        Ok(DeletedCounts {
            playlists: n("SELECT COUNT(*) FROM playlists WHERE deleted_at IS NOT NULL")?,
            tracks: n("SELECT COUNT(*) FROM library WHERE deleted_at IS NOT NULL")?,
            entries: n("SELECT COUNT(*) FROM playlist_tracks WHERE deleted_at IS NOT NULL")?,
        })
    }

    /// Bring everything soft-deleted back.
    pub fn restore_deleted(&self) -> Result<DeletedCounts> {
        let c = self.deleted_counts()?;
        let tx = self.db.unchecked_transaction()?;
        for t in ["playlists", "library", "playlist_tracks"] {
            self.db.execute(&format!("UPDATE {t} SET deleted_at = NULL WHERE deleted_at IS NOT NULL"), [])?;
        }
        tx.commit()?;
        Ok(c)
    }

    pub fn load_library(&self) -> Result<Vec<TrackState>> {
        let mut q = self.db.prepare(
            "SELECT id, track, added_at, updated_at, last_played, play_count, rating FROM library WHERE deleted_at IS NULL ORDER BY added_at, rowid",
        )?;
        let mut rows: Vec<TrackState> = q
            .query_map([], |r| {
                let track: Track = serde_json::from_str(&r.get::<_, String>(1)?)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
                let mut s = TrackState::new(track, r.get(2)?);
                s.updated_at = r.get(3)?;
                s.last_played = r.get(4)?;
                s.play_count = r.get(5)?;
                s.rating = r.get::<_, Option<u8>>(6)?;
                Ok(s)
            })?
            .collect::<Result<_>>()?;
        let mut v = self.db.prepare("SELECT track_id, at, up FROM votes ORDER BY at, id")?;
        let votes = v.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Ts>(1)?, r.get::<_, bool>(2)?)))?;
        let mut by_id: std::collections::HashMap<String, usize> =
            rows.iter().enumerate().map(|(i, s)| (s.track.id.0.clone(), i)).collect();
        for vote in votes {
            let (id, at, up) = vote?;
            if let Some(i) = by_id.get_mut(&id) {
                rows[*i].votes.record(at, up);
            }
        }
        let mut mutes = self.db.prepare("SELECT track_id, until FROM mutes")?;
        let mutes: std::collections::HashMap<String, Ts> = mutes.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Ts>(1)?)))?.collect::<Result<_>>()?;
        let mut favs = self.db.prepare("SELECT track_id FROM favorites")?;
        let favs: std::collections::HashSet<String> = favs.query_map([], |r| r.get(0))?.collect::<Result<_>>()?;
        for r in rows.iter_mut() {
            r.muted_until = mutes.get(&r.track.id.0).copied();
            r.favorite = favs.contains(&r.track.id.0);
        }
        Ok(rows)
    }

    pub fn record_vote(&self, id: &TrackId, at: Ts, up: bool) -> Result<()> {
        self.db.execute("INSERT INTO votes(track_id, at, up) VALUES(?1, ?2, ?3)", params![id.0, at, up])?;
        self.db.execute("UPDATE library SET updated_at = ?2 WHERE id = ?1", params![id.0, at])?;
        Ok(())
    }

    pub fn set_rating(&self, id: &TrackId, rating: Option<u8>) -> Result<()> {
        self.db.execute("UPDATE library SET rating = ?2 WHERE id = ?1", params![id.0, rating])?;
        Ok(())
    }

    pub fn record_play(&self, e: &PlayEvent) -> Result<()> {
        self.db.execute(
            "INSERT INTO play_events(track_id, started_at, listened_ms, duration_ms) VALUES(?1, ?2, ?3, ?4)",
            params![e.track.0, e.started_at, e.listened_ms, e.duration_ms],
        )?;
        self.db.execute(
            "UPDATE library SET play_count = play_count + 1, last_played = ?2 WHERE id = ?1",
            params![e.track.0, e.started_at],
        )?;
        Ok(())
    }

    /// Open a play row (a play = started). Returns its id for later updates.
    pub fn start_play(&self, p: &PlayStart) -> Result<i64> {
        let id = p.track.as_ref().map(|t| t.0.as_str()).unwrap_or_default();
        self.db.execute(
            "INSERT INTO play_events(track_id, started_at, listened_ms, duration_ms, mode, trigger, from_playlist)
             VALUES(?1, ?2, 0, ?3, ?4, ?5, ?6)",
            params![id, p.started_at, p.duration_ms, p.mode, p.trigger, p.from_playlist],
        )?;
        let rowid = self.db.last_insert_rowid();
        self.db.execute(
            "UPDATE library SET play_count = play_count + 1, last_played = ?2 WHERE id = ?1",
            params![id, p.started_at],
        )?;
        Ok(rowid)
    }

    pub fn update_play(&self, id: i64, listened_ms: u64, ended: Option<(&str, u64)>) -> Result<()> {
        self.db.execute("UPDATE play_events SET listened_ms = ?2 WHERE id = ?1", params![id, listened_ms])?;
        if let Some((reason, pos)) = ended {
            self.db.execute("UPDATE play_events SET ended_by = ?2, end_position_ms = ?3 WHERE id = ?1", params![id, reason, pos])?;
        }
        Ok(())
    }

    pub fn update_play_context(&self, id: i64, device: Option<&str>, device_type: Option<&str>, volume: Option<u32>, shuffle: bool) -> Result<()> {
        self.db.execute(
            "UPDATE play_events SET device = ?2, device_type = ?3, volume = ?4, shuffle = ?5 WHERE id = ?1",
            params![id, device, device_type, volume, shuffle],
        )?;
        Ok(())
    }

    pub fn play_log(&self) -> Result<Vec<PlayLogRow>> {
        let mut q = self.db.prepare(
            "SELECT id, track_id, started_at, listened_ms, duration_ms, ended_by, end_position_ms, mode, trigger,
                    from_playlist, device, device_type, volume, shuffle FROM play_events ORDER BY started_at, id",
        )?;
        q.query_map([], |r| {
            Ok(PlayLogRow {
                id: r.get(0)?, track_id: r.get(1)?, started_at: r.get(2)?, listened_ms: r.get(3)?, duration_ms: r.get(4)?,
                ended_by: r.get(5)?, end_position_ms: r.get(6)?, mode: r.get(7)?, trigger: r.get(8)?, from_playlist: r.get(9)?,
                device: r.get(10)?, device_type: r.get(11)?, volume: r.get(12)?, shuffle: r.get::<_, Option<bool>>(13)?,
            })
        })?
        .collect()
    }

    /// Backfill plays from Spotify's history. Idempotent on (track, timestamp). Counts as plays,
    /// not listens (we cannot know how much of each was heard).
    pub fn add_history_plays(&self, items: &[(TrackId, Ts, u64)]) -> Result<usize> {
        let tx = self.db.unchecked_transaction()?;
        let mut n = 0;
        for (id, at, dur) in items {
            n += self.db.execute(
                "INSERT INTO play_events(track_id, started_at, listened_ms, duration_ms, ended_by, mode, trigger)
                 SELECT ?1, ?2, 0, ?3, 'history', 'history', 'spotify-history'
                 WHERE NOT EXISTS (SELECT 1 FROM play_events WHERE track_id = ?1 AND started_at = ?2)",
                params![id.0, at, dur],
            )?;
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn has_votes(&self, id: &TrackId) -> Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM votes WHERE track_id = ?1)", [&id.0], |r| r.get(0))?)
    }

    pub fn play_events(&self) -> Result<Vec<PlayEvent>> {
        let mut q = self.db.prepare("SELECT track_id, started_at, listened_ms, duration_ms FROM play_events ORDER BY started_at")?;
        q.query_map([], |r| {
            Ok(PlayEvent { track: TrackId(r.get(0)?), started_at: r.get(1)?, listened_ms: r.get(2)?, duration_ms: r.get(3)? })
        })?
        .collect()
    }

    /// Cache an artist lookup. `None` records "looked up, no match" so we do not ask again.
    pub fn save_artist(&self, name: &str, info: Option<&ArtistInfo>, now: Ts) -> Result<()> {
        self.db.execute(
            "INSERT INTO artists(key, info, fetched_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET info = excluded.info, fetched_at = excluded.fetched_at",
            params![name.to_lowercase(), info.map(|i| serde_json::to_string(i).unwrap()), now],
        )?;
        Ok(())
    }

    pub fn get_artist(&self, name: &str) -> Result<Option<ArtistInfo>> {
        let j: Option<Option<String>> =
            self.db.query_row("SELECT info FROM artists WHERE key = ?1", [name.to_lowercase()], |r| r.get(0)).optional()?;
        Ok(j.flatten().and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Primary artists in the library that have never been looked up.
    pub fn artists_missing(&self) -> Result<Vec<String>> {
        let mut q = self.db.prepare("SELECT DISTINCT json_extract(track, '$.artist') FROM library")?;
        let all: Vec<String> = q.query_map([], |r| r.get::<_, Option<String>>(0))?.filter_map(|r| r.ok().flatten()).collect();
        let mut have = self.db.prepare("SELECT key FROM artists")?;
        let have: std::collections::HashSet<String> = have.query_map([], |r| r.get(0))?.collect::<Result<_>>()?;
        let mut seen = std::collections::HashSet::new();
        Ok(all
            .iter()
            .map(|a| primary_artist(a).to_string())
            .filter(|a| !have.contains(&a.to_lowercase()) && seen.insert(a.to_lowercase()))
            .collect())
    }

    /// Fill `tags` on library tracks whose primary artist has cached info. Returns tracks updated.
    pub fn tag_from_artist_cache(&self) -> Result<usize> {
        let mut q = self.db.prepare(
            "SELECT id, json_extract(track, '$.artist') FROM library WHERE json_array_length(json_extract(track, '$.tags')) = 0",
        )?;
        let rows: Vec<(String, String)> =
            q.query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?.collect::<Result<_>>()?;
        let tx = self.db.unchecked_transaction()?;
        let mut n = 0;
        for (id, artist) in rows {
            if let Some(info) = self.get_artist(primary_artist(&artist))? {
                let tags: Vec<&str> = info.tags.iter().take(6).map(|t| t.0.as_str()).collect();
                if !tags.is_empty() {
                    self.db.execute(
                        "UPDATE library SET track = json_set(track, '$.tags', json(?2)) WHERE id = ?1",
                        params![id, serde_json::to_string(&tags).unwrap()],
                    )?;
                    n += 1;
                }
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn artist_counts(&self) -> Result<(u32, u32)> {
        let done: u32 = self.db.query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))?;
        let missing = self.artists_missing()?.len() as u32;
        Ok((done, missing))
    }

    /// Discovery candidates: playable tracks from artists you do not own. `seed` = the library
    /// artist that surfaced them. Existing rows are kept (status preserved).
    pub fn add_candidates(&self, items: &[(Track, String)], now: Ts) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for (t, seed) in items {
            self.db.execute(
                "INSERT OR IGNORE INTO candidates(id, track, artist, seed, added_at) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![t.id.0, serde_json::to_string(t).unwrap(), t.artist.to_lowercase(), seed, now],
            )?;
        }
        tx.commit()
    }

    /// Candidates still available: status new and not already in the library.
    pub fn load_candidates(&self) -> Result<Vec<Track>> {
        let mut q = self.db.prepare(
            "SELECT track FROM candidates WHERE status = 'new' AND id NOT IN (SELECT id FROM library) ORDER BY added_at, rowid",
        )?;
        let rows: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<Result<_>>()?;
        Ok(rows.iter().filter_map(|j| serde_json::from_str(j).ok()).collect())
    }

    pub fn mark_candidate(&self, id: &TrackId, status: &str) -> Result<()> {
        self.db.execute("UPDATE candidates SET status = ?2 WHERE id = ?1", params![id.0, status])?;
        Ok(())
    }

    /// Artists (lowercase) we already hold candidates for, in any status.
    pub fn candidate_artists(&self) -> Result<std::collections::HashSet<String>> {
        let mut q = self.db.prepare("SELECT DISTINCT artist FROM candidates")?;
        q.query_map([], |r| r.get(0))?.collect()
    }

    /// Cached MusicBrainz id for an artist, from enrichment.
    pub fn artist_mbid(&self, name: &str) -> Result<Option<String>> {
        Ok(self.get_artist(name)?.and_then(|a| a.mbid))
    }

    pub fn kv_get(&self, k: &str) -> Result<Option<String>> {
        self.db.query_row("SELECT v FROM kv WHERE k = ?1", [k], |r| r.get(0)).optional()
    }

    /// Tracks with an ISRC and no analysis yet: library first (recently played, then newest), then
    /// discovery candidates. Tracks that had no usable preview are retried after `retry_secs`.
    pub fn audio_pending(&self, now: Ts, retry_secs: i64) -> Result<Vec<AudioJob>> {
        let mut q = self.db.prepare(
            "SELECT id, json_extract(track,'$.meta.isrc'), json_extract(track,'$.artist'), json_extract(track,'$.title'),
                    0 AS ord, (last_played IS NULL) AS lpn, COALESCE(last_played, 0) AS lp, added_at AS ad
               FROM library WHERE deleted_at IS NULL
             UNION ALL
             SELECT id, json_extract(track,'$.meta.isrc'), json_extract(track,'$.artist'), json_extract(track,'$.title'),
                    1, 1, 0, added_at FROM candidates WHERE status = 'new'
             ORDER BY ord, lpn, lp DESC, ad DESC",
        )?;
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_>>()?;
        let mut done = self.db.prepare("SELECT track_id, json IS NULL, at, COALESCE(v, 1) FROM audio_features")?;
        let done: std::collections::HashMap<String, (bool, Ts, i64)> = done
            .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, bool>(1)?, r.get::<_, Ts>(2)?, r.get::<_, i64>(3)?))))?
            .collect::<Result<_>>()?;
        let mut seen = std::collections::HashSet::new();
        Ok(rows
            .into_iter()
            .filter_map(|(id, isrc, artist, title)| {
                let isrc = isrc?;
                // done and current: skip. Failed recently: skip. Analysed with an older format: redo.
                let skip = done.get(&id).is_some_and(|(failed, at, v)| if *failed { now - at < retry_secs } else { *v >= AUDIO_VERSION });
                (!skip && seen.insert(id.clone())).then(|| AudioJob { id: TrackId(id), isrc, artist: artist.unwrap_or_default(), title: title.unwrap_or_default() })
            })
            .collect())
    }

    /// `None` records "no usable preview" so we do not hammer the same track.
    pub fn save_audio(&self, id: &TrackId, f: Option<&AudioFeatures>, viz: Option<&VizPattern>, now: Ts) -> Result<()> {
        self.db.execute(
            "INSERT INTO audio_features(track_id, json, viz, at, v) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(track_id) DO UPDATE SET json = excluded.json, viz = excluded.viz, at = excluded.at, v = excluded.v",
            params![id.0, f.map(|f| serde_json::to_string(f).unwrap()), viz.map(|v| serde_json::to_string(v).unwrap()), now, AUDIO_VERSION],
        )?;
        Ok(())
    }

    /// The beat-aligned visual pattern for a track, if it was analysed and has a clear beat.
    pub fn get_viz(&self, id: &TrackId) -> Result<Option<VizPattern>> {
        let j: Option<Option<String>> = self.db.query_row("SELECT viz FROM audio_features WHERE track_id = ?1", [&id.0], |r| r.get(0)).optional()?;
        Ok(j.flatten().and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Copy measured features onto the tracks themselves (`tempo`, `energy`, `meta.audio`) in the
    /// library and candidate tables. Returns rows updated.
    pub fn apply_audio_features(&self) -> Result<usize> {
        let mut q = self.db.prepare("SELECT track_id, json FROM audio_features WHERE json IS NOT NULL")?;
        let rows: Vec<(String, String)> = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_>>()?;
        let tx = self.db.unchecked_transaction()?;
        let mut n = 0;
        for (id, j) in rows {
            let f: AudioFeatures = match serde_json::from_str(&j) { Ok(f) => f, Err(_) => continue };
            for table in ["library", "candidates"] {
                n += self.db.execute(
                    &format!(
                        "UPDATE {table} SET track = json_set(track, '$.tempo', ?2, '$.energy', ?3, '$.meta.audio', json(?4))
                         WHERE id = ?1 AND json_extract(track, '$.meta.audio') IS NULL"
                    ),
                    params![id, f.tempo_bpm, f.energy, j],
                )?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// (analysed, tracks with an ISRC that could be analysed)
    pub fn audio_counts(&self) -> Result<(u32, u32)> {
        let done: u32 = self.db.query_row("SELECT COUNT(*) FROM audio_features WHERE json IS NOT NULL", [], |r| r.get(0))?;
        let total: u32 = self.db.query_row("SELECT COUNT(*) FROM library WHERE deleted_at IS NULL AND json_extract(track,'$.meta.isrc') IS NOT NULL", [], |r| r.get(0))?;
        Ok((done, total))
    }

    /// Find a track by id in the library or the discovery candidates.
    pub fn find_track(&self, id: &TrackId) -> Result<Option<Track>> {
        for table in ["library", "candidates"] {
            let j: Option<String> = self.db.query_row(&format!("SELECT track FROM {table} WHERE id = ?1"), [&id.0], |r| r.get(0)).optional()?;
            if let Some(t) = j.and_then(|j| serde_json::from_str::<Track>(&j).ok()) {
                return Ok(Some(t));
            }
        }
        Ok(None)
    }

    /// Fingerprint key for a track id (ISRC if known).
    pub fn capture_key_for(&self, id: &TrackId) -> Result<String> {
        Ok(self.find_track(id)?.map(|t| capture_key(&t)).unwrap_or_else(|| id.0.clone()))
    }

    /// One-time: re-key captures saved under provider track ids to ISRC keys. Returns rows moved.
    pub fn migrate_capture_keys(&self) -> Result<usize> {
        let mut q = self.db.prepare("SELECT track_id FROM captures WHERE track_id NOT LIKE 'isrc:%'")?;
        let old: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<Result<_>>()?;
        let mut moved = 0;
        for id in old {
            let key = self.capture_key_for(&TrackId(id.clone()))?;
            if key != id {
                moved += self.db.execute(
                    "UPDATE OR IGNORE captures SET track_id = ?2 WHERE track_id = ?1 AND NOT EXISTS (SELECT 1 FROM captures WHERE track_id = ?2)",
                    params![id, key],
                )?;
            }
        }
        Ok(moved)
    }

    /// Save a capture, MERGING with what is stored: frames already recorded are kept as they were
    /// (the fingerprint stays stable between listens) and new frames fill the gaps, so several
    /// partial listens add up to a whole-track fingerprint.
    pub fn save_capture(&self, key: &str, c: &CaptureRow, now: Ts) -> Result<CaptureRow> {
        let mut merged = c.clone();
        if let Some(old) = self.get_capture(key)? {
            if old.frame_ms == c.frame_ms && old.bands == c.bands {
                let b = c.bands as usize;
                let n = old.present.len().max(c.present.len());
                merged.frames.resize(n * b, 0);
                merged.present.resize(n, 0);
                for i in 0..old.present.len() {
                    if old.present[i] != 0 {
                        merged.frames[i * b..(i + 1) * b].copy_from_slice(&old.frames[i * b..(i + 1) * b]);
                        merged.present[i] = 1;
                    }
                }
            }
        }
        self.db.execute(
            "INSERT INTO captures(track_id, frame_ms, bands, frames, present, duration_ms, updated_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(track_id) DO UPDATE SET frame_ms = excluded.frame_ms, bands = excluded.bands, frames = excluded.frames,
               present = excluded.present, duration_ms = excluded.duration_ms, updated_at = excluded.updated_at",
            params![key, merged.frame_ms, merged.bands, merged.frames, merged.present, merged.duration_ms as i64, now],
        )?;
        Ok(merged)
    }

    pub fn get_capture(&self, key: &str) -> Result<Option<CaptureRow>> {
        self.db
            .query_row(
                "SELECT frame_ms, bands, frames, present, duration_ms FROM captures WHERE track_id = ?1",
                [key],
                |r| Ok(CaptureRow { frame_ms: r.get(0)?, bands: r.get(1)?, frames: r.get(2)?, present: r.get(3)?, duration_ms: r.get::<_, i64>(4)? as u64 }),
            )
            .optional()
    }

    /// (tracks with any capture, tracks with >= 95% coverage)
    pub fn capture_counts(&self) -> Result<(u32, u32)> {
        let mut q = self.db.prepare("SELECT present FROM captures")?;
        let rows: Vec<Vec<u8>> = q.query_map([], |r| r.get(0))?.collect::<Result<_>>()?;
        let full = rows.iter().filter(|p| !p.is_empty() && p.iter().filter(|x| **x != 0).count() as f32 / p.len() as f32 >= 0.95).count();
        Ok((rows.len() as u32, full as u32))
    }

    /// Forget all measured audio (previews + their derived tempo/energy/patterns) so analysis can
    /// start over. Recorded captures are kept: they are real recordings, not analysis.
    pub fn reset_audio(&self) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        self.db.execute("DELETE FROM audio_features", [])?;
        for t in ["library", "candidates"] {
            self.db.execute(
                &format!("UPDATE {t} SET track = json_set(track, '$.tempo', json('null'), '$.energy', json('null'), '$.meta.audio', json('null'))
                          WHERE json_extract(track, '$.meta.audio') IS NOT NULL OR json_extract(track, '$.tempo') IS NOT NULL"),
                [],
            )?;
        }
        tx.commit()
    }

    /// A tempo you set by hand. It always wins over measurement and reconciliation.
    pub fn set_user_tempo(&self, id: &TrackId, bpm: Option<f32>, now: Ts) -> Result<()> {
        match bpm {
            Some(b) => self.db.execute(
                "INSERT INTO user_tempo(track_id, bpm, at) VALUES(?1, ?2, ?3) ON CONFLICT(track_id) DO UPDATE SET bpm = excluded.bpm, at = excluded.at",
                params![id.0, b, now],
            )?,
            None => self.db.execute("DELETE FROM user_tempo WHERE track_id = ?1", [&id.0])?,
        };
        Ok(())
    }

    pub fn user_tempos(&self) -> Result<std::collections::HashMap<String, f32>> {
        let mut q = self.db.prepare("SELECT track_id, bpm FROM user_tempo")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)? as f32)))?.collect()
    }

    /// Set the `tempo` shown/used for a track (the reconciled value). The raw measurement stays
    /// in `meta.audio.tempo_bpm`.
    pub fn update_track_tempo(&self, id: &TrackId, bpm: f32) -> Result<()> {
        for table in ["library", "candidates"] {
            self.db.execute(&format!("UPDATE {table} SET track = json_set(track, '$.tempo', ?2) WHERE id = ?1"), params![id.0, bpm])?;
        }
        Ok(())
    }

    /// Mute until `until` (unix seconds); `None` unmutes. The track stays in every list.
    pub fn set_mute(&self, id: &TrackId, until: Option<Ts>) -> Result<()> {
        match until {
            Some(u) => self.db.execute("INSERT INTO mutes(track_id, until) VALUES(?1, ?2) ON CONFLICT(track_id) DO UPDATE SET until = excluded.until", params![id.0, u])?,
            None => self.db.execute("DELETE FROM mutes WHERE track_id = ?1", [&id.0])?,
        };
        Ok(())
    }

    pub fn set_favorite(&self, id: &TrackId, favorite: bool, now: Ts) -> Result<()> {
        if favorite {
            self.db.execute("INSERT OR IGNORE INTO favorites(track_id, at) VALUES(?1, ?2)", params![id.0, now])?;
        } else {
            self.db.execute("DELETE FROM favorites WHERE track_id = ?1", [&id.0])?;
        }
        Ok(())
    }

    /// Block a song (value = `capture_key`), an artist, or a tag/genre. Permanent until removed.
    pub fn add_block(&self, kind: &str, value: &str, now: Ts) -> Result<()> {
        self.db.execute("INSERT OR IGNORE INTO blocks(kind, value, at) VALUES(?1, ?2, ?3)", params![kind, value.trim(), now])?;
        Ok(())
    }

    pub fn remove_block(&self, kind: &str, value: &str) -> Result<()> {
        self.db.execute("DELETE FROM blocks WHERE kind = ?1 AND value = ?2", params![kind, value])?;
        Ok(())
    }

    pub fn list_blocks(&self) -> Result<Vec<(String, String)>> {
        let mut q = self.db.prepare("SELECT kind, value FROM blocks ORDER BY at, rowid")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    }

    pub fn kv_delete(&self, k: &str) -> Result<()> {
        self.db.execute("DELETE FROM kv WHERE k = ?1", [k])?;
        Ok(())
    }

    pub fn kv_set(&self, k: &str, v: &str) -> Result<()> {
        self.db.execute("INSERT INTO kv(k, v) VALUES(?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v", params![k, v])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(n: &str) -> Track {
        Track {
            id: TrackId::new("spotify", n), provider: "spotify".into(), title: n.into(), artist: "a".into(),
            album: None, track_number: None, duration_ms: 1000, tempo: None, energy: None, tags: vec!["x".into()], meta: Default::default(),
        }
    }

    #[test]
    fn same_recording_on_two_providers_loads_as_two_rows_the_core_folds_into_one() {
        let s = Store::memory().unwrap();
        let mut a = track("1");
        a.meta.isrc = Some("GBBQY0400035".into());
        let mut b = track("2");
        b.id = TrackId::new("applemusic", "99");
        b.provider = "applemusic".into();
        b.meta.isrc = Some("gbbqy0400035".into());
        s.add_track(&a, 10).unwrap();
        s.add_track(&b, 20).unwrap();
        s.record_vote(&a.id, 30, true).unwrap();
        s.record_vote(&b.id, 40, true).unwrap();
        let raw = s.load_library().unwrap();
        assert_eq!(raw.len(), 2, "both copies stay stored; merging is only a view");
        let merged = spinamp_core::merge::merge_by_isrc(raw);
        assert_eq!(merged.len(), 1);
        assert_eq!((merged[0].track.id.0.as_str(), merged[0].alts.len(), merged[0].votes.votes().len()), ("spotify:1", 1, 2));
    }

    #[test]
    fn roundtrips_library_votes_and_plays() {
        let s = Store::memory().unwrap();
        s.add_track(&track("1"), 10).unwrap();
        s.record_vote(&TrackId::new("spotify", "1"), 20, true).unwrap();
        s.record_vote(&TrackId::new("spotify", "1"), 30, false).unwrap();
        s.record_play(&PlayEvent { track: TrackId::new("spotify", "1"), started_at: 40, listened_ms: 900, duration_ms: 1000 }).unwrap();
        let lib = s.load_library().unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].votes.votes().len(), 2);
        assert_eq!((lib[0].play_count, lib[0].last_played), (1, Some(40)));
        assert_eq!(lib[0].track.tags, ["x"]);
        assert_eq!(s.play_events().unwrap().len(), 1);
    }

    #[test]
    fn re_adding_keeps_feedback() {
        let s = Store::memory().unwrap();
        s.add_track(&track("1"), 10).unwrap();
        s.record_vote(&TrackId::new("spotify", "1"), 20, true).unwrap();
        s.add_track(&track("1"), 99).unwrap();
        let lib = s.load_library().unwrap();
        assert_eq!((lib[0].added_at, lib[0].votes.votes().len()), (10, 1));
    }

    #[test]
    fn reimport_merges_never_removes_and_respects_removals() {
        let s = Store::memory().unwrap();
        let ts: Vec<_> = ["a", "b", "c", "d"].iter().map(|n| track(n)).collect();
        s.add_tracks(&ts, 1).unwrap();
        let ids: Vec<_> = ts.iter().map(|t| t.id.clone()).collect();

        let first = s.save_playlist("Mix", Some("r1"), &ids[..3], 5).unwrap();
        assert_eq!((first.added, first.existing), (3, 0));
        let pid = first.id;
        assert_eq!(s.playlist_track_ids(&pid).unwrap(), ids[..3]);

        // user adds a track of their own and removes one
        s.append_to_playlist(&pid, &[ids[3].clone()]).unwrap();
        s.remove_from_playlist(&pid, &ids[1], 6).unwrap();
        assert_eq!(s.playlist_track_ids(&pid).unwrap(), [ids[0].clone(), ids[2].clone(), ids[3].clone()]);

        // remote now has only a,b (c, d gone remotely), plus a new track e
        let e = track("e");
        s.add_tracks(&[e.clone()], 7).unwrap();
        let again = s.save_playlist("Renamed Remotely", Some("r1"), &[ids[0].clone(), ids[1].clone(), e.id.clone()], 8).unwrap();
        assert_eq!((again.added, again.existing, again.skipped_removed), (1, 1, 1), "e added, a kept, b stays removed");
        assert_eq!(s.playlist_track_ids(&pid).unwrap(), [ids[0].clone(), ids[2].clone(), ids[3].clone(), e.id.clone()], "c, d (not on remote) and user additions kept; order stable");
        let l = s.list_playlists().unwrap();
        assert_eq!((l.len(), l[0].count, l[0].name.as_str()), (1, 4, "Mix"), "name left alone");

        // idempotent
        let third = s.save_playlist("Mix", Some("r1"), &[ids[0].clone(), e.id.clone()], 9).unwrap();
        assert_eq!((third.added, third.existing), (0, 2));
    }

    #[test]
    fn soft_delete_hides_keeps_history_and_import_does_not_resurrect() {
        let s = Store::memory().unwrap();
        let t = track("1");
        s.add_tracks(&[t.clone()], 1).unwrap();
        s.record_vote(&t.id, 2, true).unwrap();
        let pid = s.save_playlist("P", Some("r"), &[t.id.clone()], 3).unwrap().id;

        s.delete_playlist(&pid, 10).unwrap();
        assert!(s.list_playlists().unwrap().is_empty());
        s.remove_track(&t.id, 11).unwrap();
        assert!(s.load_library().unwrap().is_empty());
        assert_eq!(s.deleted_counts().unwrap(), DeletedCounts { playlists: 1, tracks: 1, entries: 0 });

        // a bulk import skips the deleted track; the vote history survives
        let c = s.add_tracks(&[t.clone()], 12).unwrap();
        assert_eq!((c.added, c.skipped_deleted), (0, 1));
        assert!(s.load_library().unwrap().is_empty());
        assert!(s.has_votes(&t.id).unwrap());

        // an explicit action (vote / add to playlist) restores it with its history
        s.add_track(&t, 13).unwrap();
        assert_eq!(s.load_library().unwrap()[0].votes.votes().len(), 1);

        // importing the deleted playlist again brings the playlist back
        let r = s.save_playlist("P", Some("r"), &[t.id.clone()], 14).unwrap();
        assert_eq!((r.id.as_str(), r.existing), (pid.as_str(), 1));
        assert_eq!(s.list_playlists().unwrap().len(), 1);

        // restore-all
        s.remove_track(&t.id, 20).unwrap();
        assert_eq!(s.restore_deleted().unwrap().tracks, 1);
        assert_eq!(s.load_library().unwrap().len(), 1);
    }

    #[test]
    fn append_restores_an_explicitly_removed_entry() {
        let s = Store::memory().unwrap();
        let ts: Vec<_> = ["a", "b"].iter().map(|n| track(n)).collect();
        s.add_tracks(&ts, 1).unwrap();
        let id = s.save_playlist("Mine", None, &[ts[0].id.clone()], 2).unwrap().id;
        s.append_to_playlist(&id, &[ts[1].id.clone(), ts[0].id.clone()]).unwrap();
        s.remove_from_playlist(&id, &ts[1].id, 3).unwrap();
        assert_eq!(s.playlist_track_ids(&id).unwrap().len(), 1);
        s.append_to_playlist(&id, &[ts[1].id.clone()]).unwrap();
        assert_eq!(s.playlist_track_ids(&id).unwrap().len(), 2, "user re-adding brings it back");
        assert_eq!(s.load_library().unwrap().len(), 2, "tracks stay in the library throughout");
    }

    #[test]
    fn play_session_lifecycle() {
        let s = Store::memory().unwrap();
        let t = track("1");
        s.add_track(&t, 1).unwrap();
        let id = s.start_play(&PlayStart {
            track: Some(t.id.clone()), started_at: 100, duration_ms: 1000, mode: "Shuffle".into(), trigger: "auto".into(), from_playlist: None,
        }).unwrap();
        s.update_play_context(id, Some("MacBook"), Some("Computer"), Some(80), true).unwrap();
        s.update_play(id, 400, None).unwrap();
        s.update_play(id, 950, Some(("finished", 990))).unwrap();
        let log = s.play_log().unwrap();
        assert_eq!(log.len(), 1);
        let r = &log[0];
        assert_eq!((r.listened_ms, r.ended_by.as_deref(), r.mode.as_deref(), r.device.as_deref(), r.volume, r.shuffle),
                   (950, Some("finished"), Some("Shuffle"), Some("MacBook"), Some(80), Some(true)));
        assert_eq!(s.load_library().unwrap()[0].play_count, 1);
        assert_eq!(s.play_events().unwrap()[0].listened_ms, 950);
    }

    #[test]
    fn artist_enrichment_tags_tracks() {
        let s = Store::memory().unwrap();
        let mut a = track("1");
        a.artist = "Slowdive, Guest".into();
        let mut b = track("2");
        b.artist = "Nobody".into();
        for t in [&mut a, &mut b] {
            t.tags.clear();
        }
        s.add_tracks(&[a, b], 1).unwrap();
        let mut missing = s.artists_missing().unwrap();
        missing.sort();
        assert_eq!(missing, ["Nobody", "Slowdive"]);
        let info = ArtistInfo {
            name: "Slowdive".into(), mbid: Some("m".into()), country: Some("GB".into()), kind: Some("Group".into()),
            begin_year: Some(1989), tags: vec![("shoegaze".into(), 26), ("rock".into(), 1)],
        };
        s.save_artist("Slowdive", Some(&info), 5).unwrap();
        s.save_artist("Nobody", None, 5).unwrap();
        assert!(s.artists_missing().unwrap().is_empty());
        assert_eq!(s.tag_from_artist_cache().unwrap(), 1);
        let lib = s.load_library().unwrap();
        assert_eq!(lib[0].track.tags, ["shoegaze", "rock"]);
        assert!(lib[1].track.tags.is_empty());
        assert_eq!(s.get_artist("slowdive").unwrap().unwrap().country.as_deref(), Some("GB"));
        assert_eq!(s.get_artist("Nobody").unwrap(), None);
    }

    #[test]
    fn candidates_exclude_owned_and_played() {
        let s = Store::memory().unwrap();
        let (a, b, c) = (track("1"), track("2"), track("3"));
        s.add_candidates(&[(a.clone(), "seed".into()), (b.clone(), "seed".into()), (c.clone(), "seed".into())], 1).unwrap();
        s.add_candidates(&[(a.clone(), "again".into())], 2).unwrap(); // duplicate ignored
        assert_eq!(s.load_candidates().unwrap().len(), 3);
        s.mark_candidate(&b.id, "played").unwrap();
        s.add_track(&c, 3).unwrap(); // voted on -> now owned
        let left: Vec<_> = s.load_candidates().unwrap().into_iter().map(|t| t.id.0).collect();
        assert_eq!(left, ["spotify:1"]);
        assert!(s.candidate_artists().unwrap().contains("a"));
    }

    #[test]
    fn history_plays_are_idempotent() {
        let s = Store::memory().unwrap();
        let id = TrackId::new("spotify", "1");
        let items = vec![(id.clone(), 100, 1000), (id.clone(), 200, 1000)];
        assert_eq!(s.add_history_plays(&items).unwrap(), 2);
        assert_eq!(s.add_history_plays(&items).unwrap(), 0);
        let log = s.play_log().unwrap();
        assert_eq!((log.len(), log[0].ended_by.as_deref(), log[0].trigger.as_deref()), (2, Some("history"), Some("spotify-history")));
        s.add_track(&track("1"), 1).unwrap();
        assert!(!s.has_votes(&id).unwrap());
        s.record_vote(&id, 5, true).unwrap();
        assert!(s.has_votes(&id).unwrap());
    }

    #[test]
    fn audio_jobs_save_apply_and_retry() {
        let s = Store::memory().unwrap();
        let mut a = track("1");
        a.meta.isrc = Some("ISRC1".into());
        let mut b = track("2");
        b.meta.isrc = Some("ISRC2".into());
        let c = track("3"); // no ISRC: cannot be analysed
        s.add_tracks(&[a.clone(), b.clone(), c], 1).unwrap();
        s.add_candidates(&[({ let mut t = track("9"); t.meta.isrc = Some("ISRC9".into()); t }, "x".into())], 1).unwrap();
        let jobs = s.audio_pending(100, 1000).unwrap();
        assert_eq!(jobs.iter().map(|j| j.id.0.as_str()).collect::<Vec<_>>(), ["spotify:1", "spotify:2", "spotify:9"], "library first");

        let f = AudioFeatures { tempo_bpm: Some(123.0), energy: 0.7, ..Default::default() };
        let viz = VizPattern { beats: 16, subdiv: 8, beats_per_bar: 4, bands: 16, version: 2, bpm: 120.0, data: vec![7; 16 * 8 * 16] };
        s.save_audio(&a.id, Some(&f), Some(&viz), 100).unwrap();
        s.save_audio(&b.id, None, None, 100).unwrap(); // no preview
        assert_eq!(s.get_viz(&a.id).unwrap(), Some(viz.clone()));
        assert_eq!(s.get_viz(&b.id).unwrap(), None);
        assert_eq!(s.audio_pending(200, 1000).unwrap().len(), 1, "analysed and recently-failed are skipped");
        // an analysis from an older format is queued again
        s.db.execute("UPDATE audio_features SET v = 1 WHERE track_id = ?1", [&a.id.0]).unwrap();
        assert_eq!(s.audio_pending(200, 1000).unwrap().len(), 2, "old-format analysis is redone");
        s.save_audio(&a.id, Some(&f), Some(&viz), 100).unwrap();
        assert_eq!(s.audio_pending(5000, 1000).unwrap().len(), 2, "failed track is retried later");

        assert_eq!(s.apply_audio_features().unwrap(), 1);
        let lib = s.load_library().unwrap();
        assert_eq!((lib[0].track.tempo, lib[0].track.energy), (Some(123.0), Some(0.7)));
        assert_eq!(lib[0].track.meta.audio.as_ref().unwrap().tempo_bpm, Some(123.0));
        assert_eq!(s.apply_audio_features().unwrap(), 0, "idempotent");
        // re-importing the track from the provider refreshes metadata but keeps what we measured/learned
        let mut fresh = a.clone();
        fresh.title = "Renamed".into();
        fresh.tags.clear();
        s.add_track(&fresh, 9).unwrap();
        let lib = s.load_library().unwrap();
        assert_eq!(lib[0].track.title, "Renamed");
        assert_eq!((lib[0].track.tempo, lib[0].track.tags.as_slice()), (Some(123.0), &["x".to_string()][..]));
        assert!(lib[0].track.meta.audio.is_some());
        assert_eq!(s.audio_counts().unwrap(), (1, 2));
    }

    #[test]
    fn audio_jobs_prioritise_recently_played_and_skip_deleted() {
        let s = Store::memory().unwrap();
        let mut ts = vec![];
        for n in ["old", "played", "new", "gone"] {
            let mut t = track(n);
            t.meta.isrc = Some(format!("I{n}"));
            ts.push(t);
        }
        s.add_track(&ts[0], 10).unwrap(); // old, never played
        s.add_track(&ts[1], 20).unwrap();
        s.add_track(&ts[2], 30).unwrap(); // newest, never played
        s.add_track(&ts[3], 40).unwrap();
        s.record_play(&spinamp_core::stats::PlayEvent { track: ts[1].id.clone(), started_at: 50, listened_ms: 0, duration_ms: 1 }).unwrap();
        s.remove_track(&ts[3].id, 60).unwrap();
        let order: Vec<_> = s.audio_pending(100, 1000).unwrap().into_iter().map(|j| j.id.0).collect();
        assert_eq!(order, ["spotify:played", "spotify:new", "spotify:old"]);
    }

    #[test]
    fn captures_accumulate_across_partial_listens_and_reset_keeps_them() {
        let s = Store::memory().unwrap();
        let id = TrackId::new("spotify", "1");
        let mk = |range: std::ops::Range<usize>, val: u8| {
            let mut c = CaptureRow { frame_ms: 50, bands: 2, frames: vec![0; 10 * 2], present: vec![0; 10], duration_ms: 500 };
            for i in range {
                c.present[i] = 1;
                c.frames[i * 2] = val;
                c.frames[i * 2 + 1] = val;
            }
            c
        };
        let key = "isrc:USX";
        let first = s.save_capture(key, &mk(0..4, 10), 1).unwrap();
        assert_eq!(first.coverage(), 0.4);
        // second listen heard 3..8 (overlapping frame 3 with a different value)
        let merged = s.save_capture(key, &mk(3..8, 20), 2).unwrap();
        assert_eq!(merged.coverage(), 0.8);
        assert_eq!(merged.frames[3 * 2], 10, "frames already heard are kept");
        assert_eq!(merged.frames[6 * 2], 20, "gaps are filled");
        assert_eq!(s.get_capture(key).unwrap().unwrap(), merged);
        assert_eq!(s.capture_counts().unwrap(), (1, 0));
        s.reset_audio().unwrap();
        assert!(s.get_capture(key).unwrap().is_some(), "reset does not touch recordings");
        let _ = id;
    }

    #[test]
    fn fingerprints_are_keyed_by_isrc_and_old_keys_migrate() {
        let s = Store::memory().unwrap();
        let mut t = track("7");
        t.meta.isrc = Some("us-abc".into());
        assert_eq!(capture_key(&t), "isrc:US-ABC");
        t.meta.isrc = None;
        assert_eq!(capture_key(&t), "spotify:7", "no ISRC: fall back to the provider id");
        t.meta.isrc = Some("usabc123".into());
        s.add_track(&t, 1).unwrap();
        // a fingerprint saved the old way, under the provider id
        let row = CaptureRow { frame_ms: 50, bands: 2, frames: vec![5; 8], present: vec![1; 4], duration_ms: 200 };
        s.save_capture("spotify:7", &row, 2).unwrap();
        assert_eq!(s.capture_key_for(&TrackId::new("spotify", "7")).unwrap(), "isrc:USABC123");
        assert_eq!(s.migrate_capture_keys().unwrap(), 1);
        assert!(s.get_capture("spotify:7").unwrap().is_none());
        assert_eq!(s.get_capture("isrc:USABC123").unwrap().unwrap(), row);
        assert_eq!(s.migrate_capture_keys().unwrap(), 0, "idempotent");
    }

    #[test]
    fn user_tempo_roundtrip_and_track_tempo_update() {
        let s = Store::memory().unwrap();
        let t = track("1");
        s.add_track(&t, 1).unwrap();
        s.set_user_tempo(&t.id, Some(140.0), 2).unwrap();
        assert_eq!(s.user_tempos().unwrap().get("spotify:1"), Some(&140.0));
        s.update_track_tempo(&t.id, 99.5).unwrap();
        assert_eq!(s.load_library().unwrap()[0].track.tempo, Some(99.5));
        s.set_user_tempo(&t.id, None, 3).unwrap();
        assert!(s.user_tempos().unwrap().is_empty());
    }

    #[test]
    fn mutes_favorites_and_blocks_persist_and_load() {
        let s = Store::memory().unwrap();
        let t = track("1");
        s.add_track(&t, 1).unwrap();
        s.set_mute(&t.id, Some(500)).unwrap();
        s.set_favorite(&t.id, true, 2).unwrap();
        let l = &s.load_library().unwrap()[0];
        assert_eq!((l.muted_until, l.favorite), (Some(500), true));
        s.set_mute(&t.id, None).unwrap();
        s.set_favorite(&t.id, false, 3).unwrap();
        let l = &s.load_library().unwrap()[0];
        assert_eq!((l.muted_until, l.favorite), (None, false));

        s.add_block("artist", "Slowdive", 4).unwrap();
        s.add_block("artist", "Slowdive", 5).unwrap(); // idempotent
        s.add_block("tag", "polka", 6).unwrap();
        assert_eq!(s.list_blocks().unwrap(), [("artist".to_string(), "Slowdive".to_string()), ("tag".to_string(), "polka".to_string())]);
        s.remove_block("artist", "Slowdive").unwrap();
        assert_eq!(s.list_blocks().unwrap().len(), 1);
        // mutes and favourites survive a soft delete + restore of the track
        s.set_favorite(&t.id, true, 7).unwrap();
        s.remove_track(&t.id, 8).unwrap();
        s.restore_deleted().unwrap();
        assert!(s.load_library().unwrap()[0].favorite);
    }

    #[test]
    fn kv_and_cascade_delete() {
        let s = Store::memory().unwrap();
        s.kv_set("a", "1").unwrap();
        s.kv_set("a", "2").unwrap();
        assert_eq!(s.kv_get("a").unwrap().as_deref(), Some("2"));
        s.add_track(&track("1"), 1).unwrap();
        s.record_vote(&TrackId::new("spotify", "1"), 2, true).unwrap();
        s.remove_track(&TrackId::new("spotify", "1"), 9).unwrap();
        assert!(s.load_library().unwrap().is_empty());
    }
}
