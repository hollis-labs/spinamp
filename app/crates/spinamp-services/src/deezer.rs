//! Deezer public API (keyless): artist photo, fan count, top tracks.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeezerArtist {
    pub id: u64,
    pub name: String,
    pub fans: u64,
    pub picture: Option<String>,
    pub link: Option<String>,
    pub top_tracks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeezerTrack {
    pub preview: Option<String>,
    pub bpm: Option<f32>,
    pub gain: Option<f32>,
    pub rank: Option<u64>,
}

/// `Ok(None)` = Deezer does not know this ISRC. `Err` = transient/quota (retry later).
pub fn parse_track(v: &Value) -> Result<Option<DeezerTrack>, String> {
    if let Some(e) = v.get("error") {
        return match e["code"].as_u64() {
            Some(800) | Some(404) => Ok(None), // "no data"
            _ => Err(format!("deezer: {}", e["message"].as_str().unwrap_or("error"))),
        };
    }
    Ok(Some(DeezerTrack {
        preview: v["preview"].as_str().filter(|u| !u.is_empty()).map(String::from),
        bpm: v["bpm"].as_f64().filter(|b| *b > 0.0).map(|b| b as f32),
        gain: v["gain"].as_f64().map(|g| g as f32),
        rank: v["rank"].as_u64(),
    }))
}

pub async fn track_by_isrc(isrc: &str) -> Result<Option<DeezerTrack>, String> {
    let r = crate::http().get(format!("https://api.deezer.com/track/isrc:{isrc}")).send().await.map_err(|e| e.to_string())?;
    if r.status().is_server_error() {
        return Err(format!("deezer {}", r.status()));
    }
    parse_track(&r.json::<Value>().await.map_err(|e| e.to_string())?)
}

/// Download a preview clip (they are ~0.5 MB). Refuses anything suspiciously large.
pub async fn download(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let r = crate::http().get(url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("preview download {}", r.status()));
    }
    let b = r.bytes().await.map_err(|e| e.to_string())?;
    if b.len() > max_bytes {
        return Err("preview too large".into());
    }
    Ok(b.to_vec())
}

pub fn parse_artist(v: &Value, wanted: &str) -> Option<DeezerArtist> {
    let a = v["data"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(wanted)))?;
    Some(DeezerArtist {
        id: a["id"].as_u64()?,
        name: a["name"].as_str()?.into(),
        fans: a["nb_fan"].as_u64().unwrap_or(0),
        picture: a["picture_xl"].as_str().or(a["picture_big"].as_str()).map(String::from),
        link: a["link"].as_str().map(String::from),
        top_tracks: vec![],
    })
}

pub async fn artist(name: &str) -> Result<Option<DeezerArtist>, String> {
    let http = crate::http();
    let r = http
        .get("https://api.deezer.com/search/artist")
        .query(&[("q", name), ("limit", "5")])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    let Some(mut a) = parse_artist(&v, name) else { return Ok(None) };
    if let Ok(r) = http.get(format!("https://api.deezer.com/artist/{}/top", a.id)).query(&[("limit", "5")]).send().await {
        if let Ok(t) = r.json::<Value>().await {
            a.top_tracks = t["data"].as_array().map(|d| d.iter().filter_map(|x| x["title"].as_str().map(String::from)).collect()).unwrap_or_default();
        }
    }
    Ok(Some(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_track_and_errors() {
        let t = parse_track(&json!({"preview":"http://p","bpm":0,"gain":-9.7,"rank":810329})).unwrap().unwrap();
        assert_eq!((t.preview.as_deref(), t.bpm, t.gain, t.rank), (Some("http://p"), None, Some(-9.7), Some(810329)));
        assert_eq!(parse_track(&json!({"error":{"code":800,"message":"no data"}})), Ok(None));
        assert!(parse_track(&json!({"error":{"code":4,"message":"Quota limit exceeded"}})).is_err());
    }

    #[test]
    fn matches_exact_name_only() {
        let v = json!({"data":[{"id":1,"name":"Slowdive Cover Band","nb_fan":5},{"id":2021,"name":"Slowdive","nb_fan":83961,"picture_xl":"p","link":"l"}]});
        let a = parse_artist(&v, "slowdive").unwrap();
        assert_eq!((a.id, a.fans, a.picture.as_deref()), (2021, 83961, Some("p")));
        assert!(parse_artist(&v, "Nobody").is_none());
    }
}
