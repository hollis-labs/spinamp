use crate::feedback::FeedbackLedger;
use serde::{Deserialize, Serialize};

/// Unix seconds.
pub type Ts = i64;

/// Globally unique id: `"<provider>:<native id>"`, e.g. `spotify:4uLU6hMCjMI75M1A2tKUQC`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackId(pub String);

impl TrackId {
    pub fn new(provider: &str, native: &str) -> Self {
        Self(format!("{provider}:{native}"))
    }
}

/// Audio traits Spinamp measures itself from a 30s preview, plus what other services report.
/// Provider-independent: works for any track that has a preview somewhere.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioFeatures {
    pub tempo_bpm: Option<f32>,
    /// How clearly periodic the beat is, 0..1 (low for ambient / free-time music).
    pub beat_strength: f32,
    /// Average level, dBFS (RMS). Roughly -30 (very quiet) .. -6 (very loud/compressed).
    pub loudness_db: f32,
    pub peak_db: f32,
    /// Spread between quiet and loud passages (p95 - p10 of short-window level), dB.
    pub dynamic_range_db: f32,
    /// How far the loudest moments jump above the typical level, dB. High = sudden loud spikes.
    pub spike_db: f32,
    /// Spectral centroid, Hz (bright vs dark).
    pub brightness_hz: f32,
    /// Mean onset intensity (log spectral flux): how busy/percussive the texture is.
    #[serde(default)]
    pub flux: f32,
    /// Composite 0..1: loudness + spectral flux + brightness.
    pub energy: f32,
    /// Other plausible tempi with relative strength (best = 1.0), e.g. the half/double-time or
    /// 3:2 readings. Lets later passes (genre priors, other sources) pick the right one.
    #[serde(default)]
    pub tempo_candidates: Vec<(f32, f32)>,
    /// Tempo Deezer reports for the recording, when it has a non-zero value (rare, but a useful
    /// independent check for half/double-time and 3:2 mix-ups).
    #[serde(default)]
    pub deezer_bpm: Option<f32>,
    /// Musical key, e.g. "A minor" (None when the music is not clearly tonal).
    #[serde(default)]
    pub key: Option<String>,
    /// How clearly that key stands out (correlation with the best key profile, 0..1).
    #[serde(default)]
    pub key_confidence: f32,
    /// Emotional positivity proxy 0..1 (mode, brightness, tempo). A heuristic, not a measurement.
    #[serde(default)]
    pub valence: f32,
    /// Mood words derived from energy and valence, e.g. ["uplifting", "energetic", "danceable"].
    #[serde(default)]
    pub moods: Vec<String>,
    /// How suited to dancing: steady beat at a danceable tempo, 0..1.
    #[serde(default)]
    pub danceability: f32,
    /// Detected time signature numerator (3 or 4).
    #[serde(default)]
    pub beats_per_bar: Option<u8>,
    /// Where it came from ("preview:deezer", "preview:itunes").
    #[serde(default)]
    pub source: String,
    /// Deezer's track gain (dB) and rank, if known; independent cross-checks.
    #[serde(default)]
    pub deezer_gain: Option<f32>,
    #[serde(default)]
    pub deezer_rank: Option<u64>,
}

/// A beat-quantised spectrum recording of a track's preview, for driving a visualiser from
/// tempo + playback position. Stored apart from [`AudioFeatures`] because it is bulky.
///
/// `data[(beat * subdiv + sub) * bands + band]`, 0..=255, bands run low to high frequency
/// (log-spaced 40 Hz..10 kHz for version 2). Beat 0 is the detected downbeat and `beats` is a
/// whole number of bars. Version 1 was a 4-band average folded to four bars (too static).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VizPattern {
    pub beats: u8,
    pub subdiv: u8,
    pub beats_per_bar: u8,
    #[serde(default = "default_bands")]
    pub bands: u8,
    #[serde(default = "default_version")]
    pub version: u8,
    /// The tempo this pattern's beat grid was built on. Playback must use this, not the track's
    /// (possibly reconciled) tempo, or the pattern would run at the wrong speed. 0 = unknown.
    #[serde(default)]
    pub bpm: f32,
    #[serde(with = "viz_data")]
    pub data: Vec<u8>,
}

