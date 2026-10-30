import { useEffect, useState } from "react";
import { attempt, call } from "../api";
import { Icon } from "../icons";

export function Dictionary() {
  const [terms, setTerms] = useState<string[]>([]);
  const [t, setT] = useState("");
  useEffect(() => { call<string[] | null>("get_store", { name: "dictionary.json" }).then((v) => setTerms(Array.isArray(v) ? v : [])); }, []);
  const save = (next: string[]) => { setTerms(next); attempt(call("save_store", { name: "dictionary.json", value: next })); };
  const add = () => { const v = t.trim(); if (v && !terms.includes(v)) save([...terms, v]); setT(""); };
  return (
    <div className="col" style={{ maxWidth: 640 }}>
      <p className="muted">Names and terms spelled exactly as you want them: they're fixed by the rules and sent to the LLM as vocabulary.</p>
      <div className="row">
        <input className="grow" placeholder="e.g. Priya, sherpa-onnx, kubectl" value={t} onChange={(e) => setT(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
        <button className="primary" onClick={add}>Add</button>
      </div>
      <div className="row wrap">
        {terms.map((x) => (
          <span key={x} className="chip" style={{ fontSize: 13, padding: "3px 10px" }}>
            {x} <a onClick={() => save(terms.filter((y) => y !== x))}>×</a>
          </span>
        ))}
      </div>
    </div>
  );
}

export function Snippets() {
  const [map, setMap] = useState<Record<string, string>>({});
  const [k, setK] = useState("");
  const [v, setV] = useState("");
  useEffect(() => { call<Record<string, string> | null>("get_store", { name: "snippets.json" }).then((x) => setMap(x && typeof x === "object" ? x : {})); }, []);
  const save = (next: Record<string, string>) => { setMap(next); attempt(call("save_store", { name: "snippets.json", value: next })); };
  const add = () => { if (k.trim() && v.trim()) save({ ...map, [k.trim().toLowerCase()]: v }); setK(""); setV(""); };
  return (
    <div className="col" style={{ maxWidth: 760 }}>
      <p className="muted">Say the trigger inside a dictation, or “insert snippet &lt;trigger&gt;” on its own.</p>
      <div className="row">
        <input placeholder="Trigger, e.g. my meeting link" value={k} onChange={(e) => setK(e.target.value)} style={{ width: 220 }} />
        <input className="grow" placeholder="Expands to…" value={v} onChange={(e) => setV(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
        <button className="primary" onClick={add}>Add</button>
      </div>
      <div className="list">
        {Object.entries(map).map(([key, val]) => (
          <div key={key} className="list-item row">
            <b style={{ width: 220 }}>{key}</b>
            <span className="grow selectable mono" style={{ whiteSpace: "pre-wrap" }}>{val}</span>
            <button className="ghost" onClick={() => { const n = { ...map }; delete n[key]; save(n); }}><Icon name="trash" size={14} /></button>
          </div>
        ))}
      </div>
    </div>
  );
}
