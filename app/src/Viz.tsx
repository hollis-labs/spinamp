import { useEffect, useRef } from "react";

/** Beat-quantised spectrum recording from the 30 s preview (see spinamp-core `VizPattern`). */
export type Pattern = { beats: number; subdiv: number; beats_per_bar: number; bands?: number; version?: number; bpm?: number; data: ArrayLike<number> };
/** Whole-track spectrum recorded while listening (live capture), indexed by playback time. */
export type Capture = { frame_ms: number; bands: number; data: Uint8Array; present: Uint8Array; shape: Uint8Array; coverage: number };
export type LiveFrame = { trackId: string; bands: number[]; at: number };
export type VizMode = "live" | "recorded" | "preview" | "synth";

/** The backend sends pattern bytes as base64 (or, for old rows, a number array). */
export function decodePattern(raw: (Omit<Pattern, "data"> & { data: string | number[] }) | null): Pattern | null {
  if (!raw) return null;
  if (typeof raw.data !== "string") return raw as Pattern;
  return { ...raw, data: b64(raw.data) };
}
export function decodeCapture(raw: { frame_ms: number; bands: number; coverage: number; data: string; present: string; shape: string } | null): Capture | null {
  return raw ? { frame_ms: raw.frame_ms, bands: raw.bands, coverage: raw.coverage, data: b64(raw.data), present: b64(raw.present), shape: b64(raw.shape) } : null;
}
function b64(s: string) {
  const bin = atob(s), out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// Back (top, darker) -> front (bottom, lighter), as in the Resource Overview chart.
export const LAYERS = [
  { name: "BASS", color: "#f7ef6e" },
  { name: "LOW", color: "#f2ac48" },
  { name: "MID", color: "#e8773a" },
  { name: "HIGH", color: "#c92f27" },
];
const BARS = 80;
const GAP = 2;

type Props = {
  playing: boolean;
  trackId: string;
  tempo?: number;
  energy?: number;
  durationMs?: number;
  tags?: string[];
  brightnessHz?: number;
  dynamicRangeDb?: number;
  /** Last known playback position (ms); the component interpolates between updates. */
  positionMs: number;
  pattern?: Pattern | null;
  capture?: Capture | null;
  live?: { current: LiveFrame | null };
  /** Shift vs. audio output latency, ms. */
  offsetMs?: number;
  /** Horizontal LED-style segmentation. */
  led?: boolean;
  onMode?: (m: VizMode) => void;
};

/** Deterministic pseudo-random in [0,1) from a string + int. */
function rnd(seed: string, i: number) {
  let h = 2166136261 ^ i;
  for (let c = 0; c < seed.length; c++) h = Math.imul(h ^ seed.charCodeAt(c), 16777619);
  h ^= h >>> 13; h = Math.imul(h, 0x5bd1e995); h ^= h >>> 15;
  return (h >>> 0) / 4294967296;
}
const gauss = (x: number, mu: number, sd: number) => Math.exp(-(((x - mu) / sd) ** 2));
export const clamp01 = (x: number) => Math.max(0, Math.min(1, x));

/** Box blur with edge clamping; softens a layer's outline. */
function blur(src: Float32Array, r: number, out: Float32Array) {
  const n = src.length;
  for (let i = 0; i < n; i++) {
    let s = 0, c = 0;
    for (let k = -r; k <= r; k++) { const j = Math.min(n - 1, Math.max(0, i + k)); s += src[j]; c++; }
    out[i] = s / c;
  }
}

/* ---------- proxy: what a track of this kind probably does ---------- */

type Style = "dnb" | "bass" | "house" | "hiphop" | "ambient" | "rock" | "acoustic" | "pop";
const STYLE_KEYS: [Style, string[]][] = [
  ["dnb", ["drum and bass", "jungle", "liquid funk", "neurofunk", "breakcore"]],
  ["bass", ["dubstep", "brostep", "riddim", "trap", "bass", "future bass", "complextro", "glitch hop", "drumstep"]],
  ["house", ["house", "techno", "trance", "edm", "electro", "dance", "disco", "garage", "tech house", "progressive"]],
  ["hiphop", ["hip hop", "hip-hop", "rap", "boom bap", "grime", "r&b"]],
  ["ambient", ["ambient", "drone", "new age", "downtempo", "chillout", "ethereal", "dream pop", "shoegaze", "idm", "post-rock"]],
  ["rock", ["metal", "punk", "hardcore", "rock", "grunge", "djent", "math rock"]],
  ["acoustic", ["folk", "acoustic", "singer-songwriter", "bluegrass", "americana", "country", "classical", "jazz"]],
];
export function styleOf(tags: string[] = []): Style {
  const t = tags.map((x) => x.toLowerCase());
  for (const [style, keys] of STYLE_KEYS) if (t.some((x) => keys.some((k) => x.includes(k)))) return style;
  return "pop";
}
export const STYLE_BPM: Record<Style, number> = { dnb: 172, bass: 140, house: 126, hiphop: 92, ambient: 85, rock: 130, acoustic: 105, pop: 112 };

/**
 * We only hear 30 s of a track, so the rest is a *proxy arrangement*: intro build, sections of
 * 8 bars that differ in weight, breakdowns (bass out, highs up), a fill on the last beat of every
 * 4th bar, and a fade-out. Deterministic per track; `dyn` (0..1) scales how dramatic it gets.
 */
export function arrangement(id: string, durationMs: number, t: number, periodMs: number, bpb: number, dyn: number) {
  const barMs = periodMs * bpb;
  const bar = Math.floor(t / barMs);
  const sec = Math.floor(bar / 8), inSec = bar % 8;
  const r = rnd(id, sec), r2 = rnd(id, sec + 1000);
  let master = 1 - dyn * 0.3 * (1 - r);
  const g = [1, 1, 1, 1];
  if (r2 < 0.1 + 0.22 * dyn && inSec < 4 && bar >= 8) { g[0] = 0.15; g[1] = 0.45; g[3] = 1.15; master *= 0.85; } // breakdown
  if (r2 > 0.85 && inSec === 0) master *= 1 + 0.18 * dyn; // big first bar of a section
  if (bar < 8) { master *= 0.45 + 0.55 * (t / (8 * barMs)); if (bar < 4) g[0] *= 0.55; } // intro build
  const beatInBar = Math.floor((t % barMs) / periodMs);
  if (bar % 4 === 3 && beatInBar === bpb - 1) { g[3] *= 1.35; g[2] *= 1.15; } // fill
  if (durationMs > 0) {
    const left = durationMs - t;
    if (left < 12000) master *= clamp01(left / 12000) * 0.85 + 0.15; // outro
  }
  return { master, g };
}

/** Plausible band energies for a track we have no recording of: a drum pattern + bass + pad
 *  chosen by genre, brightness tilt from measured spectrum, depth from energy. */
function synthBands(style: Style, id: string, sf: number, subdiv: number, bpb: number, B: number, energy: number, tilt: number, out: number[]) {
  const beats = sf / subdiv, beatIdx = Math.floor(beats) % bpb, ph = beats % 1;
  const env = (x: number, k: number) => Math.exp(-x * k);
  const sub8 = (beats * 2) % 1, sub16 = (beats * 4) % 1;
  let kick = 0, snare = 0, hat = 0, bass = 0.35, mid = 0.3, air = 0.2, wob = 0;
  switch (style) {
    case "house": kick = env(ph, 6); snare = beatIdx % 2 === 1 ? env(ph, 9) * 0.8 : 0; hat = env((ph + 0.5) % 1, 18) * 0.9; bass = 0.4 + 0.25 * Math.sin(beats * Math.PI); mid = 0.3 + 0.2 * Math.sin(beats / 3); break;
    case "bass": kick = beatIdx === 0 ? env(ph, 5) : 0; snare = beatIdx === Math.floor(bpb / 2) ? env(ph, 4.5) * 1.1 : 0; wob = 0.5 + 0.5 * Math.sin(beats * Math.PI * 2 * (1 + Math.floor(beats / 4) % 2)); hat = env(sub16, 14) * 0.55; bass = 0.62 + 0.38 * wob; mid = 0.16 + 0.3 * wob * (0.6 + 0.4 * Math.sin(beats / 5)); air = 0.14; break;
    case "dnb": kick = (beatIdx === 0 && ph < 0.5 ? env(ph, 8) : 0) + (beatIdx === 1 ? env((ph + 0.5) % 1, 8) * 0.9 : 0); snare = beatIdx % 2 === 1 ? env(ph, 8) : 0; hat = env(sub16, 16) * 0.8; bass = 0.55 + 0.2 * Math.sin(beats / 2); mid = 0.3 + 0.25 * Math.sin(beats / 4); break;
    case "hiphop": kick = beatIdx === 0 ? env(ph, 6) : beatIdx === 2 && ph > 0.4 ? env(ph - 0.4, 6) * 0.8 : 0; snare = beatIdx % 2 === 1 ? env(ph, 7) : 0; hat = env(sub8, 16) * 0.6; bass = 0.45 + 0.2 * Math.sin(beats / 2.5); mid = 0.3 + 0.2 * Math.sin(beats / 6); break;
    case "ambient": kick = 0; snare = 0; hat = 0.06; bass = 0.3 + 0.2 * Math.sin(beats / 9); mid = 0.4 + 0.3 * Math.sin(beats / 11 + 1); air = 0.25 + 0.2 * Math.sin(beats / 7 + 2); break;
    case "rock": kick = beatIdx % 2 === 0 ? env(ph, 7) * 0.9 : 0; snare = beatIdx % 2 === 1 ? env(ph, 8) : 0; hat = env(sub8, 12) * 0.7; bass = 0.4; mid = 0.55 + 0.15 * Math.sin(beats / 3); air = 0.4; break;
    case "acoustic": kick = beatIdx === 0 ? env(ph, 7) * 0.5 : 0; snare = beatIdx % 2 === 1 ? env(ph, 10) * 0.35 : 0; hat = 0.1; bass = 0.25 + 0.1 * Math.sin(beats / 2); mid = 0.5 + 0.2 * Math.sin(beats / 4 + id.length); air = 0.18; break;
    default: kick = beatIdx % 2 === 0 ? env(ph, 6) : 0; snare = beatIdx % 2 === 1 ? env(ph, 8) * 0.9 : 0; hat = env(sub8, 14) * 0.65; bass = 0.4 + 0.15 * Math.sin(beats / 2); mid = 0.35 + 0.2 * Math.sin(beats / 5);
  }
  const punch = 0.55 + 0.45 * energy; // loud tracks hit harder
  for (let b = 0; b < B; b++) {
    const u = B > 1 ? b / (B - 1) : 0;
    const t = 1 + tilt * (u - 0.5) * 1.6; // brighter tracks lean to the treble end
    const v = punch * (kick * gauss(u, 0.1, 0.13) + snare * gauss(u, 0.45, 0.22) * 0.85 + hat * gauss(u, 0.85, 0.18)) + bass * gauss(u, 0.15, 0.2) * 0.8 + mid * gauss(u, 0.5, 0.25) * 0.75 + air * gauss(u, 0.9, 0.2) * 0.6;
    out[b] = clamp01(v * t);
  }
}

/**
 * EQ-style visualiser driven by data, not audio. What it shows, best source first:
 *  1. LIVE: the real spectrum of Spotify's audio (macOS capture), streamed by the backend;
 *  2. RECORDED: a fingerprint saved from an earlier live listen, indexed by playback time;
 *  3. PREVIEW: the track's analysed 30 s, replayed beat-locked + a proxy arrangement for the rest;
 *  4. SYNTH: a genre-shaped drum/bass/pad pattern from tempo, tags and brightness.
 * Bars get EQ physics (fast attack, slow decay, falling peak caps) and treble-weighted jitter. Four
 * layers are drawn back-to-front: dark red (highs, rough) behind, amber and yellow (bass, smooth)
 * in front, so the chart reads as stacked depth like the reference.
 */
export default function Viz(props0: Props) {
  const ref = useRef<HTMLCanvasElement>(null);
  const clock = useRef({ e: props0.positionMs, at: performance.now(), corr: 0 });
  const props = useRef(props0);
  props.current = props0;
  const lastMode = useRef<VizMode | null>(null);

  // fold position updates into a smooth clock
  useEffect(() => {
    const c = clock.current, now = performance.now();
    const predicted = c.e + (props.current.playing ? now - c.at : 0);
    const diff = predicted - props0.positionMs;
    c.corr = Math.abs(diff) < 800 ? c.corr + diff : 0; // ease small corrections, jump on seeks/track changes
    c.e = props0.positionMs; c.at = now;
  }, [props0.positionMs]);

  useEffect(() => {
    const cv = ref.current!;
    const ctx = cv.getContext("2d")!;
    const dpr = window.devicePixelRatio || 1;
    const W = cv.clientWidth, H = cv.clientHeight;
    cv.width = W * dpr; cv.height = H * dpr;
    ctx.scale(dpr, dpr);
    const FOOT = 14, plotH = H - FOOT, bw = W / BARS;

    const level = new Float32Array(BARS), peak = new Float32Array(BARS), vel = new Float32Array(BARS), noise = new Float32Array(BARS);
    const s1 = new Float32Array(BARS), s2 = new Float32Array(BARS), s3 = new Float32Array(BARS);
    const gcur = [1, 1, 1, 1];
    let master = 0.3, active = 0, raf = 0, last = performance.now(), first = true;

    const draw = (nowMs: number) => {
      const dt = Math.min(0.05, (nowMs - last) / 1000); last = nowMs;
      const p = props.current, c = clock.current;
      c.corr *= 0.92;
      const raw = c.e + (p.playing ? nowMs - c.at : 0) + c.corr;
      const pos = Math.max(0, raw - (p.offsetMs ?? 0));
      if (first) active = p.playing ? 1 : 0.1; // start settled, not ramping up from nothing
      active += ((p.playing ? 1 : 0.1) - active) * (1 - Math.exp(-dt * 3));

      const style = styleOf(p.tags);
      const pat = p.pattern && p.pattern.data.length ? p.pattern : null;
      // a recorded pattern must play at the tempo ITS grid was built on, even if the track's own
      // tempo was later reconciled to a different reading
      const bpm = pat && pat.bpm && pat.bpm > 0 ? pat.bpm : p.tempo && p.tempo > 0 ? p.tempo : STYLE_BPM[style];
      const periodMs = 60000 / bpm;
      const subdiv = pat ? pat.subdiv : 8;
      const bpb = pat ? pat.beats_per_bar : 4;

      // 1. band energies at this instant, from the best source available
      let bands: number[], mode: VizMode;
      const lf = p.live?.current;
      const cap = p.capture;
      if (lf && lf.trackId === p.trackId && nowMs - lf.at < 450) {
        bands = lf.bands.map((v) => v / 255); mode = "live";
      } else if (cap && cap.present.length) {
        const f = pos / cap.frame_ms, i0 = Math.floor(f), fr = f - i0;
        if (cap.present[i0] && cap.present[i0 + 1]) {
          bands = Array.from({ length: cap.bands }, (_, b) => (cap.data[i0 * cap.bands + b] * (1 - fr) + cap.data[(i0 + 1) * cap.bands + b] * fr) / 255);
          mode = "recorded";
        } else bands = [], mode = "synth";
      } else bands = [], mode = "synth";
      if (mode === "synth") {
        const B = pat ? pat.bands ?? 4 : 16;
        bands = new Array<number>(B).fill(0);
        const sf = pos / (periodMs / subdiv);
        if (pat) {
          const slots = pat.beats * pat.subdiv;
          const s = ((sf % slots) + slots) % slots, i0 = Math.floor(s), f = s - i0, i1 = (i0 + 1) % slots;
          for (let b = 0; b < B; b++) bands[b] = (pat.data[i0 * B + b] * (1 - f) + pat.data[i1 * B + b] * f) / 255;
          mode = "preview";
        } else {
          const tilt = p.brightnessHz ? Math.max(-0.5, Math.min(0.5, (p.brightnessHz - 1800) / 2500)) : 0;
          synthBands(style, p.trackId, sf, subdiv, bpb, B, p.energy ?? 0.6, tilt, bands);
        }
      }
      if (lastMode.current !== mode) { lastMode.current = mode; p.onMode?.(mode); }
      const B = bands.length;

      // 2. proxy arrangement (only when we are not hearing/recording the real thing)
      const real = mode === "live" || mode === "recorded";
      const dyn = p.dynamicRangeDb ? clamp01((p.dynamicRangeDb - 4) / 14) : 0.5;
      const arr = real ? { master: 1, g: [1, 1, 1, 1] } : arrangement(p.trackId, p.durationMs ?? 0, pos, periodMs, bpb, dyn);
      const ease = first ? 1 : 1 - Math.exp(-dt * 4);
      first = false;
      for (let k = 0; k < 4; k++) gcur[k] += (arr.g[k] - gcur[k]) * ease;
      master += (arr.master * (real ? 1 : 0.55 + 0.45 * (p.energy ?? 0.7)) - master) * ease;

      // 3. per-layer energy (mean of the bands in each quarter) after gains
      const grp = Math.max(1, Math.floor(B / 4));
      const e = [0, 0, 0, 0];
      for (let k = 0; k < 4; k++) {
        let s = 0, n = 0;
        for (let b = k * grp; b < Math.min(B, (k + 1) * grp); b++) { s += bands[b]; n++; }
        e[k] = clamp01((s / Math.max(1, n)) * gcur[k]);
      }

      // 4. bar heights: spectrum across frequency + jitter + EQ physics
      const jit = real ? 0.4 : 1;
      for (let i = 0; i < BARS; i++) {
        const u = i / (BARS - 1);
        const bf = u * (B - 1), b0 = Math.floor(bf), b1 = Math.min(B - 1, b0 + 1), fr = bf - b0;
        const gAt = (b: number) => gcur[Math.min(3, Math.floor((b / B) * 4))];
        let v = (bands[b0] * gAt(b0) * (1 - fr) + bands[b1] * gAt(b1) * fr) * master;
        noise[i] = noise[i] * 0.82 + (Math.random() - 0.5) * (0.06 + 0.32 * Math.pow(u, 1.3)) * jit;
        v = v * (1 + noise[i]) + Math.random() * 0.06 * u * u * active * jit;
        const target = clamp01(v) * active * 0.97;
        level[i] += (target - level[i]) * (1 - Math.exp(-dt * (target > level[i] ? 38 : 9)));
        if (level[i] >= peak[i]) { peak[i] = level[i]; vel[i] = 0; } else { vel[i] += dt * 1.6; peak[i] = Math.max(level[i], peak[i] - vel[i] * dt); }
      }
      // smoothed copies: the front layers follow the broad shape, the back layer the raw detail
      blur(level, 11, s1); blur(level, 6, s2); blur(level, 2, s3);

      ctx.globalCompositeOperation = "source-over";
      ctx.clearRect(0, 0, W, H);
      const top = plotH * 0.96;
      for (let i = 0; i < BARS; i++) {
        const x = i * bw, w = Math.max(1, bw - GAP);
        // layer tops as a share of the chart height, each capped so the layers stay clearly separate:
        // bass (front) at most 1/4, low at most 1/2, mid at most 2/3, highs (back) up to the full bar.
        // Front layers follow the smoothed shape, the back layer the raw detail.
        const lv = level[i];
        const c1 = Math.min(lv, s1[i] * 0.25 * (0.8 + 0.2 * e[0]));
        const c2 = Math.min(lv, Math.max(c1 + 0.004, s2[i] * 0.5 * (0.8 + 0.2 * e[1])));
        const c3 = Math.min(lv, Math.max(c2 + 0.004, s3[i] * (2 / 3) * (0.8 + 0.2 * e[2])));
        const c4 = Math.max(lv, c3);
        const ys = [c4, c3, c2, c1].map((v) => Math.min(top, v * top));
        // painter's algorithm: back (red) first, lighter front layers on top
        for (let k = 0; k < 4; k++) {
          ctx.fillStyle = LAYERS[3 - k].color;
          ctx.fillRect(x, plotH - ys[k], w, ys[k]);
        }
        if (peak[i] > 0.03) { ctx.fillStyle = "rgba(255,230,210,0.6)"; ctx.fillRect(x, plotH - Math.min(top, peak[i] * top) - 3, w, 2); }
      }
      // LED rows: cut thin transparent lines across the bars
      if (p.led !== false) {
        ctx.globalCompositeOperation = "destination-out";
        ctx.fillStyle = "rgba(0,0,0,0.85)";
        for (let y = plotH - 4; y > 0; y -= 4) ctx.fillRect(0, y, W, 1);
      }
      // grid goes behind everything
      ctx.globalCompositeOperation = "destination-over";
      ctx.strokeStyle = "rgba(255,255,255,0.07)"; ctx.lineWidth = 1;
      for (let i = 1; i < 4; i++) { ctx.beginPath(); ctx.moveTo(0, Math.round((plotH / 4) * i) + 0.5); ctx.lineTo(W, Math.round((plotH / 4) * i) + 0.5); ctx.stroke(); }
      for (const f of [60, 250, 1000, 4000]) { const xx = Math.round((Math.log(f / 40) / Math.log(250)) * (W - 30)) + 0.5; ctx.beginPath(); ctx.moveTo(xx, 0); ctx.lineTo(xx, plotH); ctx.stroke(); }
      ctx.globalCompositeOperation = "source-over";

      // frequency labels (log axis 40 Hz .. 10 kHz)
      ctx.fillStyle = "#666"; ctx.font = "10px 'JetBrains Mono', monospace"; ctx.textAlign = "left";
      for (const [f, label] of [[60, "60"], [250, "250"], [1000, "1K"], [4000, "4K"], [10000, "10K Hz"]] as const) {
        const xx = (Math.log(f / 40) / Math.log(250)) * (W - 30);
        ctx.fillText(label, Math.min(W - 40, xx + 3), H - 3);
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return <canvas ref={ref} className="viz" />;
}
