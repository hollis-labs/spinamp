import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as Ic from "./Icons";

type Track = { id: string; title: string; artist: string; album?: string; duration_ms: number };
type Cr = [string, string];
type TI = { position: string; title: string; duration: string; credits: Cr[] };
type R = {
  id: number; title: string; year?: number; country?: string; labels: [string, string][]; formats: string[];
  genres: string[]; styles: string[]; credits: Cr[]; tracklist: TI[]; notes: string; url: string;
};
type C = { release: R; track?: TI };

const Br = ({ k, v }: { k: string; v: string | number }) => <span className="br">{k}: <b>{v}</b></span>;
const People = ({ rows }: { rows: Cr[] }) => (
  <div className="credits">{rows.map(([n, r], i) => <div key={i}><span className="dim">{r || "Credit"}</span> <b>{n}</b></div>)}</div>
);

export default function Credits({ track, onBack }: { track: Track; onBack: () => void }) {
  const [c, setC] = useState<C | null | undefined>(undefined);
  const [hasToken, setHasToken] = useState(true);
  const [err, setErr] = useState("");
  useEffect(() => {
    setC(undefined); setErr("");
    invoke<boolean>("discogs_status").then(setHasToken);
    invoke<C | null>("discogs_credits", { track }).then(setC).catch((e) => { setErr(String(e)); setC(null); });
  }, [track.id]);

  return (
    <section className="win">
      <div className="winhead">
        <button className="btn" onClick={onBack}><Ic.Back /> BACK</button>
        <span className="crumb">CREDITS</span>
      </div>
      <div className="scroll artist">
        {c === undefined && <div className="empty">LOOKING UP {(track.album ?? track.title).toUpperCase()} ON DISCOGS...</div>}
        {c === null && (
          <div className="empty">
            {err ? `ERROR: ${err}`
              : !hasToken ? <>ADD A DISCOGS TOKEN IN SETTINGS<br />TO SEE LABELS, CREDITS AND PRESSING INFO.</>
              : !track.album ? "THIS TRACK HAS NO ALBUM, SO THERE IS NOTHING TO LOOK UP."
              : "DISCOGS HAS NOTHING FOR THIS RELEASE."}
          </div>
        )}
        {c && (
          <>
            <div className="aname">{c.release.title}</div>
            <div className="chips" style={{ marginTop: 10 }}>
              {c.release.year && <Br k="YEAR" v={c.release.year} />}
              {c.release.country && <Br k="COUNTRY" v={c.release.country} />}
              {c.release.labels.slice(0, 3).map(([l, n], i) => <Br key={i} k="LABEL" v={n ? `${l} · ${n}` : l} />)}
              {c.release.formats.slice(0, 2).map((f, i) => <Br key={i} k="FORMAT" v={f} />)}
            </div>
            {(c.release.genres.length > 0 || c.release.styles.length > 0) && (
              <div className="tags">{[...c.release.genres, ...c.release.styles].map((t) => <span className="tag" key={t}>{t.toLowerCase()}</span>)}</div>
            )}
            <div className="blk">
              <div className="label">THIS SONG{c.track?.position ? ` · ${c.track.position}` : ""}</div>
              {c.track
                ? (c.track.credits.length > 0 ? <People rows={c.track.credits} /> : <span className="dim">No song-level credits on this release.</span>)
                : <span className="dim">Couldn't find "{track.title}" in this release's tracklist.</span>}
            </div>
            {c.release.credits.length > 0 && <div className="blk"><div className="label">ALBUM CREDITS</div><People rows={c.release.credits} /></div>}
            {c.release.notes && <p className="bio">{c.release.notes}</p>}
            {c.release.tracklist.length > 0 && (
              <div className="blk"><div className="label">TRACKLIST</div>
                {c.release.tracklist.map((t, i) => (
                  <div key={i} className={t === c.track || t.title === c.track?.title ? "" : "dim"}>{t.position} {t.title} {t.duration && <span className="dim">{t.duration}</span>}</div>
                ))}
              </div>
            )}
            <div className="links">
              {c.release.url && <button className="btn small" onClick={() => openUrl(c.release.url)}>DISCOGS <Ic.Ext /></button>}
            </div>
          </>
        )}
      </div>
    </section>
  );
}
