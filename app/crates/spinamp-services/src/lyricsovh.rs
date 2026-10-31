//! lyrics.ovh: free, keyless, unofficial lyrics API. Plain text only (no timing). Used as the
//! backup when LRCLIB is down or has nothing.

use crate::lrclib::Lyrics;
use serde_json::Value;

/// Normalise line endings and drop the "Paroles de la chanson ..." banner some entries start with.
pub fn clean(raw: &str) -> Option<String> {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.first().is_some_and(|l| l.trim_start().to_lowercase().starts_with("paroles de la chanson")) {
        lines.remove(0);
    }
    let out = lines.join("\n").trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// `Ok(None)` = not found. `Err` = unreachable / server error.
pub async fn fetch(artist: &str, title: &str) -> Result<Option<Lyrics>, String> {
    let mut url = reqwest::Url::parse("https://api.lyrics.ovh/v1/").map_err(|e| e.to_string())?;
    url.path_segments_mut().map_err(|_| "bad url")?.pop_if_empty().push(artist).push(title); // encodes "/" and spaces
    let http = reqwest::Client::builder()
        .user_agent("Spinamp/0.1 (personal music player)")
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())?;
    let mut last = String::new();
    for attempt in 0..3 {
        match http.get(url.clone()).send().await {
            Ok(r) => match r.status().as_u16() {
                200 => {
                    let v: Value = r.json().await.map_err(|e| e.to_string())?;
                    return Ok(v["lyrics"].as_str().and_then(clean).map(|plain| Lyrics {
                        plain: Some(plain),
                        synced: vec![],
                        instrumental: false,
                        source: Some("lyrics.ovh".into()),
                    }));
                }
                404 => return Ok(None),
                s => last = format!("lyrics.ovh {s}"),
            },
            Err(e) => last = e.to_string(),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_banner_and_line_endings() {
        assert_eq!(clean("Paroles de la chanson Alison par Slowdive\r\nListen close\r\nand don't\r\n").as_deref(), Some("Listen close\nand don't"));
        assert_eq!(clean("  \r\n "), None);
    }
}
