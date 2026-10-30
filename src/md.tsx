import type { JSX } from "react";

// Minimal Markdown for summaries: headings, bullets, "> " user lines (Granola-style), paragraphs.
export function Markdown({ text }: { text: string }) {
  const body = text.replace(/^---\n[\s\S]*?\n---\n/, "");
  const out: JSX.Element[] = [];
  let list: JSX.Element[] = [];
  const flush = () => { if (list.length) { out.push(<ul key={out.length}>{list}</ul>); list = []; } };
  const inline = (s: string) =>
    s.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).map((p, i) =>
      p.startsWith("**") ? <b key={i}>{p.slice(2, -2)}</b> : p.startsWith("`") ? <code key={i} className="mono">{p.slice(1, -1)}</code> : p);
  body.split("\n").forEach((line, i) => {
    const l = line.trimEnd();
    if (/^#{1,6} /.test(l)) { flush(); out.push(<h3 key={i}>{inline(l.replace(/^#+ /, ""))}</h3>); }
    else if (/^>\s?/.test(l)) list.push(<li key={i} className="mine">{inline(l.replace(/^>\s?(- )?/, ""))}</li>);
    else if (/^\s*([-*]|\d+\.) /.test(l)) list.push(<li key={i}>{inline(l.replace(/^\s*([-*]|\d+\.) /, "").replace(/^>\s?/, ""))}</li>);
    else if (l.trim()) { flush(); out.push(<p key={i} style={{ margin: "4px 0" }}>{inline(l)}</p>); }
    else flush();
  });
  flush();
  return <div className="md fade-in selectable">{out}</div>;
}
