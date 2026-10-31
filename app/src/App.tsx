import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Viz, { LAYERS, decodePattern, decodeCapture, type Pattern, type Capture, type LiveFrame, type VizMode } from "./Viz";
import Settings from "./Settings";
import SongShape from "./SongShape";
import RowMenu from "./RowMenu";
import ColumnPicker, { type ColumnDef } from "./ColumnPicker";
import Lyrics from "./Lyrics";
import Artist from "./Artist";
import * as Ic from "./Icons";

type Track = {
  id: string; title: string; artist: string; album?: string;
  duration_ms: number; tempo?: number; energy?: number; tags: string[];
  meta?: { audio?: { brightness_hz?: number; dynamic_range_db?: number; key?: string | null; moods?: string[] } | null };
};
type Row = Track & { weight: number; plays: number; listens: number; cooling: boolean; favorite: boolean; muted_until?: number | null; blocked: boolean };
type AnalysisS = { batch: number; next_in_secs: number; running: boolean; done: number; total: number; failed: number; current: string; error: string; analyzed: number; eligible: number; pending: number };
type Enrich = { running: boolean; done: number; total: number; current: string; missing: number };
type Gen = {
  intent: { name: string; target_minutes?: number; include_tags: string[]; exclude_tags: string[] };
  tracks: Track[]; total_ms: number; target_ms?: number; matched: number; used_ai: boolean; note: string; tagged: number; library: number;
};
type Counts = { added: number; updated: number; skipped_deleted: number };
type ImportResult = { id: string; tracks: Counts; playlist: { added: number; existing: number; skipped_removed: number } };
const importNote = (r: ImportResult) =>
  `${r.playlist.added} ADDED, ${r.playlist.existing} ALREADY THERE` +
  (r.playlist.skipped_removed ? `, ${r.playlist.skipped_removed} YOU REMOVED EARLIER (SKIPPED)` : "") +
  (r.tracks.skipped_deleted ? `, ${r.tracks.skipped_deleted} DELETED TRACKS (SKIPPED)` : "");
type Poll = { track?: Track; is_playing: boolean; position_ms: number; matches: boolean };
type Next = { track: Track; discovery: boolean };
type Remote = { id: string; name: string; track_count: number };
type Local = { id: string; name: string; remote?: string; count: number };

const MODES = [
  ["Standard", "STD", Ic.Rows],
  ["Shuffle", "SHUFFLE", Ic.Shuffle],
  ["Discovery", "DISCOVER", Ic.Spark],
  ["ShuffleDiscovery", "MIX", Ic.Shuffle],
] as const;
type Mode = (typeof MODES)[number][0];

const SORTS: [string, string][] = [
  ["Added", "Added"], ["Updated", "Updated"], ["Artist", "Artist"], ["Title", "Title"],
  ["TrackNumber", "Track #"], ["Weight", "Weight"], ["Rating", "Rating"], ["Plays", "Plays"], ["Duration", "Length"], ["Tempo", "Tempo"], ["Energy", "Energy"], ["Loudness", "Loudness"],
];

const mmss = (ms: number) => `${Math.floor(ms / 60000)}:${String(Math.floor(ms / 1000) % 60).padStart(2, "0")}`;

/** +/- vote button that floats a +1 / -1 off itself so a click visibly registers. */
function VoteButton({ up, small, onVote, title }: { up: boolean; small?: boolean; onVote: () => void; title: string }) {
  const [bursts, setBursts] = useState<number[]>([]);
  const n = useRef(0);
  const click = (e: React.MouseEvent) => {
    e.stopPropagation();
    const id = ++n.current;
    setBursts((b) => [...b, id]);
    setTimeout(() => setBursts((b) => b.filter((x) => x !== id)), 900);
    onVote();
  };
  return (
    <button className={`btn icon vote ${up ? "good" : "bad"} ${small ? "small" : ""}`} title={title} onClick={click}>
      {up ? <Ic.Plus /> : <Ic.Minus />}
      {bursts.map((id) => <span key={id} className={`burst ${up ? "up" : "down"}`}>{up ? "+1" : "−1"}</span>)}
    </button>
  );
}

function Br({ k, v }: { k: string; v: string | number }) {
  return <span className="br">{k}: <b>{v}</b></span>;
}

/** Optional columns of the track table. TITLE and the action buttons are always shown. */
const COLS: ColumnDef[] = [
  { id: "idx", label: "#", full: "Row number", def: true },
  { id: "artist", label: "ARTIST", full: "Artist", def: true },
  { id: "album", label: "ALBUM", full: "Album", def: false },
  { id: "bpm", label: "BPM", full: "Tempo (BPM)", def: true },
  { id: "key", label: "KEY", full: "Musical key", def: true },
  { id: "energy", label: "NRG", full: "Energy", def: true },
  { id: "mood", label: "MOOD", full: "Mood", def: false },
  { id: "time", label: "TIME", full: "Length", def: true },
  { id: "plays", label: "PLAYS", full: "Plays (started)", def: true },
  { id: "heard", label: "HEARD", full: "Listens (played through)", def: false },
  { id: "weight", label: "WT", full: "Weight (thumbs, favorites)", def: true },
];
const COLS_KEY = "spinamp.columns";
const defaultCols = () => new Set(COLS.filter((c) => c.def).map((c) => c.id));

function useColumns() {
  const [visible, setVisible] = useState<Set<string>>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem(COLS_KEY) ?? "null");
      if (Array.isArray(saved)) return new Set<string>(saved.filter((id: string) => COLS.some((c) => c.id === id)));
    } catch { /* fall through to defaults */ }
    return defaultCols();
  });
  const save = (next: Set<string>) => { setVisible(next); localStorage.setItem(COLS_KEY, JSON.stringify([...next])); };
  return {
    visible,
    toggle: (id: string) => { const n = new Set(visible); n.has(id) ? n.delete(id) : n.add(id); save(n); },
    reset: () => save(defaultCols()),
  };
}

