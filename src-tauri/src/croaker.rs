//! Croaker push-to-talk pipeline (plan §6.1, §6.4).

use crate::audio::{self, Recording, Source};
use crate::cleanup::{self, Command};
use crate::profiles::{self, AppCtx};
use crate::providers;
use crate::store::{read_json, Data, Dictation};
use crate::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub struct Session {
    rec: Recording,
    ctx: AppCtx,
    pressed_at: Instant,
    locked: bool,
    live: Arc<Mutex<Live>>,
    phrases: Option<std::thread::JoinHandle<()>>,
}

/// Audio so far and the phrases already transcribed while the user speaks (plan §6.4).
#[derive(Default)]
struct Live {
    pcm: Vec<f32>,
    /// `pcm[..done]` is transcribed (or was silence); only the rest is left for key release.
    done: usize,
    texts: Vec<String>,
    stt: String,
    /// A phrase failed; transcribe the whole clip on release instead.
    failed: bool,
}

/// Cuts the recording at pauses and transcribes each finished phrase in the background.
fn transcribe_phrases(data: Data, drain: impl Fn() -> Vec<f32>, alive: Arc<std::sync::atomic::AtomicBool>, live: Arc<Mutex<Live>>) {
    while alive.load(Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
        let phrase = {
            let mut l = live.lock().unwrap();
            l.pcm.extend(drain());
            match audio::phrase_end(&l.pcm, l.done) {
                Some(end) => {
                    let p = l.pcm[l.done..end].to_vec();
                    l.done = end;
                    audio::has_speech(&p).then_some(p)
                }
                None => None,
            }
        };
        let Some(p) = phrase else { continue };
        let out = tauri::async_runtime::block_on(providers::transcribe(&data, "stt.dictation", audio::wav_bytes(&p), "en"));
        let mut l = live.lock().unwrap();
        match out {
            Ok(o) if !is_filler(&o.text) => { l.texts.push(o.text.trim().to_string()); l.stt = o.via; }
            Ok(_) => {}
            Err(_) => l.failed = true,
        }
    }
}

/// A press shorter than this counts as a tap in "auto" mode and locks recording on.
const TAP_MS: u128 = 350;

#[derive(Serialize, Clone)]
pub struct Pill {
    pub state: &'static str,
    pub text: String,
    pub chip: String,
    /// Sound id to play on open/close; empty when sounds are off.
    pub sound: String,
    pub ripples: String,
}

pub fn pill(app: &AppHandle, state: &'static str, text: impl Into<String>, chip: impl Into<String>) {
    let s = app.state::<AppState>().data.settings();
    let sound = if s.sounds { s.sound_style } else { String::new() };
    let _ = app.emit_to("pill", "pill", Pill { state, text: text.into(), chip: chip.into(), sound, ripples: s.sonar_ripples });
    if let Some(w) = app.get_webview_window("pill") {
        if state == "hidden" {
            let _ = w.hide();
        } else {
            let _ = w.show();
        }
    }
}

fn hide_later(app: &AppHandle, ms: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        let busy = app.state::<AppState>().croaker.lock().unwrap().is_some();
        if !busy {
            pill(&app, "hidden", "", "");
        }
    });
}

/// Appends a timestamped line to `logs/croaker.log` in the data folder.
fn log_line(data: &Data, line: &str) {
    use std::io::Write;
    let path = data.path("logs/croaker.log");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {line}", crate::store::now());
    }
}

pub fn dictionary(data: &Data) -> Vec<String> {
    read_json(&data.path("dictionary.json"))
}

pub fn snippets(data: &Data) -> HashMap<String, String> {
    read_json(&data.path("snippets.json"))
}

/// Hotkey entry point. Presses and releases run in order on one worker thread, so a slow mic open
/// never blocks the main thread (which has to deliver the pill's "listening" event and show it).
pub fn key(app: &AppHandle, down: bool) {
    send(app, Some(down));
}

/// Escape while recording: drop the audio, insert nothing.
fn cancel_key(app: &AppHandle) {
    send(app, None);
}

