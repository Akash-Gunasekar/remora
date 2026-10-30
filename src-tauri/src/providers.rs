//! AI provider layer (plan §7): task routing, fallback chains, keychain secrets, usage ledger.

use crate::store::{append_jsonl, now, read_json, write_json, Data};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone)]
pub struct Provider {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Route {
    pub chain: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    30_000
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Policies {
    pub local_only_mode: bool,
    pub send_audio_to_cloud: bool,
    pub per_app_local_only: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ProvidersFile {
    pub providers: Vec<Provider>,
    pub policies: Policies,
}

pub type Routing = HashMap<String, Route>;

fn p(id: &str, kind: &str, base: Option<&str>) -> Provider {
    Provider { id: id.into(), kind: kind.into(), base_url: base.map(String::from) }
}

fn r(chain: &[&str], timeout_ms: u64) -> Route {
    Route { chain: chain.iter().map(|s| s.to_string()).collect(), timeout_ms }
}

pub fn default_providers() -> ProvidersFile {
    ProvidersFile {
        providers: vec![
            p("claude", "anthropic", None),
            p("groq", "groq", None),
            p("openai", "openai", None),
            p("openrouter", "openrouter", None),
            p("ollama-local", "ollama", Some("http://localhost:11434")),
            p("lmstudio", "openai_compatible", Some("http://localhost:1234/v1")),
        ],
        policies: Policies::default(),
    }
}

pub fn default_routing() -> Routing {
    HashMap::from([
        ("stt.dictation".into(), r(&["local:whistle", "groq:whisper-large-v3-turbo", "openai:gpt-4o-mini-transcribe"], 15_000)),
        ("stt.meeting".into(), r(&["local:whistle", "groq:whisper-large-v3-turbo", "openai:gpt-4o-mini-transcribe"], 60_000)),
        ("dictation".into(), r(&["claude:claude-haiku-5-5", "ollama-local:llama3.2", "rules"], 4_000)),
        ("meeting".into(), r(&["claude:claude-sonnet-5-5", "ollama-local:llama3.2"], 120_000)),
    ])
}

/// Older builds routed each sub-task separately; keep the user's pick from the main one.
const LEGACY: &[(&str, &str)] = &[("dictation.cleanup", "dictation"), ("meeting.enhance", "meeting")];

pub fn load(data: &Data) -> (ProvidersFile, Routing) {
    let pp = data.path("config/providers.json");
    let rp = data.path("config/routing.json");
    if !pp.exists() {
        let _ = write_json(&pp, &default_providers());
    }
    if !rp.exists() {
        let _ = write_json(&rp, &default_routing());
    }
    let mut routing: Routing = read_json(&rp);
    let defaults = default_routing();
    if routing.keys().any(|k| !defaults.contains_key(k)) {
        for (old, new) in LEGACY {
            if let Some(mut v) = routing.remove(*old) {
                v.timeout_ms = v.timeout_ms.max(defaults[*new].timeout_ms);
                routing.entry(new.to_string()).or_insert(v);
            }
        }
        routing.retain(|k, _| defaults.contains_key(k));
        let _ = write_json(&rp, &routing);
    }
    for (k, v) in defaults {
        // Defaults are a floor: older builds saved a 2 s dictation timeout that Sonnet's first token often misses.
        let r = routing.entry(k).or_insert_with(|| v.clone());
        r.timeout_ms = r.timeout_ms.max(v.timeout_ms);
    }
    (read_json(&pp), routing)
}

// ---------- secrets (OS keychain only) ----------

pub fn get_key(id: &str) -> Option<String> {
    keyring::Entry::new("remora", id).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

pub fn set_key(id: &str, key: &str) -> Result<(), String> {
    let e = keyring::Entry::new("remora", id).map_err(|e| e.to_string())?;
    if key.is_empty() {
        let _ = e.delete_credential();
        Ok(())
    } else {
        e.set_password(key).map_err(|e| e.to_string())
    }
}

// ---------- plumbing ----------

pub(crate) fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .expect("http client")
    })
}

fn base_url(p: &Provider) -> String {
    p.base_url.clone().unwrap_or_else(|| {
        match p.kind.as_str() {
            "anthropic" => "https://api.anthropic.com",
            "openai" => "https://api.openai.com/v1",
            "groq" => "https://api.groq.com/openai/v1",
            "openrouter" => "https://openrouter.ai/api/v1",
            "ollama" => "http://localhost:11434",
            _ => "http://localhost:8000/v1",
        }
        .to_string()
    })
    .trim_end_matches('/')
    .to_string()
}

fn is_local(p: &Provider) -> bool {
    let u = base_url(p);
    u.contains("://localhost") || u.contains("://127.0.0.1") || u.contains("://[::1]")
}

fn split_target(t: &str) -> (&str, &str) {
    t.split_once(':').unwrap_or((t, ""))
}

fn find<'a>(pf: &'a ProvidersFile, id: &str) -> Result<&'a Provider, String> {
    pf.providers.iter().find(|p| p.id == id).ok_or(format!("unknown provider '{id}'"))
}

