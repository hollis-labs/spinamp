//! Lyrics lookup across providers: LRCLIB first (it has time-synced lyrics), lyrics.ovh as backup.

use crate::{lrclib, lyricsovh};
use lrclib::Lyrics;

/// Combine the two providers' results. `Ok(None)` only when we are sure nobody has it;
/// `Err` when a provider that might have it was unreachable (so the caller must not cache a miss).
pub fn combine(primary: Result<Option<Lyrics>, String>, backup: Result<Option<Lyrics>, String>) -> Result<Option<Lyrics>, String> {
    match (primary, backup) {
        (Ok(Some(l)), _) => Ok(Some(l)),
        (_, Ok(Some(l))) => Ok(Some(l)),
        (Ok(None), Ok(None)) => Ok(None),
        (Ok(None), Err(e)) => Err(format!("Not on LRCLIB, and the backup (lyrics.ovh) is unreachable ({e}). Try again in a moment.")),
        (Err(e), Ok(None)) => Err(format!("LRCLIB is unreachable ({e}) and lyrics.ovh does not have it. Try again in a moment.")),
        (Err(a), Err(b)) => Err(format!("Both lyrics services are unreachable (LRCLIB: {a}; lyrics.ovh: {b}). Try again in a moment.")),
    }
}

pub async fn find(artist: &str, title: &str, album: Option<&str>, duration_ms: u64) -> Result<Option<Lyrics>, String> {
    let primary = lrclib::fetch(artist, title, album, duration_ms).await;
    if let Ok(Some(l)) = &primary {
        if l.instrumental || !l.synced.is_empty() || l.plain.is_some() {
            return Ok(primary.unwrap());
        }
    }
    // LRCLIB down, missing, or returned an empty entry: ask the backup (with a cleaned title)
    let backup = lyricsovh::fetch(artist, &lrclib::clean_title(title)).await;
    combine(primary, backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(src: &str) -> Lyrics {
        Lyrics { plain: Some("x".into()), source: Some(src.into()), ..Default::default() }
    }

    /// Live network check: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_lookup_finds_lyrics() {
        let l = find("Slowdive", "Alison - Remastered", Some("Souvlaki"), 231_000).await.unwrap().expect("some lyrics");
        println!("source={:?} synced={} plain={}", l.source, l.synced.len(), l.plain.as_deref().unwrap_or("").len());
        let o = crate::lyricsovh::fetch("Slowdive", "Alison").await.unwrap().expect("ovh lyrics");
        assert!(o.plain.unwrap().len() > 100);
    }

    #[test]
    fn prefers_primary_then_backup() {
        assert_eq!(combine(Ok(Some(l("a"))), Ok(Some(l("b")))).unwrap().unwrap().source.as_deref(), Some("a"));
        assert_eq!(combine(Err("503".into()), Ok(Some(l("b")))).unwrap().unwrap().source.as_deref(), Some("b"));
        assert_eq!(combine(Ok(None), Ok(Some(l("b")))).unwrap().unwrap().source.as_deref(), Some("b"));
    }

    #[test]
    fn miss_is_only_definitive_when_both_answered() {
        assert_eq!(combine(Ok(None), Ok(None)), Ok(None));
        assert!(combine(Ok(None), Err("down".into())).is_err());
        assert!(combine(Err("down".into()), Ok(None)).is_err());
        assert!(combine(Err("a".into()), Err("b".into())).unwrap_err().contains("Both"));
    }
}
