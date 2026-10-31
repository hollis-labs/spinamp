//! Live check of a real MusicKit key against Apple's catalog (no user token needed).
//! `APPLE_P8=/path/AuthKey_X.p8 APPLE_KEY_ID=X APPLE_TEAM_ID=T cargo test --test live -- --ignored --nocapture`
use spinamp_applemusic::map::{Page, to_track};

#[tokio::test]
#[ignore]
async fn developer_token_is_accepted_by_apple() {
    let (p8, kid, team) = (std::env::var("APPLE_P8").unwrap(), std::env::var("APPLE_KEY_ID").unwrap(), std::env::var("APPLE_TEAM_ID").unwrap());
    let pem = std::fs::read_to_string(p8).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let jwt = spinamp_applemusic::token::developer_token(&pem, &kid, &team, now).unwrap();
    let r = reqwest::Client::new()
        .get("https://api.music.apple.com/v1/catalog/us/search?term=slowdive+alison&types=songs&limit=3")
        .bearer_auth(jwt)
        .send()
        .await
        .unwrap();
    let status = r.status();
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(status.is_success(), "{status}: {v}");
    let page: Page = serde_json::from_value(v["results"]["songs"].clone()).unwrap();
    let t = page.data.into_iter().filter_map(to_track).collect::<Vec<_>>();
    for x in &t {
        println!("{} | {} | {} | isrc={:?} tags={:?}", x.id.0, x.title, x.artist, x.meta.isrc, x.tags);
    }
    assert!(!t.is_empty());
}
