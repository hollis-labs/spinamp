//! Playback by remote-controlling the macOS Music app with AppleScript (the Apple Music
//! counterpart of Spotify Connect). Only tracks that are in the user's library can be started:
//! the Music app has no scripting entry point for arbitrary catalog songs.
use spinamp_core::Track;

#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub playing: bool,
    pub position_ms: u64,
    pub title: String,
    pub artist: String,
    pub duration_ms: u64,
    pub volume: Option<u32>,
    pub shuffle: bool,
}

const PLAY: &str = r#"on run argv
  set t to item 1 of argv
  set a to item 2 of argv
  set d to (item 3 of argv) as integer
  tell application "Music"
    set hits to (search library playlist 1 for t only songs)
    repeat with h in hits
      set secs to duration of h
      if (name of h) is t and (artist of h) contains a and secs > (d - 3) and secs < (d + 3) then
        play h
        return "ok"
      end if
    end repeat
  end tell
  return "notfound"
end run"#;

const STATE: &str = r#"if application "Music" is not running then return "off"
tell application "Music"
  set s to player state as text
  if s is "stopped" then return "stopped"
  set t to current track
  return s & "|||" & ((player position * 1000) as integer) & "|||" & (name of t) & "|||" & (artist of t) & "|||" & ((duration of t * 1000) as integer) & "|||" & (sound volume) & "|||" & (shuffle enabled)
end tell"#;

#[cfg(target_os = "macos")]
async fn osa(script: &str, args: &[String]) -> Result<String, String> {
    let out = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("could not run osascript: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("-1743") || err.to_lowercase().contains("not authorized") {
            Err("Spinamp is not allowed to control Music. Allow it in System Settings > Privacy & Security > Automation.".into())
        } else {
            Err(format!("Music: {err}"))
        }
    }
}

#[cfg(not(target_os = "macos"))]
async fn osa(_: &str, _: &[String]) -> Result<String, String> {
    Err("Apple Music playback control is only available on macOS for now.".into())
}

pub async fn play(track: &Track) -> Result<(), String> {
    let first_artist = track.artist.split(", ").next().unwrap_or(&track.artist).to_string();
    let args = [track.title.clone(), first_artist, (track.duration_ms / 1000).to_string()];
    match osa(PLAY, &args).await?.as_str() {
        "ok" => Ok(()),
        _ => Err(format!("\"{}\" is not in your Apple Music library, so the Music app can't play it. Add it to your library first.", track.title)),
    }
}

pub async fn pause() -> Result<(), String> {
    osa("tell application \"Music\" to pause", &[]).await.map(|_| ())
}

pub async fn resume() -> Result<(), String> {
    osa("tell application \"Music\" to play", &[]).await.map(|_| ())
}

pub async fn seek(position_ms: u64) -> Result<(), String> {
    osa("on run argv\ntell application \"Music\" to set player position to ((item 1 of argv) as integer) / 1000\nend run", &[position_ms.to_string()])
        .await
        .map(|_| ())
}

pub async fn snapshot() -> Result<Option<Snapshot>, String> {
    parse(&osa(STATE, &[]).await?)
}

fn parse(s: &str) -> Result<Option<Snapshot>, String> {
    if s == "off" || s == "stopped" || s.is_empty() {
        return Ok(None);
    }
    let p: Vec<&str> = s.split("|||").collect();
    if p.len() != 7 {
        return Err(format!("unexpected Music reply: {s}"));
    }
    Ok(Some(Snapshot {
        playing: p[0] == "playing",
        position_ms: p[1].trim().parse().unwrap_or(0),
        title: p[2].into(),
        artist: p[3].into(),
        duration_ms: p[4].trim().parse().unwrap_or(0),
        volume: p[5].trim().parse().ok(),
        shuffle: p[6].trim() == "true",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_reply() {
        let s = parse("playing|||12345|||Alison|||Slowdive|||231000|||55|||true").unwrap().unwrap();
        assert_eq!((s.playing, s.position_ms, s.title.as_str(), s.duration_ms, s.volume, s.shuffle), (true, 12345, "Alison", 231000, Some(55), true));
    }

    #[test]
    fn idle_states_are_none() {
        assert_eq!(parse("off").unwrap(), None);
        assert_eq!(parse("stopped").unwrap(), None);
        assert!(parse("garbage").is_err());
    }
}
