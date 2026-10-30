//! Local model catalog (plan §4.2, §7.5.1): Hugging Face downloads with resume + SHA-256 checks.

use crate::store::{now, read_json, write_json, Data};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::time::Instant;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub family: String,
    pub repo: String,
    /// Exact file names or suffixes to fetch; empty = every file except docs.
    pub include: Vec<String>,
    pub size: String,
    pub languages: String,
    pub runtime: String,
    pub best_for: String,
    pub license: String,
    pub default: bool,
}

fn e(id: &str, name: &str, family: &str, repo: &str, include: &[&str], size: &str, languages: &str, runtime: &str, best_for: &str, license: &str) -> Entry {
    Entry {
        id: id.into(), name: name.into(), family: family.into(), repo: repo.into(),
        include: include.iter().map(|s| s.to_string()).collect(), size: size.into(), languages: languages.into(),
        runtime: runtime.into(), best_for: best_for.into(), license: license.into(), default: family == "Cactus",
    }
}

fn default_catalog() -> Vec<Entry> {
    vec![
        e("whistle", "Cactus Whistle", "Cactus", "Cactus-Compute/whistle", &["whistle.cact", "config.json"], "≈17 MB", "EN, DE, FR, ES, IT, NL, PL", "needle engine",
          "Default speech-to-text: streaming, word timestamps, keyword biasing", "See model card"),
        e("needle3", "Cactus Needle3", "Cactus", "Cactus-Compute/needle3", &["needle3.cact", "config.json", "tokenizer.model"], "≈36 MB", "English", "needle engine",
          "Voice commands, action-item extraction, local search embeddings", "See model card"),
        e("parakeet-tdt-0.6b-v2", "NVIDIA Parakeet TDT 0.6B v2", "Other", "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8", &[], "≈640 MB", "English",
          "sherpa-onnx", "Highest-accuracy offline English", "CC-BY-4.0"),
        e("parakeet-tdt-0.6b-v3", "NVIDIA Parakeet TDT 0.6B v3", "Other", "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8", &[], "≈640 MB", "25 European languages",
          "sherpa-onnx", "Accurate offline European multilingual; meeting final pass", "CC-BY-4.0"),
        e("whisper-large-v3-turbo", "OpenAI Whisper large-v3-turbo", "Other", "ggerganov/whisper.cpp", &["ggml-large-v3-turbo-q5_0.bin"], "≈550 MB", "~99 incl. Hindi, Tamil",
          "whisper.cpp", "Broadest offline language coverage", "MIT"),
        e("whisper-small", "OpenAI Whisper small", "Other", "ggerganov/whisper.cpp", &["ggml-small.bin"], "≈470 MB", "~99",
          "whisper.cpp", "Multilingual on modest hardware", "MIT"),
        e("whisper-base", "OpenAI Whisper base", "Other", "ggerganov/whisper.cpp", &["ggml-base.bin"], "≈145 MB", "~99",
          "whisper.cpp", "Fast multilingual, lower accuracy", "MIT"),
        e("moonshine-tiny", "Moonshine tiny", "Other", "csukuangfj/sherpa-onnx-moonshine-tiny-en-int8", &[], "≈100 MB", "English",
          "sherpa-onnx", "Very fast English on weak CPUs", "MIT"),
        e("indicconformer", "AI4Bharat IndicConformer (multilingual)", "Other", "ai4bharat/indic-conformer-600m-multilingual", &[], "≈2.4 GB", "22 Indian languages",
          "ONNX Runtime", "Offline Tamil, Hindi, Kannada… (may need Hugging Face access approval)", "MIT"),
    ]
}

/// Stands for this platform's engine wheel (the one package that carries libneedle3) in an include list.
const ENGINE: &str = "@engine";

fn engine_wheel_suffix() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "win_arm64.whl",
        ("windows", _) => "win_amd64.whl",
        ("macos", "aarch64") => "macosx_11_0_arm64.whl",
        ("macos", _) => "macosx_11_0_x86_64.whl",
        (_, "aarch64") => "manylinux2014_aarch64.whl",
        _ => "manylinux2014_x86_64.whl",
    }
}

pub fn catalog(data: &Data) -> Vec<Entry> {
    let p = data.path("config/models-catalog.json");
    if !p.exists() {
        let _ = write_json(&p, &default_catalog());
    }
    let mut c: Vec<Entry> = read_json(&p);
    if c.is_empty() { return default_catalog(); }
    // Catalogs saved by older builds fetched whole repos for built-in models; adopt the tighter file lists.
    let defaults = default_catalog();
    let mut changed = false;
    for m in c.iter_mut().filter(|m| m.include.is_empty()) {
        if let Some(d) = defaults.iter().find(|d| d.id == m.id && d.repo == m.repo && !d.include.is_empty()) {
            m.include = d.include.clone();
            m.size = d.size.clone();
            changed = true;
        }
    }
    if changed { let _ = write_json(&p, &c); }
    c
}

