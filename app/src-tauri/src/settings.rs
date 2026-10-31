//! User preferences. Plain prefs live in the sqlite `kv` table; secrets (API keys) live in the
//! OS keychain and are never sent back to the UI.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub spotify_client_id: String,
    pub callback_port: u16,
    pub ai_enabled: bool,
    pub ai_model: String,
    /// Share of picks that are discovery in Shuffle+Discovery (0..1).
    pub discovery_ratio: f64,
    /// Higher = discovery stays closer to your taste.
    pub discovery_sharpness: f64,
    /// Fraction of a track that must play to count as a listen.
    pub listen_threshold: f64,
    pub quiet_enabled: bool,
    pub quiet_start: u32,
    pub quiet_end: u32,
    /// Optional text added to the MusicBrainz User-Agent (e.g. a project URL). Empty = none.
    pub musicbrainz_contact: String,
    /// Analyse audio automatically at startup (downloads ~0.5 MB of preview per new track).
    pub analyze_auto: bool,
    /// Tracks per batch (0 = everything at once).
    pub analyze_batch: u32,
    /// Minutes to wait between batches when analysing automatically.
    pub analyze_gap_minutes: u32,
    /// Shift the visualiser against playback, in ms (positive = later), to match your audio latency.
    pub viz_offset_ms: i32,
    /// Capture Spotify's audio (macOS Screen Recording permission) to drive the visualiser live
    /// and record a fingerprint of each track for later listens.
    pub capture_enabled: bool,
    /// The first-run question about capture has been answered (either way).
    pub capture_prompted: bool,
    /// Opt-in: contribute recorded song fingerprints (anonymous, keyed by ISRC) to the shared
    /// spinamp.fm database once it exists, and use others' fingerprints. Nothing is sent yet; this
    /// only records your choice.
    pub share_fingerprints: bool,
    /// LED-style horizontal segmentation on the visualiser bars.
    pub viz_led: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            spotify_client_id: String::new(),
            callback_port: 43821,
            ai_enabled: true,
            ai_model: spinamp_services::ai::DEFAULT_MODEL.into(),
            discovery_ratio: 0.25,
            discovery_sharpness: 2.0,
            listen_threshold: 0.8,
            quiet_enabled: true,
            quiet_start: 23,
            quiet_end: 7,
            musicbrainz_contact: String::new(),
            analyze_auto: false,
            analyze_batch: 150,
            analyze_gap_minutes: 15,
            viz_offset_ms: 0,
            capture_enabled: false,
            capture_prompted: false,
            share_fingerprints: false,
            viz_led: true,
        }
    }
}

impl Settings {
    pub fn sanitized(mut self) -> Self {
        self.discovery_ratio = self.discovery_ratio.clamp(0.0, 1.0);
        self.discovery_sharpness = self.discovery_sharpness.clamp(0.5, 6.0);
        self.listen_threshold = self.listen_threshold.clamp(0.1, 1.0);
        self.quiet_start %= 24;
        self.quiet_end %= 24;
        self.callback_port = self.callback_port.max(1024);
        self.analyze_batch = self.analyze_batch.min(5000);
        self.analyze_gap_minutes = self.analyze_gap_minutes.clamp(1, 24 * 60);
        self.viz_offset_ms = self.viz_offset_ms.clamp(-1000, 1000);
        self.spotify_client_id = self.spotify_client_id.trim().into();
        self
    }

    pub fn client_id(&self) -> String {
        if self.spotify_client_id.is_empty() {
            std::env::var("SPINAMP_SPOTIFY_CLIENT_ID").unwrap_or_default()
        } else {
            self.spotify_client_id.clone()
        }
    }

    pub fn contact(&self) -> Option<String> {
        Some(self.musicbrainz_contact.clone())
            .filter(|c| !c.is_empty())
            .or_else(|| std::env::var("SPINAMP_CONTACT").ok().filter(|c| !c.is_empty()))
    }
}

const SERVICE: &str = "spinamp";

/// Where raw keychain items live. A trait so the vault logic is testable without a keychain.
pub trait Backend {
    fn get(&self, account: &str) -> Option<String>;
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

pub struct KeyringBackend;

impl Backend for KeyringBackend {
    fn get(&self, account: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, account).ok()?.get_password().ok().filter(|s| !s.is_empty())
    }
    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?.set_password(value).map_err(|e| e.to_string())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

const VAULT_ACCOUNT: &str = "vault";
/// Separate keychain items from before the vault existed; moved in on first load.
const LEGACY_ACCOUNTS: &[&str] = &["ai_key", "cred:spotify"];

/// All secrets in ONE keychain item (a JSON map), read once and cached for the life of the
/// process. macOS asks permission per item, so one item means at most one prompt per launch
/// instead of one for every secret on every read.
pub struct Vault<B: Backend> {
    backend: B,
    cache: std::sync::Mutex<Option<std::collections::BTreeMap<String, String>>>,
}

impl<B: Backend> Vault<B> {
    pub fn new(backend: B) -> Self {
        Self { backend, cache: std::sync::Mutex::new(None) }
    }

