import { useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { accelerator, attempt, call, prettyKey, Settings, Status, toast } from "../api";
import { Select } from "../components/ui/Select";
import { Segmented } from "../components/ui/Segmented";
import { playSound, SOUNDS } from "../sounds";

/** Click, then press the combo. Esc cancels. */
function HotkeyInput({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [recording, setRecording] = useState(false);
  const [held, setHeld] = useState("");
  useEffect(() => {
    if (!recording) return;
    const down = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") { setRecording(false); setHeld(""); return; }
      const acc = accelerator(e);
      if (acc) {
        setRecording(false);
        setHeld("");
        if (acc !== value) onChange(acc);
      } else {
        setHeld([e.ctrlKey && "Control", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean).join("+"));
      }
    };
    window.addEventListener("keydown", down, true);
    return () => window.removeEventListener("keydown", down, true);
  }, [recording, value]);
  return (
    <button className={`hotkey ${recording ? "recording" : ""}`} onClick={() => setRecording(!recording)} onBlur={() => setRecording(false)}>
      {recording ? (held ? `${prettyKey(held)} + …` : "Press a key combination…") : <span className="kbd">{prettyKey(value)}</span>}
    </button>
  );
}

function Switch({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return <label className="switch"><input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} /><span /></label>;
}

function Row({ title, hint, children }: { title: string; hint?: string; children: ReactNode }) {
  return (
    <div className="row" style={{ padding: "8px 0", alignItems: "flex-start" }}>
      <div className="grow"><div>{title}</div>{hint && <div className="faint">{hint}</div>}</div>
      <div className="row">{children}</div>
    </div>
  );
}

export const PANES = ["AI Providers", "Croaker", "Remora Notes", "Privacy & Data"] as const;
export type Pane = (typeof PANES)[number];

export default function SettingsView({ onChange, initialPane }: { onChange: () => void; initialPane?: Pane }) {
  const [pane, setPane] = useState<Pane>(initialPane ?? "AI Providers");
  useEffect(() => { if (initialPane) setPane(initialPane); }, [initialPane]);
  const [s, setS] = useState<Settings | null>(null);
  const [cfg, setCfg] = useState<any>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [langs, setLangs] = useState<[string, string][]>([]);
  const [models, setModels] = useState<Record<string, string[]>>({});

  const load = () => {
    call<Settings>("get_settings").then(setS);
    call("get_config").then(setCfg);
    call<Status>("status").then(setStatus);
    call<Record<string, string[]>>("models_cache").then((m) => setModels(m ?? {}));
  };
  useEffect(() => { load(); call<[string, string][]>("languages").then(setLangs); }, []);
  if (!s || !cfg || !status) return null;

  const saveSettings = async (next: Settings) => {
    setS(next);
    await attempt(call("save_settings", { settings: next }));
    onChange();
  };
  const saveCfg = async (next: any, msg?: string) => {
    setCfg(next);
    await attempt(call("save_config", { providers: next.providers, routing: next.routing }), msg);
    load();
    onChange();
  };
  const policies = cfg.providers.policies;
  const setPolicy = (k: string, v: any) => saveCfg({ ...cfg, providers: { ...cfg.providers, policies: { ...policies, [k]: v } } });

  return (
    <div className="col" style={{ maxWidth: 820, gap: 16 }}>
      <Segmented style={{ alignSelf: "flex-start" }}>
        {PANES.map((p) => <button key={p} className={pane === p ? "on" : ""} onClick={() => setPane(p)}>{p}</button>)}
      </Segmented>

      {pane === "AI Providers" && (
        <>
          <Choose kind="stt" cfg={cfg} models={models} status={status} reload={load} saveCfg={saveCfg} />
          <Choose kind="chat" cfg={cfg} models={models} status={status} reload={load} saveCfg={saveCfg} />
          <p className="faint">API keys are stored only in the OS keychain (Windows Credential Manager / macOS Keychain), never in config files.</p>

          <details>
            <summary className="faint">Advanced: privacy switches, per-module models and fallbacks</summary>
            <div className="col" style={{ gap: 12, marginTop: 10 }}>
              <div className="card">
                <Row title="Local only" hint="Blocks every cloud call. Cleanup falls back to Ollama or rules.">
                  <Switch checked={policies.local_only_mode} onChange={(v) => setPolicy("local_only_mode", v)} />
                </Row>
                <Row title="Send audio to cloud speech-to-text" hint="Turned on automatically when you pick a cloud speech-to-text provider.">
                  <Switch checked={policies.send_audio_to_cloud} onChange={(v) => setPolicy("send_audio_to_cloud", v)} />
                </Row>
              </div>
              <Models cfg={cfg} models={models} save={(r) => saveCfg({ ...cfg, routing: r }, "Models saved")} />
            </div>
          </details>
        </>
      )}

      {pane === "Croaker" && (
        <div className="card">
          <Row title="Dictation hotkey" hint="Click, then press the key combination.">
            <HotkeyInput value={s.hotkey} onChange={(hotkey) => saveSettings({ ...s, hotkey })} />
          </Row>
          <Row title="Hotkey behaviour" hint={{
            auto: "Hold to talk, release to insert — or tap once to go hands-free and tap again to stop.",
            hold: "Push-to-talk: recording runs only while the keys are held.",
            toggle: "Press once to start, press again to stop and insert.",
          }[s.hotkey_mode] ?? ""}>
            <Segmented>
              {[["auto", "Hold or tap"], ["hold", "Hold"], ["toggle", "Toggle"]].map(([m, l]) => (
                <button key={m} className={s.hotkey_mode === m ? "on" : ""} onClick={() => saveSettings({ ...s, hotkey_mode: m })}>{l}</button>
              ))}
            </Segmented>
          </Row>
          <Row title="Start and stop sounds" hint="Plays when the pill opens and closes. ▶ previews the open, then the close sound.">
            <Switch checked={s.sounds} onChange={(v) => saveSettings({ ...s, sounds: v })} />
            <Select value={s.sound_style || "heartbeat"} onChange={(e) => { saveSettings({ ...s, sound_style: e.target.value }); playSound(e.target.value, true); }}>
              {SOUNDS.map(([id, label]) => <option key={id} value={id}>{label}</option>)}
            </Select>
            <button className="ghost small" title="Preview" aria-label="Preview sound"
              onClick={() => { const id = s.sound_style || "heartbeat"; playSound(id, true); setTimeout(() => playSound(id, false), 700); }}>▶</button>
          </Row>
          <Row title="Waves while listening" hint="Soft blue waves rise out of the bars while Croaker listens. Off when the system asks for reduced motion.">
            <Segmented>
              {([["on", "On"], ["subtle", "Subtle"], ["off", "Off"]] as const).map(([m, l]) => (
                <button key={m} className={(s.sonar_ripples || "on") === m ? "on" : ""} onClick={() => saveSettings({ ...s, sonar_ripples: m })}>{l}</button>
              ))}
            </Segmented>
          </Row>
          <Row title="Output language" hint="Speak English, insert in another language. One LLM call does cleanup and translation.">
            <Select value={s.output_language} onChange={(e) => saveSettings({ ...s, output_language: e.target.value })}>
              {langs.map(([c, n]) => <option key={c} value={c}>{n}</option>)}
            </Select>
          </Row>
          <Row title="Per-app output language" hint='e.g. ms-teams.exe=ta, outlook.exe=en (one per line)'>
            <textarea rows={2} style={{ width: 260 }} defaultValue={Object.entries(s.per_app_language).map(([k, v]) => `${k}=${v}`).join("\n")}
              onBlur={(e) => saveSettings({ ...s, per_app_language: Object.fromEntries(e.target.value.split("\n").map((l) => l.split("=").map((x) => x.trim())).filter((x) => x[0] && x[1])) })} />
          </Row>
          <Row title="Tone">
            <Select value={s.tone} onChange={(e) => saveSettings({ ...s, tone: e.target.value })}>
              {["friendly-professional", "professional", "casual", "concise"].map((t) => <option key={t}>{t}</option>)}
            </Select>
          </Row>
          <Row title="AI cleanup" hint="Enhance sends every dictation (except terminals) to your dictation LLM with the app's style. Clean uses fast offline rules and calls the LLM only for fillers, corrections, lists, long or email text.">
            <Segmented>
              {[["enhance", "Enhance"], ["clean", "Clean"]].map(([m, label]) => <button key={m} className={s.mode === m ? "on" : ""} onClick={() => saveSettings({ ...s, mode: m })}>{label}</button>)}
            </Segmented>
          </Row>
          <Row title="Email auto sign-off">
            <Switch checked={s.auto_signoff} onChange={(v) => saveSettings({ ...s, auto_signoff: v })} />
            <input defaultValue={s.signoff} onBlur={(e) => saveSettings({ ...s, signoff: e.target.value })} style={{ width: 140 }} />
          </Row>
          <Row title="Disable Croaker in apps" hint="Executable or app names, comma-separated (e.g. keepass.exe)">
            <input defaultValue={s.disabled_apps.join(", ")} onBlur={(e) => saveSettings({ ...s, disabled_apps: e.target.value.split(",").map((x) => x.trim()).filter(Boolean) })} style={{ width: 260 }} />
          </Row>
          <div className="divider" />
          <h3>History</h3>
          <Row title="Save history"><Switch checked={s.history_enabled} onChange={(v) => saveSettings({ ...s, history_enabled: v })} /></Row>
          <Row title="Keep last" hint="Count and age both apply; whichever removes more wins.">
            <Select value={s.history_keep_last ?? ""} onChange={(e) => saveSettings({ ...s, history_keep_last: e.target.value ? +e.target.value : null })}>
              {[25, 50, 100, 500, 1000].map((n) => <option key={n} value={n}>{n}</option>)}<option value="">Unlimited</option>
            </Select>
            <Select value={s.history_keep_days ?? ""} onChange={(e) => saveSettings({ ...s, history_keep_days: e.target.value ? +e.target.value : null })}>
              {[[1, "24 h"], [7, "7 days"], [30, "30 days"], [90, "90 days"], [365, "1 year"]].map(([n, l]) => <option key={n} value={n}>{l}</option>)}<option value="">Forever</option>
            </Select>
          </Row>
        </div>
      )}

      {pane === "Remora Notes" && (
        <div className="card">
          <Row title="Meeting hotkey" hint="Click, then press the combo. Press it once to start, again to stop.">
            <HotkeyInput value={s.meeting_hotkey} onChange={(meeting_hotkey) => saveSettings({ ...s, meeting_hotkey })} />
          </Row>
          <Row title="Keep meeting audio" hint="Transcripts and notes are kept regardless. Pinned meetings are exempt.">
            <Select value={s.meeting_audio_keep_days ?? ""} onChange={(e) => saveSettings({ ...s, meeting_audio_keep_days: e.target.value === "" ? null : +e.target.value })}>
              {[[0, "Never save"], [1, "24 h"], [7, "7 days"], [30, "30 days"]].map(([n, l]) => <option key={n} value={n}>{l}</option>)}<option value="">Forever</option>
            </Select>
          </Row>
          <Row title="Trash keeps deleted items for">
            <Select value={s.trash_days} onChange={(e) => saveSettings({ ...s, trash_days: +e.target.value })}>
              {[[0, "Delete immediately"], [7, "7 days"], [30, "30 days"]].map(([n, l]) => <option key={n} value={n}>{l}</option>)}
            </Select>
          </Row>
          <p className="faint">Speakers: your mic is labelled “You”, system audio “Remote”. Rename a speaker in the transcript to apply it to the whole meeting.</p>
        </div>
      )}

      {pane === "Privacy & Data" && <Privacy status={status} />}
    </div>
  );
}

function ProviderCard({ p, hasKey, verified, onSaved, onBaseUrl }: {
  p: any; hasKey: boolean; verified?: string[]; onSaved: () => void; onBaseUrl: (u: string) => void;
}) {
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const needsKey = !["ollama", "openai_compatible"].includes(p.type) || p.id === "ollama-cloud";
  const verify = async () => {
    setBusy(true);
    const m = await attempt(call<string[]>("probe_provider", { id: p.id }));
    setBusy(false);
    if (m) toast(`${LABEL[p.id] ?? p.id} connected — ${m.length} models available`);
    onSaved();
  };
  const saveKey = async () => {
    if ((await attempt(call("set_api_key", { id: p.id, key }))) === undefined) return;
    setKey("");
    await verify();
  };
  return (
    <div className="col" style={{ gap: 8 }}>
      <div className="row">
        <span className="grow faint">{needsKey ? (hasKey ? "" : "Paste your API key to connect.") : "Runs on this machine; make sure the server is running."}</span>
        {verified ? <span className="chip ok">Connected · {verified.length} models</span>
          : needsKey ? <span className={`chip ${hasKey ? "warn" : ""}`}>{hasKey ? "Key saved · not verified" : "No key"}</span>
          : <span className="chip">Not connected</span>}
        <button onClick={verify} disabled={busy || (needsKey && !hasKey)}>{busy ? "Connecting…" : verified ? "Refresh models" : "Connect"}</button>
      </div>
      {needsKey && (
        <div className="row">
          <input className="grow" type="password" placeholder={hasKey ? "••••••••  (saved — paste to replace)" : "Paste API key"} value={key}
            onChange={(e) => setKey(e.target.value)} onKeyDown={(e) => e.key === "Enter" && key && saveKey()} />
          <button className="primary" disabled={!key || busy} onClick={saveKey}>Save & connect</button>
          {hasKey && <button className="ghost danger" onClick={async () => { await attempt(call("set_api_key", { id: p.id, key: "" }), "Key removed"); onSaved(); }}>Remove</button>}
        </div>
      )}
      {(p.type === "ollama" || p.type === "openai_compatible") && (
        <label className="field">Base URL<input defaultValue={p.base_url ?? ""} onBlur={(e) => e.target.value !== (p.base_url ?? "") && onBaseUrl(e.target.value)} /></label>
      )}
    </div>
  );
}

const LABEL: Record<string, string> = {
  claude: "Anthropic Claude", groq: "Groq", openai: "OpenAI", openrouter: "OpenRouter",
  "ollama-local": "Ollama (on this machine)", lmstudio: "LM Studio (on this machine)",
};
/** Preferred model when a provider is first picked: [AI model, speech-to-text model]. */
const PREFERRED: Record<string, [string, string]> = {
  claude: ["claude-haiku-5-5", ""], groq: ["llama-3.3-70b-versatile", "whisper-large-v3-turbo"], openai: ["gpt-4o-mini", "gpt-4o-mini-transcribe"],
  openrouter: ["anthropic/claude-haiku-5-5", ""], "ollama-local": ["llama3.2", ""], lmstudio: ["", "whisper-1"],
};
const NOT_STT = new Set(["needle3"]);

/** One provider dropdown + one model dropdown, applied to both Croaker and Remora Notes. */
function Choose({ kind, cfg, models, status, reload, saveCfg }: {
  kind: Kind; cfg: any; models: Record<string, string[]>; status: Status; reload: () => void; saveCfg: (c: any, msg?: string) => void;
}) {
  const tasks = kind === "stt" ? ["stt.dictation", "stt.meeting"] : ["dictation", "meeting"];
  const primary: string = cfg.routing[tasks[0]]?.chain?.[0] ?? "";
  const current = primary === "rules" ? "rules" : primary.split(":")[0];
  const [shown, setShown] = useState(current);
  const [local, setLocal] = useState<LocalModel[]>([]);
  const loadLocal = () => call<LocalModel[]>("list_models").then(setLocal);
  useEffect(() => { if (kind === "stt") loadLocal(); }, []);

  const providers: any[] = cfg.providers.providers.filter((p: any) => kind === "chat" || !["anthropic", "ollama"].includes(p.type) && p.id !== "openrouter");
  const p = providers.find((x) => x.id === shown);
  const options: string[] =
    shown === "local" ? local.filter((m) => m.runnable && !NOT_STT.has(m.entry.id)).map((m) => m.entry.id)
    : (models[shown] ?? []).filter((m) => (kind === "stt" ? isStt(m) : !isStt(m) && !notChat(m)));

  const select = (target: string) => {
    const routing = { ...cfg.routing };
    for (const t of tasks) {
      const chain: string[] = routing[t]?.chain ?? [];
      routing[t] = { ...routing[t], chain: [target, ...chain.filter((c) => c !== target)] };
    }
    const policies = kind === "stt" && !target.startsWith("local:")
      ? { ...cfg.providers.policies, send_audio_to_cloud: true, local_only_mode: false }
      : cfg.providers.policies;
    saveCfg({ ...cfg, routing, providers: { ...cfg.providers, policies } }, "Saved");
  };
  const best = (id: string, list: string[]) => {
    const pref = PREFERRED[id]?.[kind === "stt" ? 1 : 0];
    return list.includes(pref) ? pref : list[0];
  };
  // Once the picked provider connects (or a local model finishes downloading), use it straight away.
  useEffect(() => {
    if (shown === current || shown === "rules" || !options.length) return;
    select(`${shown}:${best(shown, options)}`);
  }, [shown, options.join()]);

  const pick = (id: string) => {
    setShown(id);
    if (id === "rules") select("rules");
  };
  const model = primary.slice(current.length + 1);

  return (
    <div className="card col" style={{ gap: 10 }}>
      <div>
        <h3>{kind === "stt" ? "Speech-to-text" : "AI model"}</h3>
        <p className="faint">{kind === "stt" ? "Turns your voice into text, for dictation and meetings." : "Cleans up dictation, translates, and writes meeting notes, action items and answers."}</p>
      </div>
      <div className="row">
        <span style={{ width: 80 }}>Provider</span>
        <Select className="grow" value={shown} onChange={(e) => pick(e.target.value)}>
          {kind === "stt" && <option value="local">On this device (no internet)</option>}
          {providers.map((x) => <option key={x.id} value={x.id}>{LABEL[x.id] ?? x.id}{models[x.id] ? " ✓" : ""}</option>)}
          {kind === "chat" && <option value="rules">None — basic offline cleanup only</option>}
        </Select>
      </div>
      {shown !== "rules" && (
        <div className="row">
          <span style={{ width: 80 }}>Model</span>
          <Select className="grow" value={shown === current ? model : ""} disabled={!options.length} onChange={(e) => select(`${shown}:${e.target.value}`)}>
            {shown !== current && <option value="">{options.length ? "Choose a model…" : "Connect the provider first"}</option>}
            {shown === current && !options.includes(model) && <option value={model}>{model}</option>}
            {options.map((m) => <option key={m} value={m}>{shown === "local" ? local.find((l) => l.entry.id === m)?.entry.name ?? m : m}</option>)}
          </Select>
        </div>
      )}
      {p && (
        <ProviderCard p={p} hasKey={!!status.keys[p.id]} verified={models[p.id]} onSaved={reload}
          onBaseUrl={(url) => {
            const ps = cfg.providers.providers.map((x: any) => (x.id === p.id ? { ...x, base_url: url || undefined } : x));
            saveCfg({ ...cfg, providers: { ...cfg.providers, providers: ps } }, "Saved");
          }} />
      )}
      {shown === "local" && <LocalModels only={(id) => kind !== "stt" || !NOT_STT.has(id)} onChange={loadLocal} />}
      {kind === "stt" && shown !== "local" && <p className="faint">Audio is sent to {LABEL[shown] ?? shown} for transcription.</p>}
    </div>
  );
}

type Kind = "chat" | "stt";
const MODULES: { name: string; tasks: [string, string, Kind][] }[] = [
  { name: "Croaker", tasks: [["stt.dictation", "Speech-to-text", "stt"], ["dictation", "AI model · cleanup, translation, commands", "chat"]] },
  { name: "Remora Notes", tasks: [["stt.meeting", "Speech-to-text", "stt"], ["meeting", "AI model · summary, action items, decisions, Ask, translation", "chat"]] },
];
const isStt = (m: string) => /whisper|transcri|parakeet|saaras|saarika/i.test(m);
const notChat = (m: string) => /embed|moderation|dall-e|image|tts|realtime|audio|guard/i.test(m);

type LocalModel = { entry: { id: string; name: string; family: string; repo: string; size: string; languages: string; runtime: string; best_for: string; license: string }; status: string; bytes: number; runnable: boolean };

function LocalModels({ only = () => true, onChange }: { only?: (id: string) => boolean; onChange?: () => void }) {
  const [all, setList] = useState<LocalModel[]>([]);
  const [prog, setProg] = useState<Record<string, { done: number; total: number }>>({});
  const list = all.filter((m) => only(m.entry.id));
  const load = () => call<LocalModel[]>("list_models").then((l) => { setList(l); onChange?.(); });
  useEffect(() => {
    load();
    const u = listen<{ id: string; done: number; total: number }>("model-progress", (e) => setProg((p) => ({ ...p, [e.payload.id]: e.payload })));
    const u2 = listen("models-changed", () => { setProg({}); load(); });
    return () => { u.then((f) => f()); u2.then((f) => f()); };
  }, []);
  const download = async (id: string) => {
    setProg((p) => ({ ...p, [id]: { done: 0, total: 0 } }));
    await attempt(call("download_model", { id }), "Download complete");
    setProg((p) => { const n = { ...p }; delete n[id]; return n; });
    load();
  };
  const remove = async (m: LocalModel) => {
    if (!confirm(`Delete ${m.entry.name} from this machine?`)) return;
    await attempt(call("delete_model", { id: m.entry.id }), "Model deleted");
    load();
  };
  const mb = (b: number) => `${(b / 1e6).toFixed(b > 1e8 ? 0 : 1)} MB`;
  return (
    <div className="col" style={{ gap: 0 }}>
      <p className="faint" style={{ marginBottom: 8 }}>
        Download a model and Remora switches to it automatically. Files come from Hugging Face and are checksum-verified.
      </p>
      {list.map((m) => {
        const p = prog[m.entry.id];
        return (
          <div key={m.entry.id} className="row" style={{ padding: "8px 0", borderTop: "1px solid var(--border)", alignItems: "flex-start" }}>
            <div className="grow">
              <div className="row">
                <b>{m.entry.name}</b>
                {m.entry.family === "Cactus" && <span className="chip">Default</span>}
                {m.status === "installed" && <span className="chip ok">Downloaded · {mb(m.bytes)}</span>}
                {m.status === "installed" && !p && (m.runnable
                  ? <span className="chip ok">Ready offline</span>
                  : m.entry.id !== "needle3" && <span className="chip warn">Engine not built in yet</span>)}
                {m.status === "partial" && !p && <span className="chip warn">Incomplete — resume</span>}
              </div>
              <div className="faint">{m.entry.best_for}</div>
              <div className="faint">{m.entry.size} · {m.entry.languages} · {m.entry.runtime} · {m.entry.license}</div>
              {p && (
                <div style={{ marginTop: 6 }}>
                  <div style={{ height: 4, background: "var(--surface-sunken)", borderRadius: 2, overflow: "hidden" }}>
                    <div style={{ height: "100%", width: `${p.total ? (100 * p.done) / p.total : 2}%`, background: "var(--accent)", transition: "width 150ms" }} />
                  </div>
                  <div className="faint">{p.total ? `${mb(p.done)} of ${mb(p.total)}` : "Fetching file list…"}</div>
                </div>
              )}
            </div>
            {m.status !== "installed" && <button className="primary" disabled={!!p} onClick={() => download(m.entry.id)}>{p ? "Downloading…" : m.status === "partial" ? "Resume" : "Download"}</button>}
            {m.status !== "available" && !p && <button className="ghost danger" onClick={() => remove(m)}>Delete</button>}
          </div>
        );
      })}
      <div className="row" style={{ paddingTop: 8, borderTop: "1px solid var(--border)" }}>
        <span className="faint grow">Offline import: copy the model's files into its folder, then press Download to verify without re-downloading. The list lives in config/models-catalog.json.</span>
        <button onClick={() => attempt(call("reveal_models"))}>Open models folder</button>
      </div>
    </div>
  );
}

function Models({ cfg, models, save }: { cfg: any; models: Record<string, string[]>; save: (r: any) => void }) {
  const providers: any[] = cfg.providers.providers;
  const groups = (kind: Kind) =>
    providers
      .filter((p) => models[p.id]?.length && (kind === "chat" || !["anthropic", "ollama"].includes(p.type)))
      .map((p) => [p.id, models[p.id].filter((m) => (kind === "stt" ? isStt(m) : !isStt(m) && !notChat(m)))] as const)
      .filter(([, ms]) => ms.length);
  const setChain = (task: string, chain: string[]) => save({ ...cfg.routing, [task]: { ...cfg.routing[task], chain } });
  const anyVerified = providers.some((p) => models[p.id]?.length);
  const [local, setLocal] = useState<[string, string][]>([]);
  useEffect(() => {
    call<LocalModel[]>("list_models").then((l) => setLocal(l
      .filter((m) => m.runnable && !NOT_STT.has(m.entry.id))
      .map((m) => [`local:${m.entry.id}`, `${m.entry.name} · on device`])));
  }, []);

  return (
    <div className="col" style={{ gap: 12 }}>
      {!anyVerified && (
        <div className="banner">
          <p className="grow">No verified providers yet. Paste a key above and press <b>Save & verify</b>; its models then show up here.</p>
        </div>
      )}
      <p className="muted">Pick a model for each task. If it fails or times out, Remora tries the next one down.</p>
      {MODULES.map(({ name, tasks }) => (
        <div key={name} className="card col" style={{ gap: 10 }}>
          <h3>{name}</h3>
          {tasks.map(([task, label, kind], ti) => {
            const chain: string[] = cfg.routing[task]?.chain ?? [];
            const g = groups(kind);
            const known = new Set(g.flatMap(([pid, ms]) => ms.map((m) => `${pid}:${m}`)));
            const builtin: [string, string][] = kind === "chat" ? [["rules", "Offline rules (no AI)"]] : local.length ? local : [["local:whistle", "Cactus Whistle · on device (download it first)"]];
            const isBuiltin = (v: string) => builtin.some(([b]) => b === v);
            return (
          <div key={task} className="col" style={{ gap: 6, paddingTop: ti ? 10 : 0, borderTop: ti ? "1px solid var(--border)" : "none" }}>
            <b>{label}</b>
            {chain.map((entry, i) => (
              <div key={i} className="row">
                <span className="faint" style={{ width: 70 }}>{i === 0 ? "Primary" : `Fallback ${i}`}</span>
                <Select className="grow" value={entry} onChange={(e) => setChain(task, chain.map((c, j) => (j === i ? e.target.value : c)))}>
                  {!known.has(entry) && !isBuiltin(entry) && <option value={entry}>{entry} (not verified)</option>}
                  {g.map(([pid, ms]) => <optgroup key={pid} label={pid}>{ms.map((m) => <option key={m} value={`${pid}:${m}`}>{m}</option>)}</optgroup>)}
                  <optgroup label="Built-in">{builtin.map(([v, l]) => <option key={v} value={v}>{l}</option>)}</optgroup>
                </Select>
                <button className="ghost" disabled={i === 0} title="Move up" onClick={() => { const c = [...chain]; [c[i - 1], c[i]] = [c[i], c[i - 1]]; setChain(task, c); }}>↑</button>
                <button className="ghost" disabled={chain.length === 1} title="Remove" onClick={() => setChain(task, chain.filter((_, j) => j !== i))}>×</button>
              </div>
            ))}
            <a style={{ alignSelf: "flex-start" }} onClick={() => setChain(task, [...chain, g[0] ? `${g[0][0]}:${g[0][1][0]}` : builtin[0][0]])}>+ Add fallback</a>
          </div>
            );
          })}
        </div>
      ))}
      <details>
        <summary className="faint">Advanced: edit routing.json</summary>
        <RawRouting key={JSON.stringify(cfg.routing)} routing={cfg.routing} save={save} />
      </details>
    </div>
  );
}

function RawRouting({ routing, save }: { routing: any; save: (r: any) => void }) {
  const [text, setText] = useState(JSON.stringify(routing, null, 2));
  return (
    <div className="col" style={{ marginTop: 8 }}>
      <textarea className="mono" rows={18} value={text} onChange={(e) => setText(e.target.value)} />
      <button style={{ alignSelf: "flex-start" }} onClick={() => { try { save(JSON.parse(text)); } catch (e) { toast(`Invalid JSON: ${e}`, true); } }}>Save routing.json</button>
    </div>
  );
}

function Privacy({ status }: { status: Status }) {
  const [confirmText, setConfirmText] = useState("");
  const [models, setModels] = useState(false);
  const [keys, setKeys] = useState(true);
  const delCat = async (c: string, label: string) => {
    if (confirm(`Delete all ${label}?`)) await attempt(call("delete_category", { category: c }), `Deleted ${label}`);
  };
  return (
    <>
      <div className="card">
        <Row title="Data folder" hint={status.data_dir}><button onClick={() => attempt(call("reveal_data"))}>Reveal</button></Row>
        <p className="faint">No database and no cloud sync, ever. Everything is plain files in this folder; delete a file and the data is gone.</p>
      </div>
      <div className="card">
        <h3 style={{ marginBottom: 8 }}>Delete by category</h3>
        <div className="row wrap">
          {[["history", "dictation history"], ["meetings", "meetings"], ["dictionary", "dictionary"], ["snippets", "snippets"], ["people", "people"],
            ["voice-profiles", "voice profiles"], ["usage", "usage stats"], ["logs", "logs"], ["trash", "trash"], ["index", "search index"]].map(([c, l]) => (
            <button key={c} onClick={() => delCat(c, l)}>{l}</button>
          ))}
        </div>
      </div>
      <div className="card col">
        <h3>Delete all Remora data</h3>
        <label className="row"><input type="checkbox" checked={models} onChange={(e) => setModels(e.target.checked)} /> Include downloaded models</label>
        <label className="row"><input type="checkbox" checked={keys} onChange={(e) => setKeys(e.target.checked)} /> Include saved API keys</label>
        <div className="row">
          <input placeholder="Type DELETE" value={confirmText} onChange={(e) => setConfirmText(e.target.value)} />
          <button className="danger" disabled={confirmText !== "DELETE"}
            onClick={async () => { await attempt(call("delete_all_data", { confirm: confirmText, includeModels: models, includeKeys: keys }), "All data deleted"); setConfirmText(""); location.reload(); }}>
            Delete everything
          </button>
        </div>
      </div>
    </>
  );
}
