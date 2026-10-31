//! iTunes Search (keyless): fallback source of 30 s previews (AAC) when Deezer has none.
use serde_json::Value;

/// Best preview URL whose artist and title match (case-insensitive containment).
pub fn pick_preview(v: &Value, artist: &str, title: &str) -> Option<String> {
    let (a, t) = (artist.to_lowercase(), title.to_lowercase());
    v["results"].as_array()?.iter().find_map(|r| {
        let ra = r["artistName"].as_str()?.to_lowercase();
        let rt = r["trackName"].as_str()?.to_lowercase();
        let ok = (ra.contains(&a) || a.contains(&ra)) && (rt.contains(&t) || t.contains(&rt));
        ok.then(|| r["previewUrl"].as_str().map(String::from)).flatten()
    })
}

pub async fn preview(artist: &str, title: &str) -> Result<Option<String>, String> {
    let r = crate::http()
        .get("https://itunes.apple.com/search")
        .query(&[("term", format!("{artist} {title}").as_str()), ("entity", "song"), ("limit", "5")])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("itunes {}", r.status()));
    }
    Ok(pick_preview(&r.json::<Value>().await.map_err(|e| e.to_string())?, artist, title))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_artist_and_title() {
        let v = json!({"results":[
            {"artistName":"Someone Else","trackName":"Alison","previewUrl":"wrong"},
            {"artistName":"Slowdive","trackName":"Alison (Remastered)","previewUrl":"right"}]});
        assert_eq!(pick_preview(&v, "Slowdive", "Alison").as_deref(), Some("right"));
        assert_eq!(pick_preview(&v, "Nobody", "Nothing"), None);
    }
}
