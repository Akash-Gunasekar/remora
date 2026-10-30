import { useEffect, useRef, useState, type JSX } from "react";
import { listen } from "@tauri-apps/api/event";
import { attempt, call, ChatEntry, fmtTime, Meeting, MeetingFull, Segment, speakerColor, toast } from "../api";
import { Icon } from "../icons";
import { Markdown } from "../md";
import { Select } from "../components/ui/Select";
import { Segmented } from "../components/ui/Segmented";

const ts = (ms: number) => `${String(Math.floor(ms / 60000)).padStart(2, "0")}:${String(Math.floor(ms / 1000) % 60).padStart(2, "0")}`;
const isMe = (s: string) => s === "You";
/** Summaries that are the AI notes themselves, not translations of them ("01_general.ta.md"). */
const mainSummaries = (m: MeetingFull) => m.summaries.filter(([f]) => !f.replace(/\.md$/, "").includes("."));

export default function Meetings({ live }: { live: string | null }) {
  const [list, setList] = useState<Meeting[]>([]);
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<string | null>(live);
  const [templates, setTemplates] = useState<[string, string][]>([]);

  const load = () => call<Meeting[]>("list_meetings", { query }).then(setList);
  useEffect(() => { load(); }, [query]);
  useEffect(() => { call<[string, string][]>("templates").then(setTemplates); }, []);
  useEffect(() => { const u = listen("meetings-changed", load); return () => { u.then((f) => f()); }; }, [query]);
  useEffect(() => { if (live) setSel(live); }, [live]);

  const start = async () => {
    const id = await attempt(call<string>("start_meeting"));
    if (id) setSel(id);
  };

  return (
    <div className="split">
      <div className="left">
        <div className="card col" style={{ padding: 12 }}>
          {live ? (
            <>
              <div className="row"><span className="dot" /> <b>Recording</b></div>
              <button className="record" onClick={() => attempt(call("stop_meeting"), "Meeting saved")}>
                <span className="row" style={{ justifyContent: "center" }}><Icon name="stop" size={14} /> Stop</span>
              </button>
            </>
          ) : (
            <>
              <button className="primary" onClick={start}>Start meeting</button>
              <p className="faint">The title and meeting type are filled in by AI when you stop. Consent: tell everyone you're taking notes. Nothing joins the call.</p>
            </>
          )}
        </div>
        <div className="row" style={{ position: "relative" }}>
          <input className="grow" placeholder="Search meetings" value={query} onChange={(e) => setQuery(e.target.value)} style={{ paddingLeft: 28 }} />
          <span style={{ position: "absolute", left: 8, top: 7, color: "var(--text-muted)" }}><Icon name="search" size={14} /></span>
        </div>
        <div className="list">
          {list.map((m) => (
            <div key={m.id} className={`list-item ${sel === m.id ? "active" : ""}`} onClick={() => setSel(m.id)}>
              <div className="row">
                <b className="grow" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{m.title}</b>
                {m.id === live && <span className="dot" />}
              </div>
              <div className="faint">{fmtTime(m.started_at)} · {m.capture === "mic" ? "in-person" : "call"}</div>
            </div>
          ))}
          {!list.length && <p className="faint" style={{ padding: 12 }}>{query ? "No matches." : "No meetings yet."}</p>}
        </div>
      </div>
      <div className="right meeting-pane">
        {sel ? <Detail key={sel} id={sel} live={sel === live} templates={templates} onDeleted={() => { setSel(null); load(); }} /> :
          <p className="muted">Select a meeting or start a new one.</p>}
      </div>
    </div>
  );
}