async fn check(resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = body.pointer("/error/message").and_then(Value::as_str).map(String::from)
            .unwrap_or_else(|| body.to_string());
        return Err(format!("HTTP {status}: {msg}"));
    }
    Ok(body)
}

fn log_usage(data: &Data, task: &str, via: &str, input: u64, output: u64, ms: u128) {
    let month = chrono::Local::now().format("%Y-%m");
    let _ = append_jsonl(
        &data.path(&format!("usage/usage-{month}.jsonl")),
        &json!({ "time": now(), "task": task, "via": via, "input_tokens": input, "output_tokens": output, "ms": ms }),
    );
}

/// USD per million (input, output) tokens for a usage `via` like "claude:claude-haiku-5-5". Local runtimes are free.
// ponytail: list prices by model family, no cache discounts; update when providers change prices.
pub fn price_per_mtok(via: &str) -> Option<(f64, f64)> {
    let (pid, model) = split_target(via);
    let m = model.to_lowercase();
    if pid.starts_with("ollama") || pid.starts_with("lmstudio") || pid == "local" { return Some((0.0, 0.0)); }
    Some(match () {
        _ if m.contains("haiku") => (1.0, 5.0),
        _ if m.contains("sonnet") => (3.0, 15.0),
        _ if m.contains("opus") => (5.0, 25.0),
        _ if m.contains("gpt-4o-mini") => (0.15, 0.6),
        _ if m.contains("gpt-4.1-mini") => (0.4, 1.6),
        _ if m.contains("gpt-4o") => (2.5, 10.0),
        _ => return None,
    })
}

// ---------- chat ----------

pub struct ChatOut {
    pub text: String,
    pub via: String,
}

/// Hard ceiling for one streamed reply, however steadily tokens keep arriving.
const STREAM_CAP: Duration = Duration::from_secs(180);

/// Feeds each line of a streamed body to `on_line` until it returns `Ok(true)` or the stream ends.
/// `wait` bounds the wait for the first bytes, then for every gap between chunks: a long reply that keeps
/// flowing never times out, while a provider that stalls still fails fast and the chain moves on.
async fn stream_lines(mut resp: reqwest::Response, wait: Duration, mut on_line: impl FnMut(&str) -> Result<bool, String>) -> Result<(), String> {
    let mut buf: Vec<u8> = vec![];
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > STREAM_CAP { return Err(format!("reply still running after {} s", STREAM_CAP.as_secs())); }
        let chunk = match tokio::time::timeout(wait, resp.chunk()).await {
            Err(_) => return Err(format!("stalled: no data for {} ms", wait.as_millis())),
            Ok(r) => r.map_err(|e| e.to_string())?,
        };
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        while let Some(i) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line);
            if on_line(line.trim())? { return Ok(()); }
        }
    }
    if !buf.is_empty() { on_line(String::from_utf8_lossy(&buf).trim())?; }
    Ok(())
}

