mod audio;
mod cleanup;
mod croaker;
mod insert;
mod meeting_ui;
mod models;
mod needle;
mod notes;
mod profiles;
mod providers;
mod sherpa;
mod speakers;
mod store;

use serde_json::{json, Value};
use std::sync::Mutex;
use store::{read_json, write_json, Data, Settings};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub struct AppState {
    pub data: Data,
    pub croaker: Mutex<Option<croaker::Session>>,
    pub meeting: Mutex<Option<notes::Live>>,
    pub hotkey_error: Mutex<Option<String>>,
    /// The pill's level channel (~60 Hz frames while Croaker records); set when the pill page loads.
    pub pill_levels: Mutex<Option<tauri::ipc::Channel<Value>>>,
}

#[tauri::command]
fn pill_levels(st: State<AppState>, on_frame: tauri::ipc::Channel<Value>) {
    *st.pill_levels.lock().unwrap() = Some(on_frame);
}

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---------- status & settings ----------

// Commands that touch the keychain, audio devices or many files are async so they run off the main thread.
#[tauri::command]
async fn status(st: State<'_, AppState>) -> R<Value> {
    let (pf, _) = providers::load(&st.data);
    let keys: serde_json::Map<String, Value> =
        pf.providers.iter().map(|p| (p.id.clone(), json!(providers::get_key(&p.id).is_some()))).collect();
    let s = st.data.settings();
    Ok(json!({
        "data_dir": st.data.root.to_string_lossy(),
        "platform": std::env::consts::OS,
        "keys": keys,
        "whistle": false,
        "send_audio_to_cloud": pf.policies.send_audio_to_cloud,
        "local_only": pf.policies.local_only_mode,
        "hotkey": s.hotkey,
        "meeting_hotkey": s.meeting_hotkey,
        "meeting_live": st.meeting.lock().unwrap().as_ref().map(|l| l.id.clone()),
        "hotkey_error": st.hotkey_error.lock().unwrap().clone(),
        "hotkey_mode": s.hotkey_mode,
        "stt_ready": providers::stt_ready(&st.data),
        "dictations": st.data.list_dictations().len(),
        "meetings": notes::list(&st.data, "").len(),
    }))
}

#[tauri::command]
async fn mic_check() -> R<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host().default_input_device().ok_or("No microphone found")?.name().map_err(err)
}

#[tauri::command]
async fn list_models(st: State<'_, AppState>) -> R<Vec<Value>> {
    Ok(models::list(&st.data))
}

#[tauri::command]
async fn download_model(app: AppHandle, st: State<'_, AppState>, id: String) -> R<()> {
    models::download(&app, &st.data, &id).await?;
    let _ = app.emit("models-changed", ());
    Ok(())
}

#[tauri::command]
fn delete_model(app: AppHandle, st: State<AppState>, id: String) -> R<()> {
    models::delete(&st.data, &id)?;
    let _ = app.emit("models-changed", ());
    Ok(())
}

#[tauri::command]
fn reveal_models(st: State<AppState>) -> R<()> {
    let dir = st.data.path("models");
    let cmd = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(dir).spawn().map(|_| ()).map_err(err)
}

#[tauri::command]
fn get_settings(st: State<AppState>) -> Settings {
    st.data.settings()
}

#[tauri::command]
fn save_settings(app: AppHandle, st: State<AppState>, settings: Settings) -> R<()> {
    write_json(&st.data.path("config/settings.json"), &settings).map_err(err)?;
    register_hotkeys(&app)
}

#[tauri::command]
fn get_config(st: State<AppState>) -> Value {
    let (pf, routing) = providers::load(&st.data);
    json!({ "providers": pf, "routing": routing })
}

#[tauri::command]
fn save_config(st: State<AppState>, providers: providers::ProvidersFile, routing: providers::Routing) -> R<()> {
    write_json(&st.data.path("config/providers.json"), &providers).map_err(err)?;
    write_json(&st.data.path("config/routing.json"), &routing).map_err(err)
}