function Detail({ id, live, templates, onDeleted }: { id: string; live: boolean; templates: [string, string][]; onDeleted: () => void }) {
  const [m, setM] = useState<MeetingFull | null>(null);
  const [view, setView] = useState<"enhanced" | "notes">(live ? "notes" : "enhanced");
  const [file, setFile] = useState("");
  const [busy, setBusy] = useState("");
  const load = () => call<MeetingFull>("get_meeting", { id }).then(setM).catch((e) => toast(e, true));
  useEffect(() => { load(); }, [id]);
  useEffect(() => {
    const u = listen<{ meeting: string; segment: Segment }>("transcript", (e) => {
      if (e.payload.meeting === id) setM((p) => p && { ...p, transcript: [...p.transcript, e.payload.segment] });
    });
    const u2 = listen("meetings-changed", load);
    const u3 = listen<string>("meeting-enhancing", (e) => e.payload === id && setBusy("enhance"));
    const u4 = listen<{ id: string; error: string | null }>("meeting-enhanced", (e) => {
      if (e.payload.id !== id) return;
      setBusy("");
      setView("enhanced");
      setFile("");
      load();
      if (e.payload.error && !e.payload.error.startsWith("Nothing to enhance")) toast(`Auto-enhance failed: ${e.payload.error}`, true);
    });
    return () => { [u, u2, u3, u4].forEach((x) => x.then((f) => f())); };
  }, [id]);

  if (!m) return null;
  const run = async (label: string, p: Promise<unknown>, ok?: string) => {
    setBusy(label);
    await attempt(p, ok);
    setBusy("");
    load();
  };
  const del = async (part: string, name = "") => {
    const what = part === "meeting" ? "this whole meeting (moves to Trash)" : part;
    if (!confirm(`Delete ${what}? Anything already pushed to other tools isn't affected.`)) return;
    await attempt(call("delete_meeting_part", { id, part, name }));
    part === "meeting" ? onDeleted() : load();
  };

  const summaries = mainSummaries(m);
  const shown = m.summaries.find(([f]) => f === file) ?? summaries[summaries.length - 1];
  const people = [...new Set(m.transcript.map((s) => s.speaker))].filter((s) => !isMe(s));

  return (
    <div className="meeting">
      <div className="meeting-doc">
        <div className="row" style={{ alignItems: "flex-start" }}>
          <h1 className="grow meeting-title">{m.meeting.title}</h1>
          {live && <span className="chip rec"><span className="dot" /> Live</span>}
          {!live && m.has_audio && <button className="ghost small" onClick={() => del("audio")}>Delete audio</button>}
          {!live && <button className="ghost danger" aria-label="Delete meeting" onClick={() => del("meeting")}><Icon name="trash" size={14} /></button>}
        </div>
        <div className="row meeting-meta">
          <Segmented>
            <button className={view === "notes" ? "on" : ""} onClick={() => setView("notes")}>My notes</button>
            <button className={view === "enhanced" ? "on" : ""} onClick={() => setView("enhanced")}><span className="row"><Icon name="sparkle" size={13} /> Enhanced</span></button>
          </Segmented>
          <span className="chip plain">{fmtTime(m.meeting.started_at)}</span>
          <span className="chip plain" title={people.join(", ")}><Icon name="user" size={12} /> Me{people.length ? ` + ${people.length}` : ""}</span>
          {view === "enhanced" && <EnhanceControls m={m} templates={templates} busy={busy} run={run} shown={shown} setFile={setFile} del={del} />}
        </div>
        <Suggestions m={m} reload={load} />
        {view === "notes" ? <Notepad id={id} initial={m.notes} /> : shown ? (
          <>
            <Markdown text={shown[1]} />
            <Actions m={m} run={run} />
          </>
        ) : (
          <div className="empty-doc">
            <p className="muted">{busy === "enhance" ? "Writing your notes…" : "No AI notes yet. Enhance merges your rough notes with the transcript, names the meeting and lists action items."}</p>
            {busy !== "enhance" && <button className="primary" onClick={() => run("enhance", call("enhance", { id, template: "auto" }), "Notes ready")}><span className="row"><Icon name="sparkle" size={14} /> Enhance</span></button>}
          </div>
        )}
      </div>
      <Dock m={m} live={live} reload={load} onEdited={() => { setView("enhanced"); setFile(""); }} />
    </div>
  );
}

