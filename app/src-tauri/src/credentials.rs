//! Provider credentials (OAuth tokens) live in the OS keychain, one entry per provider
//! (`cred:spotify`, `cred:apple`, ...). The blob is opaque to the app: each provider's adapter
//! decides what it contains. Older installs kept Spotify tokens in sqlite; `load_or_migrate`
//! moves them once.

use spinamp_store::Store;

pub trait SecretBackend {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

pub struct Keychain;

impl SecretBackend for Keychain {
    fn get(&self, key: &str) -> Option<String> {
        crate::settings::secret_get(key)
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        crate::settings::secret_set(key, value)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        crate::settings::secret_clear(key)
    }
}

fn key(provider: &str) -> String {
    format!("cred:{provider}")
}

/// Where a provider's credentials currently are.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Storage {
    Keychain,
    /// Keychain unavailable; kept in the local database instead.
    Sqlite,
    None,
}

fn legacy_key(provider: &str) -> String {
    format!("{provider}_tokens")
}

/// Keychain first. If only the legacy sqlite copy exists, move it to the keychain (verified by
/// reading it back) and delete the sqlite copy. If the keychain refuses, keep using sqlite.
pub fn load_or_migrate(b: &dyn SecretBackend, store: &Store, provider: &str) -> (Option<String>, Storage) {
    let legacy = legacy_key(provider);
    if let Some(v) = b.get(&key(provider)) {
        let _ = store.kv_delete(&legacy); // leftover plaintext copy from before migration
        return (Some(v), Storage::Keychain);
    }
    let Some(old) = store.kv_get(&legacy).ok().flatten() else { return (None, Storage::None) };
    match b.set(&key(provider), &old) {
        Ok(()) if b.get(&key(provider)).as_deref() == Some(old.as_str()) => {
            let _ = store.kv_delete(&legacy);
            (Some(old), Storage::Keychain)
        }
        _ => (Some(old), Storage::Sqlite),
    }
}

/// Save after sign-in or token refresh. Falls back to sqlite so a keychain problem never logs you out.
pub fn save(b: &dyn SecretBackend, store: &Store, provider: &str, blob: &str) -> Storage {
    match b.set(&key(provider), blob) {
        Ok(()) => {
            let _ = store.kv_delete(&legacy_key(provider));
            Storage::Keychain
        }
        Err(_) => {
            let _ = store.kv_set(&legacy_key(provider), blob);
            Storage::Sqlite
        }
    }
}

pub fn clear(b: &dyn SecretBackend, store: &Store, provider: &str) {
    let _ = b.delete(&key(provider));
    let _ = store.kv_delete(&legacy_key(provider));
}

pub fn storage_of(b: &dyn SecretBackend, store: &Store, provider: &str) -> Storage {
    if b.get(&key(provider)).is_some() {
        Storage::Keychain
    } else if store.kv_get(&legacy_key(provider)).ok().flatten().is_some() {
        Storage::Sqlite
    } else {
        Storage::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem {
        m: RefCell<HashMap<String, String>>,
        broken: bool,
    }
    impl SecretBackend for Mem {
        fn get(&self, k: &str) -> Option<String> {
            self.m.borrow().get(k).cloned()
        }
        fn set(&self, k: &str, v: &str) -> Result<(), String> {
            if self.broken { return Err("no keychain".into()); }
            self.m.borrow_mut().insert(k.into(), v.into());
            Ok(())
        }
        fn delete(&self, k: &str) -> Result<(), String> {
            self.m.borrow_mut().remove(k);
            Ok(())
        }
    }

    #[test]
    fn migrates_legacy_sqlite_tokens_to_keychain() {
        let store = Store::memory().unwrap();
        store.kv_set("spotify_tokens", "{\"t\":1}").unwrap();
        let b = Mem::default();
        let (blob, st) = load_or_migrate(&b, &store, "spotify");
        assert_eq!((blob.as_deref(), st), (Some("{\"t\":1}"), Storage::Keychain));
        assert_eq!(b.get("cred:spotify").as_deref(), Some("{\"t\":1}"));
        assert_eq!(store.kv_get("spotify_tokens").unwrap(), None, "plaintext copy removed");
        // second launch: nothing to migrate
        assert_eq!(load_or_migrate(&b, &store, "spotify").1, Storage::Keychain);
    }

    #[test]
    fn broken_keychain_keeps_working_from_sqlite() {
        let store = Store::memory().unwrap();
        store.kv_set("spotify_tokens", "x").unwrap();
        let b = Mem { broken: true, ..Default::default() };
        let (blob, st) = load_or_migrate(&b, &store, "spotify");
        assert_eq!((blob.as_deref(), st), (Some("x"), Storage::Sqlite));
        assert_eq!(store.kv_get("spotify_tokens").unwrap().as_deref(), Some("x"), "not deleted");
        assert_eq!(save(&b, &store, "spotify", "y"), Storage::Sqlite);
        assert_eq!(store.kv_get("spotify_tokens").unwrap().as_deref(), Some("y"));
    }

    #[test]
    fn providers_are_isolated_and_clear_removes_all_copies() {
        let store = Store::memory().unwrap();
        let b = Mem::default();
        save(&b, &store, "spotify", "s");
        save(&b, &store, "apple", "a");
        assert_eq!(b.get("cred:apple").as_deref(), Some("a"));
        clear(&b, &store, "spotify");
        assert_eq!(storage_of(&b, &store, "spotify"), Storage::None);
        assert_eq!(storage_of(&b, &store, "apple"), Storage::Keychain);
    }
}