#[tauri::command]
fn set_api_key(st: State<AppState>, id: String, key: String) -> R<()> {
    if key.trim().is_empty() {
        let path = st.data.path("config/models-cache.json");
        let mut cache: serde_json::Map<String, Value> = read_json(&path);
        cache.remove(&id);
        let _ = write_json(&path, &cache);
    }
    providers::set_key(&id, key.trim())
}

#[tauri::command]
async fn probe_provider(st: State<'_, AppState>, id: String) -> R<Vec<String>> {
    let path = st.data.path("config/models-cache.json");
    let mut cache: serde_json::Map<String, Value> = read_json(&path);
    let r = providers::probe(&st.data, &id).await;
    match &r {
        Ok(models) => { cache.insert(id, json!(models)); }
        Err(_) => { cache.remove(&id); }
    }
    let _ = write_json(&path, &cache);
    r
}

/// Models listed by each provider at its last successful key test.
#[tauri::command]
fn models_cache(st: State<AppState>) -> Value {
    read_json(&st.data.path("config/models-cache.json"))
}

#[tauri::command]
fn languages() -> Vec<(&'static str, &'static str)> {
    cleanup::LANGUAGES.to_vec()
}

// ---------- Croaker ----------

#[tauri::command]
async fn list_history(st: State<'_, AppState>) -> R<Vec<store::Dictation>> {
    Ok(st.data.list_dictations())
}

#[tauri::command]
fn delete_history(st: State<AppState>, ids: Vec<String>, all: bool) -> usize {
    st.data.delete_dictations(|d| all || ids.contains(&d.id))
}

#[tauri::command]
async fn stats(st: State<'_, AppState>) -> R<Value> {
    let all = st.data.list_dictations();
    let words: usize = all.iter().map(|d| d.final_text.split_whitespace().count()).sum();
    let avg = if all.is_empty() { 0 } else { all.iter().map(|d| d.latency_ms).sum::<u64>() / all.len() as u64 };
    let month = chrono::Local::now().format("%Y-%m");
    let usage: Vec<Value> = store::read_jsonl(&st.data.path(&format!("usage/usage-{month}.jsonl")));
    let tokens: u64 = usage.iter().map(|u| u["input_tokens"].as_u64().unwrap_or(0) + u["output_tokens"].as_u64().unwrap_or(0)).sum();
    let (mut cost, mut unpriced) = (0.0, false);
    for u in &usage {
        match providers::price_per_mtok(u["via"].as_str().unwrap_or("")) {
            Some((i, o)) => cost += (u["input_tokens"].as_f64().unwrap_or(0.0) * i + u["output_tokens"].as_f64().unwrap_or(0.0) * o) / 1e6,
            None => unpriced = true,
        }
    }
    // 40 wpm typing vs speaking; plan §6.2 #17.
    Ok(json!({ "dictations": all.len(), "words": words, "avg_latency_ms": avg, "minutes_saved": words / 40,
        "tokens_this_month": tokens, "cost_this_month": cost, "cost_unpriced": unpriced }))
}

const STORES: &[&str] = &["dictionary.json", "snippets.json", "config/app-registry.json", "people/people.json"];

#[tauri::command]
fn get_store(st: State<AppState>, name: String) -> R<Value> {
    if !STORES.contains(&name.as_str()) { return Err("unknown store".into()); }
    if name == "config/app-registry.json" { profiles::registry(&st.data); }
    Ok(read_json(&st.data.path(&name)))
}

#[tauri::command]
fn save_store(st: State<AppState>, name: String, value: Value) -> R<()> {
    if !STORES.contains(&name.as_str()) { return Err("unknown store".into()); }
    write_json(&st.data.path(&name), &value).map_err(err)
}

