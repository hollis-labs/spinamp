pub mod ai;
pub mod deezer;
pub mod itunes;
pub mod listenbrainz;
pub mod lrclib;
pub mod lyrics;
pub mod lyricsovh;
pub mod musicbrainz;
pub mod wikipedia;

/// Shared HTTP client with an identifying User-Agent (app name only).
pub fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Spinamp/0.1 (personal music player)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap()
}
