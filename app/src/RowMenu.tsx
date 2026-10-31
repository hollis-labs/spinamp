import { useEffect, useRef, useState } from "react";
import * as Ic from "./Icons";

export type MenuTrack = { id: string; title: string; artist: string; tags: string[] };
export type MenuState = { favorite?: boolean; muted_until?: number | null; blocked?: boolean };

type Props = {
  track: MenuTrack;
  state: MenuState;
  /** Offer "remove from this playlist" (only when a playlist is open). */
  inPlaylist: boolean;
  small?: boolean;
  onMute: (days: number | null) => void;
  onRemoveHere: () => void;
  onRemoveAll: () => void;
  onBlock: (kind: "song" | "artist" | "tag", value: string) => void;
};

const daysLeft = (until: number) => Math.max(1, Math.ceil((until * 1000 - Date.now()) / 86_400_000));

/** The "..." menu for a track: mute, remove, block. Opens as a fixed popover so scrolling lists never clip it. */
export default function RowMenu(p: Props) {
  const [at, setAt] = useState<{ x: number; y: number } | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!at) return;
    const close = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent) { if (e.key === "Escape") setAt(null); return; }
      if (!ref.current?.contains(e.target as Node)) setAt(null);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", close);
    return () => { document.removeEventListener("mousedown", close); document.removeEventListener("keydown", close); };
  }, [at]);

  const run = (f: () => void) => () => { setAt(null); f(); };
  const artist = p.track.artist.split(", ")[0];
  const muted = p.state.muted_until ? daysLeft(p.state.muted_until) : 0;

  return (
    <>
      <button className={`btn icon ${p.small ? "small" : ""}`} title="More" onClick={(e) => {
        e.stopPropagation();
        const r = e.currentTarget.getBoundingClientRect();
        setAt({ x: Math.min(window.innerWidth - 270, r.right - 250), y: Math.min(window.innerHeight - 330, r.bottom + 4) });
      }}><Ic.More /></button>
      {at && (
        <div className="menu" ref={ref} style={{ left: Math.max(8, at.x), top: Math.max(8, at.y) }} onClick={(e) => e.stopPropagation()}>
          <div className="menuhead">{p.track.title}</div>
          {muted > 0 ? (
            <button onClick={run(() => p.onMute(null))}>UNMUTE <span className="dim">({muted}D LEFT)</span></button>
          ) : (
            <>
              <button onClick={run(() => p.onMute(1))}>MUTE FOR 1 DAY</button>
              <button onClick={run(() => p.onMute(7))}>MUTE FOR 7 DAYS</button>
              <button onClick={run(() => p.onMute(30))}>MUTE FOR 30 DAYS</button>
            </>
          )}
          <hr />
          {p.inPlaylist && <button onClick={run(p.onRemoveHere)}>REMOVE FROM THIS PLAYLIST</button>}
          <button onClick={run(p.onRemoveAll)}>REMOVE FROM LIBRARY <span className="dim">(ALL PLAYLISTS)</span></button>
          <hr />
          <button className="danger" onClick={run(() => p.onBlock("song", p.track.id))}>BLOCK THIS SONG</button>
          <button className="danger" onClick={run(() => p.onBlock("artist", artist))}>BLOCK ARTIST: {artist}</button>
          {p.track.tags.slice(0, 6).map((t) => (
            <button className="danger" key={t} onClick={run(() => p.onBlock("tag", t))}>BLOCK TAG / GENRE: {t}</button>
          ))}
        </div>
      )}
    </>
  );
}