async fn send(req: reqwest::RequestBuilder, wait: Duration) -> Result<reqwest::Response, String> {
    let resp = tokio::time::timeout(wait, req.send()).await
        .map_err(|_| format!("no response within {} ms", wait.as_millis()))?
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() { Ok(resp) } else { Err(check(resp).await.unwrap_err()) }
}

fn sse_data(line: &str) -> Option<Value> {
    serde_json::from_str(line.strip_prefix("data:")?.trim()).ok()
}

async fn chat_once(p: &Provider, model: &str, system: &str, user: &str, max_tokens: u32, wait: Duration) -> Result<(String, u64, u64), String> {
    let base = base_url(p);
    let key = get_key(&p.id);
    let (mut text, mut input, mut output) = (String::new(), 0u64, 0u64);
    match p.kind.as_str() {
        "anthropic" => {
            let key = key.ok_or("no Anthropic API key")?;
            let body = json!({
                // No `temperature`: current Claude models reject it with HTTP 400.
                "model": model, "max_tokens": max_tokens, "stream": true,
                "system": [{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }],
                "messages": [{ "role": "user", "content": user }],
            });
            let req = client().post(format!("{base}/v1/messages")).header("x-api-key", key)
                .header("anthropic-version", "2023-06-01").json(&body);
            let mut stop = String::new();
            stream_lines(send(req, wait).await?, wait, |line| {
                let Some(v) = sse_data(line) else { return Ok(false) };
                match v["type"].as_str().unwrap_or("") {
                    "message_start" => input = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0),
                    "content_block_delta" => text.push_str(v["delta"]["text"].as_str().unwrap_or("")),
                    "message_delta" => {
                        output = v["usage"]["output_tokens"].as_u64().unwrap_or(output);
                        stop = v["delta"]["stop_reason"].as_str().unwrap_or("").to_string();
                    }
                    "message_stop" => return Ok(true),
                    "error" => return Err(v["error"]["message"].as_str().unwrap_or("stream error").to_string()),
                    _ => {}
                }
                Ok(false)
            }).await?;
            if text.trim().is_empty() { return Err(format!("empty reply (stop_reason: {stop})")); }
        }
        "ollama" => {
            let body = json!({
                "model": model, "stream": true, "options": { "temperature": 0, "num_predict": max_tokens },
                "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
            });
            let mut req = client().post(format!("{base}/api/chat")).json(&body);
            if let Some(k) = key { req = req.bearer_auth(k); }
            let resp = send(req, wait).await.map_err(|e| {
                if e.contains("not found") { format!("model '{model}' isn't installed in Ollama (run: ollama pull {model})") } else { e }
            })?;
            stream_lines(resp, wait, |line| {
                let Ok(v) = serde_json::from_str::<Value>(line) else { return Ok(false) };
                if let Some(e) = v["error"].as_str() { return Err(e.to_string()); }
                text.push_str(v["message"]["content"].as_str().unwrap_or(""));
                if v["done"] == true {
                    input = v["prompt_eval_count"].as_u64().unwrap_or(0);
                    output = v["eval_count"].as_u64().unwrap_or(0);
                    return Ok(true);
                }
                Ok(false)
            }).await?;
        }
        _ => {
            let body = json!({
                "model": model, "temperature": 0, "max_tokens": max_tokens, "stream": true,
                "stream_options": { "include_usage": true },
                "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
            });
            let mut req = client().post(format!("{base}/chat/completions")).json(&body);
            if let Some(k) = key { req = req.bearer_auth(k); }
            if p.kind == "openrouter" { req = req.header("X-Title", "Remora"); }
            stream_lines(send(req, wait).await?, wait, |line| {
                if line == "data: [DONE]" { return Ok(true); }
                let Some(v) = sse_data(line) else { return Ok(false) };
                if let Some(e) = v["error"]["message"].as_str() { return Err(e.to_string()); }
                text.push_str(v["choices"][0]["delta"]["content"].as_str().unwrap_or(""));
                if v["usage"].is_object() {
                    input = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
                    output = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
                }
                Ok(false)
            }).await?;
        }
    }
    Ok((text, input, output))
}