/// Pattern bytes travel as a base64 string (3x smaller than a JSON number array); the old
/// array form is still accepted when reading.
mod viz_data {
    use base64::{Engine, engine::general_purpose::STANDARD as B64};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(v))
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Text(String),
        Array(Vec<u8>),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        match Either::deserialize(d)? {
            Either::Text(t) => B64.decode(t).map_err(serde::de::Error::custom),
            Either::Array(a) => Ok(a),
        }
    }
}

fn default_bands() -> u8 {
    4
}
fn default_version() -> u8 {
    1
}

/// Everything else a provider tells us about a track. All optional so old rows and
/// providers with less data still load.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackMeta {
    pub isrc: Option<String>,
    pub explicit: bool,
    pub release_date: Option<String>,
    pub image_url: Option<String>,
    pub album_id: Option<String>,
    pub artist_ids: Vec<String>,
    pub disc_number: Option<u32>,
    pub uri: Option<String>,
    #[serde(default)]
    pub audio: Option<AudioFeatures>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub provider: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    pub duration_ms: u64,
    /// Audio traits are optional: not every provider exposes them (Spotify has
    /// withdrawn audio-features for new apps), so they may be filled by another source.
    pub tempo: Option<f32>,
    pub energy: Option<f32>,
    pub tags: Vec<String>,
    #[serde(default)]
    pub meta: TrackMeta,
}

/// A track as it lives in a playlist/library: the track plus everything we learned about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackState {
    pub track: Track,
    pub added_at: Ts,
    pub updated_at: Ts,
    pub last_played: Option<Ts>,
    pub play_count: u32,
    /// Explicit 0..=5 star rating, independent of thumbs.
    pub rating: Option<u8>,
    pub votes: FeedbackLedger,
    /// Not played automatically until this time (unix seconds). Still listed.
    #[serde(default)]
    pub muted_until: Option<Ts>,
    /// Starred: permanently hotter in shuffle, sorting and taste learning.
    #[serde(default)]
    pub favorite: bool,
}

impl TrackState {
    pub fn new(track: Track, added_at: Ts) -> Self {
        Self {
            track,
            added_at,
            updated_at: added_at,
            last_played: None,
            play_count: 0,
            rating: None,
            votes: FeedbackLedger::default(),
            muted_until: None,
            favorite: false,
        }
    }
}

/// What we know about an artist from an external source (MusicBrainz).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtistInfo {
    pub name: String,
    pub mbid: Option<String>,
    pub country: Option<String>,
    pub kind: Option<String>,
    pub begin_year: Option<i32>,
    /// (tag, votes), strongest first.
    pub tags: Vec<(String, u32)>,
}

/// Primary artist of a joined credit: "A, B" -> "A".
pub fn primary_artist(credit: &str) -> &str {
    credit.split(", ").next().unwrap_or(credit)
}

