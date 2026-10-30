//! Remora Notes (plan §5): bot-free capture, channel-split speakers, Enhance, extraction, Ask.

use crate::audio::{self, Recording, Source};
use crate::meeting_ui;
use crate::providers;
use crate::speakers::{self, Speakers, Votes};
use crate::store::{append_jsonl, now, read_json, read_jsonl, write_atomic, write_json, write_jsonl, Data};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub template: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub capture: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Segment {
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub struct Live {
    pub id: String,
    stop: Arc<AtomicBool>,
    done: Option<tokio::sync::oneshot::Receiver<()>>,
}

pub const TEMPLATES: &[(&str, &str, &[&str])] = &[
    ("general", "General", &["Summary", "Key points", "Decisions", "Action items", "Open questions"]),
    ("one_on_one", "1:1", &["Check-in", "Updates", "Blockers", "Feedback", "Action items"]),
    ("standup", "Standup", &["Yesterday", "Today", "Blockers"]),
    ("client_discovery", "Client discovery", &["Client context", "Goals", "Pain points", "Requirements", "Risks", "Next steps"]),
    ("sprint_retro", "Sprint retro", &["What went well", "What didn't", "Ideas", "Action items"]),
    ("qa_triage", "QA triage", &["Bugs discussed", "Severity & priority", "Owners", "Decisions", "Follow-ups"]),
    ("interview", "Interview", &["Candidate background", "Strengths", "Concerns", "Signals per competency", "Recommendation"]),
    ("sales_call", "Sales call", &["Account context", "Needs", "Objections", "Budget & timeline", "Next steps"]),
    ("follow_up_email", "Follow-up email", &[]),
];

fn meeting_dir(data: &Data, id: &str) -> Result<PathBuf, String> {
    fs::read_dir(data.path("meetings"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&format!("_{id}"))))
        .ok_or_else(|| format!("meeting {id} not found"))
}

fn ts(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, (ms / 1000) % 60)
}

// ---------- live capture ----------

const CHUNK_SECS: u64 = 20;

