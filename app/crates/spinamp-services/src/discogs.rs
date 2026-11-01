//! Discogs (read-only): who made a record. Artist profile, aliases and members, and a release's
//! label, catalogue number, country, formats, genres/styles and per-track credits.
//! Needs the user's own personal access token (Discogs > Settings > Developers); search is
//! not available anonymously.
use serde::{Deserialize, Serialize};
use serde_json::Value;

const API: &str = "https://api.discogs.com";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArtistInfo {
    pub id: u64,
    pub name: String,
    pub realname: Option<String>,
    pub profile: String,
    pub aliases: Vec<String>,
    /// (name, still active)
    pub members: Vec<(String, bool)>,
    pub urls: Vec<String>,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub position: String,
    pub title: String,
    pub duration: String,
    /// (name, role) e.g. ("Rachel Goswell", "Vocals")
    pub credits: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub id: u64,
    pub title: String,
    pub year: Option<i32>,
    pub country: Option<String>,
    /// (label, catalogue number)
    pub labels: Vec<(String, String)>,
    pub formats: Vec<String>,
    pub genres: Vec<String>,
    pub styles: Vec<String>,
    pub credits: Vec<(String, String)>,
    pub tracklist: Vec<TrackInfo>,
    pub notes: String,
    pub url: String,
}

pub struct Discogs {
    token: String,
    http: reqwest::Client,
}

impl Discogs {
    pub fn new(token: &str) -> Self {
        Self { token: token.trim().into(), http: crate::http() }
    }

    async fn get(&self, path: &str, q: &[(&str, String)]) -> Result<Value, String> {
        let r = self
            .http
            .get(format!("{API}{path}"))
            .header("Authorization", format!("Discogs token={}", self.token))
            .query(q)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        match r.status().as_u16() {
            200..=299 => r.json().await.map_err(|e| e.to_string()),
            401 => Err("Discogs rejected the token.".into()),
            404 => Ok(Value::Null),
            429 => Err("Discogs rate limit hit (60 per minute); try again shortly.".into()),
            s => Err(format!("discogs {s}")),
        }
    }

    /// Username the token belongs to.
    pub async fn identity(&self) -> Result<String, String> {
        let v = self.get("/oauth/identity", &[]).await?;
        v["username"].as_str().map(String::from).ok_or_else(|| "Discogs did not say who this token is for.".into())
    }

    pub async fn artist(&self, name: &str) -> Result<Option<ArtistInfo>, String> {
        let s = self.get("/database/search", &[("q", name.into()), ("type", "artist".into()), ("per_page", "5".into())]).await?;
        let Some(id) = pick_artist(&s, name) else { return Ok(None) };
        let v = self.get(&format!("/artists/{id}"), &[]).await?;
        Ok(parse_artist(&v))
    }

    /// The best-matching release of `album` by `artist` (the master's main release when there is one).
    pub async fn release(&self, artist: &str, album: &str) -> Result<Option<ReleaseInfo>, String> {
        let s = self
            .get("/database/search", &[("type", "release".into()), ("artist", artist.into()), ("release_title", album.into()), ("per_page", "15".into())])
            .await?;
        let Some((release_id, master_id)) = pick_release(&s, album) else { return Ok(None) };
        let id = match master_id {
            Some(m) => {
                let mv = self.get(&format!("/masters/{m}"), &[]).await?;
                mv["main_release"].as_u64().unwrap_or(release_id)
            }
            None => release_id,
        };
        Ok(parse_release(&self.get(&format!("/releases/{id}"), &[]).await?))
    }
}