/// Walks the task's chain until one provider answers. "rules" entries are skipped; callers fall back to rules on Err.
pub async fn chat(data: &Data, task: &str, system: &str, user: &str, max_tokens: u32, force_local: bool) -> Result<ChatOut, String> {
    let (pf, routing) = load(data);
    let route = routing.get(task).ok_or(format!("no route for {task}"))?;
    let local_only = force_local || pf.policies.local_only_mode;
    // Longer input means a longer prompt to read before the first token; 20 ms per word on top of the route's budget.
    let wait = Duration::from_millis(route.timeout_ms + 20 * user.split_whitespace().count() as u64);
    let mut errs = vec![];
    for target in &route.chain {
        let (pid, model) = split_target(target);
        if pid == "rules" || pid == "local" { continue; }
        let p = match find(&pf, pid) { Ok(p) => p, Err(e) => { errs.push(e); continue } };
        if local_only && !is_local(p) {
            errs.push(format!("{pid}: skipped (local only)"));
            continue;
        }
        let t0 = std::time::Instant::now();
        match chat_once(p, model, system, user, max_tokens, wait).await {
            Ok((text, i, o)) => {
                log_usage(data, task, target, i, o, t0.elapsed().as_millis());
                return Ok(ChatOut { text: text.trim().to_string(), via: target.clone() });
            }
            Err(e) => errs.push(format!("{target}: {e}")),
        }
    }
    Err(errs.join("; "))
}

/// Opens the TLS connection to the first chat provider on key-down so the cleanup call skips the handshake.
pub async fn prewarm(data: &Data, task: &str) {
    let (pf, routing) = load(data);
    let Some(first) = routing.get(task).and_then(|r| r.chain.first()) else { return };
    if let Ok(p) = find(&pf, split_target(first).0) {
        let _ = tokio::time::timeout(Duration::from_secs(3), client().head(base_url(p)).send()).await;
    }
}

// ---------- speech to text ----------

#[derive(Serialize, Clone)]
pub struct SttSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub struct SttOut {
    pub text: String,
    pub segments: Vec<SttSegment>,
    pub via: String,
}

async fn stt_once(p: &Provider, model: &str, wav: Vec<u8>, language: &str) -> Result<SttOut, String> {
    if p.kind == "anthropic" || p.kind == "ollama" {
        return Err(format!("{} has no speech-to-text", p.kind));
    }
    let verbose = !model.starts_with("gpt-4o");
    let part = reqwest::multipart::Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav").map_err(|e| e.to_string())?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("temperature", "0")
        .text("response_format", if verbose { "verbose_json" } else { "json" });
    if !language.is_empty() { form = form.text("language", language.to_string()); }
    let mut req = client().post(format!("{}/audio/transcriptions", base_url(p))).multipart(form);
    if let Some(k) = get_key(&p.id) { req = req.bearer_auth(k); }
    let v = check(req.send().await.map_err(|e| e.to_string())?).await?;
    let segments = v["segments"].as_array().into_iter().flatten().map(|s| SttSegment {
        start_ms: (s["start"].as_f64().unwrap_or(0.0) * 1000.0) as u64,
        end_ms: (s["end"].as_f64().unwrap_or(0.0) * 1000.0) as u64,
        text: s["text"].as_str().unwrap_or("").trim().to_string(),
    }).collect();
    Ok(SttOut { text: v["text"].as_str().unwrap_or("").trim().to_string(), segments, via: String::new() })
}

