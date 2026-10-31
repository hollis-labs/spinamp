//! LRCLIB (lrclib.net): free, keyless, open lyrics database with time-synced lyrics for many tracks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    pub plain: Option<String>,
    /// (start ms, line). Empty when only plain lyrics exist.
    pub synced: Vec<(u64, String)>,
    pub instrumental: bool,
    /// Which service supplied it ("lrclib" / "lyrics.ovh"). Older cached entries have none.
    #[serde(default)]
    pub source: Option<String>,
}

/// "[01:23.45] text" lines -> (ms, text). Lines without a timestamp are ignored; a line with
/// several stamps ("[00:10.0][00:50.0] chorus") yields one entry per stamp.
pub fn parse_lrc(lrc: &str) -> Vec<(u64, String)> {
    let mut out = vec![];
    for line in lrc.lines() {
        let mut rest = line.trim();
        let mut stamps = vec![];
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            rest = r[end + 1..].trim_start();
            let Some((m, s)) = tag.split_once(':') else { continue };
            if let (Ok(m), Ok(s)) = (m.parse::<u64>(), s.parse::<f64>()) {
                stamps.push(m * 60_000 + (s * 1000.0).round() as u64);
            }
        }
        for t in stamps {
            out.push((t, rest.to_string()));
        }
    }
    out.sort_by_key(|x| x.0);
    out
}

/// Strip "- Remastered 2009", "(Live)", "[Deluxe]", "feat. X" so lookups match catalog titles.
pub fn clean_title(title: &str) -> String {
    let mut t = title.to_string();
    for sep in [" - ", " – "] {
        if let Some(i) = t.find(sep) {
            let tail = t[i + sep.len()..].to_lowercase();
            if ["remaster", "live", "mono", "stereo", "version", "edit", "mix", "single", "deluxe", "anniversary"].iter().any(|k| tail.contains(k)) {
                t.truncate(i);
            }
        }
    }
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let (Some(a), Some(b)) = (t.find(open), t.find(close)) {
            if b < a {
                break;
            }
            t.replace_range(a..=b, "");
        }
    }
    t.trim().to_string()
}

fn from_json(v: &Value) -> Lyrics {
    Lyrics {
        plain: v["plainLyrics"].as_str().filter(|s| !s.trim().is_empty()).map(String::from),
        synced: v["syncedLyrics"].as_str().map(parse_lrc).unwrap_or_default(),
        instrumental: v["instrumental"].as_bool().unwrap_or(false),
        source: Some("lrclib".into()),
    }
}

/// Best search hit by duration (within 5s), preferring synced lyrics.
pub fn pick_best(results: &Value, duration_s: f64) -> Option<Lyrics> {
    let mut best: Option<(f64, &Value)> = None;
    for r in results.as_array()? {
        let d = r["duration"].as_f64().unwrap_or(f64::MAX);
        let diff = (d - duration_s).abs();
        if diff > 5.0 {
            continue;
        }
        let penalty = diff + if r["syncedLyrics"].as_str().is_some_and(|s| !s.is_empty()) { 0.0 } else { 3.0 };
        if best.is_none_or(|b| penalty < b.0) {
            best = Some((penalty, r));
        }
    }
    best.map(|b| from_json(b.1))
}

/// GET with retries: LRCLIB sits behind a flaky proxy and returns sporadic 5xx that succeed on
/// the next try. `Ok(None)` = 404 (not found). `Err` only after every attempt failed.
async fn get_json(http: &reqwest::Client, url: &str, params: &[(&str, String)], attempts: u32) -> Result<Option<Value>, String> {
    let mut last = String::from("no attempt");
    for attempt in 0..attempts {
        match http.get(url).query(params).send().await {
            Ok(r) => match r.status().as_u16() {
                200 => return r.json::<Value>().await.map(Some).map_err(|e| e.to_string()),
                404 => return Ok(None),
                s @ (429 | 500..=599) => last = format!("lrclib {s}"),
                s => return Err(format!("lrclib {s}")),
            },
            Err(e) => last = e.to_string(),
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(std::time::Duration::from_millis(400 * (attempt as u64 + 1))).await;
        }
    }
    Err(last)
}

/// `Ok(None)` = LRCLIB has nothing for this track. `Err` = the service stayed unavailable.
pub async fn fetch(artist: &str, title: &str, album: Option<&str>, duration_ms: u64) -> Result<Option<Lyrics>, String> {
    let http = crate::http();
    let title = clean_title(title);
    let secs = duration_ms as f64 / 1000.0;
    let mut params = vec![("artist_name", artist.to_string()), ("track_name", title.clone()), ("duration", format!("{}", secs.round() as u64))];
    if let Some(a) = album {
        params.push(("album_name", a.to_string()));
    }
    // exact match first; if it keeps failing or is absent, try the looser search
    let exact = get_json(&http, "https://lrclib.net/api/get", &params, 5).await;
    if let Ok(Some(v)) = &exact {
        return Ok(Some(from_json(v)));
    }
    let search = get_json(&http, "https://lrclib.net/api/search", &[("track_name", title.clone()), ("artist_name", artist.to_string())], 4).await;
    match (exact, search) {
        (_, Ok(Some(v))) => Ok(pick_best(&v, secs)),
        (Ok(None), Ok(None)) => Ok(None),
        (Ok(None), Err(_)) | (Err(_), Ok(None)) => Ok(None), // one endpoint says "not found", the other is down
        (Err(e), Err(_)) => Err(format!("LRCLIB is temporarily unavailable ({e}). Try again in a moment.")),
        (Ok(Some(_)), _) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_lrc_with_multiple_stamps_and_ignores_metadata() {
        let l = parse_lrc("[ar:Slowdive]\n[00:12.50] Listen close\n[00:05.00][01:00.00] la la\n\nplain line");
        assert_eq!(l, [(5000, "la la".into()), (12500, "Listen close".into()), (60000, "la la".into())]);
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Alison - Remastered 2009"), "Alison");
        assert_eq!(clean_title("Song (feat. Someone) [Deluxe]"), "Song");
        assert_eq!(clean_title("Hello - Part 2"), "Hello - Part 2");
    }

    #[test]
    fn picks_closest_duration_preferring_synced() {
        let v = json!([
            {"duration": 300.0, "plainLyrics": "wrong length"},
            {"duration": 231.0, "plainLyrics": "plain only"},
            {"duration": 232.0, "plainLyrics": "p", "syncedLyrics": "[00:01.00] hi"}]);
        let l = pick_best(&v, 231.0).unwrap();
        assert_eq!(l.synced, [(1000, "hi".into())]);
        assert!(pick_best(&json!([{"duration": 100.0}]), 231.0).is_none());
    }
}
