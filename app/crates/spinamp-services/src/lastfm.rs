//! Last.fm (read-only): similar artists for discovery, and your scrobble history, loved tracks
//! and top artists. Needs the user's own API key (free) and their public username; no sign-in,
//! so nothing here can write to the account.
use serde_json::Value;

const API: &str = "https://ws.audioscrobbler.com/2.0/";

#[derive(Clone)]
pub struct LastFm {
    key: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scrobble {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    /// Unix seconds.
    pub at: i64,
}

#[derive(Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total_pages: u32,
}

impl LastFm {
    pub fn new(key: &str) -> Self {
        Self { key: key.trim().into(), http: crate::http() }
    }

    async fn call(&self, method: &str, params: &[(&str, String)]) -> Result<Value, String> {
        let mut q: Vec<(&str, String)> = vec![("method", method.into()), ("api_key", self.key.clone()), ("format", "json".into())];
        q.extend(params.iter().map(|(k, v)| (*k, v.clone())));
        let r = self.http.get(API).query(&q).send().await.map_err(|e| e.to_string())?;
        let status = r.status();
        let v: Value = r.json().await.map_err(|e| format!("last.fm sent something unreadable ({status}): {e}"))?;
        check(&v)?;
        Ok(v)
    }

    /// Cheap credentials check. Returns (display name, total scrobbles).
    pub async fn user_info(&self, user: &str) -> Result<(String, u64), String> {
        let v = self.call("user.getinfo", &[("user", user.into())]).await?;
        Ok((v["user"]["name"].as_str().unwrap_or(user).into(), v["user"]["playcount"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0)))
    }

    /// Artists listeners of `artist` also play, best first, with a 0..1 match.
    pub async fn similar_artists(&self, artist: &str, limit: u32) -> Result<Vec<(String, f64)>, String> {
        let v = self.call("artist.getsimilar", &[("artist", artist.into()), ("autocorrect", "1".into()), ("limit", limit.to_string())]).await?;
        Ok(parse_similar(&v))
    }

    pub async fn recent_tracks(&self, user: &str, page: u32, from: Option<i64>) -> Result<Page<Scrobble>, String> {
        let mut p = vec![("user", user.to_string()), ("limit", "200".into()), ("page", page.to_string())];
        if let Some(f) = from {
            p.push(("from", f.to_string()));
        }
        Ok(parse_recent(&self.call("user.getrecenttracks", &p).await?))
    }

    pub async fn loved_tracks(&self, user: &str, page: u32) -> Result<Page<Scrobble>, String> {
        Ok(parse_loved(&self.call("user.getlovedtracks", &[("user", user.into()), ("limit", "200".into()), ("page", page.to_string())]).await?))
    }

    pub async fn top_artists(&self, user: &str, limit: u32) -> Result<Vec<String>, String> {
        let v = self.call("user.gettopartists", &[("user", user.into()), ("period", "overall".into()), ("limit", limit.to_string())]).await?;
        Ok(many(&v["topartists"]["artist"]).iter().filter_map(|a| a["name"].as_str().map(String::from)).collect())
    }
}

/// Last.fm reports failures as `{"error": 6, "message": "..."}`, sometimes with HTTP 200.
fn check(v: &Value) -> Result<(), String> {
    match v["error"].as_u64() {
        None => Ok(()),
        Some(10) => Err("Last.fm rejected the API key.".into()),
        Some(6) => Err(format!("Last.fm: {}", v["message"].as_str().unwrap_or("not found"))),
        Some(29) => Err("Last.fm rate limit hit; try again in a minute.".into()),
        Some(n) => Err(format!("Last.fm error {n}: {}", v["message"].as_str().unwrap_or(""))),
    }
}

/// The JSON converted from XML gives a lone item as an object and several as an array.
fn many(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![v],
        _ => vec![],
    }
}

fn num(v: &Value) -> Option<i64> {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_i64())
}

