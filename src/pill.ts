import { listen } from "@tauri-apps/api/event";
import { Channel, invoke } from "@tauri-apps/api/core";
import { playSound, primeAudio } from "./sounds";
import { createLook, waveLook } from "./pill/sonarMath";
import "./pill.css";

type State = "listening" | "working" | "done" | "error" | "hidden";
type Ripples = "on" | "subtle" | "off";
type Pill = { state: State; text: string; chip: string; sound: string; ripples: Ripples };

// App name (as detected by the backend) → brand colour token for the bars. First match wins.
const APP_COLORS: [needle: string, token: string][] = [
  ["teams", "--app-teams"], ["slack", "--app-slack"], ["claude", "--app-claude"], ["chatgpt", "--app-chatgpt"],
  ["outlook", "--app-outlook"], ["word", "--app-word"], ["excel", "--app-excel"], ["powerpoint", "--app-powerpoint"],
  ["visual studio code", "--app-vscode"], ["vs code", "--app-vscode"], ["cursor", "--app-cursor"], ["chrome", "--app-chrome"],
  ["edge", "--app-edge"], ["whatsapp", "--app-whatsapp"], ["discord", "--app-discord"], ["gmail", "--app-gmail"],
  ["figma", "--app-figma"], ["zoom", "--app-zoom"], ["notion", "--app-notion"], ["github", "--app-github"],
  ["terminal", "--app-terminal"], ["powershell", "--app-terminal"],
];
let barToken = "";

const pill = document.getElementById("pill")!;
const text = document.getElementById("text")!;
const chip = document.getElementById("chip")!;
const canvas = document.getElementById("wave") as HTMLCanvasElement;
const ctx = canvas.getContext("2d")!;
const sonar = document.getElementById("sonar") as HTMLCanvasElement;
const sctx = sonar.getContext("2d")!;
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
const W = 118, H = 24, N = 20, BAR = 3.5, GAP = (W - N * BAR) / (N - 1);

// Canvas sizes follow the device pixel ratio (checked every frame: moving to another monitor changes it).
let dpr = 0;
function fitCanvases() {
  dpr = devicePixelRatio || 1;
  canvas.width = W * dpr;
  canvas.height = H * dpr;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  sonar.width = innerWidth * dpr;
  sonar.height = innerHeight * dpr;
  sctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}
fitCanvases();
addEventListener("resize", fitCanvases);

// Colours are read only when the app or theme changes, never per frame.
let barColor = "", waveColor = "";
function readColors() {
  const css = getComputedStyle(document.documentElement);
  const brand = barToken && css.getPropertyValue(barToken).trim();
  barColor = brand || css.getPropertyValue("--wave-bar").trim();
  waveColor = css.getPropertyValue("--wave-ripple").trim();
}
readColors();
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", readColors);

// A context made later inside a hidden window can start suspended and swallow the first sound.
primeAudio();

// ---------- state ----------
let target = 0;
let level = 0;
let state: State = "hidden";
let flat = 1;
let wavesMode: Ripples = "on";
let listenStart = 0;

// ~60 Hz level frames from Rust (levels only, never audio).
type LevelFrame = { t: number; rms: number; peak: number };
const levels = new Channel<LevelFrame>();
levels.onmessage = (f) => {
  // Peak → 0..1 on a dB scale so quiet laptop mics still move the bars (-55 dBFS silent, -12 dBFS full).
  const db = 20 * Math.log10(Math.max(f.peak, 1e-5));
  target = Math.min(1, Math.max(0, (db + 55) / 43));
};
void invoke("pill_levels", { onFrame: levels });

listen<Pill>("pill", (e) => {
  const prev = state;
  state = e.payload.state;
  if (state === "listening" && prev !== "listening") listenStart = performance.now();
  if (e.payload.sound) {
    if (state === "listening" && prev !== "listening") playSound(e.payload.sound, true);
    else if (prev === "listening" && state !== "listening") playSound(e.payload.sound, false);
  }
  if (e.payload.ripples) wavesMode = e.payload.ripples;
  if (e.payload.chip) {
    const app = e.payload.chip.toLowerCase();
    barToken = APP_COLORS.find(([n]) => app.includes(n))?.[1] ?? "";
    readColors();
  }
  pill.dataset.state = state;
  pill.classList.toggle("show", state !== "hidden");
  text.textContent = e.payload.text;
  chip.textContent = e.payload.chip;
});

// ---------- drawing ----------
let last = 0;
let lastDraw = 0;
let sonarDirty = false;
const look = createLook();

function drawWaves(t: number) {
  if (sonarDirty) sctx.clearRect(0, 0, innerWidth, innerHeight);
  sonarDirty = false;
  if (state !== "listening" || wavesMode === "off" || reduced) return;
  const b = canvas.getBoundingClientRect();
  const n = wavesMode === "subtle" ? 2 : 3, peak = wavesMode === "subtle" ? 0.35 : 0.6;
  const base = b.y + b.height / 2, cx = b.x + b.width / 2, travel = base - 4;
  sctx.strokeStyle = waveColor;
  for (let i = 0; i < n; i++) {
    if (!waveLook(t - listenStart, i, n, peak, look)) continue;
    const half = (b.width / 2) * look.spread, y = base - travel * look.rise, amp = 2 + 5 * look.rise;
    sctx.globalAlpha = look.opacity;
    sctx.lineWidth = look.stroke;
    sctx.beginPath();
    const steps = Math.ceil(half);
    for (let k = 0; k <= steps; k++) {
      const u = k / steps, x = -half + 2 * half * u;
      // Arched crest that only lifts above `y` and tapers to the ends, so it never dips below the bar line.
      const lift = Math.sin(Math.PI * u) * (0.6 + 0.4 * Math.sin(x / 6 - t / 180));
      if (k === 0) sctx.moveTo(cx + x, y - amp * lift);
      else sctx.lineTo(cx + x, y - amp * lift);
    }
    sctx.stroke();
    sonarDirty = true;
  }
  sctx.globalAlpha = 1;
}

function frame(t: number) {
  const dt = Math.min(64, t - last) || 16;
  last = t;
  if ((devicePixelRatio || 1) !== dpr) fitCanvases();
  if (state !== "listening") target = 0;
  // ~80 ms attack, ~200 ms release so the bars breathe instead of jittering.
  level += (target - level) * (1 - Math.exp(-dt / (target > level ? 80 : 200)));
  flat += ((state === "done" ? 1 : 0) - flat) * (1 - Math.exp(-dt / 120));

  if (!reduced || t - lastDraw > 100) {
    lastDraw = t;
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = barColor;
    for (let i = 0; i < N; i++) {
      // One travelling wave for both states: listening scales it with the voice, working keeps it calm.
      const wave = 0.5 + 0.5 * Math.sin(t / 160 - i * 0.6);
      let amp = state === "working" ? 0.35 + 0.2 * (wave * 2 - 1) : (0.25 + 0.75 * level) * (0.35 + 0.65 * wave);
      amp *= 1 - flat;
      const h = Math.max(BAR, amp * H);
      ctx.beginPath();
      ctx.roundRect(i * (BAR + GAP), (H - h) / 2, BAR, h, BAR / 2);
      ctx.fill();
    }
    drawWaves(t);
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
