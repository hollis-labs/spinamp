//! Apple Music JSON -> core model. Pure, so it is tested against fixtures.
use serde::Deserialize;
use spinamp_core::{Track, TrackId, TrackMeta};

pub const PROVIDER: &str = "applemusic";

#[derive(Debug, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub data: Vec<Resource>,
    pub next: Option<String>,
    #[serde(default)]
    pub meta: Option<PageMeta>,
}
#[derive(Debug, Deserialize)]
pub struct PageMeta {
    pub total: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct Resource {
    pub id: String,
    #[serde(default)]
    pub attributes: Attrs,
    #[serde(default)]
    pub relationships: Option<Rels>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attrs {
    pub name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub duration_in_millis: Option<u64>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub isrc: Option<String>,
    pub release_date: Option<String>,
    pub content_rating: Option<String>,
    #[serde(default)]
    pub genre_names: Vec<String>,
    pub artwork: Option<Artwork>,
    pub play_params: Option<PlayParams>,
}
#[derive(Debug, Deserialize)]
pub struct Artwork {
    pub url: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayParams {
    pub catalog_id: Option<String>,
    pub id: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct Rels {
    pub catalog: Option<Related>,
}
#[derive(Debug, Deserialize)]
pub struct Related {
    #[serde(default)]
    pub data: Vec<Resource>,
}

/// `{w}x{h}` placeholders -> a 300px square.
fn artwork(url: &str) -> String {
    url.replace("{w}", "300").replace("{h}", "300").replace("{f}", "jpg")
}

/// Genres worth keeping as tags ("Music" is Apple's catch-all).
fn tags(genres: &[String]) -> Vec<String> {
    genres.iter().filter(|g| !g.eq_ignore_ascii_case("music")).map(|g| g.to_lowercase()).collect()
}

/// A catalog song, or a library song (optionally with its catalog twin attached via
/// `include=catalog`, which is where the ISRC lives).
pub fn to_track(r: Resource) -> Option<Track> {
    let catalog = r.relationships.and_then(|x| x.catalog).and_then(|c| c.data.into_iter().next());
    let a = r.attributes;
    // Prefer the catalog id: stable across library/catalog and what play URLs need.
    let native = catalog
        .as_ref()
        .map(|c| c.id.clone())
        .or_else(|| a.play_params.as_ref().and_then(|p| p.catalog_id.clone()))
        .unwrap_or_else(|| r.id.clone());
    let title = a.name?;
    let (isrc, art, genres) = match &catalog {
        Some(c) => (
            c.attributes.isrc.clone().or(a.isrc.clone()),
            a.artwork.as_ref().or(c.attributes.artwork.as_ref()).map(|w| artwork(&w.url)),
            if a.genre_names.is_empty() { tags(&c.attributes.genre_names) } else { tags(&a.genre_names) },
        ),
        None => (a.isrc.clone(), a.artwork.as_ref().map(|w| artwork(&w.url)), tags(&a.genre_names)),
    };
    let has_catalog_id = !native.starts_with("i.") && !native.starts_with("l.");
    Some(Track {
        id: TrackId::new(PROVIDER, &native),
        provider: PROVIDER.into(),
        title,
        artist: a.artist_name.unwrap_or_default(),
        album: a.album_name,
        track_number: a.track_number,
        duration_ms: a.duration_in_millis.unwrap_or(0),
        tempo: None,
        energy: None,
        tags: genres,
        meta: TrackMeta {
            isrc,
            explicit: a.content_rating.as_deref() == Some("explicit"),
            release_date: a.release_date,
            image_url: art,
            album_id: None,
            artist_ids: vec![],
            disc_number: a.disc_number,
            uri: has_catalog_id.then(|| format!("https://music.apple.com/song/{native}")),
            audio: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_catalog_song() {
        let p: Page = serde_json::from_str(
            r#"{"data":[{"id":"1440833098","type":"songs","attributes":{"name":"Alison","artistName":"Slowdive",
              "albumName":"Souvlaki","durationInMillis":231000,"trackNumber":2,"discNumber":1,"isrc":"GBAAA9300001",
              "releaseDate":"1993-06-01","contentRating":"explicit","genreNames":["Alternative","Music"],
              "artwork":{"url":"https://x/{w}x{h}bb.{f}"}}}]}"#,
        ).unwrap();
        let t = to_track(p.data.into_iter().next().unwrap()).unwrap();
        assert_eq!((t.id.0.as_str(), t.provider.as_str(), t.artist.as_str(), t.duration_ms), ("applemusic:1440833098", "applemusic", "Slowdive", 231000));
        assert_eq!(t.tags, vec!["alternative"]);
        assert_eq!((t.meta.isrc.as_deref(), t.meta.explicit), (Some("GBAAA9300001"), true));
        assert_eq!(t.meta.image_url.as_deref(), Some("https://x/300x300bb.jpg"));
        assert_eq!(t.meta.uri.as_deref(), Some("https://music.apple.com/song/1440833098"));
    }

    #[test]
    fn library_song_takes_catalog_twin_for_id_and_isrc() {
        let p: Page = serde_json::from_str(
            r#"{"data":[{"id":"i.AbC","type":"library-songs","attributes":{"name":"Alison","artistName":"Slowdive",
              "durationInMillis":231000,"playParams":{"id":"i.AbC","catalogId":"1440833098"}},
              "relationships":{"catalog":{"data":[{"id":"1440833098","attributes":{"isrc":"GBAAA9300001","genreNames":["Alternative"]}}]}}}],
              "next":"/v1/me/library/playlists/p.1/tracks?offset=100","meta":{"total":123}}"#,
        ).unwrap();
        assert_eq!((p.next.is_some(), p.meta.as_ref().and_then(|m| m.total)), (true, Some(123)));
        let t = to_track(p.data.into_iter().next().unwrap()).unwrap();
        assert_eq!((t.id.0.as_str(), t.meta.isrc.as_deref()), ("applemusic:1440833098", Some("GBAAA9300001")));
        assert_eq!(t.tags, vec!["alternative"]);
    }

    #[test]
    fn uploaded_library_song_has_no_play_url() {
        let p: Page = serde_json::from_str(r#"{"data":[{"id":"i.Zed","attributes":{"name":"Demo","artistName":"Me"}}]}"#).unwrap();
        let t = to_track(p.data.into_iter().next().unwrap()).unwrap();
        assert_eq!((t.id.0.as_str(), t.meta.uri), ("applemusic:i.Zed", None));
    }
}
