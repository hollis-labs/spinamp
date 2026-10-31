//! Apple Music adapter: MusicKit web API (catalog + library) with a bring-your-own MusicKit key,
//! and playback by remote-controlling the Music app on macOS.
pub mod auth;
pub mod client;
pub mod map;
pub mod player;
pub mod token;

pub use auth::Session;
pub use client::{AppleMusicProvider, Creds};

/// Throwaway P-256 keys for tests, generated fresh so no private key lives in the repo.
#[cfg(test)]
pub(crate) mod testkey {
    use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    pub fn pair() -> (String, String) {
        let sk = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        (sk.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(), sk.public_key().to_public_key_pem(LineEnding::LF).unwrap())
    }
}
