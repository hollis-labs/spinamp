import { useEffect, useRef } from "react";
import { clamp01, type Capture, type LiveFrame, type Pattern } from "./Viz";

const mmss = (ms: number) => `${Math.floor(ms / 60000)}:${String(Math.floor(ms / 1000) % 60).padStart(2, "0")}`;

type Props = {
  trackId: string;
  durationMs: number;
  positionMs: number;
  playing: boolean;
  tempo?: number;
  tags?: string[];
  dynamicRangeDb?: number;
  pattern?: Pattern | null;
  capture?: Capture | null;
  live?: { current: LiveFrame | null };
  onSeek: (ms: number) => void;
};

/**
 * The song's shape across its whole length, in the same four colours as the visualiser. Solid
 * columns are real (captured on a previous listen, or captured live right now and filled in as
 * you listen). Parts not heard yet stay empty. Click to seek.
 */
// A cool, muted palette on purpose: the song shape should inform without competing with the
// warm visualiser above it. Bass (front, bottom) lightest, highs (back, top) darkest.
const SHAPE = ["#a9cfe3", "#7fa9c2", "#58829c", "#3b5d73"];

export default function SongShape(p0: Props) {
  const ref = useRef<HTMLCanvasElement>(null);
  const props = useRef(p0);
  props.current = p0;
  const clock = useRef({ e: p0.positionMs, at: performance.now() });
  const label = useRef<HTMLSpanElement>(null);

  useEffect(() => { clock.current = { e: p0.positionMs, at: performance.now() }; }, [p0.positionMs]);

  useEffect(() => {
    const cv = ref.current!;
    const ctx = cv.getContext("2d")!;
    const dpr = window.devicePixelRatio || 1;
    const W = cv.clientWidth, H = cv.clientHeight;
    cv.width = W * dpr; cv.height = H * dpr;
    ctx.scale(dpr, dpr);
    const COLW = 4, N = Math.floor(W / COLW);
    let raf = 0, capKey = "", cols: (number[] | undefined)[] = [], realH: number[] = [];

    /** Stretch a series across its own range so the structure shows (5th..95th percentile -> 0.12..1). */
    const stretch = (v: (number | undefined)[]) => {
      const xs = v.filter((x): x is number => x !== undefined).sort((a, b) => a - b);
      if (xs.length < 3) return v.map((x) => (x === undefined ? 0 : 0.5));
      const lo = xs[Math.floor(xs.length * 0.05)], hi = Math.max(lo + 0.05, xs[Math.floor(xs.length * 0.95)]);
      return v.map((x) => (x === undefined ? 0 : 0.12 + 0.88 * clamp01((x - lo) / (hi - lo))));
    };
    const mean = (g: number[]) => (g[0] + g[1] + g[2] + g[3]) / 4;

    const draw = (nowMs: number) => {
      const p = props.current;
      const D = Math.max(1, p.durationMs);
      const pos = clock.current.e + (p.playing ? nowMs - clock.current.at : 0);

      // recorded columns, from the song's loudness-shape envelope
      const ckey = p.capture ? `${p.trackId}|${p.capture.present.length}|${p.capture.coverage}|${p.capture.shape.length}` : "";
      if (ckey !== capKey) {
        capKey = ckey; cols = []; realH = [];
        const c = p.capture;
        if (c && c.shape.length) {
          for (let j = 0; j < N; j++) {
            const f0 = Math.floor(((j / N) * D) / c.frame_ms), f1 = Math.max(f0 + 1, Math.floor((((j + 1) / N) * D) / c.frame_ms));
            const acc = [0, 0, 0, 0]; let n = 0;
            for (let f = f0; f < f1 && f < c.present.length; f++) {
              if (!c.present[f]) continue;
              for (let k = 0; k < 4; k++) acc[k] += c.shape[f * 4 + k] / 255;
              n++;
            }
            if (n >= Math.max(1, (f1 - f0) * 0.3)) cols[j] = acc.map((v) => v / n);
          }
          realH = stretch(cols.map((g) => (g ? mean(g) : undefined)));
        }
      }

      ctx.clearRect(0, 0, W, H);
      let real = 0;
      const playheadX = (pos / D) * W;
      for (let j = 0; j < N; j++) {
        const x = j * COLW, w = COLW - 1;
        const rec = cols[j];
        if (!rec) {
          // not recorded yet: just a faint baseline tick so the track is visible
          ctx.globalAlpha = 0.25; ctx.fillStyle = SHAPE[3]; ctx.fillRect(x, H - 2, w, 1);
          continue;
        }
        real++;
        const total = realH[j] * (H - 6);
        const w8 = rec.map((v) => v + 0.2), sum = w8[0] + w8[1] + w8[2] + w8[3];
        // cumulative share from the bottom: bass, +low, +mid, +high = whole column
        const cum = [w8[0] / sum, (w8[0] + w8[1]) / sum, (w8[0] + w8[1] + w8[2]) / sum, 1];
        const played = x + w <= playheadX;
        ctx.globalAlpha = played ? 0.95 : 0.45;
        for (let k = 3; k >= 0; k--) {          // back (highs, darkest, tallest) first
          ctx.fillStyle = SHAPE[k];
          ctx.fillRect(x, H - total * cum[k], w, total * cum[k]);
        }
      }
      ctx.globalAlpha = 1;
      ctx.fillStyle = "#e8f4fb"; ctx.fillRect(Math.round(playheadX) - 1, 0, 2, H);
      if (label.current) label.current.textContent = `${real >= N ? "RECORDED" : real > 0 ? `RECORDING ${Math.round((real / N) * 100)}%` : "NOT RECORDED YET"} · ${mmss(D)}`;
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <div className="shape" title="Song shape, drawn from what Spinamp has actually heard. Fills in as you listen. Click to seek.">
      <canvas ref={ref} onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect();
        p0.onSeek(Math.round(((e.clientX - r.left) / r.width) * p0.durationMs));
      }} />
      <span ref={label} className="shapelabel" />
    </div>
  );
}
