//! Remora Data folder: plain files, atomic writes, append-only JSONL, trash, retention (plan §9).

use chrono::{DateTime, Duration, Local, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Data {
    pub root: PathBuf,
}

impl Data {
    pub fn open() -> Data {
        Data::at(locate())
    }

    pub fn at(root: PathBuf) -> Data {
        for sub in [
            "config/app-profiles", "models", "meetings", "dictations", "people", "voice-profiles",
            "chats-global", "index", "trash", "usage", "logs", "library",
        ] {
            let _ = fs::create_dir_all(root.join(sub));
        }
        let manifest = root.join("manifest.json");
        if !manifest.exists() {
            let _ = write_json(&manifest, &serde_json::json!({ "format_version": 1, "created_at": now() }));
        }
        Data { root }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn settings(&self) -> Settings {
        read_json(&self.path("config/settings.json"))
    }
}

fn locate() -> PathBuf {
    if let Ok(p) = std::env::var("REMORA_DATA") {
        return p.into();
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(dir) = &exe_dir {
        if dir.join("portable").exists() {
            return dir.join("Remora Data");
        }
    }
    platform_dir(exe_dir)
}

#[cfg(windows)]
fn platform_dir(exe_dir: Option<PathBuf>) -> PathBuf {
    // Per-user installs keep data beside the app; Program Files isn't writable without elevation.
    match exe_dir {
        Some(d) if !d.to_string_lossy().to_lowercase().contains("program files") => d.join("data"),
        _ => dirs::data_local_dir().unwrap_or_default().join("Remora").join("data"),
    }
}

#[cfg(not(windows))]
fn platform_dir(_: Option<PathBuf>) -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("Remora")
}

pub fn now() -> String {
    Local::now().to_rfc3339()
}

// ---------- file primitives ----------

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

pub fn write_json<T: Serialize + ?Sized>(path: &Path, v: &T) -> std::io::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(v)?)
}

pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn append_jsonl<T: Serialize>(path: &Path, v: &T) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    f.write_all(&line)?;
    f.sync_data()
}

/// Malformed lines are skipped so one bad write never hides a whole file.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn write_jsonl<T: Serialize>(path: &Path, items: &[T]) -> std::io::Result<()> {
    if items.is_empty() {
        return match fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let mut out = Vec::new();
    for it in items {
        out.extend(serde_json::to_vec(it)?);
        out.push(b'\n');
    }
    write_atomic(path, &out)
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, ext, out);
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

// ---------- trash ----------

impl Data {
    pub fn trash(&self, path: &Path) -> std::io::Result<()> {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        fs::rename(path, self.path("trash").join(format!("{}_{}", Utc::now().timestamp(), name)))
    }

    pub fn purge_trash(&self, days: u32) {
        let cutoff = Utc::now().timestamp() - days as i64 * 86_400;
        for e in fs::read_dir(self.path("trash")).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let ts: i64 = name.split('_').next().and_then(|s| s.parse().ok()).unwrap_or(0);
            if ts <= cutoff {
                let p = e.path();
                let _ = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
            }
        }
    }
}

// ---------- settings ----------

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub hotkey: String,
    /// "hold" = push-to-talk, "toggle" = press to start / press to stop, "auto" = hold, or tap to lock.
    pub hotkey_mode: String,
    pub meeting_hotkey: String,
    pub history_enabled: bool,
    pub history_keep_last: Option<usize>,
    pub history_keep_days: Option<u32>,
    pub output_language: String,
    pub per_app_language: HashMap<String, String>,
    pub tone: String,
    pub mode: String,
    pub auto_signoff: bool,
    pub signoff: String,
    pub meeting_audio_keep_days: Option<u32>,
    pub trash_days: u32,
    pub disabled_apps: Vec<String>,
    pub sounds: bool,
    /// Start/stop sound id; the pill synthesizes it (see src/sounds.ts).
    pub sound_style: String,
    /// Waves rising out of the pill's bars while listening: "on", "subtle" or "off".
    pub sonar_ripples: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey: if cfg!(target_os = "macos") { "Alt+Space" } else { "Control+Alt+Space" }.into(),
            hotkey_mode: "auto".into(),
            meeting_hotkey: "CommandOrControl+Shift+M".into(),
            history_enabled: true,
            history_keep_last: Some(500),
            history_keep_days: Some(30),
            output_language: "en".into(),
            per_app_language: HashMap::new(),
            tone: "friendly-professional".into(),
            mode: "enhance".into(),
            auto_signoff: false,
            signoff: "Best regards,".into(),
            meeting_audio_keep_days: Some(7),
            trash_days: 7,
            disabled_apps: vec![],
            sounds: true,
            sound_style: "heartbeat".into(),
            sonar_ripples: "on".into(),
        }
    }
}

// ---------- dictation history (plan §9.4) ----------

