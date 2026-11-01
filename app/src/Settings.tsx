import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import * as Ic from "./Icons";

type S = {
  spotify_client_id: string; callback_port: number; preferred_provider: string; lastfm_user: string;
  ai_enabled: boolean; ai_model: string;
  discovery_ratio: number; discovery_sharpness: number;
  listen_threshold: number; quiet_enabled: boolean; quiet_start: number; quiet_end: number;
  musicbrainz_contact: string;
  analyze_auto: boolean;
  analyze_batch: number;
  analyze_gap_minutes: number;
  viz_offset_ms: number;
  capture_enabled: boolean;
  share_fingerprints: boolean;
  viz_led: boolean;
};
type View = { settings: S; ai_key_set: boolean; ai_key_from_env: boolean };
type Analysis = { batch: number; next_in_secs: number; running: boolean; analyzed: number; eligible: number; pending: number; failed: number; error: string };
type Deleted = { playlists: number; tracks: number; entries: number };
type Cap = { supported: boolean; enabled: boolean; running: boolean; error: string; recorded: number; complete: number };
type BlockRow = { kind: string; value: string };
type Conn = { connected: boolean; storage: "keychain" | "sqlite" | "none" };
type Catalog = { running: boolean; pool: number; error: string };

const HOURS = Array.from({ length: 24 }, (_, i) => i);
const hh = (h: number) => `${String(h).padStart(2, "0")}:00`;

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="setrow">
      <div><div>{label}</div>{hint && <div className="hint">{hint}</div>}</div>
      <div className="ctl">{children}</div>
    </div>
  );
}

