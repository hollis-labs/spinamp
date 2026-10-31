//! Spotify JSON -> core model. Pure, so it is tested against fixtures.
use serde::Deserialize;
use spinamp_core::{Track, TrackId, TrackMeta};

#[derive(Debug, Deserialize)]
pub struct SpTrack {
    pub id: Option<String>,
    pub name: String,
    pub duration_ms: u64,
    pub track_number: Option<u32>,
    pub artists: Vec<SpArtist>,
    pub album: Option<SpAlbum>,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub explicit: bool,
    pub disc_number: Option<u32>,
    pub external_ids: Option<SpExternalIds>,
    pub uri: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct SpExternalIds {
    pub isrc: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct SpArtist {
    pub name: String,
    pub id: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct SpAlbum {
    pub name: String,
    pub id: Option<String>,
    pub release_date: Option<String>,
    #[serde(default)]
    pub images: Vec<SpImage>,
}
#[derive(Debug, Deserialize)]
pub struct SpImage {
    pub url: String,
    pub width: Option<u32>,
}

pub fn to_track(t: SpTrack) -> Option<Track> {
    let id = t.id.filter(|_| !t.is_local)?; // local files cannot be played via Connect
    Some(Track {
        id: TrackId::new("spotify", &id),
        provider: "spotify".into(),
        title: t.name,
        artist: t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "),
        album: t.album.as_ref().map(|a| a.name.clone()),
        track_number: t.track_number,
        duration_ms: t.duration_ms,
        tempo: None,
        energy: None,
        tags: vec![],
        meta: TrackMeta {
            isrc: t.external_ids.and_then(|e| e.isrc),
            explicit: t.explicit,
            release_date: t.album.as_ref().and_then(|a| a.release_date.clone()),
            // Prefer a mid-size image (~300px) over the 640px original.
            image_url: t.album.as_ref().and_then(|a| {
                a.images.iter().min_by_key(|i| (i.width.unwrap_or(0) as i64 - 300).abs()).map(|i| i.url.clone())
            }),
            album_id: t.album.as_ref().and_then(|a| a.id.clone()),
            artist_ids: t.artists.iter().filter_map(|a| a.id.clone()).collect(),
            disc_number: t.disc_number,
            uri: t.uri,
            audio: None,
        },
    })
}

/// `spotify:abc` -> `spotify:track:abc` URI the player endpoints want.
pub fn uri(id: &TrackId) -> String {
    format!("spotify:track:{}", id.0.strip_prefix("spotify:").unwrap_or(&id.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_track_and_joins_artists() {
        let t: SpTrack = serde_json::from_str(
            r#"{"id":"abc","name":"Alison","duration_ms":231000,"track_number":2,
                "artists":[{"name":"Slowdive"},{"name":"Guest"}],"album":{"name":"Souvlaki"}}"#,
        )
        .unwrap();
        let m = to_track(t).unwrap();
        assert_eq!(m.id.0, "spotify:abc");
        assert_eq!(m.artist, "Slowdive, Guest");
        assert_eq!(uri(&m.id), "spotify:track:abc");
        assert_eq!(m.album.as_deref(), Some("Souvlaki"));
    }

    #[test]
    fn captures_meta() {
        let t: SpTrack = serde_json::from_str(
            r#"{"id":"abc","name":"x","duration_ms":1,"explicit":true,"disc_number":1,"uri":"spotify:track:abc",
                "external_ids":{"isrc":"US123"},"artists":[{"name":"A","id":"a1"}],
                "album":{"name":"Al","id":"al1","release_date":"2025-06-06",
                 "images":[{"url":"big","width":640},{"url":"mid","width":300},{"url":"sm","width":64}]}}"#,
        ).unwrap();
        let m = to_track(t).unwrap().meta;
        assert_eq!((m.isrc.as_deref(), m.explicit, m.image_url.as_deref()), (Some("US123"), true, Some("mid")));
        assert_eq!((m.album_id.as_deref(), m.artist_ids.as_slice(), m.release_date.as_deref()), (Some("al1"), &["a1".to_string()][..], Some("2025-06-06")));
    }

    #[test]
    fn drops_local_and_idless_tracks() {
        let local: SpTrack = serde_json::from_str(
            r#"{"id":null,"name":"x","duration_ms":1,"artists":[],"is_local":true}"#,
        )
        .unwrap();
        assert!(to_track(local).is_none());
    }
}