/// Discogs disambiguates same-named artists as "Name (2)".
fn base_name(s: &str) -> String {
    let t = s.trim();
    match t.rfind(" (") {
        Some(i) if t.ends_with(')') && t[i + 2..t.len() - 1].chars().all(|c| c.is_ascii_digit()) => t[..i].to_string(),
        _ => t.to_string(),
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

pub fn pick_artist(search: &Value, name: &str) -> Option<u64> {
    let results = search["results"].as_array()?;
    let want = norm(name);
    results
        .iter()
        .filter(|r| r["type"].as_str() == Some("artist"))
        .find(|r| r["title"].as_str().is_some_and(|t| norm(&base_name(t)) == want))
        .or_else(|| results.iter().find(|r| r["type"].as_str() == Some("artist")))
        .and_then(|r| r["id"].as_u64())
}

/// (release id, master id). Prefers results whose title contains the album and that belong to a
/// master; otherwise the earliest-year match.
pub fn pick_release(search: &Value, album: &str) -> Option<(u64, Option<u64>)> {
    let want = norm(album);
    let mut hits: Vec<&Value> = search["results"]
        .as_array()?
        .iter()
        .filter(|r| r["type"].as_str().map_or(true, |t| t == "release" || t == "master"))
        .filter(|r| r["title"].as_str().is_some_and(|t| norm(t.split_once(" - ").map_or(t, |x| x.1)).contains(&want) || want.contains(&norm(t.split_once(" - ").map_or(t, |x| x.1)))))
        .collect();
    hits.sort_by_key(|r| r["year"].as_str().and_then(|y| y.parse::<i32>().ok()).unwrap_or(9999));
    let h = hits.iter().find(|r| r["master_id"].as_u64().unwrap_or(0) > 0).or(hits.first())?;
    let master = h["master_id"].as_u64().filter(|m| *m > 0);
    Some((h["id"].as_u64()?, master))
}

pub fn parse_artist(v: &Value) -> Option<ArtistInfo> {
    Some(ArtistInfo {
        id: v["id"].as_u64()?,
        name: base_name(v["name"].as_str()?),
        realname: v["realname"].as_str().filter(|s| !s.is_empty()).map(String::from),
        profile: clean_markup(v["profile"].as_str().unwrap_or("")),
        aliases: v["aliases"].as_array().map(|a| a.iter().filter_map(|x| x["name"].as_str().map(base_name)).collect()).unwrap_or_default(),
        members: v["members"].as_array().map(|a| a.iter().filter_map(|x| Some((base_name(x["name"].as_str()?), x["active"].as_bool().unwrap_or(true)))).collect()).unwrap_or_default(),
        urls: v["urls"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        url: v["uri"].as_str().unwrap_or("").into(),
    })
}

fn credits(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| Some((base_name(x["name"].as_str()?), x["role"].as_str().unwrap_or("").trim().to_string()))).collect())
        .unwrap_or_default()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn parse_release(v: &Value) -> Option<ReleaseInfo> {
    Some(ReleaseInfo {
        id: v["id"].as_u64()?,
        title: v["title"].as_str()?.into(),
        year: v["year"].as_i64().filter(|y| *y > 0).map(|y| y as i32),
        country: v["country"].as_str().filter(|s| !s.is_empty()).map(String::from),
        labels: v["labels"].as_array().map(|a| a.iter().filter_map(|l| Some((l["name"].as_str()?.into(), l["catno"].as_str().unwrap_or("").into()))).collect()).unwrap_or_default(),
        formats: v["formats"]
            .as_array()
            .map(|a| a.iter().filter_map(|f| {
                let mut parts = vec![f["name"].as_str()?.to_string()];
                parts.extend(strings(&f["descriptions"]));
                Some(parts.join(", "))
            }).collect())
            .unwrap_or_default(),
        genres: strings(&v["genres"]),
        styles: strings(&v["styles"]),
        credits: credits(&v["extraartists"]),
        tracklist: v["tracklist"]
            .as_array()
            .map(|a| a.iter().filter(|t| t["type_"].as_str().map_or(true, |k| k == "track")).filter_map(|t| Some(TrackInfo {
                position: t["position"].as_str().unwrap_or("").into(),
                title: t["title"].as_str()?.into(),
                duration: t["duration"].as_str().unwrap_or("").into(),
                credits: credits(&t["extraartists"]),
            })).collect())
            .unwrap_or_default(),
        notes: clean_markup(v["notes"].as_str().unwrap_or("")),
        url: v["uri"].as_str().unwrap_or("").into(),
    })
}

/// The tracklist entry for `title` (exact after normalising, else containing).
pub fn find_track<'a>(r: &'a ReleaseInfo, title: &str) -> Option<&'a TrackInfo> {
    let strip = |s: &str| {
        let mut out = String::new();
        let mut depth = 0u32;
        for c in s.chars() {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        norm(&out)
    };
    let want = strip(title);
    if want.is_empty() {
        return None;
    }
    r.tracklist.iter().find(|t| strip(&t.title) == want).or_else(|| r.tracklist.iter().find(|t| strip(&t.title).contains(&want)))
}

/// Discogs text uses BBCode-ish tags: `[a=Name]`, `[l=Label]`, `[url=..]text[/url]`, `[b]..[/b]`,
/// and bare id references like `[a123456]`. Keep the words, drop the markup.
pub fn clean_markup(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find(']') else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        let tag = &after[..j];
        if let Some((k, v)) = tag.split_once('=') {
            // [a=Name] [l=Label] [m=Master] [r=Release] keep the name; [url=..] keeps the text between tags
            if matches!(k, "a" | "l" | "m" | "r" | "g" | "t") && !v.chars().all(|c| c.is_ascii_digit()) {
                out.push_str(v);
            }
        } else if tag.starts_with(|c: char| c.is_ascii_alphabetic()) && tag[1..].chars().all(|c| c.is_ascii_digit()) && tag.len() > 1 {
            // [a123456] id reference: no readable name here
        } else if !matches!(tag.trim_start_matches('/'), "b" | "i" | "u" | "url" | "quote" | "img") {
            out.push('[');
            out.push_str(tag);
            out.push(']');
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ").replace(" \n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markup_keeps_words_and_drops_tags() {
        assert_eq!(clean_markup("Formed by [a=Neil Halstead] and [a=Rachel Goswell]. See [url=http://x.com]site[/url]. [b]Bold[/b] [a123]."), "Formed by Neil Halstead and Rachel Goswell. See site. Bold .");
        assert_eq!(clean_markup("on [l=Creation Records] [m=1234]"), "on Creation Records");
        assert_eq!(clean_markup("unclosed [oops"), "unclosed [oops");
    }

    #[test]
    fn base_name_drops_numeric_disambiguation_only() {
        assert_eq!(base_name("Slowdive (2)"), "Slowdive");
        assert_eq!(base_name("Portishead"), "Portishead");
        assert_eq!(base_name("Everything (But The Girl)"), "Everything (But The Girl)");
    }

    #[test]
    fn picks_exact_artist_over_first_hit() {
        let s = json!({"results":[{"id":1,"type":"artist","title":"Slowdive Tribute"},{"id":2,"type":"artist","title":"Slowdive (2)"}]});
        assert_eq!(pick_artist(&s, "Slowdive"), Some(2));
        let none = json!({"results":[]});
        assert_eq!(pick_artist(&none, "x"), None);
    }

    #[test]
    fn picks_earliest_master_backed_release_for_the_album() {
        let s = json!({"results":[
            {"id":10,"type":"release","title":"Slowdive - Souvlaki","year":"2005","master_id":0},
            {"id":11,"type":"release","title":"Slowdive - Souvlaki","year":"1993","master_id":77},
            {"id":12,"type":"release","title":"Slowdive - Just For A Day","year":"1991","master_id":5}]});
        assert_eq!(pick_release(&s, "Souvlaki"), Some((11, Some(77))));
        assert_eq!(pick_release(&s, "Nonexistent"), None);
    }

    #[test]
    fn parses_release_with_track_credits() {
        let v = json!({"id":9,"title":"Souvlaki","year":1993,"country":"UK","genres":["Electronic","Rock"],"styles":["Shoegaze"],
            "labels":[{"name":"Creation Records","catno":"CRE LP 139"}],
            "formats":[{"name":"Vinyl","descriptions":["LP","Album"]}],
            "extraartists":[{"name":"Ride (3)","role":"Mixed By"}],
            "tracklist":[{"position":"A1","title":"Alison","duration":"3:51","type_":"track","extraartists":[{"name":"Neil Halstead","role":"Vocals"}]},
                         {"position":"","title":"Side","type_":"heading"}],
            "notes":"Recorded at [l=Chipping Norton Studios].","uri":"https://www.discogs.com/release/9"});
        let r = parse_release(&v).unwrap();
        assert_eq!((r.year, r.country.as_deref(), r.labels[0].1.as_str(), r.formats[0].as_str()), (Some(1993), Some("UK"), "CRE LP 139", "Vinyl, LP, Album"));
        assert_eq!((r.credits[0].0.as_str(), r.tracklist.len(), r.notes.as_str()), ("Ride", 1, "Recorded at Chipping Norton Studios."));
        assert_eq!(find_track(&r, "Alison (Remastered)").unwrap().credits[0], ("Neil Halstead".to_string(), "Vocals".to_string()));
        assert!(find_track(&r, "Missing").is_none());
    }

    #[test]
    fn parses_artist_profile() {
        let v = json!({"id":3,"name":"Slowdive (2)","realname":"","profile":"[a=Neil Halstead] fronted it.","aliases":[{"name":"Mojave 3"}],
            "members":[{"name":"Rachel Goswell","active":true},{"name":"Ex Member (4)","active":false}],"urls":["http://slowdive.com"],"uri":"https://www.discogs.com/artist/3"});
        let a = parse_artist(&v).unwrap();
        assert_eq!((a.name.as_str(), a.realname, a.profile.as_str()), ("Slowdive", None, "Neil Halstead fronted it."));
        assert_eq!(a.members, vec![("Rachel Goswell".to_string(), true), ("Ex Member".to_string(), false)]);
    }
}
