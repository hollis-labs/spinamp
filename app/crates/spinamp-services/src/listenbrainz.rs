//! ListenBrainz Labs "similar artists": public, no key. Input is a MusicBrainz artist id
//! (which tag enrichment already collects); output is artists listeners tend to play alongside.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Similar {
    pub mbid: String,
    pub name: String,
    pub score: f64,
}

pub async fn similar_artists(mbid: &str) -> Result<Vec<Similar>, String> {
    let r = reqwest::Client::builder()
        .user_agent("Spinamp/0.1 (personal music player)")
        .build()
        .map_err(|e| e.to_string())?
        .get("https://labs.api.listenbrainz.org/similar-artists/json")
        .query(&[
            ("artist_mbids", mbid),
            ("algorithm", "session_based_days_9000_session_300_contribution_5_threshold_15_limit_50_skip_30"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("listenbrainz {}", r.status()));
    }
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    Ok(parse_similar(&v))
}

pub fn parse_similar(v: &Value) -> Vec<Similar> {
    let mut out: Vec<Similar> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Similar { mbid: x["artist_mbid"].as_str()?.into(), name: x["name"].as_str()?.into(), score: x["score"].as_f64()? })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_and_sorts_by_score() {
        let v = json!([{"artist_mbid":"a","name":"A","score":10},{"artist_mbid":"b","name":"B","score":99},{"name":"bad"}]);
        let s = parse_similar(&v);
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["B", "A"]);
        assert!(parse_similar(&json!({"error":"x"})).is_empty());
    }
}