/// Files of a repo listing worth keeping: matches the include list, and only the newest engine wheel.
fn select(paths: Vec<String>, include: &[String]) -> Vec<String> {
    let mut keep: Vec<String> = paths.into_iter().filter(|p| wanted(p, include)).collect();
    // ponytail: lexicographic version order is right while versions stay single-digit (3.3.3); parse them if 3.10 ships.
    if let Some(newest) = keep.iter().filter(|p| p.ends_with(".whl")).max().cloned() {
        keep.retain(|p| !p.ends_with(".whl") || *p == newest);
    }
    keep
}

/// Deletes files in an installed model folder that the current include list no longer wants.
pub fn prune(data: &Data, m: &Entry) {
    let dir = data.path(&format!("models/{}", m.id));
    if !dir.join("complete.json").exists() { return; }
    let mut files = vec![];
    walk(&dir, &dir, &mut files);
    let keep = select(files.clone(), &m.include);
    for f in files.iter().filter(|f| !keep.contains(f) && f.as_str() != "complete.json") {
        let _ = fs::remove_file(dir.join(f));
    }
    remove_empty_dirs(&dir);
}

fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { walk(root, &p, out) } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn remove_empty_dirs(dir: &std::path::Path) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_empty_dirs(&p);
            let _ = fs::remove_dir(&p);
        }
    }
}

fn dir_size(p: &std::path::Path) -> u64 {
    fs::read_dir(p).into_iter().flatten().flatten().map(|e| {
        let p = e.path();
        if p.is_dir() { dir_size(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }
    }).sum()
}

pub fn list(data: &Data) -> Vec<Value> {
    catalog(data).into_iter().map(|m| {
        let dir = data.path(&format!("models/{}", m.id));
        let status = if dir.join("complete.json").exists() { "installed" } else if dir.exists() { "partial" } else { "available" };
        let runnable = crate::providers::local_ready(data, &m.id);
        json!({ "entry": m, "status": status, "bytes": if dir.exists() { dir_size(&dir) } else { 0 }, "runnable": runnable })
    }).collect()
}

/// Cactus's engine package; fetched alongside any Cactus model, never listed on its own.
fn needle_engine() -> Entry {
    Entry {
        id: "_needle-engine".into(), name: "Cactus engine".into(), family: "Cactus".into(), repo: "Cactus-Compute/needle3".into(),
        include: vec![ENGINE.into()], ..Default::default()
    }
}

pub fn installed(data: &Data, id: &str) -> bool {
    data.path(&format!("models/{id}/complete.json")).exists()
}

/// The ggml file of a downloaded whisper.cpp model — the only local runtime linked so far.
pub fn whisper_file(data: &Data, id: &str) -> Option<std::path::PathBuf> {
    if !installed(data, id) || !catalog(data).iter().any(|m| m.id == id && m.runtime == "whisper.cpp") {
        return None;
    }
    fs::read_dir(data.path(&format!("models/{id}"))).ok()?.flatten().map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "bin"))
}

pub fn delete(data: &Data, id: &str) -> Result<(), String> {
    if id.contains(['/', '\\', '.']) { return Err("bad id".into()); }
    fs::remove_dir_all(data.path(&format!("models/{id}"))).map_err(|e| e.to_string())
}

fn wanted(path: &str, include: &[String]) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if include.is_empty() {
        return !(name.ends_with(".md") || name == ".gitattributes" || name.ends_with(".png") || name.ends_with(".jpg") || name.ends_with(".gif"));
    }
    include.iter().any(|i| {
        let i = if i == ENGINE { engine_wheel_suffix() } else { i.as_str() };
        path == i || name.ends_with(&i.to_lowercase())
    })
}

fn progress(app: &AppHandle, id: &str, done: u64, total: u64, file: &str) {
    let _ = app.emit("model-progress", json!({ "id": id, "done": done, "total": total, "file": file }));
}

pub async fn download(app: &AppHandle, data: &Data, id: &str) -> Result<(), String> {
    let m = catalog(data).into_iter().find(|m| m.id == id).ok_or("unknown model")?;
    fetch(app, data, &m, id).await?;
    if m.runtime == "needle engine" { ensure_engine(app, data, id).await?; }
    Ok(())
}

/// Makes sure Cactus's engine library is on disk; a no-op once it is.
pub async fn ensure_engine(app: &AppHandle, data: &Data, progress_id: &str) -> Result<(), String> {
    if crate::needle::library(data).is_some() { return Ok(()); }
    let engine = needle_engine();
    fetch(app, data, &engine, progress_id).await?;
    crate::needle::extract(data)?;
    let _ = app.emit("models-changed", ());
    Ok(())
}

/// Speaker separation models (pyannote segmentation + CAM++ English voice fingerprints), ≈31 MB total.
/// Not listed in the catalog: they aren't speech-to-text choices, meetings fetch them on first use.
pub const SPEAKER_MODELS: [(&str, &str, &str); 2] = [
    ("speaker-segmentation", "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0", "model.int8.onnx"),
    ("speaker-embedding", "csukuangfj/speaker-embedding-models", "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"),
];

