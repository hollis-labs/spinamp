import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import * as Ic from "./Icons";

type Track = { id: string; title: string; artist: string; album?: string; duration_ms: number };
type L = { plain?: string; synced: [number, string][]; instrumental: boolean; source?: string };

export default function Lyrics({ track, elapsed, onBack }: { track: Track; elapsed: number; onBack: () => void }) {
  const [l, setL] = useState<L | null | undefined>(undefined); // undefined = loading, null = none
  const [err, setErr] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const cur = useRef<HTMLDivElement>(null);

  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    setL(undefined); setErr("");
    invoke<L | null>("lyrics", { track }).then(setL).catch((e) => { setErr(String(e)); setL(null); });
  }, [track.id, attempt]);

  // current synced line: last one whose timestamp has passed (small lead so it lights up on time)
  const lines = l?.synced ?? [];
  let idx = -1;
  for (let i = 0; i < lines.length; i++) { if (lines[i][0] <= elapsed + 300) idx = i; else break; }

  useEffect(() => {
    const b = box.current, c = cur.current;
    if (b && c) b.scrollTo({ top: c.offsetTop - b.clientHeight / 2 + c.clientHeight / 2, behavior: "smooth" });
  }, [idx]);

  return (
    <section className="win">
      <div className="winhead">
        <button className="btn icon" title="Back" onClick={onBack}><Ic.Back /></button>
        <span className="crumb">LYRICS</span>
        <span className="dim">{track.title} — {track.artist}</span>
        <span style={{ flex: 1 }} />
        {lines.length > 0 && <span className="tag hot">SYNCED</span>}
        {l && lines.length === 0 && l.plain && <span className="tag">PLAIN</span>}
        {l?.source && <span className="tag">{l.source.toUpperCase()}</span>}
      </div>
      <div className="scroll lyr" ref={box}>
        {l === undefined && <div className="empty">LOOKING UP LYRICS...</div>}
        {l === null && (
          <div className="empty">
            {err ? `ERROR: ${err}` : "NO LYRICS FOUND FOR THIS TRACK."}<br />LRCLIB + LYRICS.OVH<br />
            {err && <button className="btn" style={{ marginTop: 12 }} onClick={() => setAttempt(attempt + 1)}>RETRY</button>}
          </div>
        )}
        {l?.instrumental && <div className="empty">INSTRUMENTAL</div>}
        {lines.length > 0 && lines.map(([, text], i) => (
          <div key={i} ref={i === idx ? cur : undefined} className={`lline ${i === idx ? "now" : i < idx ? "past" : ""}`}>{text || "♪"}</div>
        ))}
        {lines.length === 0 && l?.plain && <pre className="plain">{l.plain}</pre>}
      </div>
    </section>
  );
}