    fn with<T>(&self, f: impl FnOnce(&mut std::collections::BTreeMap<String, String>, &B) -> T) -> T {
        let mut g = self.cache.lock().unwrap();
        if g.is_none() {
            let mut map: std::collections::BTreeMap<String, String> =
                self.backend.get(VAULT_ACCOUNT).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
            if map.is_empty() {
                // first run with the vault: fold in the old per-secret items, then remove them
                for k in LEGACY_ACCOUNTS {
                    if let Some(v) = self.backend.get(k) {
                        map.insert((*k).into(), v);
                    }
                }
                if !map.is_empty() && self.backend.set(VAULT_ACCOUNT, &serde_json::to_string(&map).unwrap()).is_ok() {
                    for k in LEGACY_ACCOUNTS {
                        let _ = self.backend.delete(k);
                    }
                }
            }
            *g = Some(map);
        }
        f(g.as_mut().unwrap(), &self.backend)
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.with(|m, _| m.get(name).cloned().filter(|s| !s.is_empty()))
    }

    pub fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.with(|m, b| {
            let mut next = m.clone();
            next.insert(name.into(), value.into());
            b.set(VAULT_ACCOUNT, &serde_json::to_string(&next).unwrap())?;
            *m = next; // only remember it if it was actually saved
            Ok(())
        })
    }

    pub fn clear(&self, name: &str) -> Result<(), String> {
        self.with(|m, b| {
            let mut next = m.clone();
            next.remove(name);
            b.set(VAULT_ACCOUNT, &serde_json::to_string(&next).unwrap())?;
            *m = next;
            Ok(())
        })
    }
}

fn vault() -> &'static Vault<KeyringBackend> {
    static V: std::sync::OnceLock<Vault<KeyringBackend>> = std::sync::OnceLock::new();
    V.get_or_init(|| Vault::new(KeyringBackend))
}

pub fn secret_get(name: &str) -> Option<String> {
    vault().get(name)
}

pub fn secret_set(name: &str, value: &str) -> Result<(), String> {
    vault().set(name, value)
}

pub fn secret_clear(name: &str) -> Result<(), String> {
    vault().clear(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem {
        items: RefCell<HashMap<String, String>>,
        reads: Cell<u32>,
        fail_writes: bool,
    }
    impl Backend for &Mem {
        fn get(&self, a: &str) -> Option<String> {
            self.reads.set(self.reads.get() + 1);
            self.items.borrow().get(a).cloned()
        }
        fn set(&self, a: &str, v: &str) -> Result<(), String> {
            if self.fail_writes { return Err("denied".into()); }
            self.items.borrow_mut().insert(a.into(), v.into());
            Ok(())
        }
        fn delete(&self, a: &str) -> Result<(), String> {
            self.items.borrow_mut().remove(a);
            Ok(())
        }
    }

    #[test]
    fn vault_reads_the_keychain_once_and_migrates_old_items() {
        let mem = Mem::default();
        mem.items.borrow_mut().insert("ai_key".into(), "sk-1".into());
        mem.items.borrow_mut().insert("cred:spotify".into(), "{\"t\":1}".into());
        let v = Vault::new(&mem);
        assert_eq!(v.get("ai_key").as_deref(), Some("sk-1"));
        assert_eq!(v.get("cred:spotify").as_deref(), Some("{\"t\":1}"));
        for _ in 0..20 {
            v.get("ai_key");
            v.get("nothing");
        }
        // 1 vault read + 2 legacy reads during migration, then never again
        assert_eq!(mem.reads.get(), 3, "keychain touched {} times", mem.reads.get());
        let items = mem.items.borrow();
        assert!(items.contains_key("vault") && !items.contains_key("ai_key") && !items.contains_key("cred:spotify"), "old items folded in and removed");
    }

    #[test]
    fn vault_set_clear_roundtrip_and_failed_write_is_not_remembered() {
        let mem = Mem::default();
        let v = Vault::new(&mem);
        v.set("ai_key", "a").unwrap();
        v.set("cred:apple", "b").unwrap();
        assert_eq!((v.get("ai_key").as_deref(), v.get("cred:apple").as_deref()), (Some("a"), Some("b")));
        v.clear("ai_key").unwrap();
        assert_eq!(v.get("ai_key"), None);
        assert_eq!(Vault::new(&mem).get("cred:apple").as_deref(), Some("b"));
        let broken = Mem { fail_writes: true, ..Default::default() };
        let bv = Vault::new(&broken);
        assert!(bv.set("ai_key", "x").is_err());
        assert_eq!(bv.get("ai_key"), None, "unsaved secret is not pretended to exist");
    }

    #[test]
    fn sanitize_clamps_and_old_json_gets_defaults() {
        let s: Settings = serde_json::from_str(r#"{"discovery_ratio": 5, "quiet_start": 30, "callback_port": 80}"#).unwrap();
        let s = s.sanitized();
        assert_eq!((s.discovery_ratio, s.quiet_start, s.callback_port), (1.0, 6, 1024));
        assert!(s.ai_enabled && s.listen_threshold == 0.8, "missing fields take defaults");
    }
}