function EnhanceControls({ m, templates, busy, run, shown, setFile, del }: {
  m: MeetingFull; templates: [string, string][]; busy: string; run: Function; shown?: [string, string]; setFile: (f: string) => void; del: Function;
}) {
  const [template, setTemplate] = useState("auto");
  const [langs, setLangs] = useState<[string, string][]>([]);
  useEffect(() => { call<[string, string][]>("languages").then((l) => setLangs(l.filter((x) => x[0] !== "en"))); }, []);
  const label = (f: string, text: string) => `${(text.match(/template: (.*)/) || [])[1] ?? f}${/\.\w\w\.md$/.test(f) ? ` · ${f.slice(-5, -3).toUpperCase()}` : ""}`;
  return (
    <>
      <span className="grow" />
      {m.summaries.length > 1 && shown && (
        <Select value={shown[0]} onChange={(e) => setFile(e.target.value)}>
          {m.summaries.map(([f, t]) => <option key={f} value={f}>{label(f, t)}</option>)}
        </Select>
      )}
      <Select value={template} onChange={(e) => setTemplate(e.target.value)} disabled={!!busy}>
        <option value="auto">Auto ({templates.find((t) => t[0] === m.meeting.template)?.[1] ?? "AI picks"})</option>
        {templates.map(([id, l]) => <option key={id} value={id}>{l}</option>)}
      </Select>
      <button className="primary small" disabled={!!busy} onClick={() => run("enhance", call("enhance", { id: m.meeting.id, template }).then(() => setFile("")), "New version added")}>
        <span className="row"><Icon name="sparkle" size={13} /> {busy === "enhance" ? "Enhancing…" : shown ? "Re-enhance" : "Enhance"}</span>
      </button>
      {shown && (
        <>
          <button className="ghost" title="Copy as Markdown" aria-label="Copy notes" onClick={() => navigator.clipboard.writeText(shown[1].replace(/^---\n[\s\S]*?\n---\n/, "")).then(() => toast("Copied as Markdown"))}><Icon name="copy" size={14} /></button>
          <Select value="" disabled={!!busy} onChange={(e) => e.target.value && run("translate", call("translate_summary", { id: m.meeting.id, file: shown[0], lang: e.target.value }), "Translation saved")}>
            <option value="">Translate…</option>
            {langs.map(([c, n]) => <option key={c} value={c}>{n}</option>)}
          </Select>
          <button className="ghost danger" title="Delete this version" aria-label="Delete this version" onClick={() => { setFile(""); del("summary", shown[0]); }}><Icon name="trash" size={14} /></button>
        </>
      )}
    </>
  );
}

function Suggestions({ m, reload }: { m: MeetingFull; reload: () => void }) {
  const list = m.speakers?.suggestions ?? [];
  if (!list.length) return null;
  const resolve = (label: string, accept: boolean, name: string) =>
    attempt(call("resolve_speaker", { id: m.meeting.id, label, accept }), accept ? `Renamed to ${name}` : undefined).then(reload);
  return (
    <div className="col" style={{ gap: 6, margin: "10px 0" }}>
      {list.map((s) => (
        <div key={s.label} className="suggestion row">
          <Icon name="user" size={14} />
          <span className="grow">AI thinks <b>{s.label}</b> is <b>{s.name}</b>{s.evidence && <span className="faint"> · “{s.evidence}”</span>}</span>
          <button className="small primary" onClick={() => resolve(s.label, true, s.name)}>Accept</button>
          <button className="small ghost" onClick={() => resolve(s.label, false, s.name)}>Dismiss</button>
        </div>
      ))}
    </div>
  );
}

function Notepad({ id, initial }: { id: string; initial: string }) {
  const [text, setText] = useState(initial);
  const timer = useRef<number | undefined>(undefined);
  const change = (v: string) => {
    setText(v);
    clearTimeout(timer.current);
    timer.current = window.setTimeout(() => call("save_meeting_file", { id, name: "notes.md", content: v }), 500);
  };
  return (
    <div className="col">
      <textarea className="notepad" placeholder="Type rough notes. Enhance merges them with the transcript; your lines stay highlighted." value={text} onChange={(e) => change(e.target.value)} />
      <p className="faint">Saved automatically to notes.md</p>
    </div>
  );
}

function Actions({ m, run }: { m: MeetingFull; run: Function }) {
  const items: any[] = Array.isArray(m.action_items) ? m.action_items : [];
  const save = (next: any[]) => run("save", call("save_meeting_file", { id: m.meeting.id, name: "action-items.json", content: JSON.stringify(next, null, 2) }));
  const decisions: any[] = m.decisions?.decisions ?? [];
  const questions: any[] = m.decisions?.open_questions ?? [];
  if (!items.length && !decisions.length && !questions.length) return null;
  return (
    <div className="col" style={{ gap: 12, marginTop: 18 }}>
      {items.length > 0 && (
        <div className="card col">
          <h3>Action items</h3>
          {items.map((a, i) => (
            <div key={i} className="row" style={{ alignItems: "flex-start" }}>
              <input type="checkbox" checked={!!a.done} onChange={() => save(items.map((x, j) => (j === i ? { ...x, done: !x.done } : x)))} style={{ marginTop: 3 }} />
              <div className="grow">
                <div style={{ textDecoration: a.done ? "line-through" : "none" }}>{a.task}</div>
                <div className="faint">
                  {a.owner ?? "No owner"}{a.due_date ? ` · due ${a.due_date}` : ""}
                  {a.confidence < 0.6 && <span className="chip warn" style={{ marginLeft: 6 }}>low confidence</span>}
                  {a.source_quote && <> · “{a.source_quote}”</>}
                </div>
              </div>
              <button className="ghost" aria-label="Remove action item" onClick={() => save(items.filter((_, j) => j !== i))}><Icon name="trash" size={14} /></button>
            </div>
          ))}
        </div>
      )}
      {decisions.length > 0 && <div className="card"><h3>Decisions</h3><ul>{decisions.map((d, i) => <li key={i}>{d.decision}</li>)}</ul></div>}
      {questions.length > 0 && <div className="card"><h3>Open questions</h3><ul>{questions.map((q, i) => <li key={i}>{q.question}</li>)}</ul></div>}
    </div>
  );
}