#[tauri::command]
fn get_profile(st: State<AppState>, category: String) -> Value {
    let key = category.split('.').next().unwrap_or(&category).to_string();
    let o: Value = read_json(&st.data.path(&format!("config/app-profiles/{key}.json")));
    let custom = o["prompt"].as_str().unwrap_or("").to_string();
    json!({ "base": profiles::L0_BASE, "default": profiles::default_profile(&category), "custom": custom })
}

#[tauri::command]
fn save_profile(st: State<AppState>, category: String, prompt: String) -> R<()> {
    let key = category.split('.').next().unwrap_or(&category).to_string();
    let p = st.data.path(&format!("config/app-profiles/{key}.json"));
    if prompt.trim().is_empty() {
        let _ = std::fs::remove_file(p);
        return Ok(());
    }
    write_json(&p, &json!({ "prompt": prompt, "updated_at": store::now() })).map_err(err)
}

/// Styles pane live test: runs the same pipeline as push-to-talk, without audio or insertion.
#[tauri::command]
async fn test_cleanup(st: State<'_, AppState>, text: String, category: String, app: String) -> R<croaker::Processed> {
    let ctx = profiles::AppCtx { app: if app.is_empty() { "Test".into() } else { app }, category, ..Default::default() };
    croaker::process(&st.data, &ctx, &text, false).await
}

// ---------- Remora Notes ----------

#[tauri::command]
fn templates() -> Vec<(&'static str, &'static str)> {
    notes::TEMPLATES.iter().map(|t| (t.0, t.1)).collect()
}

#[tauri::command]
fn list_meetings(st: State<AppState>, query: String) -> Vec<notes::Meeting> {
    notes::list(&st.data, &query)
}

#[tauri::command]
fn get_meeting(st: State<AppState>, id: String) -> R<notes::MeetingFull> {
    notes::get(&st.data, &id)
}

#[tauri::command]
fn start_meeting(app: AppHandle, st: State<AppState>) -> R<String> {
    let mut g = st.meeting.lock().unwrap();
    if let Some(l) = g.as_ref() { return Ok(l.id.clone()); }
    let live = notes::start(&app, &st.data)?;
    let id = live.id.clone();
    *g = Some(live);
    let _ = app.emit("meeting-state", json!({ "live": id }));
    let _ = app.emit("meetings-changed", ());
    Ok(id)
}

#[tauri::command]
async fn stop_meeting(app: AppHandle, st: State<'_, AppState>) -> R<()> {
    let live = st.meeting.lock().unwrap().take();
    let Some(l) = live else { return Ok(()) };
    let id = l.id.clone();
    let _ = app.emit("meeting-state", json!({ "live": null, "stopping": true }));
    l.stop().await;
    let _ = app.emit("meeting-state", json!({ "live": null }));
    // Name, categorise and summarise straight away; failures (no key, empty meeting) leave it for a manual Enhance.
    let data = st.data.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app.emit("meeting-enhancing", &id);
        // Names first, so the notes say "Anand" rather than "Speaker 2".
        if let Err(e) = notes::suggest_names(&data, &id).await { let _ = app.emit("meeting-error", format!("Speaker names: {e}")); }
        let r = notes::enhance(&data, &id, "auto").await;
        let _ = app.emit("meeting-enhanced", json!({ "id": id, "error": r.err() }));
        let _ = app.emit("meetings-changed", ());
    });
    Ok(())
}

fn toggle_meeting(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let st = app.state::<AppState>();
        let live = st.meeting.lock().unwrap().is_some();
        let r = if live { stop_meeting(app.clone(), st).await } else { start_meeting(app.clone(), st).map(|_| ()) };
        if let Err(e) = r { let _ = app.emit("meeting-error", e); }
    });
}

#[tauri::command]
fn save_meeting_file(st: State<AppState>, id: String, name: String, content: String) -> R<()> {
    notes::save_file(&st.data, &id, &name, &content)
}

#[tauri::command]
fn rename_speaker(st: State<AppState>, id: String, from: String, to: String) -> R<()> {
    notes::rename_speaker(&st.data, &id, &from, &to)
}

