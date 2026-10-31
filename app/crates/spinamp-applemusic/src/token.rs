//! MusicKit developer token: an ES256 JWT signed with the user's own `.p8` key.
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;

/// Apple allows up to 6 months; we mint short ones and renew, so a leak is not long-lived.
pub const LIFETIME_SECS: i64 = 30 * 86_400;

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    iat: i64,
    exp: i64,
}

pub fn developer_token(key_pem: &str, key_id: &str, team_id: &str, now: i64) -> Result<String, String> {
    let key = EncodingKey::from_ec_pem(key_pem.as_bytes()).map_err(|e| format!("not a valid MusicKit .p8 key: {e}"))?;
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(key_id.to_string());
    encode(&header, &Claims { iss: team_id, iat: now, exp: now + LIFETIME_SECS }, &key).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};

    #[test]
    fn token_has_apple_shape_and_verifies() {
        let (KEY, PUB) = crate::testkey::pair();
        let t = developer_token(&KEY, "KEYID12345", "TEAMID1234", 1_000_000).unwrap();
        let h = decode_header(&t).unwrap();
        assert_eq!((h.alg, h.kid.as_deref()), (Algorithm::ES256, Some("KEYID12345")));
        let mut v = Validation::new(Algorithm::ES256);
        v.validate_exp = false;
        v.set_required_spec_claims(&["iss"]);
        let c = decode::<serde_json::Value>(&t, &DecodingKey::from_ec_pem(PUB.as_bytes()).unwrap(), &v).unwrap().claims;
        assert_eq!((c["iss"].as_str(), c["iat"].as_i64(), c["exp"].as_i64()), (Some("TEAMID1234"), Some(1_000_000), Some(1_000_000 + LIFETIME_SECS)));
    }

    #[test]
    fn rejects_garbage_key() {
        assert!(developer_token("nope", "K", "T", 0).unwrap_err().contains("valid MusicKit"));
    }
}
