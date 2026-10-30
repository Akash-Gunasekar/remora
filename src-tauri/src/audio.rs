//! Audio engine (plan §4.1): mic + system loopback via cpal, normalized to 16 kHz mono.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

pub const RATE: u32 = 16_000;

#[derive(Clone, Copy)]
pub enum Source {
    Mic,
    /// WASAPI loopback of the default output device (Windows). Other OSes return an error.
    System,
}

pub struct Recording {
    stop_tx: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    // ponytail: Mutex<Vec> instead of a lock-free ring; callback holds it for microseconds. Swap to rtrb if glitches show up.
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    /// Peak of the latest callback block (f32 bits).
    pub level: Arc<AtomicU32>,
    /// RMS of the latest callback block (f32 bits); drives the pill's onset detection.
    pub rms: Arc<AtomicU32>,
    pub alive: Arc<AtomicBool>,
}

impl Recording {
    pub fn start(src: Source) -> Result<Recording, String> {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let level = Arc::new(AtomicU32::new(0));
        let rms = Arc::new(AtomicU32::new(0));
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        let (b, l, r) = (buf.clone(), level.clone(), rms.clone());
        // cpal streams aren't Send, so the stream lives and dies on its own thread.
        let thread = std::thread::spawn(move || match build(src, b, l, r) {
            Ok((stream, rate)) => {
                let _ = ready_tx.send(Ok(rate));
                let _ = stop_rx.recv();
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        });
        let rate = ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(Recording { stop_tx, thread: Some(thread), buf, rate, level, rms, alive: Arc::new(AtomicBool::new(true)) })
    }

    pub fn drain_16k(&self) -> Vec<f32> {
        let raw = std::mem::take(&mut *self.buf.lock().unwrap());
        resample(&raw, self.rate, RATE)
    }

    /// `drain_16k` for another thread while the recording keeps running.
    pub fn drainer(&self) -> impl Fn() -> Vec<f32> + Send + 'static {
        let (buf, rate) = (self.buf.clone(), self.rate);
        move || resample(&std::mem::take(&mut *buf.lock().unwrap()), rate, RATE)
    }

    pub fn stop(mut self) -> Vec<f32> {
        self.alive.store(false, Relaxed);
        let _ = self.stop_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.drain_16k()
    }
}

fn build(src: Source, buf: Arc<Mutex<Vec<f32>>>, level: Arc<AtomicU32>, rms: Arc<AtomicU32>) -> Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let (device, config) = match src {
        Source::Mic => {
            let d = host.default_input_device().ok_or("No microphone found")?;
            let c = d.default_input_config().map_err(|e| e.to_string())?;
            (d, c)
        }
        Source::System => {
            if !cfg!(windows) {
                return Err("System audio capture is only implemented on Windows so far".into());
            }
            let d = host.default_output_device().ok_or("No output device")?;
            let c = d.default_output_config().map_err(|e| e.to_string())?;
            (d, c)
        }
    };
    let ch = config.channels() as usize;
    let rate = config.sample_rate().0;
    let fmt = config.sample_format();
    let cfg: cpal::StreamConfig = config.into();
    let err = |e| eprintln!("audio stream error: {e}");
    let stream = match fmt {
        SampleFormat::F32 => device.build_input_stream(&cfg, move |d: &[f32], _: &_| push(d, ch, &buf, &level, &rms, |x| x), err, None),
        SampleFormat::I16 => device.build_input_stream(&cfg, move |d: &[i16], _: &_| push(d, ch, &buf, &level, &rms, |x| x as f32 / 32768.0), err, None),
        SampleFormat::U16 => device.build_input_stream(&cfg, move |d: &[u16], _: &_| push(d, ch, &buf, &level, &rms, |x| (x as f32 - 32768.0) / 32768.0), err, None),
        SampleFormat::I32 => device.build_input_stream(&cfg, move |d: &[i32], _: &_| push(d, ch, &buf, &level, &rms, |x| x as f32 / 2_147_483_648.0), err, None),
        f => return Err(format!("Unsupported sample format {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

fn push<T: Copy>(d: &[T], ch: usize, buf: &Mutex<Vec<f32>>, level: &AtomicU32, rms: &AtomicU32, conv: impl Fn(T) -> f32) {
    let (mut peak, mut sq, mut n) = (0f32, 0f32, 0usize);
    let mut b = buf.lock().unwrap();
    for frame in d.chunks(ch) {
        let s = frame.iter().map(|&x| conv(x)).sum::<f32>() / ch as f32;
        peak = peak.max(s.abs());
        sq += s * s;
        n += 1;
        b.push(s);
    }
    level.store(peak.to_bits(), Relaxed);
    rms.store((sq / n.max(1) as f32).sqrt().to_bits(), Relaxed);
}

// ponytail: linear interpolation without an anti-alias filter; switch to rubato if WER suffers on 44.1/48 kHz mics.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let n = (x.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let j = p as usize;
            let f = (p - j as f64) as f32;
            let a = x[j.min(x.len() - 1)];
            let b = *x.get(j + 1).unwrap_or(&a);
            a + (b - a) * f
        })
        .collect()
}

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

