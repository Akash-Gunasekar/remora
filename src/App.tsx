import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { call, isMac, Status } from "./api";
import { Icon } from "./icons";
import Home, { HomeSkeleton } from "./views/Home";
import Meetings from "./views/Meetings";
import History from "./views/History";
import { Dictionary, Snippets } from "./views/Lists";
import Styles from "./views/Styles";
import SettingsView, { Pane } from "./views/Settings";

const VIEWS = [
  { id: "home", label: "Home", icon: "home", group: "" },
  { id: "meetings", label: "Meetings", icon: "meetings", group: "Remora Notes" },
  { id: "history", label: "Dictation history", icon: "history", group: "Croaker", croaker: true },
  { id: "styles", label: "Styles", icon: "styles", group: "Croaker", croaker: true },
  { id: "snippets", label: "Snippets", icon: "snippets", group: "Croaker", croaker: true },
  { id: "dictionary", label: "Dictionary", icon: "dictionary", group: "Croaker", croaker: true },
  { id: "settings", label: "Settings", icon: "settings", group: "" },
] as const;
type ViewId = (typeof VIEWS)[number]["id"];

function TrafficLights() {
  const w = getCurrentWindow();
  return (
    <div className="traffic">
      <button className="close" aria-label="Close" onClick={() => w.close()}>
        <svg viewBox="0 0 8 8" stroke="currentColor" strokeWidth="1.2"><path d="M1.5 1.5l5 5M6.5 1.5l-5 5" /></svg>
      </button>
      <button className="min" aria-label="Minimize" onClick={() => w.minimize()}>
        <svg viewBox="0 0 8 8" stroke="currentColor" strokeWidth="1.2"><path d="M1 4h6" /></svg>
      </button>
      <button className="max" aria-label="Maximize" onClick={() => w.toggleMaximize()}>
        <svg viewBox="0 0 8 8" fill="currentColor"><path d="M1.5 1.5h3.5l-3.5 3.5zM6.5 6.5H3l3.5-3.5z" /></svg>
      </button>
    </div>
  );
}

export default function App() {
  const [view, setView] = useState<ViewId>("home");
  const [pane, setPane] = useState<Pane | undefined>();
  const go = (v: string, p?: Pane) => { setPane(p); setView(v as ViewId); };
  const [status, setStatus] = useState<Status | null>(null);
  const [toast, setToast] = useState<{ message: string; error: boolean } | null>(null);

  const refresh = () => call<Status>("status").then(setStatus);
  useEffect(() => {
    refresh();
    const onToast = (e: Event) => {
      setToast((e as CustomEvent).detail);
      setTimeout(() => setToast(null), (e as CustomEvent).detail.error ? 7000 : 2600);
    };
    window.addEventListener("toast", onToast);
    const un = [
      listen("meeting-state", refresh),
      listen<string>("meeting-error", (e) => onToast(new CustomEvent("toast", { detail: { message: e.payload, error: true } }))),
    ];
    return () => { window.removeEventListener("toast", onToast); un.forEach((p) => p.then((f) => f())); };
  }, []);

  const current = VIEWS.find((v) => v.id === view)!;
  let lastGroup = "";
  return (
    <div className="app">
      <aside className="sidebar" data-tauri-drag-region>
        {isMac ? <div className="mac-spacer" data-tauri-drag-region /> : <TrafficLights />}
        <div className="brand" data-tauri-drag-region>
          <img src="/app-icon.png" alt="" /> Remora
        </div>
        {VIEWS.map((v) => {
          const header = v.group && v.group !== lastGroup ? <div className="section">{v.group}</div> : null;
          lastGroup = v.group || lastGroup;
          return (
            <div key={v.id}>
              {header}
              <div className={`nav ${"croaker" in v ? "croaker" : ""} ${view === v.id ? "active" : ""}`} onClick={() => go(v.id)}>
                <Icon name={v.icon} /> {v.label}
                {v.id === "meetings" && status?.meeting_live && <span className="badge dot" />}
              </div>
            </div>
          );
        })}
        <div className="spacer" />
        <div className="faint" style={{ padding: "0 8px" }}>Local-first · data stays on this machine</div>
      </aside>
      <main className="content">
        <header className="titlebar" data-tauri-drag-region>
          <h2>{current.label}</h2>
        </header>
        <div className="view">
          {view === "home" && (status ? <Home status={status} go={go} refresh={refresh} /> : <HomeSkeleton />)}
          {view === "meetings" && <Meetings live={status?.meeting_live ?? null} />}
          {view === "history" && <History />}
          {view === "styles" && <Styles />}
          {view === "snippets" && <Snippets />}
          {view === "dictionary" && <Dictionary />}
          {view === "settings" && <SettingsView onChange={refresh} initialPane={pane} />}
        </div>
      </main>
      {toast && <div className={`toast ${toast.error ? "error" : ""}`}>{toast.message}</div>}
    </div>
  );
}