// ---------- dock: transcript + Ask ----------

function Dock({ m, live, reload, onEdited }: { m: MeetingFull; live: boolean; reload: () => void; onEdited: () => void }) {
  const [tab, setTab] = useState<"transcript" | "ask">("transcript");
  const [open, setOpen] = useState(live);
  const [find, setFind] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const [busy, setBusy] = useState(false);
  const id = m.meeting.id;

  const send = async () => {
    const text = q.trim();
    if (!text || busy) return;
    setBusy(true);
    setTab("ask");
    setOpen(true);
    const r = await attempt(call<ChatEntry>("ask", { id, question: text }));
    setBusy(false);
    if (r) setQ("");
    if (r?.edit) onEdited();
    reload();
  };
  const copy = () => navigator.clipboard.writeText(m.transcript.map((s) => `[${ts(s.start_ms)}] ${s.speaker}: ${s.text}`).join("\n")).then(() => toast("Transcript copied"));
  const check = async () => {
    const r = await attempt(call<{ app: string; participants: string[]; speaking: string[]; elements: number }>("check_speaker_names"));
    if (r) toast(`${r.app}: ${r.participants.length} participant(s)${r.participants.length ? ` (${r.participants.slice(0, 4).join(", ")})` : ""}; speaking now: ${r.speaking.join(", ") || "nobody detected"}. Saved the window's labels to logs/meeting-ui.txt.`);
  };

  return (
    <div className={`dock ${open ? "open" : ""}`}>
      <div className="dock-head row">
        <Segmented>
          <button className={tab === "transcript" ? "on" : ""} onClick={() => { setTab("transcript"); setOpen(true); }}>Transcript</button>
          <button className={tab === "ask" ? "on" : ""} onClick={() => { setTab("ask"); setOpen(true); }}>Ask</button>
        </Segmented>
        {live && <span className="chip rec"><span className="dot" /> Live</span>}
        <span className="grow" />
        {open && tab === "transcript" && (
          find === null
            ? <button className="ghost" aria-label="Search transcript" onClick={() => setFind("")}><Icon name="search" size={14} /></button>
            : <input autoFocus className="dock-find" placeholder="Find in transcript" value={find} onChange={(e) => setFind(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setFind(null)} onBlur={() => !find && setFind(null)} />
        )}
        {live && <button className="ghost small" title="Show what Remora can read from the open Teams, Zoom or Meet window" onClick={check}>Check speaker names</button>}
        <button className="ghost" aria-label="Copy transcript" title="Copy transcript" onClick={copy}><Icon name="copy" size={14} /></button>
        <button className="ghost" aria-label={open ? "Minimise" : "Expand"} onClick={() => setOpen(!open)}><Icon name={open ? "minus" : "chevronUp"} size={14} /></button>
      </div>
      {open && (tab === "transcript" ? <Bubbles m={m} live={live} find={find ?? ""} reload={reload} /> : <Chat m={m} busy={busy} reload={reload} />)}
      <div className="dock-ask row">
        <input className="grow" placeholder="Ask anything, or tell me how to change the notes…" value={q} disabled={busy}
          onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && send()} />
        <button className="primary" aria-label="Send" disabled={busy || !q.trim()} onClick={send}>{busy ? "Thinking…" : <Icon name="send" size={14} />}</button>
      </div>
    </div>
  );
}

