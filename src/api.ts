import { invoke } from "@tauri-apps/api/core";

export const call = <T = any>(cmd: string, args?: Record<string, unknown>) => invoke<T>(cmd, args);

export const isMac = navigator.userAgent.includes("Mac");
export const mod = isMac ? "⌘" : "Ctrl";
export const prettyKey = (k: string) =>
  k.split("+").map((p) => ({
    CommandOrControl: mod, Control: "Ctrl", Alt: isMac ? "⌥" : "Alt", Shift: isMac ? "⇧" : "Shift", Super: isMac ? "⌘" : "Win",
  } as Record<string, string>)[p] ?? p.replace(/^Key|^Digit/, "")).join(" + ");

/** Browser KeyboardEvent → Tauri global-shortcut accelerator (W3C key codes). Null while only modifiers are held. */
export function accelerator(e: KeyboardEvent): string | null {
  if (["Control", "Shift", "Alt", "Meta", "OS"].includes(e.key)) return null;
  const mods = [e.ctrlKey && "Control", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean) as string[];
  const fkey = /^F\d+$/.test(e.code);
  if (!mods.length && !fkey) return null;
  return [...mods, e.code].join("+");
}

export function toast(message: string, error = false) {
  window.dispatchEvent(new CustomEvent("toast", { detail: { message: String(message), error } }));
}

/** Wraps an async call so failures surface as a toast instead of disappearing. */
export async function attempt<T>(p: Promise<T>, ok?: string): Promise<T | undefined> {
  try {
    const v = await p;
    if (ok) toast(ok);
    return v;
  } catch (e) {
    toast(String(e), true);
    return undefined;
  }
}

export type Status = {
  data_dir: string; platform: string; keys: Record<string, boolean>; whistle: boolean;
  send_audio_to_cloud: boolean; local_only: boolean; hotkey: string; meeting_hotkey: string; meeting_live: string | null; hotkey_error: string | null;
  hotkey_mode: string; stt_ready: string | null; dictations: number; meetings: number;
};

export type Settings = {
  hotkey: string; hotkey_mode: string; meeting_hotkey: string; history_enabled: boolean;
  history_keep_last: number | null; history_keep_days: number | null;
  output_language: string; per_app_language: Record<string, string>; tone: string; mode: string;
  auto_signoff: boolean; signoff: string; meeting_audio_keep_days: number | null; trash_days: number;
  disabled_apps: string[]; sounds: boolean; sound_style: string; sonar_ripples: "on" | "subtle" | "off";
};

export type Dictation = { id: string; time: string; app: string; category: string; language: string; raw: string; final: string; via: string; stt?: string; latency_ms: number };
export type Meeting = { id: string; title: string; started_at: string; ended_at: string | null; template: string; tags: string[]; pinned: boolean; capture: string };
export type Segment = { speaker: string; start_ms: number; end_ms: number; text: string };
export type MeetingFull = {
  meeting: Meeting; transcript: Segment[]; notes: string; summaries: [string, string][];
  action_items: any; decisions: any; chat: ChatEntry[]; has_audio: boolean; speakers: SpeakerInfo | null;
};
export type ChatEntry = {
  role: string; text: string; undone?: boolean;
  edit?: { file: string; backup: string }; renames?: [string, string][];
};
export type SpeakerInfo = {
  app?: string; participants?: string[];
  speakers?: { label: string; name: string; source: "app" | "voice" | "ai"; evidence?: string }[];
  suggestions?: { label: string; name: string; confidence: number; evidence: string }[];
};

export const CATEGORIES = [
  ["ai_agent.ide", "AI agent · IDE (Cursor, VS Code, Windsurf)"],
  ["ai_agent.cli", "AI agent · CLI (Claude Code, Codex)"],
  ["ai_agent.chat", "AI agent · Chat (Claude, ChatGPT, Devin)"],
  ["terminal", "Terminal"],
  ["work_chat", "Work chat (Teams, Slack)"],
  ["email", "Email (Outlook, Gmail)"],
  ["docs", "Docs (Word, Notion)"],
  ["general", "General"],
] as const;

export function fmtTime(iso: string) {
  const d = new Date(iso);
  return isNaN(+d) ? iso : d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

export function speakerColor(name: string) {
  if (name === "You" || name === "Room") return "var(--spk-1)";
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return `var(--spk-${2 + (h % 7)})`;
}