/// Merge a freshly imported `new` copy of a track into what we already hold (`old`).
/// Imports only ever ADD: provider fields refresh when the provider has a value, derived and
/// user-built data (tags, tempo, energy, measured audio) is never dropped, and nothing that
/// was present is blanked out.
pub fn merge_track(old: &Track, new: &Track) -> Track {
    fn pick<T: Clone>(new: &Option<T>, old: &Option<T>) -> Option<T> {
        new.clone().or_else(|| old.clone())
    }
    fn union(a: &[String], b: &[String]) -> Vec<String> {
        let mut out: Vec<String> = a.to_vec();
        for x in b {
            if !out.iter().any(|y| y.eq_ignore_ascii_case(x)) {
                out.push(x.clone());
            }
        }
        out
    }
    let text = |new: &str, old: &str| if new.trim().is_empty() { old.to_string() } else { new.to_string() };
    Track {
        id: old.id.clone(),
        provider: old.provider.clone(),
        title: text(&new.title, &old.title),
        artist: text(&new.artist, &old.artist),
        album: pick(&new.album, &old.album),
        track_number: pick(&new.track_number, &old.track_number),
        duration_ms: if new.duration_ms > 0 { new.duration_ms } else { old.duration_ms },
        tempo: pick(&old.tempo, &new.tempo),   // keep what we measured; only fill a gap
        energy: pick(&old.energy, &new.energy),
        tags: union(&old.tags, &new.tags),
        meta: TrackMeta {
            isrc: pick(&new.meta.isrc, &old.meta.isrc),
            explicit: new.meta.explicit || old.meta.explicit,
            release_date: pick(&new.meta.release_date, &old.meta.release_date),
            image_url: pick(&new.meta.image_url, &old.meta.image_url),
            album_id: pick(&new.meta.album_id, &old.meta.album_id),
            artist_ids: union(&old.meta.artist_ids, &new.meta.artist_ids),
            disc_number: pick(&new.meta.disc_number, &old.meta.disc_number),
            uri: pick(&new.meta.uri, &old.meta.uri),
            audio: pick(&old.meta.audio, &new.meta.audio),
        },
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn base() -> Track {
        Track {
            id: TrackId::new("spotify", "1"), provider: "spotify".into(), title: "Old Title".into(), artist: "A".into(),
            album: Some("Old Album".into()), track_number: Some(3), duration_ms: 1000, tempo: Some(120.0), energy: Some(0.6),
            tags: vec!["shoegaze".into()],
            meta: TrackMeta { isrc: Some("ISRC".into()), artist_ids: vec!["a1".into()], audio: Some(AudioFeatures { energy: 0.6, ..Default::default() }), ..Default::default() },
        }
    }

    #[test]
    fn merge_adds_new_and_never_removes_or_blanks() {
        let old = base();
        let mut new = base();
        new.title = "New Title".into();
        new.album = None; // provider dropped it: keep ours
        new.tempo = Some(99.0); // must not overwrite a measurement
        new.tags = vec!["Shoegaze".into(), "dream pop".into()];
        new.meta = TrackMeta { isrc: None, release_date: Some("2020".into()), artist_ids: vec!["a2".into()], ..Default::default() };
        let m = merge_track(&old, &new);
        assert_eq!(m.title, "New Title");
        assert_eq!(m.album.as_deref(), Some("Old Album"));
        assert_eq!(m.tempo, Some(120.0));
        assert_eq!(m.tags, ["shoegaze", "dream pop"], "union, case-insensitive, order kept");
        assert_eq!(m.meta.isrc.as_deref(), Some("ISRC"));
        assert_eq!(m.meta.release_date.as_deref(), Some("2020"));
        assert_eq!(m.meta.artist_ids, ["a1", "a2"]);
        assert!(m.meta.audio.is_some());
    }

    #[test]
    fn merge_is_idempotent() {
        let old = base();
        let once = merge_track(&old, &old);
        assert_eq!(once, old);
        assert_eq!(merge_track(&once, &old), once);
    }
}

#[cfg(test)]
mod viz_tests {
    use super::*;

    #[test]
    fn pattern_roundtrips_as_base64_and_reads_old_arrays() {
        let v = VizPattern { beats: 8, subdiv: 8, beats_per_bar: 4, bands: 16, version: 2, bpm: 120.0, data: (0..=255u8).cycle().take(8 * 8 * 16).collect() };
        let j = serde_json::to_string(&v).unwrap();
        assert!(j.contains("\"data\":\""), "string form: {}", &j[..80.min(j.len())]);
        assert!(j.len() < 1500, "compact: {} bytes", j.len());
        assert_eq!(serde_json::from_str::<VizPattern>(&j).unwrap(), v);
        let old = r#"{"beats":16,"subdiv":8,"beats_per_bar":4,"data":[1,2,3]}"#;
        let o: VizPattern = serde_json::from_str(old).unwrap();
        assert_eq!((o.bands, o.version, o.data.as_slice()), (4, 1, &[1u8, 2, 3][..]));
    }
}