pub async fn ensure_speaker_models(app: &AppHandle, data: &Data) -> Result<(), String> {
    for (id, repo, file) in SPEAKER_MODELS {
        if installed(data, id) { continue; }
        let m = Entry { id: id.into(), name: "Speaker recognition".into(), repo: repo.into(), include: vec![file.into()], ..Default::default() };
        fetch(app, data, &m, "speakers").await?;
    }
    Ok(())
}

/// Cactus models installed by a build that didn't fetch the engine yet.
pub fn needs_engine(data: &Data) -> bool {
    crate::needle::library(data).is_none()
        && catalog(data).iter().any(|m| m.runtime == "needle engine" && installed(data, &m.id))
}

/// Downloads one entry's files; progress events are reported under `progress_id`.
async fn fetch(app: &AppHandle, data: &Data, m: &Entry, progress_id: &str) -> Result<(), String> {
    let id = progress_id;
    let client = crate::providers::client();
    let tree: Vec<Value> = client
        .get(format!("https://huggingface.co/api/models/{}/tree/main?recursive=true", m.repo))
        .send().await.map_err(|e| e.to_string())?
        .error_for_status().map_err(|e| format!("{} isn't available on Hugging Face ({e}). Fix its repo in models-catalog.json.", m.repo))?
        .json().await.map_err(|e| e.to_string())?;
    let all: Vec<(String, u64, Option<String>)> = tree.iter()
        .filter(|f| f["type"] == "file")
        .filter_map(|f| Some((f["path"].as_str()?.to_string(), f["size"].as_u64().unwrap_or(0), f["lfs"]["oid"].as_str().map(String::from))))
        .collect();
    let keep = select(all.iter().map(|f| f.0.clone()).collect(), &m.include);
    let files: Vec<_> = all.into_iter().filter(|f| keep.contains(&f.0)).collect();
    if files.is_empty() {
        return Err(format!("No matching files in {}", m.repo));
    }
    let total: u64 = files.iter().map(|f| f.1).sum();
    let dir = data.path(&format!("models/{}", m.id));
    let mut done = 0u64;
    for (path, size, sha) in &files {
        let dest = dir.join(path);
        if let Some(p) = dest.parent() { fs::create_dir_all(p).map_err(|e| e.to_string())?; }
        // Files already present with the right size (e.g. copied in for offline import) are kept.
        if dest.metadata().map(|md| md.len() == *size).unwrap_or(false) {
            done += size;
            progress(app, id, done, total, path);
            continue;
        }
        let part = dest.with_file_name(format!("{}.part", dest.file_name().unwrap_or_default().to_string_lossy()));
        let mut have = part.metadata().map(|md| md.len()).unwrap_or(0);
        let mut hasher = Sha256::new();
        if have > 0 {
            let mut f = fs::File::open(&part).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 { break; }
                hasher.update(&buf[..n]);
            }
        }
        let url = format!("https://huggingface.co/{}/resolve/main/{}", m.repo, path);
        let mut req = client.get(&url);
        if have > 0 { req = req.header(reqwest::header::RANGE, format!("bytes={have}-")); }
        let mut resp = req.send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?;
        if have > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            have = 0;
            hasher = Sha256::new();
        }
        let mut f = fs::OpenOptions::new().create(true).write(true).append(have > 0).truncate(have == 0).open(&part).map_err(|e| e.to_string())?;
        let mut tick = Instant::now();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            f.write_all(&chunk).map_err(|e| e.to_string())?;
            hasher.update(&chunk);
            have += chunk.len() as u64;
            if tick.elapsed().as_millis() > 150 {
                progress(app, id, done + have, total, path);
                tick = Instant::now();
            }
        }
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        if let Some(oid) = sha {
            if format!("{:x}", hasher.finalize()) != *oid {
                let _ = fs::remove_file(&part);
                return Err(format!("Checksum mismatch for {path}; the partial file was removed, try again."));
            }
        }
        fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        done += size;
        progress(app, id, done, total, path);
    }
    write_json(&dir.join("complete.json"), &json!({ "repo": m.repo, "files": files.iter().map(|f| &f.0).collect::<Vec<_>>(), "downloaded_at": now() }))
        .map_err(|e| e.to_string())?;
    prune(data, m);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_filter() {
        assert!(wanted("ggml-small.bin", &["ggml-small.bin".into()]));
        assert!(!wanted("ggml-small.en.bin", &["ggml-small.bin".into()]));
        assert!(wanted("model.cact", &[]));
        assert!(!wanted("README.md", &[]));
        assert!(!wanted(".gitattributes", &[]));
        let inc: Vec<String> = ["needle3.cact", "tokenizer.model", ENGINE].map(String::from).to_vec();
        let wheel = |v: &str| format!("python/cactus_needle-{v}-py3-none-{}", engine_wheel_suffix());
        let got = select(
            ["needle3.cact", "checkpoints/needle3.safetensors", "tokenizer/tokenizer.model", "windows-x86_64/needle.exe"]
                .map(String::from).into_iter().chain([wheel("3.0.0"), wheel("3.3.3"), wheel("3.1.0")]).collect(),
            &inc,
        );
        assert_eq!(got, vec!["needle3.cact".to_string(), "tokenizer/tokenizer.model".into(), wheel("3.3.3")]);
    }
}
