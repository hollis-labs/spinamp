import { useEffect, useRef, useState } from "react";
import * as Ic from "./Icons";

export type ColumnDef = { id: string; label: string; full: string; def: boolean };

type Props = { cols: ColumnDef[]; visible: Set<string>; onToggle: (id: string) => void; onReset: () => void };

/** "Columns" button: a popover with a checkbox per optional column. */
export default function ColumnPicker({ cols, visible, onToggle, onReset }: Props) {
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

  return (
    <>
      <button className="btn icon" title="Choose columns" onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect();
        setAt({ x: Math.max(8, Math.min(window.innerWidth - 250, r.right - 236)), y: r.bottom + 4 });
      }}><Ic.Columns /></button>
      {at && (
        <div className="menu" ref={ref} style={{ left: at.x, top: at.y, width: 236 }}>
          <div className="menuhead">COLUMNS</div>
          <div className="colrow dim">TITLE and actions are always shown</div>
          {cols.map((c) => (
            <label className="colrow" key={c.id}>
              <input type="checkbox" checked={visible.has(c.id)} onChange={() => onToggle(c.id)} style={{ accentColor: "var(--orange)" }} />
              <span>{c.full}</span>
            </label>
          ))}
          <hr />
          <button onClick={() => { onReset(); setAt(null); }}>RESET TO DEFAULT</button>
        </div>
      )}
    </>
  );
}
