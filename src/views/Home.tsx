import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { attempt, call, prettyKey, Status } from "../api";
import type { Pane } from "./Settings";

type Step = { done: boolean; title: string; hint: string; optional?: boolean; action?: [string, () => void]; busy?: boolean };
type Model = { entry: { id: string; name: string; family: string; size: string }; status: string };

export function HomeSkeleton() {
  return (
    <div className="col" style={{ gap: 16, maxWidth: 880 }} aria-busy="true" aria-label="Loading">
      <div className="card col" style={{ gap: 14 }}>
        <div className="row"><div className="skeleton" style={{ width: 200, height: 16 }} /><span className="grow" /><div className="skeleton" style={{ width: 120, height: 18 }} /></div>
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <div key={i} className="row" style={{ gap: 12 }}>
            <div className="skeleton" style={{ width: 14, height: 14, borderRadius: 7 }} />
            <div className="col grow" style={{ gap: 6 }}>
              <div className="skeleton" style={{ width: `${40 + ((i * 17) % 30)}%`, height: 12 }} />
              <div className="skeleton" style={{ width: `${55 + ((i * 23) % 35)}%`, height: 10 }} />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

export default function Home({ status, go, refresh }: { status: Status; go: (v: string, p?: Pane) => void; refresh: () => void }) {
  const [stats, setStats] = useState<any>(null);
  const [mic, setMic] = useState<{ ok: boolean; name: string } | null>(null);
  const [cactus, setCactus] = useState<Model[] | null>(null);
  const [downloading, setDownloading] = useState(false);
  const loadModels = () => call<Model[]>("list_models").then((l) => setCactus(l.filter((m) => m.entry.family === "Cactus")));
  useEffect(() => {
    refresh();
    call("stats").then(setStats);
    call<string>("mic_check").then((name) => setMic({ ok: true, name }), (e) => setMic({ ok: false, name: String(e) }));
    loadModels();
    const u = listen("models-changed", () => { loadModels(); refresh(); });
    return () => { u.then((f) => f()); };
  }, []);
  const missing = (cactus ?? []).filter((m) => m.status !== "installed");
  const downloadMissing = async () => {
    setDownloading(true);
    for (const m of missing) await attempt(call("download_model", { id: m.entry.id }));
    setDownloading(false);
    loadModels();
    refresh();
  };

  const providers = () => go("settings", "AI Providers");
  const steps: Step[] = [
    {
      done: !!mic?.ok, title: "Microphone available",
      hint: mic ? (mic.ok ? mic.name : `${mic.name}. Check Windows Settings → Privacy & security → Microphone.`) : "Checking…",
    },
    {
      done: !status.hotkey_error, title: "Dictation hotkey works",
      hint: status.hotkey_error ?? `${prettyKey(status.hotkey)} — ${status.hotkey_mode === "toggle" ? "press to start, press again to stop" : status.hotkey_mode === "hold" ? "hold while speaking" : "hold to talk, or tap to go hands-free"}.`,
      action: status.hotkey_error ? ["Choose hotkey", () => go("settings", "Croaker")] : undefined,
    },
    {
      done: !!status.stt_ready, title: "Speech-to-text ready",
      hint: status.stt_ready
        ? `Using ${status.stt_ready}.`
        : "Pick “On this device” and download Cactus Whistle (17 MB, works offline), or pick Groq / OpenAI and paste a key.",
      action: status.stt_ready ? undefined : ["Set up", providers],
    },
    {
      done: !!status.keys.claude, title: "Anthropic key for cleanup, translation and notes",
      hint: status.keys.claude ? "Claude is connected." : "Without it, Croaker uses offline rules and Enhance/Ask won't work.",
      action: status.keys.claude ? undefined : ["Add key", providers],
    },
    {
      done: !!cactus && !missing.length, optional: true, title: "Download the Cactus on-device models",
      hint: !cactus ? "Checking…"
        : !missing.length ? "Whistle (speech) and Needle3 (commands) are downloaded."
        : downloading ? `Downloading ${missing.map((m) => m.entry.name).join(" and ")}…`
        : missing.length < cactus.length
          ? `${cactus.filter((m) => m.status === "installed").map((m) => m.entry.name).join(", ")} downloaded. Still missing: ${missing.map((m) => `${m.entry.name} (${m.entry.size})`).join(", ")}.`
          : "Whistle (speech, about 17 MB) and Needle3 (commands, about 36 MB). Other local models are in Settings → AI Providers.",
      action: missing.length ? [missing.length === 1 ? `Download ${missing[0].entry.name.replace(/^Cactus /, "")}` : "Download", downloadMissing] : undefined,
      busy: downloading,
    },
    {
      done: status.dictations > 0, title: "Try your first dictation",
      hint: status.dictations > 0 ? `${status.dictations} dictations so far.` : `Click into any text field, hold ${prettyKey(status.hotkey)}, speak, release.`,
    },
    {
      done: status.meetings > 0, optional: true, title: "Record a meeting",
      hint: status.meetings > 0 ? `${status.meetings} meetings in your library.` : `Press ${prettyKey(status.meeting_hotkey)} or start one from Meetings.`,
      action: status.meetings > 0 ? undefined : ["Open Meetings", () => go("meetings")],
    },
  ];
  const required = steps.filter((s) => !s.optional);
  const left = required.filter((s) => !s.done).length;

  return (
    <div className="col" style={{ gap: 16, maxWidth: 880 }}>
      <div className="card col" style={{ gap: 4 }}>
        <div className="row">
          <h3 className="grow">{left ? `Get started — ${left} step${left > 1 ? "s" : ""} left` : "You're all set"}</h3>
          <span className={`chip ${left ? "warn" : "ok"}`}>{required.length - left} of {required.length} required done</span>
        </div>
        {steps.map((s) => (
          <div key={s.title} className="row" style={{ padding: "8px 0", borderTop: "1px solid var(--border)", alignItems: "flex-start" }}>
            <span style={{ width: 20, textAlign: "center", color: s.done ? "var(--success)" : "var(--text-muted)", fontWeight: 600 }}>{s.done ? "✓" : "○"}</span>
            <div className="grow">
              <div style={{ fontWeight: s.done ? 400 : 500, color: s.done ? "var(--text-secondary)" : "var(--text-primary)" }}>
                {s.title} {s.optional && <span className="faint">(optional)</span>}
              </div>
              <div className="faint">{s.hint}</div>
            </div>
            {s.action && !s.done && <button className={s.optional ? "" : "primary"} disabled={s.busy} onClick={s.action[1]}>{s.busy ? "Downloading…" : s.action[0]}</button>}
          </div>
        ))}
      </div>

      {stats && stats.dictations > 0 && (
        <div className="grid2">
          <div className="card"><div className="stat">{stats.words.toLocaleString()}</div><div className="muted">words dictated</div></div>
          <div className="card"><div className="stat">{stats.minutes_saved}</div><div className="muted">minutes saved vs typing</div></div>
          <div className="card"><div className="stat">{stats.avg_latency_ms.toLocaleString()} ms</div><div className="muted">average release → text</div></div>
          <div className="card"><div className="stat">{stats.tokens_this_month.toLocaleString()}</div><div className="muted">LLM tokens this month</div></div>
          <div className="card" title="List prices per model family, without cache discounts">
            <div className="stat">${(stats.cost_this_month ?? 0).toFixed(stats.cost_this_month >= 1 ? 2 : 4)}{stats.cost_unpriced ? "+" : ""}</div>
            <div className="muted">estimated LLM cost this month{stats.cost_unpriced ? " · some models unpriced" : ""}</div>
          </div>
        </div>
      )}

      <div className="row wrap faint">
        <span className={`chip ${status.local_only ? "ok" : ""}`}>{status.local_only ? "🔒 Local only" : "☁️ Cloud models allowed"}</span>
        <span className={`chip ${status.send_audio_to_cloud ? "warn" : "ok"}`}>{status.send_audio_to_cloud ? "Audio may be sent to cloud STT" : "🔒 Audio stays on device"}</span>
        <span className="mono">{status.data_dir}</span>
      </div>
    </div>
  );
}
