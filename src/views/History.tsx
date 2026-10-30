import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { attempt, call, Dictation, fmtTime, toast } from "../api";
import { Icon } from "../icons";

export default function History() {
  const [items, setItems] = useState<Dictation[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const load = () => call<Dictation[]>("list_history").then(setItems);
  useEffect(() => { load(); const u = listen("history-changed", load); return () => { u.then((f) => f()); }; }, []);

  const del = async (ids: string[], all = false) => {
    if (all && !confirm("Delete all dictation history? This can't be undone.")) return;
    const n = await attempt(call<number>("delete_history", { ids, all }));
    if (n !== undefined) toast(`Deleted ${n}`);
    setSelected(new Set());
    load();
  };
  const toggle = (id: string) => setSelected((s) => { const n = new Set(s); n.has(id) ? n.delete(id) : n.add(id); return n; });

  return (
    <div className="col" style={{ maxWidth: 900 }}>
      <div className="row">
        <span className="muted grow">{items.length} dictations · retention is set in Settings → Croaker</span>
        {selected.size > 0 && <button onClick={() => del([...selected])}>Delete {selected.size} selected</button>}
        <button className="ghost danger" disabled={!items.length} onClick={() => del([], true)}>Delete all history</button>
      </div>
      <div className="list">
        {items.map((d) => (
          <div key={d.id} className="list-item">
            <div className="row" style={{ alignItems: "flex-start" }}>
              <input type="checkbox" checked={selected.has(d.id)} onChange={() => toggle(d.id)} style={{ marginTop: 3 }} />
              <div className="grow">
                <div className="texts selectable">
                  <span className="k">Heard</span><span className="v raw">{d.raw || "—"}</span>
                  <span className="k">{d.via.startsWith("rules") || d.via === "command" ? "Inserted" : "Cleaned"}</span>
                  <span className="v">{d.final}{d.final.trim() === d.raw.trim() && <span className="faint"> · unchanged</span>}</span>
                </div>
                <div className="faint row wrap" style={{ marginTop: 6 }}>
                  <span>{fmtTime(d.time)}</span><span>· {d.app}</span>
                  <span className="chip croaker">{d.category}</span>
                  {d.language !== "en" && <span className="chip">EN → {d.language.toUpperCase()}</span>}
                  {d.stt && <span>· 🎙 {d.stt.replace(/^local:/, "on device · ")}</span>}
                  {d.via.startsWith("rules · AI failed")
                    ? <span className="danger-text selectable">· ⚠ {d.via.slice("rules · ".length)}</span>
                    : <span>· {d.via.startsWith("rules") ? "🔒 offline rules (no LLM)" : `✨ ${d.via}`}</span>}
                  <span className="num">· {d.latency_ms.toLocaleString()} ms</span>
                </div>
              </div>
              <button className="ghost" title="Copy" onClick={() => navigator.clipboard.writeText(d.final).then(() => toast("Copied"))}><Icon name="copy" size={14} /></button>
              <button className="ghost" title="Copy what was heard" onClick={() => navigator.clipboard.writeText(d.raw).then(() => toast("Copied raw transcript"))}>Raw</button>
              <button className="ghost" title="Delete" onClick={() => del([d.id])}><Icon name="trash" size={14} /></button>
            </div>
          </div>
        ))}
        {!items.length && <p className="muted">Nothing yet. Hold the Croaker hotkey and speak.</p>}
      </div>
    </div>
  );
}