/// Title and template start as placeholders; Enhance fills both from the content.
pub fn start(app: &AppHandle, data: &Data) -> Result<Live, String> {
    let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
    let local = chrono::Local::now();
    let title = format!("Meeting · {}", local.format("%d %b, %H:%M"));
    let template = "";
    let dir = data.path(&format!("meetings/{}_meeting_{}", local.format("%Y-%m-%d_%H%M"), id));
    fs::create_dir_all(dir.join("summaries")).map_err(|e| e.to_string())?;

    let mic = Recording::start(Source::Mic)?;
    // In-person mode when there's no loopback: mic only, everything is "You/Room".
    let system = Recording::start(Source::System).ok();
    let meeting = Meeting {
        id: id.clone(), title, started_at: now(), template: template.into(),
        capture: if system.is_some() { "mic+system" } else { "mic" }.into(), ..Default::default()
    };
    write_json(&dir.join("meeting.json"), &meeting).map_err(|e| e.to_string())?;
    let _ = write_atomic(&dir.join("notes.md"), b"");

    let keep_audio = data.settings().meeting_audio_keep_days != Some(0);
    let mut wavs = if keep_audio {
        let _ = fs::create_dir_all(dir.join("audio"));
        (audio::wav_file(&dir.join("audio/mic.wav")).ok(),
         system.as_ref().and_then(|_| audio::wav_file(&dir.join("audio/system.wav")).ok()))
    } else {
        (None, None)
    };

    let stop = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    // Names shown by Teams / Zoom / Meet, and the speaker models (fetched once, ≈31 MB, used from the next chunk on).
    let ui = meeting_ui::Shared::default();
    meeting_ui::watch(stop.clone(), std::time::Instant::now(), ui.clone());
    {
        let (app, data) = (app.clone(), data.clone());
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::models::ensure_speaker_models(&app, &data).await {
                let _ = app.emit("meeting-error", format!("Speaker recognition is off: {e}"));
            }
        });
    }
    let (app, data, stop2, mid) = (app.clone(), data.clone(), stop.clone(), id.clone());
    let you = if system.is_some() { "You" } else { "Room" };
    // The track with several voices: everyone else on a call, or the room when in person.
    let multi = if system.is_some() { 1 } else { 0 };
    tauri::async_runtime::spawn(async move {
        let mut offset = [0u64; 2];
        let mut tracks: [Option<Recording>; 2] = [Some(mic), system];
        let mut spk: Option<Arc<Mutex<Speakers>>> = None;
        let mut votes = Votes::default();
        // Label currently written in the transcript for each diarized speaker.
        let mut shown: HashMap<usize, String> = HashMap::new();
        loop {
            for _ in 0..(CHUNK_SECS * 4) {
                if stop2.load(Relaxed) { break; }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            let last = stop2.load(Relaxed);
            for i in 0..2 {
                let samples = if last {
                    match tracks[i].take() { Some(r) => r.stop(), None => continue }
                } else {
                    match &tracks[i] { Some(r) => r.drain_16k(), None => continue }
                };
                let w = if i == 0 { &mut wavs.0 } else { &mut wavs.1 };
                if let Some(w) = w { audio::write_samples(w, &samples); }
                let start_ms = offset[i];
                offset[i] += samples.len() as u64 * 1000 / audio::RATE as u64;
                if !audio::has_speech(&samples) { continue; }
                if i == multi && spk.is_none() { spk = Speakers::load(&data).map(|s| Arc::new(Mutex::new(s))); }
                let turns = match &spk {
                    Some(s) if i == multi => {
                        let (s, x) = (s.clone(), samples.clone());
                        tauri::async_runtime::spawn_blocking(move || s.lock().unwrap().turns(&x)).await.unwrap_or_default()
                    }
                    _ => vec![],
                };
                // Each speaker turn is transcribed on its own so a chunk never mixes two voices under one name.
                let pieces: Vec<(usize, usize, Option<usize>)> = if turns.is_empty() {
                    vec![(0, samples.len(), None)]
                } else {
                    turns.iter().map(|t| (t.start, t.end, Some(t.speaker))).collect()
                };
                for (a, b, sp) in pieces {
                    let x = &samples[a..b];
                    if !audio::has_speech(x) { continue; }
                    let at = start_ms + a as u64 * 1000 / audio::RATE as u64;
                    let end = start_ms + b as u64 * 1000 / audio::RATE as u64;
                    let timeline = ui.timeline.lock().unwrap().clone();
                    let speaker = match sp {
                        Some(sp) => {
                            votes.add(sp, &timeline, at, end);
                            let name = votes.name(sp).unwrap_or_else(|| speakers::label(sp));
                            // A speaker just got their name: relabel what they said earlier too.
                            if let Some(old) = shown.insert(sp, name.clone()).filter(|o| *o != name) {
                                let _ = rename_speaker(&data, &mid, &old, &name);
                                let _ = app.emit("meetings-changed", ());
                            }
                            name
                        }
                        None if i == 1 => speakers::dominant(&timeline, at, end).unwrap_or_else(|| "Remote".into()),
                        None => you.into(),
                    };
                    match providers::transcribe(&data, "stt.meeting", audio::wav_bytes(x), "en").await {
                        Ok(out) => {
                            let segs: Vec<Segment> = if out.segments.is_empty() {
                                vec![Segment { speaker: speaker.clone(), start_ms: at, end_ms: end, text: out.text }]
                            } else {
                                out.segments.into_iter().map(|s| Segment {
                                    speaker: speaker.clone(), start_ms: at + s.start_ms, end_ms: at + s.end_ms, text: s.text,
                                }).collect()
                            };
                            for s in segs.into_iter().filter(|s| !s.text.trim().is_empty()) {
                                if let Ok(dir) = meeting_dir(&data, &mid) {
                                    let _ = append_jsonl(&dir.join("transcript.jsonl"), &s);
                                }
                                let _ = app.emit("transcript", serde_json::json!({ "meeting": mid, "segment": s }));
                            }
                        }
                        Err(e) => { let _ = app.emit("meeting-error", e); }
                    }
                }
            }
            if last { break; }
        }
        for w in [wavs.0, wavs.1].into_iter().flatten() { let _ = w.finalize(); }
        // Final pass: join speakers that turned out to be one voice, then apply names that firmed up late.
        if let Some(s) = &spk {
            let merges = s.lock().unwrap().merges();
            for (from, into) in merges {
                votes.merge(from, into);
                if let Some(old) = shown.remove(&from) {
                    let to = shown.get(&into).cloned().unwrap_or_else(|| speakers::label(into));
                    let _ = rename_speaker(&data, &mid, &old, &to);
                }
            }
        }
        for (sp, old) in shown.clone() {
            if let Some(name) = votes.name(sp).filter(|n| *n != old) {
                let _ = rename_speaker(&data, &mid, &old, &name);
                shown.insert(sp, name);
            }
        }
        if let Ok(dir) = meeting_dir(&data, &mid) {
            let mut m: Meeting = read_json(&dir.join("meeting.json"));
            m.ended_at = Some(now());
            let _ = write_json(&dir.join("meeting.json"), &m);
            let named: Vec<Value> = shown.iter().map(|(sp, name)| serde_json::json!({
                "label": speakers::label(*sp), "name": name, "source": if votes.name(*sp).is_some() { "app" } else { "voice" },
            })).collect();
            let _ = write_json(&dir.join("speakers.json"), &serde_json::json!({
                "app": *ui.app.lock().unwrap(),
                "participants": *ui.participants.lock().unwrap(),
                "speakers": named,
                "timeline": *ui.timeline.lock().unwrap(),
                "suggestions": [],
            }));
        }
        let _ = app.emit("meetings-changed", ());
        let _ = done_tx.send(());
    });
    Ok(Live { id, stop, done: Some(done_rx) })
}

