// Croaker start/stop sounds, synthesized with Web Audio so nothing ships as audio files.
// Each sound gets "start" (pill opens) and "stop" (pill closes); stop is the calmer, lower half.

export const SOUNDS: [id: string, label: string][] = [
  ["heartbeat", "Heartbeat"], ["impact", "Impact"], ["power", "Power up"], ["stab", "Synth stab"],
  ["laser", "Laser"], ["warp", "Warp"], ["sparkle", "Sparkle"],
  ["drop", "Water drop"], ["sonar", "Sonar ping"], ["chime", "Chime"],
  ["bubble", "Bubble"], ["marimba", "Marimba"], ["glass", "Glass tap"], ["pop", "Pop"],
  ["click", "Click"], ["swoosh", "Swoosh"], ["arcade", "Arcade"], ["tick", "Soft tick"],
];

let ctx: AudioContext | null = null;
function audio() {
  ctx ??= new AudioContext();
  if (ctx.state !== "running") void ctx.resume();
  return ctx;
}

type Tone = { type?: OscillatorType; from: number; to?: number; at?: number; dur: number; gain: number; attack?: number };
function tone(a: AudioContext, { type = "sine", from, to = from, at = 0, dur, gain, attack = 0.006 }: Tone) {
  const t = a.currentTime + 0.01 + at;
  const o = a.createOscillator();
  const v = a.createGain();
  o.type = type;
  o.frequency.setValueAtTime(from, t);
  if (to !== from) o.frequency.exponentialRampToValueAtTime(to, t + dur * 0.6);
  v.gain.setValueAtTime(0, t);
  v.gain.linearRampToValueAtTime(gain, t + attack);
  v.gain.exponentialRampToValueAtTime(0.0001, t + dur);
  o.connect(v).connect(a.destination);
  o.start(t);
  o.stop(t + dur + 0.02);
}

type Noise = { at?: number; dur: number; gain: number; from: number; to?: number; q?: number };
function noise(a: AudioContext, { at = 0, dur, gain, from, to = from, q = 1 }: Noise) {
  const t = a.currentTime + 0.01 + at;
  const buf = a.createBuffer(1, Math.ceil(a.sampleRate * dur), a.sampleRate);
  const d = buf.getChannelData(0);
  for (let i = 0; i < d.length; i++) d[i] = Math.random() * 2 - 1;
  const src = a.createBufferSource();
  src.buffer = buf;
  const f = a.createBiquadFilter();
  f.type = "bandpass";
  f.Q.value = q;
  f.frequency.setValueAtTime(from, t);
  f.frequency.exponentialRampToValueAtTime(to, t + dur);
  const v = a.createGain();
  v.gain.setValueAtTime(0, t);
  v.gain.linearRampToValueAtTime(gain, t + dur * 0.3);
  v.gain.exponentialRampToValueAtTime(0.0001, t + dur);
  src.connect(f).connect(v).connect(a.destination);
  src.start(t);
}

