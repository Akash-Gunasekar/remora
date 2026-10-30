//! Cactus needle engine (runs Whistle speech-to-text on device). The engine ships as a prebuilt
//! shared library inside Cactus's own package, so it's downloaded next to the model and loaded at runtime.

use crate::store::Data;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const ENGINE_DIR: &str = "models/_needle-engine";
const WHISTLE_LANGS: &[&str] = &["en", "de", "fr", "es", "it", "nl", "pl"];

type Load = unsafe extern "C" fn(*const u8, u64) -> c_int;
type LastError = unsafe extern "C" fn() -> *const c_char;
type Transcribe = unsafe extern "C" fn(*const f32, c_int, *const c_char, *const c_char, c_int, *mut c_char, c_int) -> c_int;

struct Engine {
    load: Load,
    last_error: LastError,
    transcribe: Transcribe,
    speech_model: Option<PathBuf>,
}
// The library is never unloaded, so the function pointers stay valid for the process lifetime.
unsafe impl Send for Engine {}

/// The engine keeps one process-global, non-thread-safe model per kind; this lock serialises every call.
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// Path of the extracted engine library, if present.
pub fn library(data: &Data) -> Option<PathBuf> {
    std::fs::read_dir(data.path(ENGINE_DIR)).ok()?.flatten().map(|e| e.path()).find(|p| {
        let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        n.starts_with("libneedle") && (n.ends_with(".dll") || n.ends_with(".dylib") || n.ends_with(".so"))
    })
}

/// Pulls the shared library out of the downloaded wheel (a zip). `tar` reads zips on Windows 10+, macOS and most Linux.
pub fn extract(data: &Data) -> Result<PathBuf, String> {
    let dir = data.path(ENGINE_DIR);
    if let Some(p) = library(data) { return Ok(p); }
    let wheel = walk_find(&dir, |n| n.ends_with(".whl")).ok_or("engine package missing")?;
    let tmp = dir.join("unpacked");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let st = std::process::Command::new("tar").arg("-xf").arg(&wheel).arg("-C").arg(&tmp).status()
        .map_err(|e| format!("couldn't unpack the engine: {e}"))?;
    if !st.success() { return Err("couldn't unpack the engine package".into()); }
    let lib = walk_find(&tmp, |n| n.starts_with("libneedle") && (n.ends_with(".dll") || n.ends_with(".dylib") || n.ends_with(".so")))
        .ok_or("engine library not found in package")?;
    let dest = dir.join(lib.file_name().unwrap());
    std::fs::rename(&lib, &dest).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(dest)
}

fn walk_find(dir: &Path, f: impl Fn(&str) -> bool + Copy) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(x) = walk_find(&p, f) { return Some(x); }
        } else if f(&p.file_name().unwrap_or_default().to_string_lossy().to_lowercase()) {
            return Some(p);
        }
    }
    None
}

/// Whistle can run right now: model and engine are both on disk.
pub fn ready(data: &Data) -> bool {
    crate::models::installed(data, "whistle") && library(data).is_some()
}

fn open(lib: &Path) -> Result<Engine, String> {
    let h = sys::open(lib)?;
    let sym = |name: &str| sys::sym(h, name).ok_or(format!("engine is missing {name}"));
    unsafe {
        Ok(Engine {
            load: std::mem::transmute::<*mut c_void, Load>(sym("needle_load")?),
            last_error: std::mem::transmute::<*mut c_void, LastError>(sym("needle_last_error")?),
            transcribe: std::mem::transmute::<*mut c_void, Transcribe>(sym("needle_transcribe")?),
            speech_model: None,
        })
    }
}

fn err(e: &Engine) -> String {
    unsafe {
        let p = (e.last_error)();
        if p.is_null() { "engine error".into() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

/// Transcribes 16 kHz mono PCM with Whistle. Blocking; call from a worker thread.
pub fn transcribe(data: &Data, pcm: &[f32], language: &str, keywords: &str) -> Result<String, String> {
    if !crate::models::installed(data, "whistle") { return Err("Whistle isn't downloaded".into()); }
    let lib = library(data).ok_or("engine not downloaded")?;
    let mut guard = ENGINE.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() { *guard = Some(open(&lib)?); }
    let e = guard.as_mut().unwrap();
    let model = data.path("models/whistle/whistle.cact");
    if e.speech_model.as_deref() != Some(model.as_path()) {
        let bytes = std::fs::read(&model).map_err(|x| x.to_string())?;
        if unsafe { (e.load)(bytes.as_ptr(), bytes.len() as u64) } < 0 { return Err(err(e)); }
        e.speech_model = Some(model);
    }
    let lang = WHISTLE_LANGS.contains(&language).then(|| CString::new(language).unwrap());
    let kw = (!keywords.is_empty()).then(|| CString::new(keywords.replace('\0', "")).unwrap());
    let mut out = vec![0u8; 1 << 20];
    let n = unsafe {
        (e.transcribe)(
            pcm.as_ptr(), pcm.len() as c_int,
            lang.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            kw.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            0, out.as_mut_ptr() as *mut c_char, out.len() as c_int,
        )
    };
    if n < 0 { return Err(err(e)); }
    let json = unsafe { CStr::from_ptr(out.as_ptr() as *const c_char) }.to_string_lossy();
    let v: serde_json::Value = serde_json::from_str(&json).map_err(|x| format!("engine output: {x}"))?;
    Ok(v["text"].as_str().unwrap_or("").trim().to_string())
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
    }
    pub fn open(p: &std::path::Path) -> Result<*mut c_void, String> {
        let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
        let h = unsafe { LoadLibraryW(w.as_ptr()) };
        if h.is_null() { Err(format!("couldn't load {} ({})", p.display(), std::io::Error::last_os_error())) } else { Ok(h) }
    }
    pub fn sym(h: *mut c_void, name: &str) -> Option<*mut c_void> {
        let n = format!("{name}\0");
        let f = unsafe { GetProcAddress(h, n.as_ptr()) };
        (!f.is_null()).then_some(f)
    }
}

#[cfg(unix)]
mod sys {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    extern "C" {
        fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(h: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }
    pub fn open(p: &std::path::Path) -> Result<*mut c_void, String> {
        let c = CString::new(p.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let h = unsafe { dlopen(c.as_ptr(), 2) }; // RTLD_NOW
        if h.is_null() {
            let e = unsafe { dlerror() };
            Err(if e.is_null() { "dlopen failed".into() } else { unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned() })
        } else { Ok(h) }
    }
    pub fn sym(h: *mut c_void, name: &str) -> Option<*mut c_void> {
        let n = CString::new(name).ok()?;
        let f = unsafe { dlsym(h, n.as_ptr()) };
        (!f.is_null()).then_some(f)
    }
}