impl Live {
    /// Signals the capture loop and waits for the final chunk to be transcribed.
    pub async fn stop(mut self) {
        self.stop.store(true, Relaxed);
        if let Some(rx) = self.done.take() { let _ = rx.await; }
    }
}

// ---------- library ----------

#[derive(Serialize)]
pub struct MeetingFull {
    pub meeting: Meeting,
    pub transcript: Vec<Segment>,
    pub notes: String,
    pub summaries: Vec<(String, String)>,
    pub action_items: Value,
    pub decisions: Value,
    pub chat: Vec<Value>,
    pub has_audio: bool,
    /// speakers.json: meeting app, participants, how each speaker got their name, AI name suggestions.
    pub speakers: Value,
}

pub fn list(data: &Data, query: &str) -> Vec<Meeting> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<Meeting> = fs::read_dir(data.path("meetings")).into_iter().flatten().flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let dir = e.path();
            let m: Meeting = read_json(&dir.join("meeting.json"));
            if m.id.is_empty() { return None; }
            // ponytail: linear scan over files; add the Tantivy index when the library gets slow.
            if q.is_empty() || m.title.to_lowercase().contains(&q) || m.tags.iter().any(|t| t.to_lowercase().contains(&q)) {
                return Some(m);
            }
            let hay = ["transcript.jsonl", "notes.md"].iter()
                .map(|f| fs::read_to_string(dir.join(f)).unwrap_or_default())
                .chain(fs::read_dir(dir.join("summaries")).into_iter().flatten().flatten().map(|s| fs::read_to_string(s.path()).unwrap_or_default()))
                .any(|t| t.to_lowercase().contains(&q));
            hay.then_some(m)
        })
        .collect();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    out
}

pub fn get(data: &Data, id: &str) -> Result<MeetingFull, String> {
    let dir = meeting_dir(data, id)?;
    let mut summaries: Vec<(String, String)> = fs::read_dir(dir.join("summaries")).into_iter().flatten().flatten()
        .map(|e| (e.file_name().to_string_lossy().to_string(), fs::read_to_string(e.path()).unwrap_or_default()))
        .collect();
    summaries.sort();
    Ok(MeetingFull {
        meeting: read_json(&dir.join("meeting.json")),
        transcript: read_jsonl(&dir.join("transcript.jsonl")),
        notes: fs::read_to_string(dir.join("notes.md")).unwrap_or_default(),
        summaries,
        action_items: read_json::<Value>(&dir.join("action-items.json")),
        decisions: read_json::<Value>(&dir.join("decisions.json")),
        chat: read_jsonl(&dir.join("chats/main.jsonl")),
        has_audio: dir.join("audio").exists(),
        speakers: read_json::<Value>(&dir.join("speakers.json")),
    })
}

pub fn save_file(data: &Data, id: &str, name: &str, content: &str) -> Result<(), String> {
    if !["notes.md", "action-items.json", "decisions.json", "meeting.json"].contains(&name) {
        return Err("not editable".into());
    }
    write_atomic(&meeting_dir(data, id)?.join(name), content.as_bytes()).map_err(|e| e.to_string())
}