/// Some(true) = press, Some(false) = release, None = cancel.
fn send(app: &AppHandle, msg: Option<bool>) {
    static TX: std::sync::OnceLock<std::sync::mpsc::Sender<Option<bool>>> = std::sync::OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Option<bool>>();
        let app = app.clone();
        std::thread::spawn(move || {
            for msg in rx {
                match msg {
                    Some(true) => key_down(&app),
                    Some(false) => key_up(&app),
                    None => cancel(&app),
                }
            }
        });
        tx
    });
    let _ = tx.send(msg);
}

/// Escape is a global shortcut only while recording, so it keeps working normally in every app otherwise.
fn escape_cancels(app: &AppHandle, on: bool) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let gs = app.global_shortcut();
    if on {
        let _ = gs.on_shortcut("Escape", |app, _, ev| {
            if matches!(ev.state, ShortcutState::Pressed) { cancel_key(app); }
        });
    } else {
        let _ = gs.unregister("Escape");
    }
}

fn cancel(app: &AppHandle) {
    escape_cancels(app, false);
    let Some(s) = app.state::<AppState>().croaker.lock().unwrap().take() else { return };
    // Stopping the recording also ends the phrase and level threads (they watch `alive`).
    let _ = s.rec.stop();
    pill(app, "hidden", "", "");
}

fn key_down(app: &AppHandle) {
    let st = app.state::<AppState>();
    let mut guard = st.croaker.lock().unwrap();
    if let Some(s) = guard.as_ref() {
        // Second press of a locked (toggle) recording stops it; the matching release is then a no-op.
        if s.locked {
            let s = guard.take().unwrap();
            drop(guard);
            finish(app, s);
        }
        return;
    }
    let t_key = Instant::now();
    let ctx = profiles::detect(&st.data);
    let detect_ms = t_key.elapsed().as_millis();
    let settings = st.data.settings();
    if settings.disabled_apps.iter().any(|a| a.eq_ignore_ascii_case(&ctx.exe) || a.eq_ignore_ascii_case(&ctx.app)) {
        return;
    }
    // Pill and start sound first: opening the device can take a while (Bluetooth headsets switch profile).
    pill(app, "listening", "", ctx.app.clone());
    let t_mic = Instant::now();
    let started = Recording::start(Source::Mic);
    log_line(&st.data, &format!("key-down: detect {detect_ms} ms, mic open {} ms ({})", t_mic.elapsed().as_millis(),
        if started.is_ok() { "ok" } else { "failed" }));
    match started {
        Ok(rec) => {
            let (level, rms, alive) = (rec.level.clone(), rec.rms.clone(), rec.alive.clone());
            let live = Arc::new(Mutex::new(Live::default()));
            let phrases = {
                let (d, drain, a, l) = (st.data.clone(), rec.drainer(), alive.clone(), live.clone());
                std::thread::spawn(move || { providers::warm_local_stt(&d); transcribe_phrases(d, drain, a, l) })
            };
            // Measured after the mic opened: the key-up of a quick tap is only handled once this returns.
            *guard = Some(Session { rec, ctx, pressed_at: Instant::now(), locked: false, live, phrases: Some(phrases) });
            drop(guard);
            escape_cancels(app, true);
            // Levels only (never audio) at ~60 Hz on the pill's channel; reads atomics, so the audio thread never waits.
            if let Some(ch) = st.pill_levels.lock().unwrap().clone() {
                std::thread::spawn(move || {
                    let t0 = Instant::now();
                    while alive.load(Relaxed) {
                        let _ = ch.send(serde_json::json!({
                            "t": t0.elapsed().as_millis() as u64,
                            "rms": f32::from_bits(rms.load(Relaxed)),
                            "peak": f32::from_bits(level.load(Relaxed)),
                        }));
                        std::thread::sleep(Duration::from_millis(16));
                    }
                });
            }
            let data = st.data.clone();
            tauri::async_runtime::spawn(async move { providers::prewarm(&data, "dictation").await });
        }
        Err(e) => {
            drop(guard);
            pill(app, "error", format!("Mic unavailable: {e}"), "");
            hide_later(app, 3000);
        }
    }
}

