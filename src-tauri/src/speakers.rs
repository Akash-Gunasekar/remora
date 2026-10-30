//! Who said what on a shared audio track (plan §5.2). Each chunk is split into speaker turns by
//! sherpa-onnx diarization; a voice fingerprint per turn group ties chunks together into
//! "Speaker 1, 2, …"; names come from the meeting app's active-speaker timeline when there is one.

use crate::models::{installed, SPEAKER_MODELS};
use crate::store::Data;
use serde::Serialize;
use sherpa_rs::diarize::{Diarize, DiarizeConfig};
use sherpa_rs::speaker_id::{EmbeddingExtractor, ExtractorConfig};
use std::collections::HashMap;

const RATE: usize = 16_000;
/// Cosine similarity at which a chunk's voice joins an existing speaker. Tuned for CAM++ (same-voice pairs score well above).
const SAME_VOICE: f32 = 0.5;
/// End-of-meeting merge: speakers this similar were one person split early on, when fingerprints were short.
const MERGE_VOICE: f32 = 0.5;
/// Turns shorter than this are folded into the previous one: too short to transcribe or fingerprint well.
const MIN_TURN: usize = RATE * 6 / 10;
/// Less speech than this gives a fingerprint too noisy to start a new speaker on; it joins the closest voice.
const MIN_PRINT: usize = RATE * 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub start: usize,
    pub end: usize,
    pub speaker: usize,
}

pub fn label(speaker: usize) -> String {
    format!("Speaker {}", speaker + 1)
}

pub struct Speakers {
    diar: Diarize,
    emb: EmbeddingExtractor,
    /// Running sum of normalised fingerprints per speaker; its direction is the speaker's voice.
    voices: Vec<Vec<f32>>,
}

impl Speakers {
    /// None until both models are downloaded.
    pub fn load(data: &Data) -> Option<Self> {
        if !SPEAKER_MODELS.iter().all(|m| installed(data, m.0)) { return None; }
        let path = |i: usize| data.path(&format!("models/{}/{}", SPEAKER_MODELS[i].0, SPEAKER_MODELS[i].2));
        let config = DiarizeConfig {
            num_clusters: Some(-1), // unknown speaker count: cluster by threshold
            // Higher = fewer clusters; 0.5 split one compressed call voice into several.
            threshold: Some(0.7),
            min_duration_on: Some(0.3),
            min_duration_off: Some(0.5),
            ..Default::default()
        };
        let diar = Diarize::new(path(0), path(1), config).ok()?;
        let emb = EmbeddingExtractor::new(ExtractorConfig { model: path(1).to_string_lossy().into(), ..Default::default() }).ok()?;
        Some(Self { diar, emb, voices: vec![] })
    }

    /// Speaker turns in one chunk of 16 kHz audio, labelled with meeting-wide speaker numbers.
    pub fn turns(&mut self, x: &[f32]) -> Vec<Turn> {
        let Ok(segs) = self.diar.compute(x.to_vec(), None) else { return vec![] };
        let bounds = |s: &sherpa_rs::diarize::Segment| {
            let a = ((s.start.max(0.0) * RATE as f32) as usize).min(x.len());
            (a, ((s.end.max(0.0) * RATE as f32) as usize).clamp(a, x.len()))
        };
        let mut locals: Vec<i32> = segs.iter().map(|s| s.speaker).collect();
        locals.sort();
        locals.dedup();
        let mut global = HashMap::new();
        for l in locals {
            let mut audio = vec![];
            for s in segs.iter().filter(|s| s.speaker == l) {
                let (a, b) = bounds(s);
                audio.extend_from_slice(&x[a..b]);
                if audio.len() > 30 * RATE { break; }
            }
            let solid = audio.len() >= MIN_PRINT;
            let id = match self.emb.compute_speaker_embedding(audio, RATE as u32) {
                Ok(e) => assign(&mut self.voices, normalized(e), solid),
                // Too short to fingerprint: the most recent speaker is the best guess.
                // (an empty placeholder voice takes the first real fingerprint).
                Err(_) => {
                    if self.voices.is_empty() { self.voices.push(vec![]); }
                    self.voices.len() - 1
                }
            };
            global.insert(l, id);
        }
        let raw: Vec<Turn> = segs.iter().map(|s| { let (a, b) = bounds(s); Turn { start: a, end: b, speaker: global[&s.speaker] } }).collect();
        merge_turns(raw)
    }

    /// Pairs (from, into) of speakers whose voices turned out to be the same person.
    pub fn merges(&self) -> Vec<(usize, usize)> {
        let c: Vec<Vec<f32>> = self.voices.iter().map(|v| normalized(v.clone())).collect();
        let mut into: Vec<usize> = (0..c.len()).collect();
        for j in 1..c.len() {
            if let Some(i) = (0..j).filter(|&i| into[i] == i).max_by(|&a, &b| cos(&c[a], &c[j]).total_cmp(&cos(&c[b], &c[j]))) {
                if cos(&c[i], &c[j]) >= MERGE_VOICE { into[j] = i; }
            }
        }
        into.iter().enumerate().filter(|(j, i)| j != *i).map(|(j, &i)| (j, i)).collect()
    }
}

fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 { v.iter_mut().for_each(|x| *x /= n); }
    v
}

fn cos(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return -1.0; }
    let (ab, aa, bb) = a.iter().zip(b).fold((0.0, 0.0, 0.0), |(ab, aa, bb), (x, y)| (ab + x * y, aa + x * x, bb + y * y));
    if aa == 0.0 || bb == 0.0 { -1.0 } else { ab / (aa.sqrt() * bb.sqrt()) }
}