pub fn rename_speaker(data: &Data, id: &str, from: &str, to: &str) -> Result<(), String> {
    let p = meeting_dir(data, id)?.join("transcript.jsonl");
    let segs: Vec<Segment> = read_jsonl::<Segment>(&p).into_iter()
        .map(|mut s| { if s.speaker == from { s.speaker = to.to_string(); } s })
        .collect();
    write_jsonl(&p, &segs).map_err(|e| e.to_string())
}

pub fn delete(data: &Data, id: &str, part: &str, name: &str) -> Result<(), String> {
    let dir = meeting_dir(data, id)?;
    let r = match part {
        "meeting" => data.trash(&dir),
        "audio" => fs::remove_dir_all(dir.join("audio")),
        "transcript" => fs::remove_file(dir.join("transcript.jsonl")),
        "summary" if !name.contains(['/', '\\']) && !name.contains("..") => fs::remove_file(dir.join("summaries").join(name)),
        "chat" => fs::remove_dir_all(dir.join("chats")),
        _ => return Err("unknown part".into()),
    };
    r.map_err(|e| e.to_string())
}

/// Audio retention: pinned meetings are exempt.
pub fn prune_audio(data: &Data, keep_days: Option<u32>) {
    let Some(days) = keep_days else { return };
    let cutoff = chrono::Local::now() - chrono::Duration::days(days as i64);
    for m in list(data, "") {
        let old = chrono::DateTime::parse_from_rfc3339(&m.started_at).map(|t| t < cutoff).unwrap_or(false);
        if old && !m.pinned {
            if let Ok(dir) = meeting_dir(data, &m.id) { let _ = fs::remove_dir_all(dir.join("audio")); }
        }
    }
}

// ---------- AI ----------

fn transcript_text(segs: &[Segment]) -> String {
    segs.iter().map(|s| format!("[{}] {}: {}", ts(s.start_ms), s.speaker, s.text)).collect::<Vec<_>>().join("\n")
}

pub async fn enhance(data: &Data, id: &str, template: &str) -> Result<String, String> {
    let full = get(data, id)?;
    if full.transcript.is_empty() && full.notes.trim().is_empty() {
        return Err("Nothing to enhance yet: no transcript or notes.".into());
    }
    // "auto": reuse the type the AI picked last time, otherwise let it pick in this same call.
    let template = if template == "auto" || template.is_empty() { full.meeting.template.as_str() } else { template };
    let fixed = TEMPLATES.iter().find(|t| t.0 == template).copied();
    let notes_rules = "Merge the user's own rough notes with the transcript. Every bullet that comes from the user's \
        notes must start with '> ' so it stays visually distinct. Concise bullets.";
    let doc_rules = match fixed {
        Some((_, _, [])) => "Write a concise follow-up email recapping this meeting for the attendees: thanks, key decisions, \
            action items with owners and dates, next steps.".to_string(),
        Some((_, _, sections)) => format!(
            "Write clean, structured Markdown notes. Sections (as '## ' headings, omit empty ones): {}.\n{notes_rules}",
            sections.join(", ")
        ),
        None => format!(
            "First decide which meeting type fits best:\n{}\nThen write clean, structured Markdown notes using that type's \
             sections (as '## ' headings, omit empty ones).\n{notes_rules}",
            TEMPLATES.iter().filter(|t| !t.2.is_empty()).map(|t| format!("- {}: {}", t.0, t.2.join(", "))).collect::<Vec<_>>().join("\n")
        ),
    };
    let system = format!(
        "You process meeting material in one pass.\n{doc_rules}\n\
         Use only facts from the material. Keep names, numbers and dates exact.\n\
         After the document, output a line containing only {EXTRACT_MARK} followed by ONE JSON object:\n\
         {{\"title\":str (3-7 words naming what the meeting was about),\"template\":str (meeting type id used),\
         \"action_items\":[{{\"task\":str,\"owner\":str|null,\"due_date\":str|null,\"source_quote\":str,\"confidence\":0-1,\"done\":false}}],\
         \"decisions\":[{{\"decision\":str,\"source_quote\":str}}],\"open_questions\":[{{\"question\":str}}]}}\n\
         Only include items actually stated."
    );
    let user = format!(
        "<meeting title=\"{}\" started=\"{}\"/>\n<my_notes>\n{}\n</my_notes>\n<transcript>\n{}\n</transcript>",
        full.meeting.title, full.meeting.started_at, full.notes, transcript_text(&full.transcript)
    );
    // ponytail: whole transcript in one call; map-reduce only needed beyond the model's context window.
    let out = providers::chat(data, "meeting", &system, &user, 6144, false).await?;
    let (summary, extracted) = split_extract(&out.text);
    let dir = meeting_dir(data, id)?;
    let picked = extracted.as_ref().and_then(|v| v["template"].as_str()).and_then(|t| TEMPLATES.iter().find(|x| x.0 == t));
    let (template, label) = match fixed.or(picked.copied()) {
        Some(t) => (t.0, t.1),
        None => (TEMPLATES[0].0, TEMPLATES[0].1),
    };
    let ai_title = extracted.as_ref().and_then(|v| v["title"].as_str()).map(str::trim).filter(|t| !t.is_empty());
    let mut meeting: Meeting = read_json(&dir.join("meeting.json"));
    if let Some(t) = ai_title { meeting.title = t.chars().take(80).collect(); }
    if meeting.template.is_empty() && !template.starts_with("follow_up") { meeting.template = template.into(); }
    write_json(&dir.join("meeting.json"), &meeting).map_err(|e| e.to_string())?;
    if let Some(v) = extracted {
        // Keep ticks the user already made on items that survive a re-run.
        let done: Vec<String> = read_json::<Vec<Value>>(&dir.join("action-items.json")).iter()
            .filter(|a| a["done"] == true).filter_map(|a| a["task"].as_str().map(String::from)).collect();
        let mut items = v["action_items"].as_array().cloned().unwrap_or_default();
        for a in &mut items {
            if a["task"].as_str().is_some_and(|t| done.iter().any(|d| d == t)) { a["done"] = true.into(); }
        }
        write_json(&dir.join("action-items.json"), &items).map_err(|e| e.to_string())?;
        write_json(&dir.join("decisions.json"), &serde_json::json!({ "decisions": v["decisions"], "open_questions": v["open_questions"] }))
            .map_err(|e| e.to_string())?;
    }
    let dir = dir.join("summaries");
    let n = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) + 1;
    let name = format!("{n:02}_{template}.md");
    let doc = format!("---\ntemplate: {label}\nmodel: {}\ncreated_at: {}\npinned: false\n---\n{}\n", out.via, now(), summary);
    write_atomic(&dir.join(&name), doc.as_bytes()).map_err(|e| e.to_string())?;
    Ok(name)
}

