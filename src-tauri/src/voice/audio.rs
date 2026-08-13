// Microphone capture for voice input.
//
// whisper.cpp accepts exactly one format: 16 kHz mono 16-bit PCM. A microphone
// offers whatever its hardware prefers — 44.1/48 kHz, one or two channels, and
// samples as f32, i16 or u16 — so everything is converted here rather than being
// pushed onto the caller.
//
// Recording runs on cpal's own audio thread and the samples are collected into a
// shared buffer. Nothing is written to disk until the recording stops: a dictated
// phrase is seconds long, so the whole thing fits in memory comfortably and a
// cancelled recording then leaves no file behind at all.

use std::sync::{Arc, Mutex};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// What whisper.cpp requires. Not configurable — the model is trained on it.
pub const TARGET_RATE: u32 = 16_000;

/// A recording in progress. The stream is kept alive by holding it here: dropping
/// a cpal stream stops the capture.
pub struct Recording {
    stream: Box<dyn StreamTrait>,
    samples: Arc<Mutex<Vec<f32>>>,
    source_rate: u32,
    source_channels: u16,
}

// cpal streams are not Send on every platform, but the recording only ever lives
// inside the Tauri state behind a mutex and is never moved across threads while
// active. The bound is needed because Tauri's managed state requires it.
unsafe impl Send for Recording {}

/// Mixes interleaved channels down to mono by averaging.
///
/// Averaging rather than taking the first channel: on a stereo mic the speech
/// often sits in both, and dropping one loses half the signal level.
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let n = channels as usize;
    samples
        .chunks(n)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Resamples to 16 kHz by linear interpolation.
///
/// Linear rather than a windowed-sinc filter: speech recognition is unaffected by
/// the aliasing this leaves behind — whisper's own front end reduces the signal to
/// an 80-bin mel spectrogram — and it avoids a resampler dependency for a step
/// that runs once per dictated phrase.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((samples.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = samples[idx];
        // The last sample has no successor to interpolate towards.
        let b = *samples.get(idx + 1).unwrap_or(&a);
        out.push(a + (b - a) * frac);
    }
    out
}

/// Converts a captured buffer into what whisper expects: mono, 16 kHz, i16.
pub fn prepare(samples: &[f32], source_rate: u32, channels: u16) -> Vec<i16> {
    let mono = to_mono(samples, channels);
    let resampled = resample(&mono, source_rate, TARGET_RATE);
    resampled
        .iter()
        // Clamped before scaling: a sample slightly outside [-1, 1] (which cpal
        // does not forbid) would wrap around and turn a loud sound into noise.
        .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

/// Starts capturing from the default input device.
pub fn start() -> Result<Recording, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "Микрофон не найден.".to_string())?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("Не удалось открыть микрофон: {e}"))?;

    let source_rate = config.sample_rate();
    let source_channels = config.channels();
    let sample_format = config.sample_format();
    // config is consumed by the conversion, so the format is read off beforehand.
    let stream_config: cpal::StreamConfig = config.into();
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

    // Errors on the audio thread are dropped deliberately: there is nobody to
    // return them to, and a failed capture surfaces as an empty recording, which
    // stop() already reports.
    let err_fn = |_| {};

    // Every sample format is normalised to f32 here so the rest of the module has
    // one case to handle.
    let buf = samples.clone();
    let stream: Box<dyn StreamTrait> = match sample_format {
        cpal::SampleFormat::F32 => Box::new(
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &_| buf.lock().unwrap().extend_from_slice(data),
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Не удалось начать запись: {e}"))?,
        ),
        cpal::SampleFormat::I16 => Box::new(
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &_| {
                        let mut b = buf.lock().unwrap();
                        b.extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Не удалось начать запись: {e}"))?,
        ),
        cpal::SampleFormat::U16 => Box::new(
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[u16], _: &_| {
                        let mut b = buf.lock().unwrap();
                        // u16 is unsigned with silence at the midpoint, not at zero.
                        b.extend(data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0));
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Не удалось начать запись: {e}"))?,
        ),
        other => return Err(format!("Микрофон отдаёт неподдерживаемый формат: {other:?}")),
    };

    stream.play().map_err(|e| format!("Не удалось начать запись: {e}"))?;

    Ok(Recording { stream, samples, source_rate, source_channels })
}

impl Recording {
    /// Stops the capture and writes the result as a 16 kHz mono WAV.
    pub fn stop_to_wav(self, path: &std::path::Path) -> Result<(), String> {
        // Dropping the stream is what actually stops the device.
        drop(self.stream);

        let raw = self.samples.lock().unwrap().clone();
        let pcm = prepare(&raw, self.source_rate, self.source_channels);
        if pcm.is_empty() {
            return Err("Запись пуста — микрофон не дал звука.".into());
        }

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: TARGET_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
        for s in pcm {
            writer.write_sample(s).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Averaging, not "take the first channel": on a stereo mic the speech sits in
    // both and dropping one halves the signal.
    #[test]
    fn stereo_is_averaged_into_mono() {
        let stereo = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(to_mono(&stereo, 2), vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn mono_is_left_alone() {
        let mono = [0.1, 0.2, 0.3];
        assert_eq!(to_mono(&mono, 1), mono.to_vec());
    }

    // 48 kHz is what most microphones default to, so this is the common path.
    #[test]
    fn resampling_48k_to_16k_keeps_a_third_of_the_samples() {
        let input: Vec<f32> = (0..300).map(|i| i as f32).collect();
        let out = resample(&input, 48_000, TARGET_RATE);
        assert_eq!(out.len(), 100);
    }

    #[test]
    fn resampling_is_a_noop_at_the_target_rate() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample(&input, TARGET_RATE, TARGET_RATE), input);
    }

    // A constant signal must survive resampling unchanged: interpolation between
    // equal neighbours cannot invent a different value, and a drift here would mean
    // the index arithmetic is wrong.
    #[test]
    fn resampling_preserves_a_constant_signal() {
        let input = vec![0.5f32; 480];
        let out = resample(&input, 48_000, TARGET_RATE);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6), "постоянный сигнал исказился");
    }

    // Out-of-range samples must clamp rather than wrap: without the clamp a value
    // just above 1.0 overflows i16 and a loud sound turns into noise.
    #[test]
    fn out_of_range_samples_clamp_instead_of_wrapping() {
        let loud = [1.5f32, -1.5];
        let pcm = prepare(&loud, TARGET_RATE, 1);
        assert_eq!(pcm, vec![i16::MAX, -i16::MAX]);
    }

    #[test]
    fn prepare_produces_mono_at_the_target_rate() {
        // 480 stereo frames at 48 kHz -> 160 mono samples at 16 kHz.
        let stereo: Vec<f32> = (0..960).map(|i| (i % 2) as f32 * 0.1).collect();
        let pcm = prepare(&stereo, 48_000, 2);
        assert_eq!(pcm.len(), 160);
    }

    #[test]
    fn empty_input_stays_empty() {
        assert!(prepare(&[], 48_000, 2).is_empty());
        assert!(resample(&[], 48_000, TARGET_RATE).is_empty());
    }
}
