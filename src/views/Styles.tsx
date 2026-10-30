import { useEffect, useState } from "react";
import { attempt, call, CATEGORIES } from "../api";
import { Select } from "../components/ui/Select";

export default function Styles() {
  const [category, setCategory] = useState<string>("work_chat");
  const [profile, setProfile] = useState<{ base: string; default: string; custom: string } | null>(null);
  const [draft, setDraft] = useState("");
  const [sample, setSample] = useState("hey so the build is broken again because of the uh windows signing step, Priya's looking at it, should be fixed by like 3, no make that 4");
  const [out, setOut] = useState<any>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    call("get_profile", { category }).then((p: any) => { setProfile(p); setDraft(p.custom || p.default); });
  }, [category]);

  const save = async (reset = false) => {
    await attempt(call("save_profile", { category, prompt: reset ? "" : draft === profile?.default ? "" : draft }), reset ? "Reset to default" : "Profile saved");
    const p: any = await call("get_profile", { category });
    setProfile(p);
    setDraft(p.custom || p.default);
  };
  const test = async () => {
    setBusy(true);
    const label = CATEGORIES.find((c) => c[0] === category)?.[1] ?? "";
    const app = label.match(/\(([^,)]+)/)?.[1] ?? label;
    setOut(await attempt(call("test_cleanup", { text: sample, category, app })));
    setBusy(false);
  };

  return (
    <div className="col" style={{ gap: 16, maxWidth: 980 }}>
      <p className="muted">
        Croaker detects the focused app and picks a profile. The prompt is the base rules (L0) plus this profile (L1–L2), your tone and mode (L4),
        and the transcript with vocabulary (L5). Translation is never applied to agents, terminals or code editors.
      </p>
      <div className="row">
        <Select value={category} onChange={(e) => { setCategory(e.target.value); setOut(null); }}>
          {CATEGORIES.map(([id, label]) => <option key={id} value={id}>{label}</option>)}
        </Select>
        {profile?.custom ? <span className="chip croaker">Custom</span> : <span className="chip">Built-in</span>}
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 16 }}>
        <div className="col">
          <label className="field">Profile prompt</label>
          <textarea className="mono" rows={18} value={draft} onChange={(e) => setDraft(e.target.value)} />
          <div className="row">
            <button className="primary" onClick={() => save()}>Save</button>
            <button className="ghost" onClick={() => save(true)} disabled={!profile?.custom}>Reset to default</button>
          </div>
          <details>
            <summary className="faint">Base prompt (L0, applies to every app)</summary>
            <pre className="mono selectable" style={{ whiteSpace: "pre-wrap" }}>{profile?.base}</pre>
          </details>
        </div>
        <div className="col">
          <label className="field">Live test — type or paste a raw transcript</label>
          <textarea rows={6} value={sample} onChange={(e) => setSample(e.target.value)} />
          <button onClick={test} disabled={busy} style={{ alignSelf: "flex-start" }}>{busy ? "Running…" : "Run cleanup"}</button>
          {out && (
            <div className="card col">
              <div className="row faint"><span className="chip croaker">{out.via}</span>{out.is_command && <span>command: {out.label}</span>}{out.notice && <span className="chip warn">{out.notice}</span>}</div>
              <div className="selectable" style={{ whiteSpace: "pre-wrap", fontSize: 15 }}>{out.insert ?? "(no text inserted)"}</div>
            </div>
          )}
          <p className="faint">Save the profile first to test edits. Without an Anthropic key this shows the offline rules output.</p>
        </div>
      </div>
    </div>
  );
}