/** Live transcript as chat bubbles: you on the right, everyone else on the left under their name. */
function Bubbles({ m, live, find, reload }: { m: MeetingFull; live: boolean; find: string; reload: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const [atEnd, setAtEnd] = useState(true);
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState("");
  const f = find.trim().toLowerCase();
  const segs = f ? m.transcript.filter((s) => s.text.toLowerCase().includes(f) || s.speaker.toLowerCase().includes(f)) : m.transcript;

  useEffect(() => { if (atEnd) box.current?.scrollTo({ top: box.current.scrollHeight }); }, [segs.length]);
  const onScroll = () => {
    const b = box.current!;
    setAtEnd(b.scrollHeight - b.scrollTop - b.clientHeight < 40);
  };
  const rename = async (from: string) => {
    const to = name.trim();
    if (to && to !== from) await attempt(call("rename_speaker", { id: m.meeting.id, from, to }), `Renamed to ${to}`);
    setEditing(null);
    reload();
  };

  if (!m.transcript.length) return <div className="dock-body"><p className="muted" style={{ padding: 12 }}>{live ? "Listening… the first lines appear about 20 seconds in." : "No transcript."}</p></div>;

  const rows: JSX.Element[] = [];
  let lastSpeaker = "", lastMark = -Infinity, lastEnd = 0;
  segs.forEach((s, i) => {
    // A time marker after a minute of silence or every five minutes.
    if (s.start_ms - lastEnd > 60_000 || s.start_ms - lastMark > 300_000) {
      rows.push(<div key={`t${i}`} className="tmark">{ts(s.start_ms)}</div>);
      lastMark = s.start_ms;
      lastSpeaker = "";
    }
    const me = isMe(s.speaker);
    if (!me && s.speaker !== lastSpeaker) {
      rows.push(editing === s.speaker && !rows.some((r) => r.key === `e-${s.speaker}`) ? (
        <input key={`e-${s.speaker}`} autoFocus className="who-edit" value={name} onChange={(e) => setName(e.target.value)}
          onBlur={() => rename(s.speaker)} onKeyDown={(e) => e.key === "Enter" && rename(s.speaker)} />
      ) : (
        <button key={`n${i}`} className="who-name" style={{ color: speakerColor(s.speaker) }} title="Rename this speaker everywhere"
          onClick={() => { setEditing(s.speaker); setName(s.speaker); }}>{s.speaker}</button>
      ));
    }
    rows.push(<div key={i} className={`tb ${me ? "me" : ""} selectable`} title={ts(s.start_ms)}>{s.text}</div>);
    lastSpeaker = s.speaker;
    lastEnd = s.end_ms;
  });

  return (
    <div className="dock-body" ref={box} onScroll={onScroll}>
      {rows}
      {!segs.length && <p className="faint" style={{ padding: 12 }}>No lines match.</p>}
      {!atEnd && <button className="jump" aria-label="Jump to latest" onClick={() => { box.current?.scrollTo({ top: box.current.scrollHeight, behavior: "smooth" }); }}><Icon name="down" size={16} /></button>}
    </div>
  );
}

function Chat({ m, busy, reload }: { m: MeetingFull; busy: boolean; reload: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => { box.current?.scrollTo({ top: box.current.scrollHeight }); }, [m.chat.length, busy]);
  const undo = (index: number) => attempt(call("undo_ask", { id: m.meeting.id, index }), "Undone").then(reload);
  return (
    <div className="dock-body" ref={box}>
      {!m.chat.length && !busy && (
        <p className="faint" style={{ padding: 12 }}>Ask about the meeting (“what did Saurav promise?”) or change the notes (“shorten next steps”, “add an owner to each action item”, “Speaker 2 is Anand”).</p>
      )}
      {m.chat.map((c, i) => (
        <div key={i} className={`tb ${c.role === "user" ? "me" : ""} selectable`}>
          {c.text}
          {(c.edit || c.renames?.length) && (
            <div className="row" style={{ marginTop: 6 }}>
              {c.undone ? <span className="faint">Undone</span> : (
                <button className="ghost small" onClick={() => undo(i)}><span className="row"><Icon name="undo" size={13} /> Undo</span></button>
              )}
            </div>
          )}
        </div>
      ))}
      {busy && <div className="tb faint">Thinking…</div>}
      {m.chat.length > 0 && !busy && (
        <button className="ghost danger small" style={{ alignSelf: "flex-start", marginTop: 6 }}
          onClick={() => attempt(call("delete_meeting_part", { id: m.meeting.id, part: "chat", name: "" })).then(reload)}>Delete chat</button>
      )}
    </div>
  );
}
