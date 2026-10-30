//! sherpa-onnx runtime for the Parakeet (NeMo transducer) and Moonshine downloads.

use crate::store::Data;
use sherpa_rs::moonshine::{MoonshineConfig, MoonshineRecognizer};
use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

enum Recognizer {
    Transducer(TransducerRecognizer),
    Moonshine(MoonshineRecognizer),
}

// ponytail: one cached recognizer; switching models reloads it (a few seconds for Parakeet).
static CACHE: Mutex<Option<(PathBuf, Recognizer)>> = Mutex::new(None);

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { files(&p, out) } else { out.push(p) }
    }
}

/// First `.onnx` whose name starts with `prefix`, preferring the int8 build.
fn pick(all: &[PathBuf], prefix: &str) -> Option<String> {
    let mut m: Vec<&PathBuf> = all.iter().filter(|p| {
        let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        n.starts_with(prefix) && n.ends_with(".onnx")
    }).collect();
    m.sort_by_key(|p| !p.to_string_lossy().contains("int8"));
    m.first().map(|p| p.to_string_lossy().into_owned())
}

fn tokens(all: &[PathBuf]) -> Option<String> {
    all.iter().find(|p| p.file_name().is_some_and(|n| n == "tokens.txt")).map(|p| p.to_string_lossy().into_owned())
}

/// Model folder of a downloaded sherpa-onnx entry.
pub fn model_dir(data: &Data, id: &str) -> Option<PathBuf> {
    let ok = crate::models::installed(data, id)
        && crate::models::catalog(data).iter().any(|m| m.id == id && m.runtime == "sherpa-onnx");
    ok.then(|| data.path(&format!("models/{id}")))
}

fn build(dir: &Path) -> Result<Recognizer, String> {
    let mut all = vec![];
    files(dir, &mut all);
    let tokens = tokens(&all).ok_or("tokens.txt missing from the model folder")?;
    let threads = std::thread::available_parallelism().map(|n| n.get().min(4) as i32).unwrap_or(2);
    if let Some(preprocessor) = pick(&all, "preprocess") {
        let cfg = MoonshineConfig {
            preprocessor,
            encoder: pick(&all, "encode").ok_or("encoder missing")?,
            uncached_decoder: pick(&all, "uncached_decode").ok_or("uncached decoder missing")?,
            cached_decoder: pick(&all, "cached_decode").ok_or("cached decoder missing")?,
            tokens,
            num_threads: Some(threads),
            ..Default::default()
        };
        return MoonshineRecognizer::new(cfg).map(Recognizer::Moonshine).map_err(|e| e.to_string());
    }
    let cfg = TransducerConfig {
        encoder: pick(&all, "encoder").ok_or("encoder missing")?,
        decoder: pick(&all, "decoder").ok_or("decoder missing")?,
        joiner: pick(&all, "joiner").ok_or("joiner missing")?,
        tokens,
        model_type: "nemo_transducer".into(),
        decoding_method: "greedy_search".into(),
        sample_rate: crate::audio::RATE as i32,
        feature_dim: 80,
        num_threads: threads,
        ..Default::default()
    };
    TransducerRecognizer::new(cfg).map(Recognizer::Transducer).map_err(|e| e.to_string())
}

/// Transcribes 16 kHz mono PCM. Blocking; call from a worker thread.
pub fn transcribe(dir: &Path, pcm: &[f32]) -> Result<String, String> {
    let mut slot = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_ref().map(|(p, _)| p.as_path()) != Some(dir) {
        *slot = None;
        *slot = Some((dir.to_path_buf(), build(dir)?));
    }
    let rate = crate::audio::RATE;
    let text = match &mut slot.as_mut().unwrap().1 {
        Recognizer::Transducer(r) => r.transcribe(rate, pcm),
        Recognizer::Moonshine(r) => r.transcribe(rate, pcm).text,
    };
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    /// REMORA_TEST_SHERPA=<model folder> REMORA_TEST_WAV=<16 kHz mono wav saying "hello world…"> cargo test sherpa
    #[test]
    fn sherpa_transcribes() {
        let (Ok(dir), Ok(wav)) = (std::env::var("REMORA_TEST_SHERPA"), std::env::var("REMORA_TEST_WAV")) else { return };
        let pcm: Vec<f32> = hound::WavReader::open(wav).unwrap().into_samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        let text = super::transcribe(dir.as_ref(), &pcm).unwrap();
        assert!(text.to_lowercase().contains("hello"), "{text}");
    }
}