// ponytail: energy threshold stands in for Silero VAD; good enough to skip silent chunks.
pub fn has_speech(x: &[f32]) -> bool {
    // 0.008 RMS ≈ -42 dBFS: quiet laptop mic arrays still clear it while room hiss doesn't.
    x.chunks(RATE as usize / 10).filter(|c| rms(c) > 0.008).count() >= 3
}

/// End of a finished phrase in `x[from..]`: at least 2 s of audio that ends in ~350 ms of silence.
/// Cutting there lets dictation transcribe phrase by phrase while the user is still talking.
/// Speech with no pause is cut at its quietest moment once 20 s pile up, so it never waits for key release.
pub fn phrase_end(x: &[f32], from: usize) -> Option<usize> {
    let tail = RATE as usize * 35 / 100;
    let n = x.len().saturating_sub(from);
    if n >= 2 * RATE as usize && rms(&x[x.len() - tail..]) < 0.008 { return Some(x.len()); }
    if n < 20 * RATE as usize { return None; }
    let f = RATE as usize / 10;
    (x.len() - 4 * RATE as usize..x.len() - f).step_by(f / 2)
        .min_by(|&a, &b| rms(&x[a..a + f]).total_cmp(&rms(&x[b..b + f])))
        .map(|a| a + f / 2)
}

fn spec() -> hound::WavSpec {
    hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }
}

pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec()).expect("wav");
        for s in samples {
            let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let _ = w.finalize();
    }
    cur.into_inner()
}

pub type WavFile = hound::WavWriter<std::io::BufWriter<std::fs::File>>;

pub fn wav_file(path: &std::path::Path) -> Result<WavFile, String> {
    hound::WavWriter::create(path, spec()).map_err(|e| e.to_string())
}

pub fn write_samples(w: &mut WavFile, samples: &[f32]) {
    for s in samples {
        let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_length_and_wav() {
        let x = vec![0.5f32; 48_000];
        let y = resample(&x, 48_000, RATE);
        assert_eq!(y.len(), 16_000);
        assert!(y.iter().all(|v| (*v - 0.5).abs() < 1e-6));
        let wav = wav_bytes(&y);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 32_000);
        assert!(!has_speech(&vec![0.0; 16_000]));
        assert!(has_speech(&y));
    }

    #[test]
    fn phrase_end_needs_length_and_pause() {
        let mut x = vec![0.3f32; RATE as usize * 3];
        assert_eq!(phrase_end(&x, 0), None); // still talking
        x.extend(vec![0.0; RATE as usize / 2]);
        assert_eq!(phrase_end(&x, 0), Some(x.len()));
        assert_eq!(phrase_end(&x, x.len() - RATE as usize), None); // too little new audio
        // 25 s without a pause: cut at the quiet dip at 23 s, not at the end.
        let r = RATE as usize;
        let mut y = vec![0.3f32; 25 * r];
        y[23 * r..23 * r + r / 10].fill(0.0);
        let cut = phrase_end(&y, 0).unwrap();
        assert!((23 * r..=23 * r + r / 10).contains(&cut), "{cut}");
    }
}