const EXTRACT_MARK: &str = "===EXTRACT===";

/// Splits the model's reply into the Markdown document and the trailing JSON extraction (if parseable).
fn split_extract(s: &str) -> (String, Option<Value>) {
    let Some(i) = s.rfind(EXTRACT_MARK) else { return (s.trim().to_string(), None) };
    let tail = &s[i + EXTRACT_MARK.len()..];
    let json = tail.find('{').zip(tail.rfind('}')).and_then(|(a, b)| serde_json::from_str(&tail[a..=b]).ok());
    (s[..i].trim().to_string(), json)
}

// ---------- speaker names ----------

/// Labels that still need a person's name.
fn unnamed(label: &str) -> bool {
    label == "Remote" || label.strip_prefix("Speaker ").is_some_and(|n| n.parse::<u32>().is_ok())
}

/// AI pass over the transcript for speakers the meeting app didn't name: people address each other
/// by name ("Akash, can I drop off?"). Confident names are applied, the rest become suggestions.
pub async fn suggest_names(data: &Data, id: &str) -> Result<(), String> {
    let full = get(data, id)?;
    let mut labels: Vec<&str> = full.transcript.iter().map(|s| s.speaker.as_str()).filter(|s| unnamed(s)).collect();
    labels.sort();
    labels.dedup();
    if labels.is_empty() { return Ok(()); }
    let participants = full.speakers["participants"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let system = "You identify who is speaking in a meeting transcript. \"You\" is the person who recorded it. \
        For each label you are given, find that person's real name from clear evidence only: someone addresses them by name \
        right before they reply, they introduce themselves, or they are thanked or answered by name. \"Remote\" can be several \
        people: name it only if it is clearly one person. Never give one name to two labels and never guess. \
        Reply with ONE JSON object and nothing else: {\"<label>\": {\"name\": string or null, \"confidence\": 0-1, \"evidence\": \"short quote\"}}.";
    let text: String = transcript_text(&full.transcript).chars().take(60_000).collect();
    let user = format!("Labels: {}\nKnown participants: {}\n<transcript>\n{text}\n</transcript>", labels.join(", "),
        if participants.is_empty() { "unknown" } else { &participants });
    let out = providers::chat(data, "meeting", system, &user, 800, false).await?;
    let t = &out.text;
    let v: Value = t.find('{').zip(t.rfind('}')).and_then(|(a, b)| serde_json::from_str(&t[a..=b]).ok()).ok_or("no JSON in the reply")?;
    let dir = meeting_dir(data, id)?;
    let mut doc: Value = read_json(&dir.join("speakers.json"));
    if !doc.is_object() { doc = serde_json::json!({}); }
    let mut suggestions = vec![];
    for label in labels {
        let Some(name) = v[label]["name"].as_str().map(str::trim).filter(|n| !n.is_empty()) else { continue };
        let confidence = v[label]["confidence"].as_f64().unwrap_or(0.0);
        let evidence = v[label]["evidence"].as_str().unwrap_or("");
        if confidence >= 0.85 {
            rename_speaker(data, id, label, name)?;
            let entry = serde_json::json!({ "label": label, "name": name, "source": "ai", "evidence": evidence });
            match doc["speakers"].as_array_mut() { Some(a) => a.push(entry), None => doc["speakers"] = serde_json::json!([entry]) }
        } else if confidence >= 0.4 {
            suggestions.push(serde_json::json!({ "label": label, "name": name, "confidence": confidence, "evidence": evidence }));
        }
    }
    doc["suggestions"] = suggestions.into();
    write_json(&dir.join("speakers.json"), &doc).map_err(|e| e.to_string())
}

/// Accepts (renames everywhere) or dismisses one AI name suggestion.
pub fn resolve_suggestion(data: &Data, id: &str, label: &str, accept: bool) -> Result<(), String> {
    let dir = meeting_dir(data, id)?;
    let mut doc: Value = read_json(&dir.join("speakers.json"));
    let Some(list) = doc["suggestions"].as_array_mut() else { return Ok(()) };
    let Some(i) = list.iter().position(|s| s["label"] == label) else { return Ok(()) };
    let s = list.remove(i);
    if accept {
        if let Some(name) = s["name"].as_str() { rename_speaker(data, id, label, name)?; }
    }
    write_json(&dir.join("speakers.json"), &doc).map_err(|e| e.to_string())
}

// ---------- Ask: answers and edits ----------

/// The document Ask edits: the newest summary (not a translation), else the user's own notes.
fn main_doc(dir: &std::path::Path) -> String {
    let mut s: Vec<String> = fs::read_dir(dir.join("summaries")).into_iter().flatten().flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".md") && !n.trim_end_matches(".md").contains('.'))
        .collect();
    s.sort();
    s.pop().map(|n| format!("summaries/{n}")).unwrap_or_else(|| "notes.md".into())
}