fn key_up(app: &AppHandle) {
    let st = app.state::<AppState>();
    let mut guard = st.croaker.lock().unwrap();
    let Some(s) = guard.as_mut() else { return };
    if s.locked {
        return;
    }
    let settings = st.data.settings();
    let mode = settings.hotkey_mode;
    if mode == "toggle" || (mode == "auto" && s.pressed_at.elapsed().as_millis() < TAP_MS) {
        s.locked = true;
        return;
    }
    let s = guard.take().unwrap();
    drop(guard);
    finish(app, s);
}

/// Whisper-family models invent these on near-silence; on their own they count as no speech.
fn is_filler(text: &str) -> bool {
    let t: String = text.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().to_lowercase();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.is_empty() || matches!(t.as_str(),
        "thank you" | "thank you very much" | "thanks" | "thanks for watching" | "thank you for watching"
        | "you" | "bye" | "bye bye" | "subtitles by the amaraorg community" | "music" | "blankaudio")
}

fn finish(app: &AppHandle, mut session: Session) {
    escape_cancels(app, false);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let t0 = Instant::now();
        let rest = session.rec.stop();
        pill(&app, "working", "", session.ctx.app.clone());
        // The phrase thread exits once the recording stops; wait for a phrase it may be mid-way through.
        if let Some(h) = session.phrases.take() {
            let _ = tauri::async_runtime::spawn_blocking(move || h.join()).await;
        }
        let live = std::mem::take(&mut *session.live.lock().unwrap());
        let mut samples = live.pcm;
        samples.extend(rest);
        if samples.len() < (audio::RATE as usize * 3 / 10) {
            pill(&app, "hidden", "", "");
            return;
        }
        // Silence makes Whisper-style models hallucinate ("Thank you."), so it never reaches STT or the LLM.
        if !audio::has_speech(&samples) {
            pill(&app, "done", "Didn't hear anything", "");
            hide_later(&app, 900);
            return;
        }
        let data = app.state::<AppState>().data.clone();
        // Only the unfinished tail is left unless a phrase failed mid-way.
        let (mut texts, from) = if live.failed { (vec![], 0) } else { (live.texts, live.done) };
        let mut stt = live.stt;
        let tail = &samples[from.min(samples.len())..];
        if audio::has_speech(tail) {
            match providers::transcribe(&data, "stt.dictation", audio::wav_bytes(tail), "en").await {
                Ok(o) => {
                    if !is_filler(&o.text) { texts.push(o.text.trim().to_string()); }
                    stt = o.via;
                }
                Err(e) if texts.is_empty() => {
                    pill(&app, "error", e, "");
                    hide_later(&app, 6000);
                    return;
                }
                Err(_) => {}
            }
        }
        let raw = texts.join(" ");
        if is_filler(&raw) {
            pill(&app, "done", "Didn't hear anything", "");
            hide_later(&app, 900);
            return;
        }
        match process(&data, &session.ctx, &raw, true).await {
            Ok(mut r) => {
                if let Some(t) = r.insert.clone() {
                    let pasted = tauri::async_runtime::spawn_blocking(move || crate::insert::paste(&t)).await;
                    if !matches!(pasted, Ok(Ok(()))) {
                        r.notice = "Couldn't insert (elevated window?) — saved in History".into();
                    }
                }
                let latency = t0.elapsed().as_millis() as u64;
                save_history(&data, &session.ctx, &r, &raw, &stt, latency);
                let _ = app.emit_to("main", "history-changed", ());
                // The inserted text is the confirmation; the pill only lingers to report a problem.
                if r.notice.is_empty() {
                    pill(&app, "hidden", "", "");
                } else {
                    pill(&app, "error", r.notice.clone(), "");
                    hide_later(&app, 3000);
                }
            }
            Err(e) => {
                pill(&app, "error", e, "");
                hide_later(&app, 4000);
            }
        }
    });
}

#[derive(Serialize, Default)]
pub struct Processed {
    pub insert: Option<String>,
    pub label: String,
    pub notice: String,
    pub via: String,
    pub language: String,
    pub is_command: bool,
}