#[derive(Serialize, Deserialize, Clone)]
pub struct Dictation {
    pub id: String,
    pub time: String,
    pub app: String,
    pub category: String,
    pub language: String,
    pub raw: String,
    #[serde(rename = "final")]
    pub final_text: String,
    pub via: String,
    /// Speech-to-text model that produced `raw`; empty in entries from older builds.
    #[serde(default)]
    pub stt: String,
    pub latency_ms: u64,
}

impl Data {
    pub fn add_dictation(&self, d: &Dictation) -> std::io::Result<()> {
        let day = Local::now();
        let path = self.path(&format!(
            "dictations/{}/{}.jsonl",
            day.format("%Y/%m"),
            day.format("%Y-%m-%d")
        ));
        append_jsonl(&path, d)
    }

    fn dictation_files(&self) -> Vec<PathBuf> {
        let mut v = vec![];
        walk(&self.path("dictations"), "jsonl", &mut v);
        v
    }

    /// Newest first.
    pub fn list_dictations(&self) -> Vec<Dictation> {
        let mut all: Vec<Dictation> =
            self.dictation_files().iter().flat_map(|p| read_jsonl::<Dictation>(p)).collect();
        all.sort_by(|a, b| b.time.cmp(&a.time));
        all
    }

    /// Rewrites only the day files that contain a removed entry.
    pub fn delete_dictations(&self, remove: impl Fn(&Dictation) -> bool) -> usize {
        let mut n = 0;
        for p in self.dictation_files() {
            let items: Vec<Dictation> = read_jsonl(&p);
            let keep: Vec<Dictation> = items.iter().filter(|d| !remove(d)).cloned().collect();
            if keep.len() != items.len() {
                n += items.len() - keep.len();
                let _ = write_jsonl(&p, &keep);
            }
        }
        n
    }

    /// Count and age limits both apply, so whichever removes more wins.
    pub fn prune_dictations(&self, keep_last: Option<usize>, keep_days: Option<u32>) -> usize {
        let all = self.list_dictations();
        let cutoff = keep_days.map(|d| Local::now() - Duration::days(d as i64));
        let doomed: std::collections::HashSet<String> = all
            .iter()
            .enumerate()
            .filter(|(i, d)| {
                keep_last.is_some_and(|n| *i >= n)
                    || cutoff.is_some_and(|c| {
                        DateTime::parse_from_rfc3339(&d.time).map(|t| t < c).unwrap_or(false)
                    })
            })
            .map(|(_, d)| d.id.clone())
            .collect();
        if doomed.is_empty() {
            return 0;
        }
        self.delete_dictations(|d| doomed.contains(&d.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> Data {
        let p = std::env::temp_dir().join(format!("remora-test-{}", uuid::Uuid::new_v4()));
        Data::at(p)
    }

    fn dict(id: &str, time: String) -> Dictation {
        Dictation {
            id: id.into(), time, app: "x".into(), category: "general".into(), language: "en".into(),
            raw: "r".into(), final_text: "f".into(), via: "rules".into(), stt: String::new(), latency_ms: 1,
        }
    }

    #[test]
    fn atomic_write_and_jsonl_roundtrip() {
        let d = tmp();
        let p = d.path("a/b.json");
        write_json(&p, &serde_json::json!({"k": 1})).unwrap();
        let v: serde_json::Value = read_json(&p);
        assert_eq!(v["k"], 1);
        let j = d.path("x.jsonl");
        append_jsonl(&j, &1).unwrap();
        append_jsonl(&j, &2).unwrap();
        fs::OpenOptions::new().append(true).open(&j).unwrap().write_all(b"garbage\n").unwrap();
        assert_eq!(read_jsonl::<i32>(&j), vec![1, 2]);
    }

    #[test]
    fn retention_count_and_age() {
        let d = tmp();
        for i in 0..5 {
            d.add_dictation(&dict(&format!("n{i}"), (Local::now() - Duration::minutes(i)).to_rfc3339())).unwrap();
        }
        d.add_dictation(&dict("old", (Local::now() - Duration::days(40)).to_rfc3339())).unwrap();
        assert_eq!(d.prune_dictations(None, Some(30)), 1);
        assert_eq!(d.prune_dictations(Some(3), Some(30)), 2);
        let ids: Vec<String> = d.list_dictations().into_iter().map(|x| x.id).collect();
        assert_eq!(ids, vec!["n0", "n1", "n2"]);
    }

    #[test]
    fn trash_and_purge() {
        let d = tmp();
        let f = d.path("meetings/m1");
        fs::create_dir_all(&f).unwrap();
        d.trash(&f).unwrap();
        assert!(!f.exists());
        d.purge_trash(0);
        assert_eq!(fs::read_dir(d.path("trash")).unwrap().count(), 0);
    }
}