/// Splits a summary's `---` front matter from its Markdown body.
fn split_front(s: &str) -> (&str, &str) {
    if let Some(rest) = s.strip_prefix("---\n") {
        if let Some(i) = rest.find("\n---\n") { return s.split_at(4 + i + 5); }
    }
    ("", s)
}

const EDIT_MARK: &str = "===EDIT===";
const RENAME_MARK: &str = "===RENAME===";

pub async fn ask(data: &Data, id: &str, question: &str) -> Result<Value, String> {
    let full = get(data, id)?;
    let dir = meeting_dir(data, id)?;
    let file = main_doc(&dir);
    let current = fs::read_to_string(dir.join(&file)).unwrap_or_default();
    let (front, body) = split_front(&current);
    let system = format!("You help with one meeting: answer questions about it, or change its notes when asked.\n\
        - Question: reply in plain text using only the material. Cite transcript timestamps like [12:34]. If it isn't there, say so.\n\
        - Change to the notes (rewrite, shorten, add, remove, reorder, reformat, fix a name or fact in them): reply with a first line \
        exactly `{EDIT_MARK} <what you changed, under 12 words>`, then the COMPLETE updated notes in Markdown. Keep everything the \
        user didn't ask to change exactly as it is. Use only facts from the material.\n\
        - Rename a speaker in the transcript (\"Speaker 2 is Anand\"): one line per speaker `{RENAME_MARK} <label as it appears> => <name>`, \
        then one short sentence.");
    let history: String = full.chat.iter()
        .map(|m| format!("{}: {}\n", m["role"].as_str().unwrap_or(""), m["text"].as_str().unwrap_or("")))
        .collect();
    let user = format!(
        "<notes file=\"{file}\">\n{body}\n</notes>\n<my_rough_notes>\n{}\n</my_rough_notes>\n<transcript>\n{}\n</transcript>\n<chat>\n{history}</chat>\nRequest: {question}",
        full.notes, transcript_text(&full.transcript)
    );
    let out = providers::chat(data, "meeting", &system, &user, 6144, false).await?;
    let text = out.text.trim();
    let mut reply = serde_json::json!({ "role": "assistant", "time": now(), "via": out.via });
    if let Some(rest) = text.strip_prefix(EDIT_MARK) {
        let (what, doc) = rest.split_once('\n').unwrap_or((rest, ""));
        let doc = doc.trim();
        if doc.is_empty() { return Err("The AI described an edit but sent no document; try again.".into()); }
        // The previous version is kept so the edit can be undone from the chat.
        let backup = format!("versions/{}_{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), file.replace('/', "_"));
        fs::create_dir_all(dir.join("versions")).map_err(|e| e.to_string())?;
        fs::copy(dir.join(&file), dir.join(&backup)).or_else(|_| fs::write(dir.join(&backup), "").map(|_| 0)).map_err(|e| e.to_string())?;
        write_atomic(&dir.join(&file), format!("{front}{doc}\n").as_bytes()).map_err(|e| e.to_string())?;
        reply["text"] = format!("Updated the notes: {}", what.trim()).into();
        reply["edit"] = serde_json::json!({ "file": file, "backup": backup });
    } else if text.contains(RENAME_MARK) {
        let mut renames = vec![];
        for line in text.lines().filter_map(|l| l.trim().strip_prefix(RENAME_MARK)) {
            if let Some((from, to)) = line.split_once("=>").map(|(a, b)| (a.trim(), b.trim())) {
                if !from.is_empty() && !to.is_empty() && full.transcript.iter().any(|s| s.speaker == from) {
                    rename_speaker(data, id, from, to)?;
                    renames.push(serde_json::json!([from, to]));
                }
            }
        }
        let note: String = text.lines().filter(|l| !l.contains(RENAME_MARK)).collect::<Vec<_>>().join("\n");
        reply["text"] = if note.trim().is_empty() { format!("Renamed {} speaker(s).", renames.len()) } else { note.trim().to_string() }.into();
        reply["renames"] = renames.into();
    } else {
        reply["text"] = text.into();
    }
    let p = dir.join("chats/main.jsonl");
    let _ = append_jsonl(&p, &serde_json::json!({ "role": "user", "text": question, "time": now() }));
    let _ = append_jsonl(&p, &reply);
    Ok(reply)
}

/// Reverts the edit or renames made by chat message `index`.
pub fn undo(data: &Data, id: &str, index: usize) -> Result<(), String> {
    let dir = meeting_dir(data, id)?;
    let p = dir.join("chats/main.jsonl");
    let mut chat: Vec<Value> = read_jsonl(&p);
    let m = chat.get_mut(index).ok_or("no such message")?;
    if m["undone"] == true { return Ok(()); }
    if let (Some(file), Some(backup)) = (m["edit"]["file"].as_str(), m["edit"]["backup"].as_str()) {
        if file.contains("..") || backup.contains("..") { return Err("bad path".into()); }
        let old = fs::read(dir.join(backup)).map_err(|e| e.to_string())?;
        write_atomic(&dir.join(file), &old).map_err(|e| e.to_string())?;
    }
    for r in m["renames"].as_array().into_iter().flatten().rev() {
        if let (Some(from), Some(to)) = (r[0].as_str(), r[1].as_str()) { rename_speaker(data, id, to, from)?; }
    }
    m["undone"] = true.into();
    write_jsonl(&p, &chat).map_err(|e| e.to_string())
}

pub async fn translate_summary(data: &Data, id: &str, file: &str, lang: &str) -> Result<String, String> {
    let dir = meeting_dir(data, id)?.join("summaries");
    if file.contains(['/', '\\']) { return Err("bad file".into()); }
    let src = fs::read_to_string(dir.join(file)).map_err(|e| e.to_string())?;
    let system = format!(
        "Translate this Markdown meeting summary into {}. Keep Markdown structure, names, numbers, dates, \
         product names and anything in backticks unchanged. Keep the front-matter block as is. Output only the document.",
        crate::cleanup::language_name(lang)
    );
    let out = providers::chat(data, "meeting", &system, &src, 4096, false).await?;
    let name = format!("{}.{lang}.md", file.trim_end_matches(".md"));
    write_atomic(&dir.join(&name), out.text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(ts(754_000), "12:34");
        let (doc, v) = split_extract("## Summary\n- hi\n===EXTRACT===\n```json\n{\"action_items\":[{\"task\":\"ship\"}]}\n```");
        assert_eq!(doc, "## Summary\n- hi");
        assert_eq!(v.unwrap()["action_items"][0]["task"], "ship");
        assert_eq!(split_extract("just notes").1, None);
        assert_eq!(split_front("---\nmodel: x\n---\n## Notes\n"), ("---\nmodel: x\n---\n", "## Notes\n"));
        assert_eq!(split_front("## Notes"), ("", "## Notes"));
        assert!(unnamed("Speaker 3") && unnamed("Remote") && !unnamed("Speaker Anand") && !unnamed("You"));
    }

    #[test]
    fn edit_target_and_undo() {
        let d = Data::at(std::env::temp_dir().join(format!("remora-u-{}", uuid::Uuid::new_v4())));
        let dir = d.path("meetings/2026-10-29_1400_x_def456");
        fs::create_dir_all(dir.join("summaries")).unwrap();
        write_json(&dir.join("meeting.json"), &Meeting { id: "def456".into(), ..Default::default() }).unwrap();
        assert_eq!(main_doc(&dir), "notes.md");
        fs::write(dir.join("summaries/01_general.md"), "a").unwrap();
        fs::write(dir.join("summaries/01_general.ta.md"), "b").unwrap();
        assert_eq!(main_doc(&dir), "summaries/01_general.md", "translations are never the edit target");
        fs::create_dir_all(dir.join("versions")).unwrap();
        fs::write(dir.join("versions/old.md"), "before").unwrap();
        fs::write(dir.join("summaries/01_general.md"), "after").unwrap();
        write_jsonl(&dir.join("transcript.jsonl"), &[Segment { speaker: "Anand".into(), start_ms: 0, end_ms: 1, text: "hi".into() }]).unwrap();
        fs::create_dir_all(dir.join("chats")).unwrap();
        write_jsonl(&dir.join("chats/main.jsonl"), &[serde_json::json!({ "role": "assistant", "edit": { "file": "summaries/01_general.md", "backup": "versions/old.md" }, "renames": [["Speaker 2", "Anand"]] })]).unwrap();
        undo(&d, "def456", 0).unwrap();
        assert_eq!(fs::read_to_string(dir.join("summaries/01_general.md")).unwrap(), "before");
        assert_eq!(get(&d, "def456").unwrap().transcript[0].speaker, "Speaker 2");
        assert_eq!(get(&d, "def456").unwrap().chat[0]["undone"], true);
    }

    #[test]
    fn rename_and_delete_parts() {
        let d = Data::at(std::env::temp_dir().join(format!("remora-n-{}", uuid::Uuid::new_v4())));
        let dir = d.path("meetings/2026-10-29_1400_x_abc123");
        fs::create_dir_all(dir.join("summaries")).unwrap();
        write_json(&dir.join("meeting.json"), &Meeting { id: "abc123".into(), title: "X".into(), ..Default::default() }).unwrap();
        let seg = |s: &str| Segment { speaker: s.into(), start_ms: 0, end_ms: 1, text: "hello priya".into() };
        write_jsonl(&dir.join("transcript.jsonl"), &[seg("Remote"), seg("You")]).unwrap();
        rename_speaker(&d, "abc123", "Remote", "Priya").unwrap();
        let t = get(&d, "abc123").unwrap().transcript;
        assert_eq!(t[0].speaker, "Priya");
        assert_eq!(list(&d, "priya").len(), 1);
        assert_eq!(list(&d, "nomatch").len(), 0);
        delete(&d, "abc123", "transcript", "").unwrap();
        assert!(get(&d, "abc123").unwrap().transcript.is_empty());
        delete(&d, "abc123", "meeting", "").unwrap();
        assert!(get(&d, "abc123").is_err());
    }
}