/** "F# minor" -> "F#m", "A major" -> "A": narrow, and what musicians write. */
const shortKey = (k?: string | null) => (k ? k.replace(" major", "").replace(" minor", "m") : "—");

function TrackTable(p: {
  rows: (Track & Partial<Row>)[]; nowId?: string; full?: boolean;
  onPlay?: (id: string) => void; onVote?: (id: string, up: boolean) => void;
  onAdd?: (t: Track) => void; onRemove?: (id: string) => void;
  onTempo?: (id: string, bpm: number | null) => void;
  onFav?: (id: string, fav: boolean) => void; onMute?: (id: string, days: number | null) => void;
  onRemoveAll?: (id: string) => void; onBlock?: (kind: "song" | "artist" | "tag", value: string) => void;
  /** Visible optional columns (full tables only). Other tables show a fixed basic set. */
  cols?: Set<string>;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const max = Math.max(1, ...p.rows.map((r) => Math.abs(r.weight ?? 0)));
  const show = (id: string) => (p.full ? (p.cols ?? defaultCols()).has(id) : ["idx", "artist", "bpm", "time"].includes(id));
  const head = (id: string) => { const c = COLS.find((x) => x.id === id)!; return <th key={id} title={c.full} className={["bpm", "energy", "time", "plays", "heard", "weight"].includes(id) ? "num" : ""}>{c.label}</th>; };
  if (p.rows.length === 0) return <div className="empty">NO TRACKS</div>;
  return (
    <div className="scroll">
      <table className="tracks">
        <thead><tr>
          {show("idx") && head("idx")}<th>TITLE</th>
          {["artist", "album", "bpm", "key", "energy", "mood", "time", "plays", "heard", "weight"].filter(show).map(head)}
          {p.full && <th />}
          {p.onAdd && <th />}
        </tr></thead>
        <tbody>
          {p.rows.slice(0, 1000).map((r, i) => (
            <tr key={r.id + i} className={`${p.nowId === r.id ? "now" : ""} ${r.cooling ? "cool" : ""} ${r.blocked ? "blocked" : ""} ${r.muted_until ? "muted" : ""}`} onClick={() => p.onPlay?.(r.id)}>
              {show("idx") && <td className="dim">{String(i + 1).padStart(2, "0")}</td>}
              <td><span className="ell t">{r.title}</span>{r.blocked && <span className="tag hot" style={{ marginLeft: 8 }}>BLOCKED</span>}{r.muted_until ? <span className="tag" style={{ marginLeft: 8 }}>MUTED {Math.max(1, Math.ceil((r.muted_until * 1000 - Date.now()) / 86_400_000))}D</span> : null}</td>
              {show("artist") && <td className="dim"><span className="ell a">{r.artist}</span></td>}
              {show("album") && <td className="dim"><span className="ell a">{r.album ?? "—"}</span></td>}
              {show("bpm") && (
                <td className="num dim" title={p.onTempo ? "Double-click to set the tempo yourself (empty = back to automatic)" : undefined}
                  onDoubleClick={(e) => { e.stopPropagation(); if (p.onTempo) setEditing(r.id); }} onClick={(e) => editing === r.id && e.stopPropagation()}>
                  {editing === r.id ? (
                    <input className="field" style={{ width: 42, textAlign: "right" }} autoFocus defaultValue={r.tempo ? Math.round(r.tempo) : ""}
                      onKeyDown={(e) => {
                        if (e.key === "Escape") setEditing(null);
                        if (e.key === "Enter") { const v = parseFloat((e.target as HTMLInputElement).value); p.onTempo?.(r.id, Number.isFinite(v) ? v : null); setEditing(null); }
                      }} onBlur={() => setEditing(null)} />
                  ) : r.tempo ? Math.round(r.tempo) : "—"}
                </td>
              )}
              {show("key") && <td className="dim nw" title={r.meta?.audio?.key ?? undefined}>{shortKey(r.meta?.audio?.key)}</td>}
              {show("energy") && <td className="num dim">{r.energy != null
                ? <span className="w"><span className="meter" style={{ width: r.energy * 26, opacity: 0.5 + r.energy * 0.5 }} />{Math.round(r.energy * 100)}</span>
                : "—"}</td>}
              {show("mood") && <td className="dim nw">{r.meta?.audio?.moods?.[0] ?? "—"}</td>}
              {show("time") && <td className="num dim">{mmss(r.duration_ms)}</td>}
              {show("plays") && <td className="num dim">{r.plays ?? 0}</td>}
              {show("heard") && <td className="num dim">{r.listens ?? 0}</td>}
              {show("weight") && <td className="num"><span className="w">
                <span className={`meter ${(r.weight ?? 0) < 0 ? "neg" : ""}`} style={{ width: (Math.abs(r.weight ?? 0) / max) * 30 }} />
                {(r.weight ?? 0) >= 0 ? "+" : ""}{(r.weight ?? 0).toFixed(1)}
              </span></td>}
              {p.full && (
                <td onClick={(e) => e.stopPropagation()}><span className="rowact">
                  <VoteButton small up title="Plus: like this more" onVote={() => p.onVote?.(r.id, true)} />
                  <VoteButton small up={false} title="Minus: like this less" onVote={() => p.onVote?.(r.id, false)} />
                  <button className={`btn small icon star ${r.favorite ? "on" : ""}`} title={r.favorite ? "Unfavorite" : "Favorite: plays more often and teaches your taste"} onClick={() => p.onFav?.(r.id, !r.favorite)}><Ic.Star on={!!r.favorite} /></button>
                  <RowMenu small track={{ id: r.id, title: r.title, artist: r.artist, tags: r.tags }} state={r} inPlaylist={!!p.onRemove}
                    onMute={(d) => p.onMute?.(r.id, d)} onRemoveHere={() => p.onRemove?.(r.id)} onRemoveAll={() => p.onRemoveAll?.(r.id)} onBlock={(k, v) => p.onBlock?.(k, v)} />
                </span></td>
              )}
              {p.onAdd && <td onClick={(e) => e.stopPropagation()}><span className="rowact" style={{ opacity: 1 }}>
                <button className="btn small icon good" title="Add to selected playlist" onClick={() => p.onAdd?.(r)}><Ic.Plus /></button>
              </span></td>}
            </tr>
          ))}
        </tbody>
      </table>
      {p.rows.length > 1000 && <div className="empty">SHOWING 1000 OF {p.rows.length}</div>}
    </div>
  );
}

export default function App() {
  const columns = useColumns();
  const [rows, setRows] = useState<Row[]>([]);
  const [mode, setMode] = useState<Mode>("ShuffleDiscovery");
  const [now, setNow] = useState<Next | null>(null);
  const [playing, setPlaying] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [sortKey, setSortKey] = useState("Added");
  const [desc, setDesc] = useState(false);

  const [tab, setTab] = useState<"library" | "spotify" | "search" | "generate">("library");
  const [view, setView] = useState<string | null | undefined>(undefined); // undefined = playlist list
  const [locals, setLocals] = useState<Local[]>([]);
  const [allCount, setAllCount] = useState(0);
  const [remotes, setRemotes] = useState<Remote[] | null>(null);
  const [preview, setPreview] = useState<{ remote: Remote; tracks: Track[] } | null>(null);
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  const [screen, setScreen] = useState<"main" | "settings" | "lyrics" | "artist">("main");
  const [pool, setPool] = useState<{ running: boolean; pool: number } | null>(null);
  const [pattern, setPattern] = useState<Pattern | null>(null);
  const [capture, setCapture] = useState<Capture | null>(null);
  const [vizMode, setVizMode] = useState<VizMode>("synth");
  const [vizLed, setVizLed] = useState(true);
  const [askCapture, setAskCapture] = useState(false);
  const [shareFp, setShareFp] = useState(false);
  const liveRef = useRef<LiveFrame | null>(null);
  const [vizOffset, setVizOffset] = useState(0);
  const [analysis, setAnalysis] = useState<AnalysisS | null>(null);
  const [enrich, setEnrich] = useState<Enrich | null>(null);
  const [aiOn, setAiOn] = useState(false);
  const [likesCount, setLikesCount] = useState(true);
  const [prompt, setPrompt] = useState("");
  const [genMin, setGenMin] = useState("");
  const [gen, setGen] = useState<Gen | null>(null);
  const [q, setQ] = useState("");
  const [results, setResults] = useState<Track[] | null>(null);
  const [target, setTarget] = useState("");
  const [filter, setFilter] = useState("");
  const [naming, setNaming] = useState<string | null>(null);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try { setMsg(""); await f(); } catch (e) { setMsg("ERROR: " + String(e)); } finally { setBusy(false); }
  };
  const refresh = async () => {
    setLocals(await invoke<Local[]>("local_playlists"));
    setAllCount(await invoke<number>("library_count"));
    setRows(await invoke<Row[]>("library"));
  };

  useEffect(() => {
    refresh();
    invoke<Remote[]>("provider_playlists", { id: "spotify" }).then(setRemotes).catch(() => setRemotes([]));
  }, []);

  /* ----- visualiser pattern for the playing track ----- */
  useEffect(() => {
    setPattern(null);
    if (!now?.track.id) return;
    invoke<Parameters<typeof decodePattern>[0]>("viz_pattern", { trackId: now.track.id }).then((r) => setPattern(decodePattern(r))).catch(() => setPattern(null));
  }, [now?.track.id, analysis?.analyzed]);
  useEffect(() => { invoke<{ settings: { viz_offset_ms: number; viz_led: boolean } }>("get_settings").then((c) => { setVizOffset(c.settings.viz_offset_ms); setVizLed(c.settings.viz_led); }); }, [screen]);
  // first run: ask once whether Spinamp may listen to Spotify to learn each song's shape
  useEffect(() => {
    (async () => {
      const [cfg, cap] = await Promise.all([
        invoke<{ settings: { capture_prompted: boolean } }>("get_settings"),
        invoke<{ supported: boolean }>("capture_status"),
      ]);
      if (cap.supported && !cfg.settings.capture_prompted) setAskCapture(true);
    })().catch(() => {});
  }, []);
  const answerCapture = async (enable: boolean) => {
    setAskCapture(false);
    await invoke("capture_answer", { enable, share: enable && shareFp });
    if (enable) {
      try { setMsg(await invoke<string>("capture_check")); } catch (e) { setMsg("ERROR: " + String(e)); }
    }
  };
  // real spectrum streamed from the backend while live capture is running
  useEffect(() => {
    const un = listen<{ track_id: string; bands: number[] }>("viz-live", (e) => {
      liveRef.current = { trackId: e.payload.track_id, bands: e.payload.bands, at: performance.now() };
    });
    return () => { un.then((f) => f()); };
  }, []);
  // fingerprint recorded on an earlier live listen (reloaded when the track changes or a listen ends)
  useEffect(() => {
    setCapture(null);
    if (!now?.track.id) return;
    invoke<Parameters<typeof decodeCapture>[0]>("viz_capture", { trackId: now.track.id }).then((r) => setCapture(decodeCapture(r))).catch(() => setCapture(null));
  }, [now?.track.id, playing]);
  // while a track is being recorded live, refresh the song shape as it fills in
  useEffect(() => {
    if (vizMode !== "live" || !playing || !now?.track.id) return;
    const id = setInterval(() => {
      invoke<Parameters<typeof decodeCapture>[0]>("viz_capture", { trackId: now.track.id }).then((r) => setCapture(decodeCapture(r))).catch(() => {});
    }, 5000);
    return () => clearInterval(id);
  }, [vizMode, playing, now?.track.id]);

  /* ----- audio analysis (tempo / energy / loudness from previews) ----- */
  const pollAnalysis = async () => setAnalysis(await invoke<AnalysisS>("analysis_status"));
  const startAnalysis = async () => { await invoke("analysis_start", { all: false }); pollAnalysis(); };
  useEffect(() => {
    (async () => {
      const st = await invoke<AnalysisS>("analysis_status");
      setAnalysis(st);
      const cfg = await invoke<{ settings: { analyze_auto: boolean } }>("get_settings");
      if (cfg.settings.analyze_auto && st.pending > 0 && !st.running) startAnalysis();
    })();
  }, []);
  useEffect(() => {
    if (!analysis?.running) return;
    const id = setInterval(async () => {
      const st = await invoke<AnalysisS>("analysis_status");
      setAnalysis(st);
      if (!st.running) refresh(); // pick up the measurements
    }, 3000);
    return () => clearInterval(id);
  }, [analysis?.running]);

  /* ----- tag enrichment (MusicBrainz) ----- */
  const pollEnrich = async () => setEnrich(await invoke<Enrich>("enrich_status"));
  const startEnrich = async () => { await invoke("enrich_start"); pollEnrich(); };
  useEffect(() => {
    invoke("catalog_refill", { force: false }).catch(() => {});
    const id = setInterval(() => invoke<{ running: boolean; pool: number }>("catalog_status").then(setPool).catch(() => {}), 5000);
    invoke<{ running: boolean; pool: number }>("catalog_status").then(setPool).catch(() => {});
    return () => clearInterval(id);
  }, []);
  useEffect(() => {
    invoke<boolean>("ai_enabled").then(setAiOn);
    invoke<Enrich>("enrich_status").then((e) => { setEnrich(e); if (e.missing > 0 && !e.running) startEnrich(); });
  }, []);
  useEffect(() => {
    if (!enrich?.running) return;
    const id = setInterval(async () => {
      const e = await invoke<Enrich>("enrich_status");
      setEnrich(e);
      if (!e.running) refresh(); // pick up the new tags
    }, 3000);
    return () => clearInterval(id);
  }, [enrich?.running]);

  /* ----- playback ----- */
  const start = (n: Next | null) => {
    if (!n) return;
    setNow(n); setElapsed(0); setPlaying(true);
    refresh();
  };
  const skip = useCallback(async () => {
    try { start(await invoke<Next | null>("next_track", { mode })); } catch (e) { setMsg("ERROR: " + String(e)); }
  }, [mode]);
  const playId = async (id: string) => {
    try { start(await invoke<Next | null>("play_track", { id })); } catch (e) { setMsg("ERROR: " + String(e)); }
  };
  const toggle = async () => {
    if (!now) return skip();
    try {
      await invoke(playing ? "playback_pause" : "playback_resume");
      setPlaying(!playing);
    } catch (e) { setMsg("ERROR: " + String(e)); }
  };

  // sync with the real player every 2s; this is also what records listening time
  const nowRef = useRef(now);
  nowRef.current = now;
  useEffect(() => {
    invoke("set_tz_offset", { minutesWest: new Date().getTimezoneOffset() });
    const id = setInterval(async () => {
      try {
        const r = await invoke<Poll>("poll_playback");
        if (!r.matches) return;
        if (r.track && nowRef.current?.track.id !== r.track.id) { setNow({ track: r.track, discovery: false }); refresh(); }
        setElapsed(r.position_ms);
        setPlaying(r.is_playing);
      } catch { /* offline or not connected: keep the local clock */ }
    }, 2000);
    return () => clearInterval(id);
  }, []);

  // progress clock between polls; advance when the track ends
  const skipRef = useRef(skip);
  skipRef.current = skip;
  useEffect(() => {
    if (!playing || !now) return;
    const id = setInterval(() => {
      setElapsed((e) => {
        if (e + 250 >= now.track.duration_ms - 1200) { setTimeout(() => skipRef.current(), 0); return 0; }
        return e + 250;
      });
    }, 250);
    return () => clearInterval(id);
  }, [playing, now]);

  const vote = async (id: string, up: boolean) => setRows(await invoke<Row[]>("vote", { id, up }));
  // Works for discovery tracks too: the vote adds the track to the library so it is remembered.
  const voteTrack = async (up: boolean) => {
    if (!cur) return;
    setRows(await invoke<Row[]>("vote_track", { track: cur, up }));
    setAllCount(await invoke<number>("library_count"));
  };
  const sort = async (key: string, d: boolean) => {
    setSortKey(key); setDesc(d);
    setRows(await invoke<Row[]>("sort_library", { key, descending: d }));
  };

  /* ----- library / spotify navigation ----- */
  const openList = async (id: string | null) => {
    setRows(await invoke<Row[]>("select_playlist", { id }));
    setFilter("");
    setView(id);
  };
  const backToLists = async () => { setView(undefined); setPreview(null); setMsg(""); };
  const connect = () => run(async () => {
    setMsg("OPENING BROWSER...");
    await invoke("provider_connect", { id: "spotify" });
    setRemotes(await invoke<Remote[]>("provider_playlists", { id: "spotify" }));
    setMsg("");
  });
  const openRemote = (r: Remote) => run(async () => {
    setMsg(`LOADING ${r.name}...`);
    setPreview({ remote: r, tracks: await invoke<Track[]>("provider_preview", { id: "spotify", playlistId: r.id }) });
    setMsg("");
  });
  const importPlaylist = () => run(async () => {
    if (!preview) return;
    const r = await invoke<ImportResult>("import_playlist", { name: preview.remote.name, remoteId: preview.remote.id, tracks: preview.tracks });
    const name = preview.remote.name;
    setPreview(null);
    await refresh();
    setTab("library");
    await openList(r.id);
    setMsg(`${name.toUpperCase()}: ${importNote(r)}`);
  });
  const importTracks = () => run(async () => {
    if (!preview) return;
    const c = await invoke<Counts>("import_tracks", { tracks: preview.tracks });
    setPreview(null);
    await refresh();
    setMsg(`LIBRARY: ${c.added} ADDED, ${c.updated} UPDATED` + (c.skipped_deleted ? `, ${c.skipped_deleted} DELETED EARLIER (SKIPPED)` : ""));
  });
  const viaRows = async (cmd: string, args: Record<string, unknown>) => {
    try { setRows(await invoke<Row[]>(cmd, args)); } catch (e) { setMsg("ERROR: " + String(e)); }
  };
  const fav = (id: string, favorite: boolean) => viaRows("set_favorite", { trackId: id, favorite });
  const mute = async (id: string, days: number | null) => { await viaRows("mute_track", { trackId: id, days }); setMsg(days ? `MUTED FOR ${days} DAY${days > 1 ? "S" : ""}` : "UNMUTED"); };
  const removeAll = async (id: string) => {
    await viaRows("remove_track_everywhere", { trackId: id });
    setAllCount(await invoke<number>("library_count")); setLocals(await invoke<Local[]>("local_playlists"));
    setMsg("REMOVED FROM LIBRARY (SETTINGS > DATA CAN RESTORE IT)");
  };
  const block = async (kind: "song" | "artist" | "tag", value: string) => {
    await viaRows("add_block", { kind, value });
    setMsg(`BLOCKED ${kind === "tag" ? "TAG / GENRE" : kind.toUpperCase()}${kind === "song" ? "" : ": " + value.toUpperCase()}. MANAGE IN SETTINGS.`);
  };
  const removePlaylist = (id: string) => run(async () => {
    await invoke("delete_playlist", { id });
    await refresh();
    await backToLists();
  });

  useEffect(() => { if (!target && locals.length) setTarget(locals[0].id); }, [locals, target]);
  const targetName = locals.find((l) => l.id === target)?.name ?? "";
  const doSearch = () => run(async () => {
    if (!q.trim()) return;
    setMsg("SEARCHING...");
    const r = await invoke<Track[]>("search", { query: q });
    setResults(r);
    setMsg("");
  });
  const addTo = (t: Track) => run(async () => {
    if (!target) { setMsg("ERROR: CREATE A PLAYLIST FIRST (LIBRARY > + NEW)"); return; }
    await invoke("add_to_playlist", { playlistId: target, tracks: [t] });
    await refresh();
    setMsg(`ADDED "${t.title}" TO ${targetName.toUpperCase()}`);
  });
  const createPlaylist = () => run(async () => {
    const name = naming?.trim();
    if (!name) { setNaming(null); return; }
    const id = await invoke<string>("create_playlist", { name });
    setNaming(null);
    await refresh();
    setTarget(id);
    setMsg(`CREATED ${name.toUpperCase()}`);
  });
  const removeFrom = (trackId: string) => run(async () => {
    setRows(await invoke<Row[]>("remove_from_playlist", { playlistId: view, trackId }));
    setLocals(await invoke<Local[]>("local_playlists"));
  });
  const doGenerate = () => run(async () => {
    if (!prompt.trim()) return;
    setMsg("GENERATING...");
    const m = parseFloat(genMin);
    setGen(await invoke<Gen>("generate_playlist", { prompt, minutes: Number.isFinite(m) && m > 0 ? m : null }));
    setMsg("");
  });
  const saveGenerated = () => run(async () => {
    if (!gen) return;
    const r = await invoke<ImportResult>("import_playlist", { name: gen.intent.name || "Generated", remoteId: null, tracks: gen.tracks });
    const n = gen.tracks.length, name = gen.intent.name;
    setGen(null);
    await refresh();
    setTab("library");
    await openList(r.id);
    setMsg(`SAVED PLAYLIST ${name.toUpperCase()} (${n} TRACKS)`);
  });
  const shownRows = rows.filter((r) => !filter || `${r.title} ${r.artist}`.toLowerCase().includes(filter.toLowerCase()));

  const cur = now?.track;
  const inTracks = tab === "library" && view !== undefined;
  const playlistName = view === null ? "ALL TRACKS" : locals.find((l) => l.id === view)?.name ?? "";
  const importedRemotes = new Set(locals.map((l) => l.remote).filter(Boolean));
  const title = cur ? `${cur.title} — ${cur.artist}` : "NOTHING PLAYING";

  return (
    <div className="app">
      <div className="topbar" data-tauri-drag-region>
        <span className="logo" data-tauri-drag-region>SPINAMP</span>
        <span className="segs" data-tauri-drag-region>{Array.from({ length: 9 }, (_, i) => <i key={i} className={i % 3 === 0 ? "on" : ""} />)}</span>
        {analysis?.running ? (
          <button className="btn small" onClick={() => invoke("analysis_stop")} title={`Measuring: ${analysis.current}. Click to stop.${analysis.error ? " " + analysis.error : ""}`}>
            {analysis.next_in_secs > 0 ? `NEXT BATCH IN ${Math.ceil(analysis.next_in_secs / 60)}M` : `ANALYZING ${analysis.done}/${analysis.total}`} ✕
          </button>
        ) : analysis && analysis.pending > 0 ? (
          <button className="btn small" onClick={startAnalysis} title="Downloads a 30s preview per track (~0.5 MB each), measures tempo, energy and loudness locally, and keeps only the numbers.">
            ANALYZE {analysis.batch > 0 ? Math.min(analysis.batch, analysis.pending) : analysis.pending} OF {analysis.pending}
          </button>
        ) : null}
        {enrich?.running ? (
          <button className="btn small" onClick={() => invoke("enrich_stop")} title={`Looking up tags: ${enrich.current}. Click to stop.`}>
            TAGGING {enrich.done}/{enrich.total} ✕
          </button>
        ) : enrich && enrich.missing > 0 ? (
          <button className="btn small" onClick={startEnrich}>TAG {enrich.missing} ARTISTS</button>
        ) : null}
        {pool && (mode === "Discovery" || mode === "ShuffleDiscovery") && <Br k={pool.running ? "FINDING" : "POOL"} v={pool.pool} />}
        <Br k="LIB" v={allCount} />
        <button className="btn small icon" title="Settings" onClick={() => setScreen(screen === "settings" ? "main" : "settings")}><Ic.Gear /></button>
      </div>

      {askCapture && (
        <div className="ask">
          <div>
            <b>LET SPINAMP LISTEN TO SPOTIFY?</b>
            <div className="dim" style={{ marginTop: 4, textTransform: "none", lineHeight: 1.5 }}>
              While you listen, Spinamp can hear the Spotify desktop app (only Spotify) and remember each song's shape, so the visualiser and the
              seek bar match the real music from then on. Audio is turned into a few numbers and thrown away immediately. macOS will ask for
              Screen &amp; System Audio Recording permission. You can change this any time in Settings.
            </div>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap", justifyContent: "flex-end" }}>
            <label className="pill" style={{ height: 36, fontSize: 11, maxWidth: 250, textTransform: "none", whiteSpace: "normal", lineHeight: 1.25 }} title="Nothing is sent yet. This records your choice for when sharing launches at spinamp.fm.">
              <input type="checkbox" checked={shareFp} onChange={(e) => setShareFp(e.target.checked)} style={{ accentColor: "var(--orange)" }} /> Also share song fingerprints with spinamp.fm when it launches (anonymous)
            </label>
            <button className="btn primary" style={{ width: "auto", padding: "0 14px", height: 36 }} onClick={() => answerCapture(true)}>TURN ON</button>
            <button className="btn" onClick={() => answerCapture(false)}>NOT NOW</button>
          </div>
        </div>
      )}

      {/* ---- player ---- */}
      {screen !== "settings" && <section className="player">
        <div className="vizwrap">
          <Viz playing={playing} trackId={cur?.id ?? "idle"} tempo={cur?.tempo} energy={cur?.energy} durationMs={cur?.duration_ms} tags={cur?.tags} brightnessHz={cur?.meta?.audio?.brightness_hz} dynamicRangeDb={cur?.meta?.audio?.dynamic_range_db} positionMs={elapsed} pattern={pattern} capture={capture} live={liveRef} offsetMs={vizOffset} led={vizLed} onMode={setVizMode} />
          <div className="vizlegend">
            {LAYERS.map((l) => <span className="br" key={l.name}><i style={{ background: l.color }} />{l.name}</span>)}
            <span style={{ flex: 1 }} />
            {cur?.tempo ? <span className="br">BPM: <b>{Math.round(cur.tempo)}</b></span> : null}
            {pattern ? <span className="br">TIME: <b>{pattern.beats_per_bar}/4</b></span> : null}
            {cur ? <span className="dim" style={{ fontSize: 10 }}>{({ live: "LIVE: REAL SPECTRUM FROM SPOTIFY", recorded: "RECORDED FROM AN EARLIER LISTEN", preview: "ANALYSED PREVIEW + PROXY ARRANGEMENT", synth: "SYNTHESISED FROM TEMPO + GENRE" } as Record<VizMode, string>)[vizMode]}</span> : null}
          </div>
        </div>
        <div className="display">
          <div className="clock">
            <div className="t">{mmss(elapsed)}</div>
            <div className={`s ${playing ? "live" : ""}`}>{playing ? "▶ PLAYING" : cur ? "❚❚ PAUSED" : "■ STOPPED"}</div>
          </div>
          <div className="info">
            <div className={`marq ${title.length > 34 ? "scroll" : ""}`}><span>{title}</span></div>
            <div className="chips">
              {now && <span className="br"><b style={{ color: now.discovery ? "var(--red)" : "var(--yellow)" }}>{now.discovery ? "DISCOVERY" : "LIBRARY"}</b></span>}
              {cur?.tempo && <Br k="BPM" v={Math.round(cur.tempo)} />}
              {cur?.meta?.audio?.key && <Br k="KEY" v={cur.meta.audio.key.toUpperCase()} />}
              {cur?.meta?.audio?.moods?.slice(0, 2).map((m) => <span className="tag hot" key={m}>{m}</span>)}
              {cur?.tags.slice(0, 2).map((t) => <span className="tag" key={t}>{t}</span>)}
            </div>
          </div>
        </div>
        {cur ? (
          <SongShape trackId={cur.id} durationMs={cur.duration_ms} positionMs={elapsed} playing={playing} tempo={cur.tempo} tags={cur.tags}
            dynamicRangeDb={cur.meta?.audio?.dynamic_range_db} pattern={pattern} capture={capture} live={liveRef}
            onSeek={async (ms) => { setElapsed(ms); try { await invoke("playback_seek", { positionMs: ms }); } catch (e) { setMsg("ERROR: " + String(e)); } }} />
        ) : <div className="seek" />}
        <div className="deck">
          <button className="btn primary" onClick={toggle} title={playing ? "Pause" : "Play"}>{playing ? <Ic.Pause /> : <Ic.Play />}</button>
          <button className="btn icon" onClick={skip} title="Skip"><Ic.Next /></button>
          <VoteButton up title="Plus: like this more" onVote={() => voteTrack(true)} />
          <VoteButton up={false} title="Minus: like this less, and skip" onVote={() => { voteTrack(false); setTimeout(skip, 350); }} />
          {cur && (() => {
            const row = rows.find((r) => r.id === cur.id);
            return (
              <>
                <button className={`btn icon star ${row?.favorite ? "on" : ""}`} title="Favorite" disabled={!row} onClick={() => fav(cur.id, !row?.favorite)}><Ic.Star on={!!row?.favorite} /></button>
                <RowMenu track={{ id: cur.id, title: cur.title, artist: cur.artist, tags: cur.tags }} state={row ?? {}} inPlaylist={view !== null && view !== undefined}
                  onMute={(d) => mute(cur.id, d)} onRemoveHere={() => removeFrom(cur.id)} onRemoveAll={() => removeAll(cur.id)} onBlock={block} />
              </>
            );
          })()}
          <button className={`btn ${screen === "lyrics" ? "on" : ""}`} disabled={!cur} onClick={() => setScreen(screen === "lyrics" ? "main" : "lyrics")}><Ic.Lyr /> LYRICS</button>
          <button className={`btn ${screen === "artist" ? "on" : ""}`} disabled={!cur} onClick={() => setScreen(screen === "artist" ? "main" : "artist")}><Ic.User /> ARTIST</button>
          <span className="sp" />
          <div className="seg">
            {MODES.map(([k, label, Icon]) => (
              <button key={k} className={mode === k ? "on" : ""} onClick={() => setMode(k)} title={k}><Icon />{label}</button>
            ))}
          </div>
        </div>
      </section>}

      {screen === "settings" && <Settings onClose={() => { setScreen("main"); invoke<boolean>("ai_enabled").then(setAiOn); }} />}

      {screen === "lyrics" && cur && <Lyrics track={cur} elapsed={elapsed} onBack={() => setScreen("main")} />}
      {screen === "artist" && cur && <Artist name={cur.artist.split(", ")[0]} onBack={() => setScreen("main")} />}

      {/* ---- playlist window ---- */}
      {screen === "main" && <section className="win">
        <div className="winhead">
          {(inTracks || preview) && <button className="btn" onClick={backToLists}><Ic.Back /> BACK</button>}
          <span className="crumb">
            {preview ? preview.remote.name.toUpperCase() : inTracks ? playlistName.toUpperCase() : tab === "library" ? "PLAYLISTS" : tab === "search" ? "SEARCH" : tab === "generate" ? "GENERATE" : "SPOTIFY"}
          </span>
          <span className="sp" style={{ flex: 1 }} />

          {tab === "library" && view === undefined && (naming === null
            ? <button className="btn" onClick={() => setNaming("")}><Ic.Plus /> NEW</button>
            : <label className="pill"><input autoFocus className="field" placeholder="Playlist name" value={naming}
                onChange={(e) => setNaming(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") createPlaylist(); if (e.key === "Escape") setNaming(null); }} /></label>)}
          {!inTracks && !preview && (
            <div className="seg">
              <button className={tab === "library" ? "on" : ""} onClick={() => setTab("library")}>LIBRARY</button>
              <button className={tab === "spotify" ? "on" : ""} onClick={() => setTab("spotify")}>SPOTIFY</button>
              <button className={tab === "search" ? "on" : ""} onClick={() => setTab("search")}>SEARCH</button>
              <button className={tab === "generate" ? "on" : ""} onClick={() => setTab("generate")}>GENERATE</button>
            </div>
          )}
          {inTracks && <>
            <label className="pill"><input className="field" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} /></label>
            <label className="pill">Sort:
              <select value={sortKey} onChange={(e) => sort(e.target.value, desc)}>
                {SORTS.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
              </select><Ic.Chevron />
            </label>
            <button className="btn icon" title={desc ? "Descending" : "Ascending"} onClick={() => sort(sortKey, !desc)}><Ic.Dir desc={desc} /></button>
            <ColumnPicker cols={COLS} visible={columns.visible} onToggle={columns.toggle} onReset={columns.reset} />
            {view && <button className="btn bad" onClick={() => removePlaylist(view)}>DELETE</button>}
          </>}
          {preview && <>
            <button className="btn primary" style={{ width: "auto", padding: "0 14px", height: 36 }} onClick={importPlaylist}>IMPORT PLAYLIST + TRACKS</button>
            <button className="btn" onClick={importTracks}>TRACKS ONLY</button>
          </>}
        </div>

        {(msg || busy) && <div className={`status ${msg.startsWith("ERROR") ? "err" : ""}`}>{msg || "WORKING..."}</div>}

        {/* library: playlists table */}
        {tab === "library" && view === undefined && (
          <div className="scroll"><table>
            <thead><tr><th>#</th><th>PLAYLIST</th><th>SOURCE</th><th className="num">TRACKS</th></tr></thead>
            <tbody>
              <tr onClick={() => openList(null)}><td className="dim">00</td><td>ALL TRACKS</td><td><span className="tag">LIBRARY</span></td><td className="num">{allCount}</td></tr>
              {locals.map((l, i) => (
                <tr key={l.id} onClick={() => openList(l.id)}>
                  <td className="dim">{String(i + 1).padStart(2, "0")}</td>
                  <td>{l.name}</td>
                  <td><span className={`tag ${l.remote ? "hot" : ""}`}>{l.remote ? "SPOTIFY" : l.name === "Demo" ? "DEMO" : "LOCAL"}</span></td>
                  <td className="num">{l.count}</td>
                </tr>
              ))}
            </tbody>
          </table></div>
        )}

        {/* library: a playlist's tracks */}
        {inTracks && <TrackTable full cols={columns.visible} rows={shownRows} nowId={cur?.id} onPlay={playId} onVote={vote} onFav={fav} onMute={mute} onRemoveAll={removeAll} onBlock={block} onTempo={async (id, bpm) => { try { setRows(await invoke<Row[]>("set_tempo", { trackId: id, bpm })); } catch (e) { setMsg("ERROR: " + String(e)); } }} onRemove={view ? removeFrom : undefined} />}

        {/* generate: prompt -> playlist */}
        {tab === "generate" && (
          <>
            <div className="winhead">
              <label className="pill" style={{ flex: 1 }}><input autoFocus className="field" style={{ width: "100%" }}
                placeholder='Describe it: "45 minute chill commute"' value={prompt}
                onChange={(e) => setPrompt(e.target.value)} onKeyDown={(e) => e.key === "Enter" && doGenerate()} /></label>
              <label className="pill">Min:<input className="field" style={{ width: 44 }} placeholder="auto" value={genMin}
                onChange={(e) => setGenMin(e.target.value)} onKeyDown={(e) => e.key === "Enter" && doGenerate()} /></label>
              <button className="btn primary" style={{ width: "auto", padding: "0 14px", height: 36 }} onClick={doGenerate}><Ic.Spark /> GENERATE</button>
            </div>
            <div className="status" style={{ color: "var(--dim)" }}>
              AI: {aiOn ? "ON (CLAUDE READS YOUR PROMPT + TAG NAMES ONLY)" : "OFF (KEYWORD MATCHING; ADD A KEY IN SETTINGS)"}
            </div>
            {gen ? (
              <>
                <div className="winhead">
                  <b>{gen.intent.name.toUpperCase()}</b>
                  <Br k="TRACKS" v={gen.tracks.length} />
                  <Br k="TIME" v={`${mmss(gen.total_ms)}${gen.target_ms ? ` / ${mmss(gen.target_ms)}` : ""}`} />
                  <Br k="TAG MATCH" v={`${gen.matched}/${gen.tracks.length}`} />
                  <span style={{ flex: 1 }} />
                  <button className="btn" onClick={doGenerate}><Ic.Shuffle /> REROLL</button>
                  <button className="btn primary" style={{ width: "auto", padding: "0 14px", height: 36 }} onClick={saveGenerated}>SAVE PLAYLIST</button>
                </div>
                {(gen.intent.include_tags.length > 0 || gen.note) && (
                  <div className="status" style={{ color: "var(--dim)" }}>
                    {gen.intent.include_tags.length > 0 && <>TAGS: {gen.intent.include_tags.join(" · ")}. </>}{gen.note}
                  </div>
                )}
                {gen.tracks.length === 0 ? <div className="empty">NOTHING FIT. TRY FEWER CONSTRAINTS.</div> : <TrackTable rows={gen.tracks} />}
              </>
            ) : <div className="empty">DESCRIBE A MOOD, ACTIVITY OR LENGTH.<br />{gen === null && enrich && enrich.total > 0 ? "MORE TAGS = BETTER MATCHES; TAGGING RUNS IN THE BACKGROUND." : ""}</div>}
          </>
        )}

        {/* search: remote results */}
        {tab === "search" && (
          <>
            <div className="winhead">
              <label className="pill" style={{ flex: 1 }}><input autoFocus className="field" style={{ width: "100%" }} placeholder="Search Spotify (Enter)"
                value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && doSearch()} /></label>
              <button className="btn" onClick={doSearch}>SEARCH</button>
              <label className="pill">Add to:
                <select value={target} onChange={(e) => setTarget(e.target.value)}>
                  {locals.length === 0 && <option value="">(no playlists)</option>}
                  {locals.map((l) => <option key={l.id} value={l.id}>{l.name}</option>)}
                </select><Ic.Chevron />
              </label>
            </div>
            {results === null ? <div className="empty">SEARCH SPOTIFY, THEN ＋ TO ADD A TRACK TO "{targetName.toUpperCase() || "A PLAYLIST"}"</div>
              : results.length === 0 ? <div className="empty">NO RESULTS (OR NOT CONNECTED TO SPOTIFY)</div>
              : <TrackTable rows={results} onAdd={addTo} />}
          </>
        )}

        {/* spotify: import your listening history */}
        {tab === "spotify" && !preview && remotes && remotes.length > 0 && (
          <div className="winhead">
            <span className="label" style={{ margin: 0 }}>IMPORT</span>
            {([["liked", "LIKED SONGS"], ["top", "TOP TRACKS"], ["recent", "RECENT"], ["artists", "TOP ARTISTS"], ["all", "EVERYTHING"]] as const).map(([k, label]) => (
              <button key={k} className="btn small" disabled={busy} onClick={() => run(async () => {
                setMsg(`IMPORTING ${label}...`);
                const r = await invoke<string>("spotify_import_history", { kind: k, votes: likesCount });
                await refresh();
                setMsg("IMPORTED: " + r.toUpperCase());
              })}>{label}</button>
            ))}
            <label className="pill" style={{ height: 26, fontSize: 11 }}>
              <input type="checkbox" checked={likesCount} onChange={(e) => setLikesCount(e.target.checked)} style={{ accentColor: "var(--orange)" }} /> LIKED = +1
            </label>
          </div>
        )}

        {/* spotify: remote playlists table */}
        {tab === "spotify" && !preview && (
          remotes && remotes.length > 0 ? (
            <div className="scroll"><table>
              <thead><tr><th>#</th><th>PLAYLIST</th><th>STATUS</th><th className="num">TRACKS</th></tr></thead>
              <tbody>
                {remotes.map((r, i) => (
                  <tr key={r.id} onClick={() => openRemote(r)}>
                    <td className="dim">{String(i + 1).padStart(2, "0")}</td>
                    <td>{r.name}</td>
                    <td>{importedRemotes.has(r.id) ? <span className="tag hot">IMPORTED</span> : <span className="tag">REMOTE</span>}</td>
                    <td className="num">{r.track_count}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="empty"><button className="btn" onClick={connect}><Ic.Plug /> RECONNECT</button></div></div>
          ) : (
            <div className="empty">
              {remotes === null ? "LOADING..." : "NOT CONNECTED TO SPOTIFY"}<br />
              <button className="btn primary" style={{ width: "auto", padding: "0 16px", marginTop: 10 }} onClick={connect}><Ic.Plug /> CONNECT</button>
            </div>
          )
        )}

        {/* spotify: preview before import */}
        {tab === "spotify" && preview && (
          <>
            <div className="status" style={{ color: "var(--dim)" }}>PREVIEW · {preview.tracks.length} TRACKS · NOTHING SAVED YET</div>
            {preview.tracks.length === 0
              ? <div className="empty">NO TRACKS RETURNED.<br />SPOTIFY ONLY SHARES TRACKS FOR PLAYLISTS YOU OWN OR COLLABORATE ON.</div>
              : <TrackTable rows={preview.tracks} />}
          </>
        )}
      </section>}
      <div className="foot">PLAYBACK RUNS ON YOUR ACTIVE SPOTIFY DEVICE · SPOTIFY PREMIUM REQUIRED · LIVE CAPTURE IS OPTIONAL AND ONLY HEARS THE SPOTIFY APP</div>
    </div>
  );
}
