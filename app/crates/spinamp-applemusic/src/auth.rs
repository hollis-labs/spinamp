//! Music User Token via MusicKit JS. Apple only issues it through its JS SDK, so we serve a tiny
//! page on a loopback port, the user approves in their normal browser (already signed in to
//! Apple), and the page posts the token back to us.
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Session {
    listener: TcpListener,
    state: String,
    page: String,
    pub authorize_url: String,
}

fn page(dev_token: &str, state: &str) -> String {
    format!(
        r#"<!doctype html><meta charset="utf-8"><title>Spinamp · Apple Music</title>
<style>body{{background:#0b0d0f;color:#cfe;font:15px ui-monospace,monospace;display:grid;place-items:center;height:100vh;margin:0}}
button{{background:#ffd23a;border:0;padding:14px 22px;font:inherit;font-weight:700;cursor:pointer}}#m{{margin-top:16px;opacity:.8}}</style>
<div><button id="b" disabled>CONNECT APPLE MUSIC</button><div id="m">loading MusicKit…</div></div>
<script src="https://js-cdn.music.apple.com/musickit/v3/musickit.js" async></script>
<script>
const STATE={state:?}, DEV={dev:?};
const m=document.getElementById('m'), b=document.getElementById('b');
const log=(x)=>{{ m.textContent=x; }};
const send=(o)=>fetch('/token',{{method:'POST',body:JSON.stringify(Object.assign({{state:STATE}},o))}});
async function deliver(mk, t) {{
  const r=await send({{token:t}});
  if(!r.ok) throw new Error('Spinamp did not accept the token ('+r.status+')');
  log('LINKED. You can close this tab and go back to Spinamp.'); b.disabled=true;
}}
document.addEventListener('musickitloaded', async()=>{{
  try {{
    const mk=await MusicKit.configure({{developerToken:DEV,app:{{name:'Spinamp',build:'1'}}}});
    // Apple sometimes finishes sign-in by redirecting back to this page instead of a popup:
    // the token is then already stored, so hand it over without another click.
    if (mk.isAuthorized && mk.musicUserToken) {{ log('already signed in, sending to Spinamp…'); await deliver(mk, mk.musicUserToken); return; }}
    b.disabled=false; log('ready');
    b.onclick=async()=>{{ try {{
      log('waiting for Apple…');
      const t=(await mk.authorize())||mk.musicUserToken;
      if(!t) throw new Error('Apple returned no token');
      await deliver(mk, t);
    }} catch(e) {{ await send({{error:String(e)}}); log('Not linked: '+e); }} }};
  }} catch(e) {{ await send({{error:String(e)}}); log('MusicKit failed: '+e); }}
}});
window.addEventListener('error',(e)=>log('page error: '+e.message));
</script>"#,
        state = state,
        dev = dev_token,
    )
}

impl Session {
    pub async fn begin(dev_token: &str, port: u16) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| format!("port {port} is busy ({e}); change the callback port in Settings"))?;
        let state: String = (0..24).map(|_| char::from(b'a' + rand::random_range(0..26u8))).collect();
        Ok(Self { authorize_url: format!("http://127.0.0.1:{port}/"), page: page(dev_token, &state), state, listener })
    }

    /// Serve the page, wait for the token POST, return the Music User Token.
    /// Every connection gets its own task: browsers open idle spare connections and would
    /// otherwise stall the single accept loop (and drop the POST that arrives later).
    pub async fn finish(self) -> Result<String, String> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<String, String>>();
        let page: std::sync::Arc<str> = self.page.into();
        let state: std::sync::Arc<str> = self.state.into();
        let listener = self.listener;
        let accept = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { return };
                let (tx, page, state) = (tx.clone(), page.clone(), state.clone());
                tokio::spawn(async move { serve(sock, &page, &state, tx).await });
            }
        });
        let r = tokio::time::timeout(std::time::Duration::from_secs(300), rx.recv()).await;
        accept.abort();
        match r {
            Ok(Some(res)) => res,
            Ok(None) => Err("sign-in was interrupted".into()),
            Err(_) => Err("sign-in timed out".into()),
        }
    }
}

async fn serve(mut sock: tokio::net::TcpStream, page: &str, state: &str, tx: tokio::sync::mpsc::UnboundedSender<Result<String, String>>) {
    let Some((head, body)) = read_request(&mut sock).await else { return };
    let first = head.lines().next().unwrap_or("");
    crate::log(&format!("sign-in server: {first} ({} body bytes)", body.len()));
    if first.starts_with("POST /token") {
        let _ = sock.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").await;
        let res = parse_token(&body, state);
        crate::log(&format!("token post: {}", if res.is_ok() { "accepted".to_string() } else { format!("{:?}", res.as_ref().err()) }));
        let _ = tx.send(res);
        return;
    }
    let (status, ctype, out) =
        if first.starts_with("GET / ") || first.starts_with("GET /?") { ("200 OK", "text/html; charset=utf-8", page) } else { ("404 Not Found", "text/plain", "") };
    let _ = sock
        .write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{out}", out.len()).as_bytes())
        .await;
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(60), sock.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            return None;
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some(i) = text.find("\r\n\r\n") {
            let head = &text[..i];
            let want: usize = head
                .lines()
                .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                .unwrap_or(0);
            let body = &text[i + 4..];
            if body.len() >= want {
                return Some((head.to_string(), body[..want].to_string()));
            }
        }
    }
}

pub fn parse_token(body: &str, state: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    if v["state"].as_str() != Some(state) {
        return Err("sign-in state mismatch".into());
    }
    if let Some(e) = v["error"].as_str() {
        return Err(format!("Apple Music sign-in failed: {e}"));
    }
    v["token"].as_str().filter(|t| !t.is_empty()).map(String::from).ok_or_else(|| "no token received".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_post_parsing() {
        assert_eq!(parse_token(r#"{"state":"s","token":"T"}"#, "s"), Ok("T".into()));
        assert!(parse_token(r#"{"state":"x","token":"T"}"#, "s").is_err());
        assert!(parse_token(r#"{"state":"s","error":"denied"}"#, "s").unwrap_err().contains("denied"));
    }

    #[tokio::test]
    async fn token_arrives_despite_idle_spare_connections_and_a_slow_user() {
        use tokio::io::AsyncWriteExt;
        let s = Session::begin("DEV", 0).await.unwrap_or_else(|_| panic!("bind"));
        let port = s.listener.local_addr().unwrap().port();
        let state = s.state.clone();
        let h = tokio::spawn(s.finish());
        // a spare connection that never sends anything, like a browser's preconnect
        let _idle = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut get = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        get.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut page = vec![0u8; 8192];
        let n = tokio::io::AsyncReadExt::read(&mut get, &mut page).await.unwrap();
        assert!(String::from_utf8_lossy(&page[..n]).contains("musickit.js"));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let body = format!(r#"{{"state":"{state}","token":"USERTOKEN"}}"#);
        let mut post = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        post.write_all(format!("POST /token HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        assert_eq!(h.await.unwrap(), Ok("USERTOKEN".into()));
    }

    #[test]
    fn page_embeds_state_and_token_as_js_strings() {
        let p = page("DEV.TOK.EN", "abc");
        assert!(p.contains("\"abc\"") && p.contains("\"DEV.TOK.EN\"") && p.contains("musickit.js"));
    }
}