/// Transcript → command or cleaned (and maybe translated) text. Shared by push-to-talk and the Styles test box.
pub async fn process(data: &Data, ctx: &AppCtx, raw: &str, execute: bool) -> Result<Processed, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Processed { label: "Nothing heard".into(), ..Default::default() });
    }
    let settings = data.settings();
    let snippets = snippets(data);
    if let Some(cmd) = cleanup::match_command(raw, &snippets) {
        let agentish = ctx.category == "terminal" || ctx.category.starts_with("ai_agent");
        let label = match &cmd {
            Command::Undo => "Undo".to_string(),
            Command::SelectAll => "Select all".to_string(),
            Command::Enter if agentish => "“send” is disabled in terminals and agent inputs".to_string(),
            Command::Enter => "Enter".to_string(),
            Command::Snippet(_) => "Snippet".to_string(),
        };
        if execute {
            let r = match &cmd {
                Command::Undo => crate::insert::undo(),
                Command::SelectAll => crate::insert::select_all(),
                Command::Enter if !agentish => crate::insert::enter(),
                _ => Ok(()),
            };
            r?;
        }
        let insert = if let Command::Snippet(s) = cmd { Some(s) } else { None };
        return Ok(Processed { insert, label, via: "command".into(), is_command: true, ..Default::default() });
    }

    let (prefix_lang, body) = cleanup::language_prefix(raw);
    let target = prefix_lang
        .map(String::from)
        .or_else(|| settings.per_app_language.get(&ctx.exe).or(settings.per_app_language.get(&ctx.app)).cloned())
        .unwrap_or(settings.output_language.clone());
    let translate = target != "en" && !profiles::never_translate(&ctx.category);
    let dict = dictionary(data);
    let ruled = cleanup::rules(&body, &ctx.category, &dict, &snippets);
    let mut out = Processed { insert: Some(ruled.clone()), label: "Inserted".into(), via: "rules".into(), language: "en".into(), ..Default::default() };

    if translate || cleanup::needs_llm(&body, &ctx.category, &settings.mode) {
        let (pf, _) = providers::load(data);
        let local = pf.policies.per_app_local_only.iter().any(|a| a.eq_ignore_ascii_case(&ctx.exe) || a.eq_ignore_ascii_case(&ctx.app));
        let (system, user) = profiles::build_prompt(data, ctx, &settings, &dict, if translate { &target } else { "en" }, &body);
        // A cap, not a target: unused output tokens cost nothing, but a tight cap can end the reply before any text.
        let max_tokens = ((body.split_whitespace().count() as f32 * 1.3 * 2.0) as u32 + 64).clamp(1024, 4096);
        match providers::chat(data, "dictation", &system, &user, max_tokens, local).await {
            Ok(o) if !o.text.is_empty() => {
                out.insert = Some(o.text);
                out.via = o.via;
                if translate {
                    out.language = target.clone();
                    out.label = format!("Inserted · EN → {}", target.to_uppercase());
                }
            }
            Ok(_) | Err(_) if translate => out.notice = "Translation unavailable — inserted English".into(),
            Ok(_) => out.via = "rules · AI failed: empty reply".into(),
            Err(e) => {
                out.notice = "AI cleanup failed — see History".into();
                out.via = format!("rules · AI failed: {e}");
            }
        }
    }
    Ok(out)
}

fn save_history(data: &Data, ctx: &AppCtx, r: &Processed, raw: &str, stt: &str, latency_ms: u64) {
    let s = data.settings();
    if !s.history_enabled || r.is_command {
        return;
    }
    let d = Dictation {
        id: uuid::Uuid::new_v4().to_string(),
        time: crate::store::now(),
        app: ctx.app.clone(),
        category: ctx.category.clone(),
        language: r.language.clone(),
        raw: raw.to_string(),
        final_text: r.insert.clone().unwrap_or_default(),
        via: r.via.clone(),
        stt: stt.to_string(),
        latency_ms,
    };
    let _ = data.add_dictation(&d);
    data.prune_dictations(s.history_keep_last, s.history_keep_days);
}

#[cfg(test)]
mod tests {
    #[test]
    fn filler_is_not_speech() {
        for t in ["Thank you.", " thank you! ", "[BLANK_AUDIO]", "Thanks for watching!", "", "..."] {
            assert!(super::is_filler(t), "{t}");
        }
        for t in ["Thank you for the update.", "Send it to Priya", "you there?"] {
            assert!(!super::is_filler(t), "{t}");
        }
    }
}