export default function Settings({ onClose }: { onClose: () => void }) {
  const [v, setV] = useState<View | null>(null);
  const [s, setS] = useState<S | null>(null);
  const [key, setKey] = useState("");
  const [msg, setMsg] = useState("");
  const [cat, setCat] = useState<Catalog | null>(null);
  const [conn, setConn] = useState<Conn | null>(null);
  const [am, setAm] = useState<Conn | null>(null);
  const [lf, setLf] = useState<{ key_set: boolean; user: string } | null>(null);
  const [lfKey, setLfKey] = useState("");
  const [dg, setDg] = useState(false);
  const [dgTok, setDgTok] = useState("");
  const [lfVotes, setLfVotes] = useState(true);
  const [amKey, setAmKey] = useState({ source: "", keyId: "", teamId: "" });
  const [an, setAn] = useState<Analysis | null>(null);
  const [del, setDel] = useState<Deleted | null>(null);
  const [cap, setCap] = useState<Cap | null>(null);
  const [blocks, setBlocks] = useState<BlockRow[]>([]);
  const [capMsg, setCapMsg] = useState("");

  const load = async () => { const r = await invoke<View>("get_settings"); setV(r); setS(r.settings); };
  useEffect(() => { load(); invoke<Catalog>("catalog_status").then(setCat); invoke<Conn>("provider_status", { id: "spotify" }).then(setConn); invoke<Conn>("provider_status", { id: "applemusic" }).then(setAm); invoke<{ key_set: boolean; user: string }>("lastfm_status").then(setLf); invoke<boolean>("discogs_status").then(setDg); invoke<Analysis>("analysis_status").then(setAn); invoke<Deleted>("deleted_counts").then(setDel); invoke<Cap>("capture_status").then(setCap); invoke<BlockRow[]>("list_blocks").then(setBlocks); }, []);
  useEffect(() => {
    if (!cat?.running) return;
    const id = setInterval(async () => setCat(await invoke<Catalog>("catalog_status")), 2000);
    return () => clearInterval(id);
  }, [cat?.running]);

  if (!v || !s) return <section className="win"><div className="empty">LOADING...</div></section>;
  const set = <K extends keyof S>(k: K, val: S[K]) => setS({ ...s, [k]: val });
  const act = async (f: () => Promise<string | void>) => {
    try { setMsg("..."); setMsg((await f()) || ""); } catch (e) { setMsg("ERROR: " + String(e)); }
  };
  const save = () => act(async () => { setV(await invoke<View>("save_settings", { settings: s })); return "SAVED"; });

  return (
    <section className="win">
      <div className="winhead">
        <button className="btn" onClick={onClose}><Ic.Back /> BACK</button>
        <span className="crumb">SETTINGS</span>
        <span style={{ flex: 1 }} />
        {msg && <span className={msg.startsWith("ERROR") ? "bad-text" : "ok-text"}>{msg}</span>}
        <button className="btn primary" style={{ width: "auto", padding: "0 16px", height: 36 }} onClick={save}>SAVE</button>
      </div>
      <div className="scroll setbody">
        <h3>SPOTIFY</h3>
        <Row label="Connection" hint={conn?.connected ? (conn.storage === "keychain" ? "Signed in. Tokens are stored in the macOS Keychain." : "Signed in. Keychain unavailable, so tokens are in the local database.") : "Not signed in. Use RECONNECT in the SPOTIFY tab after saving your Client ID."}>
          <span className={conn?.connected ? "ok-text" : "dim"}>{conn?.connected ? "CONNECTED" : "NOT CONNECTED"}</span>
          {conn?.connected && <button className="btn bad" onClick={() => act(async () => { await invoke("provider_disconnect", { id: "spotify" }); setConn(await invoke<Conn>("provider_status", { id: "spotify" })); return "DISCONNECTED"; })}>DISCONNECT</button>}
        </Row>
        <Row label="Client ID" hint="From your app at developer.spotify.com.">
          <input className="inp" value={s.spotify_client_id} onChange={(e) => set("spotify_client_id", e.target.value)} placeholder="paste client id" />
        </Row>
        <Row label="Callback port" hint={`Redirect URI to register: http://127.0.0.1:${s.callback_port}/callback`}>
          <input className="inp short" type="number" value={s.callback_port} onChange={(e) => set("callback_port", Number(e.target.value))} />
        </Row>

        <h3>PLAYBACK</h3>
        <Row label="Play from" hint="When a song is on more than one service (matched by ISRC), play it from this one. AUTO stays on whichever is already playing, to avoid switching apps.">
          {([["", "AUTO"], ["spotify", "SPOTIFY"], ["applemusic", "APPLE MUSIC"]] as const).map(([id, label]) => (
            <button key={id} className={`btn ${s.preferred_provider === id ? "on" : ""}`} onClick={() => set("preferred_provider", id)}>{label}</button>
          ))}
        </Row>

        <h3>APPLE MUSIC</h3>
        <Row label="Connection" hint={am?.connected ? "Signed in. Your key and token are stored in the macOS Keychain." : "Needs your own MusicKit key (developer.apple.com > Keys), then CONNECT in the SOURCES tab. Playback uses the Music app and only plays tracks in your library."}>
          <span className={am?.connected ? "ok-text" : "dim"}>{am?.connected ? "CONNECTED" : "NOT CONNECTED"}</span>
          {am?.connected && <button className="btn bad" onClick={() => act(async () => { await invoke("provider_disconnect", { id: "applemusic" }); setAm(await invoke<Conn>("provider_status", { id: "applemusic" })); return "DISCONNECTED"; })}>DISCONNECT</button>}
        </Row>
        <Row label="MusicKit key" hint="Path to your AuthKey_XXXX.p8 (or paste its contents), with its Key ID and your Team ID. Saved to the Keychain, never to disk or the repo.">
          <input className="inp" value={amKey.source} onChange={(e) => setAmKey({ ...amKey, source: e.target.value })} placeholder="~/Downloads/AuthKey_XXXXXXXXXX.p8" autoComplete="off" />
        </Row>
        <Row label="Key ID / Team ID" hint="Key ID is in the .p8 file name. Team ID is under Membership in your Apple Developer account.">
          <input className="inp short" value={amKey.keyId} onChange={(e) => setAmKey({ ...amKey, keyId: e.target.value })} placeholder="Key ID" />
          <input className="inp short" value={amKey.teamId} onChange={(e) => setAmKey({ ...amKey, teamId: e.target.value })} placeholder="Team ID" />
          <button className="btn" disabled={!amKey.source.trim() || !amKey.keyId.trim() || !amKey.teamId.trim()} onClick={() => act(async () => {
            await invoke("applemusic_set_key", { source: amKey.source, keyId: amKey.keyId, teamId: amKey.teamId });
            setAmKey({ source: "", keyId: "", teamId: "" });
            setAm(await invoke<Conn>("provider_status", { id: "applemusic" }));
            return "KEY SAVED. NOW CONNECT IN THE SOURCES TAB";
          })}>SAVE KEY</button>
        </Row>

        <h3>LAST.FM</h3>
        <Row label="Account" hint="Free: create an API account at last.fm/api/account/create, then paste the API key here with your username. Read-only: Spinamp never writes to Last.fm. Similar-artist data improves Discovery.">
          <input className="inp short" value={s.lastfm_user} onChange={(e) => set("lastfm_user", e.target.value)} placeholder="username" autoComplete="off" />
          <input className="inp" type="password" value={lfKey} onChange={(e) => setLfKey(e.target.value)} placeholder={lf?.key_set ? "•••••••• (saved)" : "API key"} autoComplete="off" />
          <button className="btn" disabled={!lfKey.trim()} onClick={() => act(async () => { await invoke("set_lastfm_key", { key: lfKey }); setLfKey(""); setLf(await invoke("lastfm_status")); return "KEY SAVED"; })}>SAVE KEY</button>
          {lf?.key_set && <button className="btn bad" onClick={() => act(async () => { await invoke("clear_lastfm_key"); setLf(await invoke("lastfm_status")); return "KEY REMOVED"; })}>REMOVE</button>}
          <button className="btn" disabled={!lf?.key_set || !s.lastfm_user.trim()} onClick={() => act(async () => { setV(await invoke<View>("save_settings", { settings: s })); return await invoke<string>("lastfm_test", { user: s.lastfm_user }); })}>TEST</button>
        </Row>
        <Row label="Import" hint="Matches your scrobbles and loved tracks to songs already in your library (plays are backfilled with their real dates, repeats are skipped). Big histories take a few minutes.">
          {([["scrobbles", "SCROBBLES"], ["loved", "LOVED"], ["artists", "TOP ARTISTS"], ["all", "EVERYTHING"]] as const).map(([k, label]) => (
            <button key={k} className="btn" disabled={!lf?.key_set || !s.lastfm_user.trim()} onClick={() => act(async () => {
              setMsg(`IMPORTING ${label}...`);
              return "IMPORTED: " + (await invoke<string>("lastfm_import", { user: s.lastfm_user, kind: k, votes: lfVotes })).toUpperCase();
            })}>{label}</button>
          ))}
          <label className="pill" style={{ height: 26, fontSize: 11 }}>
            <input type="checkbox" checked={lfVotes} onChange={(e) => setLfVotes(e.target.checked)} style={{ accentColor: "var(--orange)" }} /> LOVED = +1
          </label>
        </Row>

        <h3>DISCOGS</h3>
        <Row label="Personal access token" hint="Free: discogs.com > Settings > Developers > Generate new token. Adds labels, pressings and per-song credits (CREDITS button) and aliases/members (ARTIST panel). Read-only; saved to the Keychain.">
          <input className="inp" type="password" value={dgTok} onChange={(e) => setDgTok(e.target.value)} placeholder={dg ? "•••••••• (saved)" : "token"} autoComplete="off" />
          <button className="btn" disabled={!dgTok.trim()} onClick={() => act(async () => { await invoke("set_discogs_token", { token: dgTok }); setDgTok(""); setDg(await invoke<boolean>("discogs_status")); return "TOKEN SAVED"; })}>SAVE TOKEN</button>
          {dg && <button className="btn bad" onClick={() => act(async () => { await invoke("clear_discogs_token"); setDg(await invoke<boolean>("discogs_status")); return "TOKEN REMOVED"; })}>REMOVE</button>}
          <button className="btn" disabled={!dg} onClick={() => act(async () => await invoke<string>("discogs_test"))}>TEST</button>
        </Row>

        <h3>AI</h3>
        <Row label="Use AI for playlist prompts" hint="Claude reads your prompt and tag names only. Never track titles or history.">
          <button className={`btn ${s.ai_enabled ? "on" : ""}`} onClick={() => set("ai_enabled", !s.ai_enabled)}>{s.ai_enabled ? "ON" : "OFF"}</button>
        </Row>
        <Row label="API key" hint={v.ai_key_set ? "Stored in the macOS Keychain." : v.ai_key_from_env ? "Using the key from your environment." : "No key set."}>
          <input className="inp" type="password" value={key} onChange={(e) => setKey(e.target.value)}
            placeholder={v.ai_key_set ? "•••••••• (saved)" : "sk-ant-..."} autoComplete="off" />
          <button className="btn" disabled={!key.trim()} onClick={() => act(async () => {
            setV(await invoke<View>("set_ai_key", { key })); setKey(""); return "KEY SAVED";
          })}>SAVE KEY</button>
          {v.ai_key_set && <button className="btn bad" onClick={() => act(async () => { setV(await invoke<View>("clear_ai_key")); return "KEY REMOVED"; })}>REMOVE</button>}
          <button className="btn" onClick={() => act(async () => await invoke<string>("test_ai"))}>TEST</button>
        </Row>
        <Row label="Model"><input className="inp" value={s.ai_model} onChange={(e) => set("ai_model", e.target.value)} /></Row>

        <h3>DISCOVERY</h3>
        <Row label="Discovery share in MIX" hint="How often a MIX pick is new music.">
          <input type="range" min={0} max={100} value={Math.round(s.discovery_ratio * 100)} onChange={(e) => set("discovery_ratio", Number(e.target.value) / 100)} />
          <span className="val">{Math.round(s.discovery_ratio * 100)}%</span>
        </Row>
        <Row label="Taste strictness" hint="Low = more surprises. High = closer to what you already like.">
          <input type="range" min={5} max={60} value={Math.round(s.discovery_sharpness * 10)} onChange={(e) => set("discovery_sharpness", Number(e.target.value) / 10)} />
          <span className="val">{s.discovery_sharpness.toFixed(1)}</span>
        </Row>
        <Row label="Discovery pool" hint={cat?.error || "Tracks by similar artists you do not own, found via ListenBrainz + Spotify."}>
          <span className="val">{cat?.pool ?? 0} TRACKS</span>
          <button className="btn" disabled={cat?.running} onClick={async () => { await invoke("catalog_refill", { force: true }); setCat(await invoke<Catalog>("catalog_status")); }}>
            {cat?.running ? "FINDING..." : "FIND MORE"}
          </button>
        </Row>

        <h3>AUDIO ANALYSIS</h3>
        <Row label="Measured tracks" hint={an?.error || "Tempo, energy, loudness, loud spikes and a beat pattern, computed on this Mac from 30-second previews. Only the numbers are kept; the audio is discarded after each track."}>
          <span className="val">{an?.analyzed ?? 0} / {an?.eligible ?? 0}</span>
          <button className="btn" disabled={an?.running || !an?.pending} onClick={async () => { await invoke("analysis_start", { all: false }); setAn(await invoke<Analysis>("analysis_status")); }}>
            {an?.running ? "RUNNING..." : an?.pending ? `NEXT BATCH` : "UP TO DATE"}
          </button>
          <button className="btn" disabled={an?.running || !an?.pending} onClick={async () => { await invoke("analysis_start", { all: true }); setAn(await invoke<Analysis>("analysis_status")); }}>ALL {an?.pending ?? 0}</button>
        </Row>
        <Row label="Batch size" hint="Tracks per batch, about 0.5 MB each. 0 = everything at once. Most-played and newest tracks go first.">
          <input className="inp short" type="number" min={0} value={s.analyze_batch} onChange={(e) => set("analyze_batch", Math.max(0, Number(e.target.value)))} />
        </Row>
        <Row label="Roll through the library automatically" hint="Runs a batch at startup, waits, then does the next, until everything is measured. Stops when you quit.">
          <button className={`btn ${s.analyze_auto ? "on" : ""}`} onClick={() => set("analyze_auto", !s.analyze_auto)}>{s.analyze_auto ? "ON" : "OFF"}</button>
          <input className="inp short" type="number" min={1} value={s.analyze_gap_minutes} onChange={(e) => set("analyze_gap_minutes", Math.max(1, Number(e.target.value)))} />
          <span className="val">MIN APART</span>
        </Row>
        <Row label="Start over" hint="Forgets all measured tempo, energy and patterns so analysis runs again from scratch. Your library, votes, plays and recorded fingerprints are kept.">
          <button className="btn bad" disabled={an?.running} onClick={() => act(async () => { await invoke("analysis_reset"); setAn(await invoke<Analysis>("analysis_status")); return "ANALYSIS CLEARED"; })}>RESET ANALYSIS</button>
        </Row>
        <Row label="Visualiser timing" hint="Nudge the visualiser if it runs ahead of or behind what you hear (Bluetooth speakers usually play late, so try a positive value of a few hundred ms).">
          <input type="range" min={-500} max={500} step={10} value={s.viz_offset_ms} onChange={(e) => set("viz_offset_ms", Number(e.target.value))} />
          <span className="val">{s.viz_offset_ms} MS</span>
        </Row>

        <h3>LIVE CAPTURE (macOS)</h3>
        <Row label="Capture Spotify's audio" hint="Hears the Spotify desktop app (only Spotify, not other apps) to drive the visualiser with the real spectrum and to record a fingerprint of each track, so later listens match the real song. Audio is reduced to 16 numbers every 50 ms and discarded immediately. Needs the Screen & System Audio Recording permission.">
          <button className={`btn ${s.capture_enabled ? "on" : ""}`} disabled={!cap?.supported} onClick={() => set("capture_enabled", !s.capture_enabled)}>{s.capture_enabled ? "ON" : "OFF"}</button>
          <button className="btn" disabled={!cap?.supported} onClick={async () => { try { setCapMsg(await invoke<string>("capture_check")); } catch (e) { setCapMsg("ERROR: " + String(e)); } }}>CHECK PERMISSION</button>
        </Row>
        <Row label="Status" hint={capMsg || cap?.error || (cap?.supported ? "Save, then play a track from the Spotify desktop app. Capture starts by itself." : "Not available on this platform yet.")}>
          <span className="val">{cap ? `${cap.recorded} RECORDED · ${cap.complete} COMPLETE` : "..."}</span>
          <span className={cap?.running ? "ok-text" : "dim"}>{cap?.running ? "CAPTURING" : "IDLE"}</span>
        </Row>
        <Row label="Share fingerprints" hint="Opt in to sharing the song fingerprints you record (anonymous, identified only by the recording's ISRC code) with the shared spinamp.fm database, so only one person ever has to play a song, and to use fingerprints others contributed. Nothing is sent yet: sharing launches with the beta, and this just saves your choice.">
          <button className={`btn ${s.share_fingerprints ? "on" : ""}`} onClick={() => set("share_fingerprints", !s.share_fingerprints)}>{s.share_fingerprints ? "OPTED IN" : "OFF"}</button>
        </Row>
        <Row label="LED look" hint="Horizontal segments across the visualiser bars."><button className={`btn ${s.viz_led ? "on" : ""}`} onClick={() => set("viz_led", !s.viz_led)}>{s.viz_led ? "ON" : "OFF"}</button></Row>

        <h3>BLOCKED</h3>
        <Row label="Never played automatically" hint="Blocked songs, artists and tags/genres are skipped by shuffle, discovery and generated playlists, and never suggested. They stay visible in your lists, marked BLOCKED. Block from the ... menu on any track.">
          <span className="val">{blocks.length} BLOCKED</span>
        </Row>
        {blocks.map((b) => (
          <Row key={b.kind + b.value} label={`${b.kind === "tag" ? "TAG / GENRE" : b.kind.toUpperCase()}: ${b.kind === "song" ? b.value.replace(/^isrc:/, "ISRC ") : b.value}`}>
            <button className="btn" onClick={async () => setBlocks(await invoke<BlockRow[]>("remove_block", { kind: b.kind, value: b.value }))}>UNBLOCK</button>
          </Row>
        ))}

        <h3>DATA</h3>
        <Row label="Deleted items" hint="Nothing is ever hard-deleted. Deleted playlists, tracks and playlist entries are hidden, and imports will not bring them back. Your votes and play history are always kept.">
          <span className="val">{del ? `${del.playlists} PLAYLISTS · ${del.tracks} TRACKS · ${del.entries} ENTRIES` : "..."}</span>
          <button className="btn" disabled={!del || del.playlists + del.tracks + del.entries === 0}
            onClick={() => act(async () => { const c = await invoke<Deleted>("restore_deleted"); setDel(await invoke<Deleted>("deleted_counts")); return `RESTORED ${c.playlists + c.tracks + c.entries} ITEMS`; })}>RESTORE ALL</button>
        </Row>

        <h3>LISTENING STATS</h3>
        <Row label="Counts as a listen at" hint="Share of the track that must play.">
          <input type="range" min={10} max={100} value={Math.round(s.listen_threshold * 100)} onChange={(e) => set("listen_threshold", Number(e.target.value) / 100)} />
          <span className="val">{Math.round(s.listen_threshold * 100)}%</span>
        </Row>
        <Row label="Ignore quiet hours" hint="Overnight playback is not counted as plays or listens.">
          <button className={`btn ${s.quiet_enabled ? "on" : ""}`} onClick={() => set("quiet_enabled", !s.quiet_enabled)}>{s.quiet_enabled ? "ON" : "OFF"}</button>
          <label className="pill"><select value={s.quiet_start} onChange={(e) => set("quiet_start", Number(e.target.value))}>{HOURS.map((h) => <option key={h} value={h}>{hh(h)}</option>)}</select></label>
          <span className="val">to</span>
          <label className="pill"><select value={s.quiet_end} onChange={(e) => set("quiet_end", Number(e.target.value))}>{HOURS.map((h) => <option key={h} value={h}>{hh(h)}</option>)}</select></label>
        </Row>

        <h3>TAG LOOKUPS</h3>
        <Row label="MusicBrainz contact" hint="Optional text (e.g. a project URL) added to the User-Agent. Empty sends nothing personal.">
          <input className="inp" value={s.musicbrainz_contact} onChange={(e) => set("musicbrainz_contact", e.target.value)} placeholder="optional" />
        </Row>
      </div>
    </section>
  );
}
