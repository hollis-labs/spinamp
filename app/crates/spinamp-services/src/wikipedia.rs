//! Wikipedia REST summary: description, short bio, thumbnail. Keyless.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub title: String,
    pub description: Option<String>,
    pub extract: String,
    pub thumbnail: Option<String>,
    pub url: Option<String>,
}

const MUSIC_WORDS: &[&str] = &["band", "musician", "singer", "rapper", "dj", "producer", "duo", "group", "composer", "songwriter", "vocalist", "guitarist", "trio", "ensemble", "orchestra"];

/// Parse a REST summary; reject disambiguation pages and non-music subjects.
pub fn parse_summary(v: &Value) -> Option<Summary> {
    if v["type"].as_str() == Some("disambiguation") {
        return None;
    }
    let desc = v["description"].as_str().unwrap_or("");
    let extract = v["extract"].as_str()?.to_string();
    let hay = format!("{} {}", desc, extract.chars().take(200).collect::<String>()).to_lowercase();
    if !MUSIC_WORDS.iter().any(|w| hay.contains(w)) {
        return None;
    }
    Some(Summary {
        title: v["title"].as_str()?.into(),
        description: v["description"].as_str().map(String::from),
        extract,
        thumbnail: v["thumbnail"]["source"].as_str().map(String::from),
        url: v["content_urls"]["desktop"]["page"].as_str().map(String::from),
    })
}

async fn summary_for(title: &str) -> Result<Option<Summary>, String> {
    let url = format!("https://en.wikipedia.org/api/rest_v1/page/summary/{}", title.replace(' ', "_").replace('?', "%3F").replace('/', "%2F"));
    let r = crate::http().get(url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Ok(None);
    }
    Ok(parse_summary(&r.json::<Value>().await.map_err(|e| e.to_string())?))
}

/// Prefer the exact Wikipedia page title (from MusicBrainz links); otherwise try the plain
/// name and common disambiguations.
pub async fn artist_summary(name: &str, linked_title: Option<&str>) -> Result<Option<Summary>, String> {
    let mut tries: Vec<String> = linked_title.map(|t| vec![t.to_string()]).unwrap_or_default();
    tries.extend([name.to_string(), format!("{name} (band)"), format!("{name} (musician)"), format!("{name} (singer)")]);
    for t in tries {
        if let Some(s) = summary_for(&t).await? {
            return Ok(Some(s));
        }
    }
    Ok(None)
}

/// `https://en.wikipedia.org/wiki/Slowdive` -> `Slowdive`
pub fn title_from_url(url: &str) -> Option<String> {
    url.split("/wiki/").nth(1).map(|t| t.replace('_', " ")).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_music_pages_only() {
        let ok = json!({"title":"Slowdive","description":"British shoegaze band","extract":"Slowdive is an English rock band.",
                        "thumbnail":{"source":"http://img"},"content_urls":{"desktop":{"page":"http://w"}}});
        assert_eq!(parse_summary(&ok).unwrap().thumbnail.as_deref(), Some("http://img"));
        assert!(parse_summary(&json!({"title":"Slowdive","description":"A dive technique","extract":"Slow diving is a swimming style."})).is_none());
        assert!(parse_summary(&json!({"type":"disambiguation","title":"X","extract":"X may refer to a band"})).is_none());
    }

    #[test]
    fn title_from_wiki_url() {
        assert_eq!(title_from_url("https://en.wikipedia.org/wiki/My_Bloody_Valentine_(band)").as_deref(), Some("My Bloody Valentine (band)"));
    }
}
