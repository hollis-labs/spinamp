use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const SCOPES: &str = "user-read-private playlist-read-private playlist-read-collaborative \
user-library-read user-read-playback-state user-modify-playback-state user-read-currently-playing \
user-top-read user-read-recently-played user-follow-read";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Pkce {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    let verifier: String = (0..96).map(|_| CHARS[rng.random_range(0..CHARS.len())] as char).collect();
    let challenge = challenge_for(&verifier);
    Pkce { verifier, challenge }
}

pub fn challenge_for(verifier: &str) -> String {
    B64.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn authorize_url(client_id: &str, redirect: &str, challenge: &str, state: &str) -> String {
    let mut u = url::Url::parse("https://accounts.spotify.com/authorize").unwrap();
    u.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect)
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge)
        .append_pair("state", state);
    u.into()
}

/// Pull `code` out of the loopback request line, verifying `state`.
pub fn parse_callback(request_line: &str, expect_state: &str) -> Result<String, String> {
    let path = request_line.split_whitespace().nth(1).ok_or("malformed request")?;
    let u = url::Url::parse(&format!("http://127.0.0.1{path}")).map_err(|e| e.to_string())?;
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    if let Some(e) = q.get("error") {
        return Err(format!("spotify denied: {e}"));
    }
    if q.get("state").map(String::as_str) != Some(expect_state) {
        return Err("state mismatch".into());
    }
    q.get("code").cloned().ok_or_else(|| "no code".into())
}

pub struct Session {
    pub authorize_url: String,
    listener: TcpListener,
    verifier: String,
    state: String,
    client_id: String,
    redirect: String,
}

impl Session {
    /// Bind a loopback port and build the URL to open in the browser.
    /// Register `http://127.0.0.1:<port>/callback` in the Spotify dashboard (use a fixed port).
    pub async fn begin(client_id: &str, port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();
        let redirect = format!("http://127.0.0.1:{port}/callback");
        let p = pkce();
        let state: String = (0..16).map(|_| format!("{:x}", rand::rng().random_range(0..16))).collect();
        Ok(Self {
            authorize_url: authorize_url(client_id, &redirect, &p.challenge, &state),
            listener,
            verifier: p.verifier,
            state,
            client_id: client_id.into(),
            redirect,
        })
    }

    /// Wait for the browser redirect, then exchange the code for tokens.
    pub async fn finish(self, now: i64) -> Result<Tokens, String> {
        let (mut sock, _) = tokio::time::timeout(std::time::Duration::from_secs(300), self.listener.accept())
            .await
            .map_err(|_| "sign-in timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 4096];
        let n = sock.read(&mut buf).await.map_err(|e| e.to_string())?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let result = parse_callback(req.lines().next().unwrap_or(""), &self.state);
        let body = if result.is_ok() { "SPINAMP: LINKED. YOU MAY CLOSE THIS TAB." } else { "SPINAMP: AUTH FAILED." };
        let _ = sock
            .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
            .await;
        let code = result?;
        token_request(
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &self.redirect),
                ("client_id", &self.client_id),
                ("code_verifier", &self.verifier),
            ],
            None,
            now,
        )
        .await
    }
}

pub async fn refresh(client_id: &str, t: &Tokens, now: i64) -> Result<Tokens, String> {
    token_request(
        &[("grant_type", "refresh_token"), ("refresh_token", &t.refresh_token), ("client_id", client_id)],
        Some(&t.refresh_token),
        now,
    )
    .await
}

async fn token_request(form: &[(&str, &str)], keep_refresh: Option<&str>, now: i64) -> Result<Tokens, String> {
    let r = reqwest::Client::new()
        .post("https://accounts.spotify.com/api/token")
        .form(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("token endpoint: {} {}", r.status(), r.text().await.unwrap_or_default()));
    }
    let t: TokenResponse = r.json().await.map_err(|e| e.to_string())?;
    Ok(Tokens {
        access_token: t.access_token,
        // Spotify may omit a new refresh token on refresh; keep the old one.
        refresh_token: t.refresh_token.or(keep_refresh.map(String::from)).ok_or("no refresh token")?,
        expires_at: now + t.expires_in - 30,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_rfc7636_vector() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_is_valid_length() {
        let p = pkce();
        assert!((43..=128).contains(&p.verifier.len()));
    }

    #[test]
    fn callback_parsing() {
        assert_eq!(parse_callback("GET /callback?code=abc&state=s1 HTTP/1.1", "s1"), Ok("abc".into()));
        assert!(parse_callback("GET /callback?code=abc&state=bad HTTP/1.1", "s1").is_err());
        assert!(parse_callback("GET /callback?error=access_denied&state=s1 HTTP/1.1", "s1").is_err());
    }

    #[test]
    fn url_carries_pkce_params() {
        let u = authorize_url("cid", "http://127.0.0.1:8888/callback", "CH", "S");
        assert!(u.contains("code_challenge_method=S256") && u.contains("client_id=cid") && u.contains("state=S"));
    }
}