/// The artist is `{"#text": ..}` in recent tracks and `{"name": ..}` elsewhere.
fn artist_name(v: &Value) -> Option<String> {
    v["#text"].as_str().or_else(|| v["name"].as_str()).or_else(|| v.as_str()).map(String::from).filter(|s| !s.is_empty())
}

pub fn parse_similar(v: &Value) -> Vec<(String, f64)> {
    many(&v["similarartists"]["artist"])
        .iter()
        .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["match"].as_str().and_then(|s| s.parse().ok()).or_else(|| a["match"].as_f64())?)))
        .collect()
}

fn pages(v: &Value) -> u32 {
    num(&v["@attr"]["totalPages"]).unwrap_or(1).max(0) as u32
}

pub fn parse_recent(v: &Value) -> Page<Scrobble> {
    let rt = &v["recenttracks"];
    let items = many(&rt["track"])
        .into_iter()
        .filter(|t| t["@attr"]["nowplaying"].as_str() != Some("true")) // no timestamp yet
        .filter_map(|t| {
            Some(Scrobble {
                artist: artist_name(&t["artist"])?,
                title: t["name"].as_str()?.into(),
                album: t["album"]["#text"].as_str().filter(|s| !s.is_empty()).map(String::from),
                at: num(&t["date"]["uts"])?,
            })
        })
        .collect();
    Page { items, total_pages: pages(rt) }
}

pub fn parse_loved(v: &Value) -> Page<Scrobble> {
    let lt = &v["lovedtracks"];
    let items = many(&lt["track"])
        .into_iter()
        .filter_map(|t| Some(Scrobble { artist: artist_name(&t["artist"])?, title: t["name"].as_str()?.into(), album: None, at: num(&t["date"]["uts"]).unwrap_or(0) }))
        .collect();
    Page { items, total_pages: pages(lt) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_similar_artists_with_string_matches() {
        let v = json!({"similarartists":{"artist":[{"name":"Ride","match":"1"},{"name":"Lush","match":"0.83"}]}});
        assert_eq!(parse_similar(&v), vec![("Ride".to_string(), 1.0), ("Lush".to_string(), 0.83)]);
    }

    #[test]
    fn single_item_object_is_treated_as_a_list() {
        let v = json!({"similarartists":{"artist":{"name":"Ride","match":"0.5"}}});
        assert_eq!(parse_similar(&v).len(), 1);
        assert!(parse_similar(&json!({"similarartists":{}})).is_empty());
    }

    #[test]
    fn recent_tracks_skip_now_playing_and_read_timestamps() {
        let v = json!({"recenttracks":{"track":[
            {"artist":{"#text":"Slowdive","mbid":""},"name":"Alison","album":{"#text":"Souvlaki"},"@attr":{"nowplaying":"true"}},
            {"artist":{"#text":"Slowdive"},"name":"Alison","album":{"#text":""},"date":{"uts":"1700000000","#text":"x"}}
        ],"@attr":{"page":"1","totalPages":"7"}}});
        let p = parse_recent(&v);
        assert_eq!(p.total_pages, 7);
        assert_eq!(p.items, vec![Scrobble { artist: "Slowdive".into(), title: "Alison".into(), album: None, at: 1_700_000_000 }]);
    }

    #[test]
    fn loved_tracks_use_artist_name_field() {
        let v = json!({"lovedtracks":{"track":[{"name":"Alison","artist":{"name":"Slowdive"},"date":{"uts":"5"}}],"@attr":{"totalPages":"1"}}});
        let p = parse_loved(&v);
        assert_eq!((p.items[0].artist.as_str(), p.items[0].at, p.total_pages), ("Slowdive", 5, 1));
    }

    #[test]
    fn errors_are_translated() {
        assert!(check(&json!({"error":10,"message":"Invalid API key"})).unwrap_err().contains("API key"));
        assert!(check(&json!({"error":6,"message":"User not found"})).unwrap_err().contains("User not found"));
        assert!(check(&json!({"similarartists":{}})).is_ok());
    }
}
