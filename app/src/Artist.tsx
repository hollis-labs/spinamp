import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as Ic from "./Icons";

type P = {
  name: string; mbid?: string; description?: string; extract?: string; wiki_url?: string; image?: string;
  country?: string; area?: string; kind?: string; begin_year?: number; tags: string[];
  links: [string, string][]; members: string[]; fans?: number; deezer_url?: string; top_tracks: string[];
  tracks: number; plays: number; listens: number;
};

const compact = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}K` : String(n));
const Br = ({ k, v }: { k: string; v: string | number }) => <span className="br">{k}: <b>{v}</b></span>;

export default function Artist({ name, onBack }: { name: string; onBack: () => void }) {
  const [p, setP] = useState<P | null | undefined>(undefined);
  const [err, setErr] = useState("");
  useEffect(() => {
    setP(undefined); setErr("");
    invoke<P>("artist_profile", { name }).then(setP).catch((e) => { setErr(String(e)); setP(null); });
  }, [name]);

  return (
    <section className="win">
      <div className="winhead">
        <button className="btn" onClick={onBack}><Ic.Back /> BACK</button>
        <span className="crumb">ARTIST</span>
      </div>
      <div className="scroll artist">
        {p === undefined && <div className="empty">LOADING {name.toUpperCase()}... (FIRST LOOKUP TAKES A FEW SECONDS)</div>}
        {p === null && <div className="empty">ERROR: {err}</div>}
        {p && (
          <>
            <div className="ahead">
              {p.image ? <img className="aimg" src={p.image} alt="" /> : <div className="aimg ph"><Ic.User /></div>}
              <div className="ameta">
                <div className="aname">{p.name}</div>
                {p.description && <div className="dim">{p.description}</div>}
                <div className="chips" style={{ marginTop: 10 }}>
                  {p.country && <Br k="FROM" v={p.area && p.area !== p.country ? `${p.area}` : p.country} />}
                  {p.kind && <Br k="TYPE" v={p.kind} />}
                  {p.begin_year && <Br k="SINCE" v={p.begin_year} />}
                  {p.fans != null && <Br k="FANS" v={compact(p.fans)} />}
                </div>
                <div className="chips" style={{ marginTop: 10 }}>
                  <Br k="YOUR TRACKS" v={p.tracks} /><Br k="PLAYS" v={p.plays} /><Br k="LISTENS" v={p.listens} />
                </div>
              </div>
            </div>
            {p.tags.length > 0 && <div className="tags">{p.tags.map((t) => <span className="tag" key={t}>{t}</span>)}</div>}
            {p.extract && <p className="bio">{p.extract}</p>}
            {p.members.length > 0 && <div className="blk"><div className="label">MEMBERS</div>{p.members.join(" · ")}</div>}
            {p.top_tracks.length > 0 && <div className="blk"><div className="label">POPULAR (DEEZER)</div>{p.top_tracks.join(" · ")}</div>}
            <div className="links">
              {p.wiki_url && <button className="btn small" onClick={() => openUrl(p.wiki_url!)}>WIKIPEDIA <Ic.Ext /></button>}
              {p.links.filter((l) => l[0] !== "wikipedia" && l[0] !== "wikidata").slice(0, 8).map(([k, u]) => (
                <button className="btn small" key={u} onClick={() => openUrl(u)}>{k.replace("free ", "").toUpperCase()} <Ic.Ext /></button>
              ))}
              {p.deezer_url && <button className="btn small" onClick={() => openUrl(p.deezer_url!)}>DEEZER <Ic.Ext /></button>}
            </div>
            {!p.extract && !p.mbid && <div className="empty">NOT MUCH FOUND FOR THIS ARTIST.</div>}
          </>
        )}
      </div>
    </section>
  );
}