#[tauri::command]
fn delete_meeting_part(st: State<AppState>, id: String, part: String, name: String) -> R<()> {
    notes::delete(&st.data, &id, &part, &name)
}

#[tauri::command]
async fn enhance(st: State<'_, AppState>, id: String, template: String) -> R<String> {
    notes::enhance(&st.data, &id, &template).await
}

#[tauri::command]
async fn ask(st: State<'_, AppState>, id: String, question: String) -> R<Value> {
    notes::ask(&st.data, &id, &question).await
}

#[tauri::command]
fn undo_ask(st: State<AppState>, id: String, index: usize) -> R<()> {
    notes::undo(&st.data, &id, index)
}

#[tauri::command]
fn resolve_speaker(st: State<AppState>, id: String, label: String, accept: bool) -> R<()> {
    notes::resolve_suggestion(&st.data, &id, &label, accept)
}

/// "Check speaker names": what Remora can read from the open meeting app right now. The full label
/// list is saved to logs/ so unsupported layouts can be fixed.
#[tauri::command]
fn check_speaker_names(st: State<AppState>) -> R<Value> {
    let snap = meeting_ui::snapshot()?;
    let path = st.data.path("logs/meeting-ui.txt");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(&path, format!("{} — {}\n\n{}", snap.app, snap.window, snap.dump));
    Ok(json!({ "app": snap.app, "window": snap.window, "elements": snap.elements, "participants": snap.participants,
        "speaking": snap.speaking, "log": path.to_string_lossy() }))
}

#[tauri::command]
async fn translate_summary(st: State<'_, AppState>, id: String, file: String, lang: String) -> R<String> {
    notes::translate_summary(&st.data, &id, &file, &lang).await
}

// ---------- privacy & data ----------

#[tauri::command]
fn reveal_data(st: State<AppState>) -> R<()> {
    let cmd = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(&st.data.root).spawn().map(|_| ()).map_err(err)
}

#[tauri::command]
fn delete_category(st: State<AppState>, category: String) -> R<()> {
    let rel = match category.as_str() {
        "history" => "dictations", "meetings" => "meetings", "voice-profiles" => "voice-profiles",
        "usage" => "usage", "logs" => "logs", "trash" => "trash", "index" => "index",
        "dictionary" => "dictionary.json", "snippets" => "snippets.json", "people" => "people",
        _ => return Err("unknown category".into()),
    };
    let p = st.data.path(rel);
    let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    Data::at(st.data.root.clone());
    Ok(())
}

#[tauri::command]
fn delete_all_data(st: State<AppState>, confirm: String, include_models: bool, include_keys: bool) -> R<()> {
    if confirm != "DELETE" { return Err("Type DELETE to confirm".into()); }
    if st.meeting.lock().unwrap().is_some() { return Err("Stop the live meeting first".into()); }
    if include_keys {
        for p in providers::load(&st.data).0.providers { let _ = providers::set_key(&p.id, ""); }
    }
    for e in std::fs::read_dir(&st.data.root).map_err(err)?.flatten() {
        if !include_models && e.file_name() == "models" { continue; }
        let p = e.path();
        let _ = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    }
    Data::at(st.data.root.clone());
    Ok(())
}

// ---------- wiring ----------