fn whisper_local(file: &std::path::Path, wav: &[u8], language: &str) -> Result<SttOut, String> {
    use std::sync::{Arc, Mutex};
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
    // ponytail: one cached model; switching models reloads it (~1 s for base).
    static CTX: Mutex<Option<(std::path::PathBuf, Arc<WhisperContext>)>> = Mutex::new(None);
    let ctx = {
        let mut slot = CTX.lock().map_err(|e| e.to_string())?;
        match slot.as_ref() {
            Some((p, c)) if p == file => c.clone(),
            _ => {
                let c = Arc::new(WhisperContext::new_with_params(file, WhisperContextParameters::default())
                    .map_err(|e| format!("couldn't load model: {e}"))?);
                *slot = Some((file.to_path_buf(), c.clone()));
                c
            }
        }
    };
    let samples: Vec<f32> = hound::WavReader::new(std::io::Cursor::new(wav)).map_err(|e| e.to_string())?
        .into_samples::<i16>().filter_map(Result::ok).map(|s| s as f32 / 32768.0).collect();
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(if language.is_empty() { "auto" } else { language }));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    let mut state = ctx.create_state().map_err(|e| e.to_string())?;
    state.full(params, &samples).map_err(|e| e.to_string())?;
    let segments: Vec<SttSegment> = state.as_iter().map(|s| SttSegment {
        start_ms: s.start_timestamp() as u64 * 10,
        end_ms: s.end_timestamp() as u64 * 10,
        text: s.to_string().trim().to_string(),
    }).collect();
    let text = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
    Ok(SttOut { text, segments, via: String::new() })
}

/// A downloaded on-device model whose runtime is linked into this build.
pub fn local_ready(data: &Data, model: &str) -> bool {
    if model == "whistle" { return crate::needle::ready(data); }
    crate::models::whisper_file(data, model).is_some() || crate::sherpa::model_dir(data, model).is_some()
}

/// Loads the first ready on-device model of the dictation STT chain so the first dictation skips the load
/// (≈15 s for Parakeet). No-op when that model is already warm. Blocking.
pub fn warm_local_stt(data: &Data) {
    static WARM: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let (_, routing) = load(data);
    let Some(model) = routing.get("stt.dictation").into_iter().flat_map(|r| r.chain.iter())
        .filter_map(|t| t.strip_prefix("local:")).find(|m| local_ready(data, m)) else { return };
    let mut warm = WARM.lock().unwrap_or_else(|p| p.into_inner());
    if *warm == model { return; }
    if local_stt(data, model, &crate::audio::wav_bytes(&vec![0.0; crate::audio::RATE as usize / 2]), "en").is_ok() {
        *warm = model.to_string();
    }
}

/// Blocking; call from a worker thread.
fn local_stt(data: &Data, model: &str, wav: &[u8], language: &str) -> Result<SttOut, String> {
    if let Some(file) = crate::models::whisper_file(data, model) { return whisper_local(&file, wav, language); }
    let pcm: Vec<f32> = hound::WavReader::new(std::io::Cursor::new(wav)).map_err(|e| e.to_string())?
        .into_samples::<i16>().filter_map(Result::ok).map(|s| s as f32 / 32768.0).collect();
    let text = match crate::sherpa::model_dir(data, model) {
        Some(dir) => crate::sherpa::transcribe(&dir, &pcm)?,
        None => crate::needle::transcribe(data, &pcm, language, "")?,
    };
    Ok(SttOut { text, segments: vec![], via: String::new() })
}

pub async fn transcribe(data: &Data, task: &str, wav: Vec<u8>, language: &str) -> Result<SttOut, String> {
    let (pf, routing) = load(data);
    let route = routing.get(task).ok_or(format!("no route for {task}"))?;
    let mut errs = vec![];
    for target in &route.chain {
        let (pid, model) = split_target(target);
        if pid == "local" {
            if !local_ready(data, model) {
                let why = if crate::models::installed(data, model) { "downloaded, but its runtime isn't linked in this build" } else { "not downloaded" };
                errs.push(format!("{target}: {why}"));
                continue;
            }
            let (d, m, w, lang) = (data.clone(), model.to_string(), wav.clone(), language.to_string());
            match tauri::async_runtime::spawn_blocking(move || local_stt(&d, &m, &w, &lang)).await {
                Ok(Ok(mut out)) => {
                    out.via = target.clone();
                    return Ok(out);
                }
                Ok(Err(e)) => errs.push(format!("{target}: {e}")),
                Err(e) => errs.push(format!("{target}: {e}")),
            }
            continue;
        }
        let p = match find(&pf, pid) { Ok(p) => p, Err(e) => { errs.push(e); continue } };
        if !is_local(p) && (!pf.policies.send_audio_to_cloud || pf.policies.local_only_mode) {
            errs.push(format!("{target}: skipped (\"Send audio to cloud\" is off)"));
            continue;
        }
        if !is_local(p) && get_key(pid).is_none() {
            errs.push(format!("{target}: no API key"));
            continue;
        }
        match tokio::time::timeout(Duration::from_millis(route.timeout_ms), stt_once(p, model, wav.clone(), language)).await {
            Ok(Ok(mut out)) => {
                out.via = target.clone();
                return Ok(out);
            }
            Ok(Err(e)) => errs.push(format!("{target}: {e}")),
            Err(_) => errs.push(format!("{target}: timed out")),
        }
    }
    Err(format!("No speech-to-text model available. {}", errs.join("; ")))
}