/// Closest known voice if similar enough (and folds the fingerprint into it), otherwise a new speaker.
/// A short (`solid` = false) fingerprint only ever joins the closest voice, and doesn't reshape it.
fn assign(voices: &mut Vec<Vec<f32>>, e: Vec<f32>, solid: bool) -> usize {
    let best = (0..voices.len()).max_by(|&a, &b| cos(&voices[a], &e).total_cmp(&cos(&voices[b], &e)));
    match best {
        Some(i) if voices[i].is_empty() => { voices[i] = e; i }
        Some(i) if !solid => i,
        Some(i) if cos(&voices[i], &e) >= SAME_VOICE => {
            voices[i].iter_mut().zip(&e).for_each(|(s, x)| *s += x);
            i
        }
        _ => { voices.push(e); voices.len() - 1 }
    }
}

/// Joins same-speaker turns separated by short pauses and folds too-short turns into the previous one.
fn merge_turns(raw: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = vec![];
    for t in raw {
        match out.last_mut() {
            Some(p) if p.speaker == t.speaker && t.start.saturating_sub(p.end) < RATE / 2 => p.end = t.end,
            Some(p) if t.end - t.start < MIN_TURN => p.end = p.end.max(t.end),
            _ => out.push(t),
        }
    }
    out
}

// ---------- names ----------

/// A stretch of meeting time (ms since start) where the meeting app showed `name` as speaking.
#[derive(Debug, Clone, Serialize)]
pub struct Span {
    pub start_ms: u64,
    pub end_ms: u64,
    pub name: String,
}

/// Milliseconds each name was shown as speaking during [start, end).
fn overlap(timeline: &[Span], start: u64, end: u64) -> HashMap<String, u64> {
    let mut m = HashMap::new();
    for s in timeline {
        let o = s.end_ms.min(end).saturating_sub(s.start_ms.max(start));
        if o > 0 { *m.entry(s.name.clone()).or_insert(0) += o; }
    }
    m
}

/// The name that covers at least 60% of the named time, once at least `min_ms` is named.
fn winner(votes: &HashMap<String, u64>, min_ms: u64) -> Option<String> {
    let total: u64 = votes.values().sum();
    let (name, ms) = votes.iter().max_by_key(|(_, ms)| **ms)?;
    (total >= min_ms && *ms * 10 >= total * 6).then(|| name.clone())
}

/// Name for one stretch of audio on its own (no diarization): whoever the app highlighted most.
pub fn dominant(timeline: &[Span], start: u64, end: u64) -> Option<String> {
    winner(&overlap(timeline, start, end), (end - start) / 3)
}

/// Accumulates active-speaker overlap per diarized speaker across the meeting.
#[derive(Default)]
pub struct Votes(HashMap<usize, HashMap<String, u64>>);

impl Votes {
    pub fn add(&mut self, speaker: usize, timeline: &[Span], start: u64, end: u64) {
        let v = self.0.entry(speaker).or_default();
        for (n, ms) in overlap(timeline, start, end) { *v.entry(n).or_insert(0) += ms; }
    }
    /// The speaker's name once ≥3 s of their speech lines up with one name ≥60% of the time.
    pub fn name(&self, speaker: usize) -> Option<String> {
        winner(self.0.get(&speaker)?, 3000)
    }
    pub fn merge(&mut self, from: usize, into: usize) {
        if let Some(v) = self.0.remove(&from) {
            let t = self.0.entry(into).or_default();
            for (n, ms) in v { *t.entry(n).or_insert(0) += ms; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voices_and_turns() {
        let mut v = vec![];
        assert_eq!(assign(&mut v, normalized(vec![1.0, 0.0, 0.1]), true), 0);
        assert_eq!(assign(&mut v, normalized(vec![0.9, 0.1, 0.0]), true), 0, "similar voice joins speaker 1");
        assert_eq!(assign(&mut v, normalized(vec![0.0, 0.0, 1.0]), false), 0, "a short fingerprint never starts a speaker");
        assert_eq!(assign(&mut v, normalized(vec![0.0, 1.0, 0.0]), true), 1, "different voice is new");
        let t = |s: usize, e: usize, sp: usize| Turn { start: s * RATE / 10, end: e * RATE / 10, speaker: sp };
        // 0-2 s A, pause 0.3 s, 2.3-4 s A → one turn; 0.2 s blip of B folds into it; 5-8 s B stays.
        assert_eq!(merge_turns(vec![t(0, 20, 0), t(23, 40, 0), t(41, 43, 1), t(50, 80, 1)]), vec![t(0, 43, 0), t(50, 80, 1)]);
    }

    #[test]
    fn names_from_timeline() {
        let span = |a: u64, b: u64, n: &str| Span { start_ms: a, end_ms: b, name: n.into() };
        let tl = [span(0, 4000, "Saurav Ashok"), span(4000, 5000, "Anand Rajan")];
        let mut v = Votes::default();
        v.add(0, &tl, 0, 5000);
        assert_eq!(v.name(0).as_deref(), Some("Saurav Ashok"));
        assert_eq!(v.name(1), None);
        v.add(1, &tl, 4000, 5000);
        assert_eq!(v.name(1), None, "1 s of evidence is not enough");
        assert_eq!(dominant(&tl, 4000, 5000).as_deref(), Some("Anand Rajan"));
        assert_eq!(dominant(&[span(0, 2500, "A"), span(2500, 5000, "B")], 0, 5000), None, "50/50 is ambiguous");
    }
}