/// Registers each hotkey independently so one taken chord doesn't disable the other.
fn register_hotkeys(app: &AppHandle) -> R<()> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let st = app.state::<AppState>();
    let s = st.data.settings();
    let mut errs = vec![];
    if let Err(e) = gs.on_shortcut(s.hotkey.as_str(), |app, _, ev| match ev.state {
        ShortcutState::Pressed => croaker::key(app, true),
        ShortcutState::Released => croaker::key(app, false),
    }) {
        errs.push(format!("Croaker hotkey {} is unavailable ({e}). Pick another in Settings → Croaker.", s.hotkey));
    }
    if let Err(e) = gs.on_shortcut(s.meeting_hotkey.as_str(), |app, _, ev| {
        if matches!(ev.state, ShortcutState::Pressed) { toggle_meeting(app.clone()); }
    }) {
        errs.push(format!("Meeting hotkey {} is unavailable ({e}). Pick another in Settings → Remora Notes.", s.meeting_hotkey));
    }
    let msg = errs.join(" ");
    *st.hotkey_error.lock().unwrap() = (!msg.is_empty()).then(|| msg.clone());
    if msg.is_empty() { Ok(()) } else { Err(msg) }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data = Data::open();
    let s = data.settings();
    data.prune_dictations(s.history_keep_last, s.history_keep_days);
    data.purge_trash(s.trash_days);
    notes::prune_audio(&data, s.meeting_audio_keep_days);
    providers::load(&data);
    profiles::registry(&data);
    for m in models::catalog(&data) { models::prune(&data, &m); }
    if models::needs_engine(&data) {
        let (h, d) = (app.handle().clone(), data.clone());
        tauri::async_runtime::spawn(async move {
            if let Err(e) = models::ensure_engine(&h, &d, "whistle").await { eprintln!("Cactus engine: {e}"); }
        });
    }
    let warm = data.clone();
    std::thread::spawn(move || providers::warm_local_stt(&warm));
    app.manage(AppState { data, croaker: Mutex::new(None), meeting: Mutex::new(None), hotkey_error: Mutex::new(None), pill_levels: Mutex::new(None) });

    if let Err(e) = register_hotkeys(app.handle()) {
        eprintln!("{e}");
    }

    if let Some(main) = app.get_webview_window("main") {
        #[cfg(windows)]
        let _ = window_vibrancy::apply_mica(&main, None);
        #[cfg(target_os = "macos")]
        let _ = window_vibrancy::apply_vibrancy(&main, window_vibrancy::NSVisualEffectMaterial::Sidebar, None, None);
        let _ = &main;
    }

    if let Some(pill) = app.get_webview_window("pill") {
        let _ = pill.set_ignore_cursor_events(true);
        if let Ok(Some(m)) = pill.primary_monitor() {
            let (sz, pos, k) = (m.size(), m.position(), m.scale_factor());
            // 476×128 = the 420×72 pill area plus a 28 px transparent margin for the listening waves;
            // 68 (= 96 − 28) keeps the visible pill where it always was.
            let (w, h) = ((476.0 * k) as i32, (128.0 * k) as i32);
            let _ = pill.set_position(PhysicalPosition::new(
                pos.x + (sz.width as i32 - w) / 2,
                pos.y + sz.height as i32 - h - (68.0 * k) as i32,
            ));
        }
    }

    let open = MenuItem::with_id(app, "open", "Open Remora", true, None::<&str>)?;
    let meeting = MenuItem::with_id(app, "meeting", "Start / stop meeting notes", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Remora", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &meeting, &quit])?;
    let mut tray = TrayIconBuilder::with_id("remora").tooltip("Remora").menu(&menu).on_menu_event(|app, e| {
        match e.id.as_ref() {
            "open" => show_main(app),
            "meeting" => toggle_meeting(app.clone()),
            "quit" => app.exit(0),
            _ => {}
        }
    });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show_main(app)))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(setup)
        .on_window_event(|w, e| {
            // Closing the main window keeps Remora in the tray so hotkeys keep working.
            if let WindowEvent::CloseRequested { api, .. } = e {
                if w.label() == "main" {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            status, pill_levels, get_settings, save_settings, get_config, save_config, set_api_key, probe_provider, models_cache, languages, mic_check,
            list_models, download_model, delete_model, reveal_models,
            list_history, delete_history, stats, get_store, save_store, get_profile, save_profile, test_cleanup,
            templates, list_meetings, get_meeting, start_meeting, stop_meeting, save_meeting_file, rename_speaker,
            delete_meeting_part, enhance, ask, undo_ask, resolve_speaker, check_speaker_names, translate_summary,
            reveal_data, delete_category, delete_all_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Remora");
}