/// First entry in the dictation STT chain that can actually run right now.
pub fn stt_ready(data: &Data) -> Option<String> {
    let (pf, routing) = load(data);
    routing.get("stt.dictation")?.chain.iter().find(|t| {
        let (pid, model) = split_target(t);
        if pid == "local" { return local_ready(data, model); }
        match find(&pf, pid) {
            Ok(p) if is_local(p) => true,
            Ok(p) if p.kind != "anthropic" && p.kind != "ollama" =>
                pf.policies.send_audio_to_cloud && !pf.policies.local_only_mode && get_key(pid).is_some(),
            _ => false,
        }
    }).cloned()
}

// ---------- probe ----------

/// Lists the provider's models; doubles as the "test key" button.
pub async fn probe(data: &Data, id: &str) -> Result<Vec<String>, String> {
    let (pf, _) = load(data);
    let p = find(&pf, id)?;
    let base = base_url(p);
    let key = get_key(id);
    let req = match p.kind.as_str() {
        "anthropic" => client().get(format!("{base}/v1/models")).header("x-api-key", key.ok_or("no API key saved")?)
            .header("anthropic-version", "2023-06-01"),
        "ollama" => client().get(format!("{base}/api/tags")),
        _ => {
            let r = client().get(format!("{base}/models"));
            match key { Some(k) => r.bearer_auth(k), None => r }
        }
    };
    let v = check(tokio::time::timeout(Duration::from_secs(10), req.send()).await.map_err(|_| "timed out")?.map_err(|e| e.to_string())?).await?;
    let list = v["data"].as_array().or(v["models"].as_array()).cloned().unwrap_or_default();
    Ok(list.iter().filter_map(|m| m["id"].as_str().or(m["name"].as_str()).map(String::from)).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sse_lines() {
        let v = super::sse_data(r#"data: {"type":"content_block_delta","delta":{"text":"Hi"}}"#).unwrap();
        assert_eq!(v["delta"]["text"], "Hi");
        assert!(super::sse_data("event: ping").is_none());
        assert!(super::sse_data("data: [DONE]").is_none());
    }

    /// REMORA_TEST_WHISPER=<ggml .bin> REMORA_TEST_WAV=<16 kHz mono wav saying "hello world…"> cargo test whisper
    #[test]
    fn whisper_transcribes() {
        let (Ok(model), Ok(wav)) = (std::env::var("REMORA_TEST_WHISPER"), std::env::var("REMORA_TEST_WAV")) else { return };
        let out = super::whisper_local(model.as_ref(), &std::fs::read(wav).unwrap(), "en").unwrap();
        assert!(out.text.to_lowercase().contains("hello"), "{}", out.text);
    }

    fn words(s: &str) -> Vec<String> {
        s.to_uppercase().split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>())
            .filter(|w| !w.is_empty()).collect()
    }

    /// Word error rate: word-level edit distance / reference length.
    fn wer(reference: &str, hyp: &str) -> f64 {
        let (r, h) = (words(reference), words(hyp));
        let mut row: Vec<usize> = (0..=h.len()).collect();
        for i in 1..=r.len() {
            let mut prev = row[0];
            row[0] = i;
            for j in 1..=h.len() {
                let cur = row[j];
                row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + (r[i - 1] != h[j - 1]) as usize);
                prev = cur;
            }
        }
        row[h.len()] as f64 / r.len().max(1) as f64
    }

    #[test]
    fn prices() {
        assert_eq!(super::price_per_mtok("claude:claude-haiku-5-5"), Some((1.0, 5.0)));
        assert_eq!(super::price_per_mtok("ollama-local:llama3.2"), Some((0.0, 0.0)));
        assert_eq!(super::price_per_mtok("groq:mystery-model"), None);
    }

    #[test]
    fn wer_counts_edits() {
        assert_eq!(wer("the cat sat", "the cat sat."), 0.0);
        assert!((wer("the cat sat", "the bat sat") - 1.0 / 3.0).abs() < 1e-9);
        assert!((wer("a b c d", "a c d e") - 0.5).abs() < 1e-9);
    }

    /// REMORA_BENCH_DATA=<data dir> REMORA_BENCH_CLIPS=<folder with *.wav + trans.txt> cargo test --release stt_benchmark -- --ignored --nocapture
    #[test]
    #[ignore]
    fn stt_benchmark() {
        let data = crate::store::Data::at(std::env::var("REMORA_BENCH_DATA").unwrap().into());
        let clips = std::path::PathBuf::from(std::env::var("REMORA_BENCH_CLIPS").unwrap());
        let refs: Vec<(String, String)> = std::fs::read_to_string(clips.join("trans.txt")).unwrap().lines()
            .filter_map(|l| l.split_once(' ').map(|(f, t)| (f.to_string(), t.to_string()))).collect();
        let wavs: Vec<(String, String, Vec<u8>, f64)> = refs.iter().map(|(f, t)| {
            let mut r = hound::WavReader::open(clips.join(f)).unwrap();
            let rate = r.spec().sample_rate;
            let pcm: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
            // ponytail: linear resample, fine for a benchmark of 8/16 kHz clips.
            let step = rate as f64 / crate::audio::RATE as f64;
            let pcm: Vec<f32> = (0..(pcm.len() as f64 / step) as usize).map(|i| {
                let x = i as f64 * step;
                let (a, b) = (pcm[x as usize], *pcm.get(x as usize + 1).unwrap_or(&pcm[x as usize]));
                a + (b - a) * (x.fract() as f32)
            }).collect();
            let secs = pcm.len() as f64 / crate::audio::RATE as f64;
            (f.clone(), t.clone(), crate::audio::wav_bytes(&pcm), secs)
        }).collect();

        let ids: Vec<String> = crate::models::catalog(&data).into_iter().map(|m| m.id)
            .filter(|id| id != "needle3" && super::local_ready(&data, id)).collect();
        println!("\nBENCH models: {ids:?}");
        for id in ids {
            let t = std::time::Instant::now();
            if let Err(e) = super::local_stt(&data, &id, &wavs[0].2, "en") {
                println!("BENCH {id}: failed: {e}");
                continue;
            }
            let load_ms = t.elapsed().as_millis();
            let (mut errs, mut ref_words, mut ms_total, mut audio_total) = (0.0, 0usize, 0.0, 0.0);
            for (f, reference, wav, secs) in &wavs {
                let t = std::time::Instant::now();
                let text = super::local_stt(&data, &id, wav, "en").map(|o| o.text).unwrap_or_else(|e| format!("ERROR {e}"));
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let w = wer(reference, &text);
                let n = words(reference).len();
                errs += w * n as f64;
                ref_words += n;
                ms_total += ms;
                audio_total += secs;
                println!("BENCH {id} | {f} | {secs:.1}s audio | {ms:.0} ms | RTF {:.3} | WER {:.1}% | {text}", ms / 1000.0 / secs, w * 100.0);
            }
            println!("BENCH SUMMARY {id} | first call (load+run) {load_ms} ms | avg {:.0} ms/clip | RTF {:.3} | WER {:.1}%",
                ms_total / wavs.len() as f64, ms_total / 1000.0 / audio_total, errs / ref_words as f64 * 100.0);
        }
    }
}