const PLAY: Record<string, (a: AudioContext, start: boolean) => void> = {
  drop: (a, s) => {
    // deep drop: wide low sweep, a body tone for weight (kept above ~170 Hz so laptops play it), a faint echo
    const [f, t, g] = s ? [320, 960, 0.2] : [760, 260, 0.16];
    tone(a, { from: f, to: t, dur: 0.24, gain: g });
    tone(a, { from: f / 1.5, to: t / 1.5, dur: 0.26, gain: g * 0.6 });
    tone(a, { from: f * 1.5, to: t * 1.5, at: 0.035, dur: 0.18, gain: g * 0.3 });
    tone(a, { from: t * 0.9, to: t * 0.8, at: 0.2, dur: 0.22, gain: g * 0.18 });
  },
  heartbeat: (a, s) => {
    // lub-dub; closing is a single softer beat. Laptop speakers drop everything under ~200 Hz, so the
    // thump lives at 120-320 Hz and a short click gives the attack they can play.
    // Two plain sine layers only: a soft rounded attack and a faint overtone keep it calm, not crowded.
    const beat = (at: number, g: number) => {
      tone(a, { from: 160, to: 75, at, dur: 0.26, gain: g, attack: 0.012 });
      tone(a, { from: 300, to: 150, at, dur: 0.1, gain: g * 0.22, attack: 0.008 });
    };
    beat(0, s ? 0.34 : 0.26);
    if (s) beat(0.3, 0.24);
  },
  power: (a, s) => {
    // power-up sweep with a bright cap; closing powers down
    const [f, t] = s ? [220, 880] : [880, 220];
    tone(a, { type: "sawtooth", from: f, to: t, dur: 0.28, gain: 0.08 });
    tone(a, { type: "square", from: f * 1.5, to: t * 1.5, dur: 0.28, gain: 0.04 });
    if (s) tone(a, { from: 1760, at: 0.2, dur: 0.3, gain: 0.08 });
  },
  stab: (a, s) => {
    // synth chord stab over a sub punch
    const notes = s ? [523, 659, 784, 988] : [392, 494, 587];
    for (const f of notes) tone(a, { type: "sawtooth", from: f, dur: 0.2, gain: 0.035, attack: 0.002 });
    tone(a, { from: s ? 262 : 196, to: s ? 131 : 98, dur: 0.22, gain: 0.35, attack: 0.002 });
  },
  impact: (a, s) => {
    // cinematic hit: noise crack, body thump, falling boom
    const g = s ? 1 : 0.65;
    noise(a, { dur: 0.3, gain: 0.45 * g, from: 3500, to: 250, q: 0.7 });
    tone(a, { type: "triangle", from: 360, to: 120, dur: 0.14, gain: 0.35 * g, attack: 0.002 });
    tone(a, { from: 200, to: 60, dur: 0.45, gain: 0.5 * g, attack: 0.002 });
  },
  laser: (a, s) => {
    tone(a, { type: "square", from: s ? 1800 : 300, to: s ? 300 : 1400, dur: 0.18, gain: 0.07, attack: 0.002 });
    tone(a, { from: s ? 3600 : 600, to: s ? 600 : 2800, dur: 0.14, gain: 0.05, attack: 0.002 });
  },
  warp: (a, s) => {
    const [f, t] = s ? [200, 1600] : [1600, 200];
    tone(a, { from: f, to: t, dur: 0.32, gain: 0.18 });
    tone(a, { type: "triangle", from: f * 2, to: t * 2, dur: 0.32, gain: 0.06 });
  },
  sparkle: (a, s) => {
    const notes = s ? [1319, 1568, 2093, 2637] : [2637, 2093, 1568, 1319];
    notes.forEach((f, i) => tone(a, { from: f, at: i * 0.045, dur: 0.28, gain: 0.07, attack: 0.002 }));
  },
  sonar: (a, s) => {
    const f = s ? 1320 : 990;
    tone(a, { from: f, dur: 0.6, gain: 0.12 });
    tone(a, { from: f, at: 0.22, dur: 0.45, gain: 0.04 }); // faint echo
  },
  chime: (a, s) => {
    const [n1, n2] = s ? [1047, 1319] : [1319, 1047];
    for (const [f, at] of [[n1, 0], [n2, 0.09]]) {
      tone(a, { from: f, at, dur: 0.5, gain: 0.08 });
      tone(a, { from: f * 2.76, at, dur: 0.25, gain: 0.015 });
    }
  },
  bubble: (a, s) => {
    // a deep resonant body under the rising gloop, then a smaller second bubble
    tone(a, { from: s ? 110 : 220, to: s ? 220 : 110, dur: 0.18, gain: 0.26, attack: 0.004 });
    tone(a, { from: s ? 260 : 820, to: s ? 820 : 260, dur: 0.15, gain: 0.3, attack: 0.004 });
    tone(a, { type: "triangle", from: s ? 480 : 1300, to: s ? 1300 : 480, at: 0.08, dur: 0.12, gain: 0.14, attack: 0.003 });
  },
  marimba: (a, s) => {
    const notes = s ? [523, 784] : [784, 523];
    notes.forEach((f, i) => {
      tone(a, { type: "triangle", from: f, at: i * 0.08, dur: 0.22, gain: 0.18, attack: 0.002 });
      tone(a, { from: f * 4, at: i * 0.08, dur: 0.05, gain: 0.03, attack: 0.001 });
    });
  },
  glass: (a, s) => {
    const f = s ? 1760 : 1480;
    tone(a, { from: f, dur: 0.7, gain: 0.07, attack: 0.002 });
    tone(a, { from: f * 2.4, dur: 0.3, gain: 0.025, attack: 0.002 });
  },
  pop: (a, s) => {
    // snap + body + low thump so it lands with weight instead of a thin blip
    tone(a, { from: s ? 330 : 600, to: s ? 640 : 300, dur: 0.09, gain: 0.42, attack: 0.002 });
    tone(a, { from: s ? 140 : 120, to: s ? 70 : 60, dur: 0.11, gain: 0.3, attack: 0.002 });
    noise(a, { dur: 0.02, gain: 0.16, from: 2500, q: 1.5 });
  },
  click: (a, s) => noise(a, { dur: 0.025, gain: s ? 0.35 : 0.25, from: s ? 3500 : 2500, q: 2 }),
  swoosh: (a, s) => noise(a, { dur: 0.28, gain: 0.22, from: s ? 400 : 3000, to: s ? 3000 : 400, q: 1.4 }),
  arcade: (a, s) => {
    const notes = s ? [523, 659, 784] : [784, 659, 523];
    notes.forEach((f, i) => tone(a, { type: "square", from: f, at: i * 0.055, dur: 0.07, gain: 0.05, attack: 0.002 }));
  },
  tick: (a, s) => tone(a, { from: s ? 2000 : 1600, dur: 0.04, gain: 0.08, attack: 0.001 }),
};

export const primeAudio = () => void audio();

export function playSound(id: string, start: boolean) {
  (PLAY[id] ?? PLAY.heartbeat)(audio(), start);
}
