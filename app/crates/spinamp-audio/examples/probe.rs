//! Calibration probe: `cargo run --release --example probe -- <isrc>...`
//! Downloads each ISRC's Deezer preview, analyses it, prints the numbers next to Deezer's own
//! BPM/gain so we can sanity-check against known tracks.
use std::time::Duration;

fn main() {
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).user_agent("Spinamp/0.1").build().unwrap();
    println!("{:<34} {:>6} {:>7} {:>5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}", "track", "bpm", "dzBPM", "beat", "loud", "peak", "dyn", "spike", "bright", "flux", "energy");
    for isrc in std::env::args().skip(1) {
        let meta: serde_json::Value = match http.get(format!("https://api.deezer.com/track/isrc:{isrc}")).send().and_then(|r| r.json::<serde_json::Value>()) {
            Ok(v) if v.get("error").is_none() => v,
            _ => { println!("{isrc}: not on deezer"); continue; }
        };
        let title = format!("{} - {}", meta["artist"]["name"].as_str().unwrap_or("?"), meta["title"].as_str().unwrap_or("?"));
        let Some(url) = meta["preview"].as_str().filter(|u| !u.is_empty()) else { println!("{title}: no preview"); continue };
        let resp = http.get(url).send().unwrap();
        let status = resp.status();
        let bytes = resp.bytes().unwrap().to_vec();
        if std::env::var("PROBE_DEBUG").is_ok() { println!("  [{status}] {} bytes, head={:02x?}", bytes.len(), &bytes[..bytes.len().min(12)]); }
        let (pcm, sr) = match spinamp_audio::decode(bytes) { Ok(x) => x, Err(e) => { println!("{title}: decode {e}"); continue } };
        match spinamp_audio::analyze(&pcm, sr) {
            Some(an) => { if let (Ok(dir), Some(v)) = (std::env::var("PROBE_DUMP"), an.viz.as_ref()) { std::fs::write(format!("{dir}/{isrc}.json"), serde_json::to_string(v).unwrap()).unwrap(); } let a = an.features; println!("{:<34} {:>6} {:>7} {:>5.2} {:>6.1} {:>6.1} {:>6.1} {:>6.1} {:>6.0} {:>6.1} {:>6.2}",
                title.chars().take(34).collect::<String>(),
                a.tempo_bpm.map_or("-".into(), |t| format!("{t:.1}")), meta["bpm"].as_f64().filter(|b| *b > 0.0).map_or("-".into(), |b| format!("{b:.0}")),
                a.beat_strength, a.loudness_db, a.peak_db, a.dynamic_range_db, a.spike_db, a.brightness_hz, a.flux, a.energy); println!("      key={:?} ({:.2}) valence={:.2} moods={:?} dance={:.2}", a.key, a.key_confidence, a.valence, a.moods, a.danceability); println!("      meter={:?} candidates={:?} pattern={}", a.beats_per_bar, a.tempo_candidates.iter().map(|c| (c.0.round(), (c.1 * 100.0).round() / 100.0)).collect::<Vec<_>>(), an.viz.is_some()); }
            None => println!("{title}: too quiet/short"),
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}
