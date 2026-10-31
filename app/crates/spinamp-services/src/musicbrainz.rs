//! MusicBrainz artist lookup: one request per artist returns genre-ish tags with vote counts.
//! Public API, no key. Etiquette: <= 1 request/second and an identifying User-Agent
//! (we send the app name only, never personal contact details unless the user sets one).

use serde_json::Value;
use spinamp_core::ArtistInfo;

/// Links, members and place for an artist page (from the MusicBrainz relationships).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArtistDetails {
    /// (kind, url): "official homepage", "bandcamp", "wikipedia", "social network", ...
    pub links: Vec<(String, String)>,
    pub members: Vec<String>,
    pub area: Option<String>,
    pub disambiguation: Option<String>,
}

pub fn parse_details(v: &Value) -> ArtistDetails {
    let mut d = ArtistDetails { area: v["area"]["name"].as_str().map(String::from), disambiguation: v["disambiguation"].as_str().filter(|s| !s.is_empty()).map(String::from), ..Default::default() };
    for r in v["relations"].as_array().into_iter().flatten() {
        match r["type"].as_str().unwrap_or("") {
            "member of band" if r["direction"].as_str() == Some("backward") => {
                if let Some(n) = r["artist"]["name"].as_str() {
                    if r["ended"] != true && !d.members.iter().any(|m| m == n) {
                        d.members.push(n.into());
                    }
                }
            }
            kind => {
                if let Some(u) = r["url"]["resource"].as_str() {
                    let k = match kind {
                        "official homepage" | "bandcamp" | "wikipedia" | "social network" | "youtube" | "streaming" | "free streaming" | "discogs" | "allmusic" | "wikidata" | "bandsintown" | "songkick" => kind,
                        _ => continue,
                    };
                    d.links.push((k.to_string(), u.to_string()));
                }
            }
        }
    }
    d
}

pub struct MusicBrainz {
    http: reqwest::Client,
}

impl MusicBrainz {
    /// `contact` is optional text appended to the User-Agent (e.g. a project URL).
    pub fn new(contact: Option<&str>) -> Self {
        let ua = match contact {
            Some(c) if !c.is_empty() => format!("Spinamp/0.1 ({c})"),
            _ => "Spinamp/0.1 (personal music player)".into(),
        };
        Self { http: reqwest::Client::builder().user_agent(ua).build().unwrap() }
    }

    /// Relationships for an artist page. One request.
    pub async fn artist_details(&self, mbid: &str) -> Result<ArtistDetails, String> {
        let r = self
            .http
            .get(format!("https://musicbrainz.org/ws/2/artist/{mbid}"))
            .query(&[("inc", "url-rels+artist-rels"), ("fmt", "json")])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("musicbrainz {}", r.status()));
        }
        Ok(parse_details(&r.json::<Value>().await.map_err(|e| e.to_string())?))
    }

    /// `Ok(None)` = searched fine, no confident match. `Err` = transient (rate limit, network).
    pub async fn lookup_artist(&self, name: &str) -> Result<Option<ArtistInfo>, String> {
        let q = format!("artist:\"{}\"", name.replace('"', ""));
        let r = self
            .http
            .get("https://musicbrainz.org/ws/2/artist")
            .query(&[("query", q.as_str()), ("fmt", "json"), ("limit", "5")])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("musicbrainz {}", r.status()));
        }
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        Ok(parse_artist_search(&v, name))
    }
}

/// Pick the confident exact-name match from a search response.
pub fn parse_artist_search(v: &Value, wanted: &str) -> Option<ArtistInfo> {
    let want = wanted.to_lowercase();
    let a = v["artists"].as_array()?.iter().find(|a| {
        a["score"].as_u64().unwrap_or(0) >= 90
            && (a["name"].as_str().map(|n| n.to_lowercase()) == Some(want.clone())
                || a["sort-name"].as_str().map(|n| n.to_lowercase()) == Some(want.clone()))
    })?;
    let mut tags: Vec<(String, u32)> = a["tags"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(|t| Some((t["name"].as_str()?.to_lowercase(), t["count"].as_u64()? as u32)))
                .filter(|(_, c)| *c >= 1)
                .collect()
        })
        .unwrap_or_default();
    tags.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    tags.truncate(10);
    Some(ArtistInfo {
        name: a["name"].as_str()?.into(),
        mbid: a["id"].as_str().map(String::from),
        country: a["country"].as_str().map(String::from),
        kind: a["type"].as_str().map(String::from),
        begin_year: a["life-span"]["begin"].as_str().and_then(|b| b.get(..4)).and_then(|y| y.parse().ok()),
        tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_exact_confident_match_and_sorts_tags() {
        let v = json!({"artists":[
            {"id":"x","name":"Slowdive Tribute","score":95,"tags":[{"name":"cover","count":3}]},
            {"id":"a16","name":"Slowdive","sort-name":"Slowdive","score":100,"country":"GB","type":"Group",
             "life-span":{"begin":"1989-10"},
             "tags":[{"count":1,"name":"rock"},{"count":26,"name":"Shoegaze"},{"count":0,"name":"pop"},{"count":5,"name":"post-rock"}]}]});
        let a = parse_artist_search(&v, "slowdive").unwrap();
        assert_eq!(a.mbid.as_deref(), Some("a16"));
        assert_eq!(a.tags, [("shoegaze".to_string(), 26), ("post-rock".to_string(), 5), ("rock".to_string(), 1)]);
        assert_eq!((a.country.as_deref(), a.begin_year), (Some("GB"), Some(1989)));
    }

    #[test]
    fn parses_links_and_current_members() {
        let v = json!({"area":{"name":"United Kingdom"},"disambiguation":"","relations":[
            {"type":"official homepage","url":{"resource":"https://slowdive.com"}},
            {"type":"wikipedia","url":{"resource":"https://en.wikipedia.org/wiki/Slowdive"}},
            {"type":"member of band","direction":"backward","ended":false,"artist":{"name":"Rachel Goswell"}},
            {"type":"member of band","direction":"backward","ended":true,"artist":{"name":"Former"}},
            {"type":"lyrics","url":{"resource":"https://x"}}]});
        let d = parse_details(&v);
        assert_eq!(d.links.len(), 2);
        assert_eq!(d.members, ["Rachel Goswell"]);
        assert_eq!(d.area.as_deref(), Some("United Kingdom"));
    }

    #[test]
    fn rejects_low_score_or_name_mismatch() {
        let v = json!({"artists":[{"id":"x","name":"Other","score":100},{"id":"y","name":"Slowdive","score":60}]});
        assert!(parse_artist_search(&v, "Slowdive").is_none());
        assert!(parse_artist_search(&json!({}), "x").is_none());
    }
}
