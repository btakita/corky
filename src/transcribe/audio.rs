//! Audio file decoding — any format → f32 PCM, 16kHz, mono.

use anyhow::{bail, Result};
use std::path::Path;

/// Formats that symphonia can decode natively.
const SYMPHONIA_EXTS: &[&str] = &[
    "wav", "mp3", "flac", "ogg", "m4a", "mp4", "aac", "aiff",
];

/// Video/media formats that symphonia can't handle but ffmpeg can.
const VIDEO_EXTS: &[&str] = &["mov", "mkv", "webm", "avi", "ts", "mts"];

/// Decode an audio file to f32 PCM samples at 16kHz mono.
pub fn decode_audio(path: &Path) -> Result<Vec<f32>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // For WAV files, try hound first (simpler, more reliable for PCM WAV)
    if ext == "wav"
        && let Ok(samples) = decode_wav_hound(path) {
            return Ok(samples);
        }

    // Video formats go straight to ffmpeg (no point trying symphonia)
    if VIDEO_EXTS.contains(&ext.as_str()) {
        return decode_ffmpeg(path);
    }

    // For formats symphonia supports, try it first, fall back to ffmpeg
    if SYMPHONIA_EXTS.contains(&ext.as_str()) {
        match decode_symphonia(path) {
            Ok(samples) => return Ok(samples),
            Err(e) => {
                eprintln!("  symphonia failed ({}), trying ffmpeg...", e);
                return decode_ffmpeg(path);
            }
        }
    }

    // For unsupported formats (AMR, etc.), fall back to ffmpeg
    decode_ffmpeg(path)
}

/// Decode via ffmpeg subprocess — converts any format to 16kHz mono f32le WAV on stdout.
fn decode_ffmpeg(path: &Path) -> Result<Vec<f32>> {
    let ffmpeg = which_ffmpeg()?;
    eprintln!("  Using ffmpeg for format: {}", path.extension().and_then(|e| e.to_str()).unwrap_or("unknown"));

    let output = std::process::Command::new(ffmpeg)
        .args([
            "-i", &path.to_string_lossy(),
            "-f", "f32le",        // raw f32 little-endian
            "-acodec", "pcm_f32le",
            "-ar", "16000",       // 16kHz
            "-ac", "1",           // mono
            "-v", "error",
            "pipe:1",             // output to stdout
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "ffmpeg failed to decode {}:\n{}",
            path.display(),
            stderr.trim()
        );
    }

    if output.stdout.is_empty() {
        bail!("ffmpeg produced no audio output from {}", path.display());
    }

    // Convert raw bytes to f32 samples
    let samples: Vec<f32> = output
        .stdout
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();

    Ok(samples)
}

fn which_ffmpeg() -> Result<String> {
    for name in &["ffmpeg"] {
        if let Ok(output) = std::process::Command::new(name).arg("-version").output()
            && output.status.success() {
                return Ok(name.to_string());
            }
    }
    bail!(
        "ffmpeg not found. Install ffmpeg for AMR and other format support.\n\
         On Arch: pacman -S ffmpeg\n\
         On Ubuntu: apt install ffmpeg\n\
         On macOS: brew install ffmpeg"
    );
}

/// Decode WAV via hound (handles PCM WAV reliably).
fn decode_wav_hound(path: &Path) -> Result<Vec<f32>> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let sample_rate = spec.sample_rate;

    let samples_i: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max_val))
                .collect::<std::result::Result<Vec<_>, _>>()?
        }
        hound::SampleFormat::Float => {
            reader
                .into_samples::<f32>()
                .collect::<std::result::Result<Vec<_>, _>>()?
        }
    };

    // Convert to mono
    let mono = to_mono(&samples_i, channels);

    // Resample to 16kHz if needed
    if sample_rate != 16000 {
        resample(&mono, sample_rate, 16000)
    } else {
        Ok(mono)
    }
}

/// Decode any audio format via symphonia.
fn decode_symphonia(path: &Path) -> Result<Vec<f32>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| anyhow::anyhow!("No audio track found"))?;
    let track_id = track.id;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1);
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow::anyhow!("Unknown sample rate"))?;

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => bail!("Failed to read packet: {}", e),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => bail!("Decode error: {}", e),
        };

        let spec = *decoded.spec();
        let duration = decoded.capacity();
        let mut sample_buf = SampleBuffer::<f32>::new(duration as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        all_samples.extend_from_slice(sample_buf.samples());
    }

    if all_samples.is_empty() {
        bail!("No audio samples decoded from {}", path.display());
    }

    // Convert to mono
    let mono = to_mono(&all_samples, channels);

    // Resample to 16kHz if needed
    if sample_rate != 16000 {
        resample(&mono, sample_rate, 16000)
    } else {
        Ok(mono)
    }
}

/// Convert interleaved multi-channel audio to mono by averaging channels.
fn to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }

    samples
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Resample audio from one sample rate to another using rubato.
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction, Resampler};

    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let mut resampler = SincFixedIn::<f32>::new(
        to_rate as f64 / from_rate as f64,
        2.0,
        params,
        samples.len().min(1024),
        1, // mono
    )?;

    let mut output = Vec::new();
    let chunk_size = resampler.input_frames_max();

    for chunk in samples.chunks(chunk_size) {
        let input = vec![chunk.to_vec()];
        let result = resampler.process(&input, None)?;
        output.extend_from_slice(&result[0]);
    }

    Ok(output)
}

/// Split audio samples into chunks of a given duration.
///
/// Returns `(offset_secs, chunk_samples)` pairs. The last chunk may be shorter.
pub fn chunk_audio(samples: &[f32], chunk_duration_secs: f64, sample_rate: u32) -> Vec<(f64, Vec<f32>)> {
    let chunk_size = (chunk_duration_secs * sample_rate as f64) as usize;
    if chunk_size == 0 || samples.is_empty() {
        return vec![(0.0, samples.to_vec())];
    }

    samples
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, chunk)| {
            let offset = i as f64 * chunk_duration_secs;
            (offset, chunk.to_vec())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_passthrough() {
        let samples = vec![1.0, 2.0, 3.0];
        assert_eq!(to_mono(&samples, 1), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn stereo_to_mono() {
        let samples = vec![1.0, 0.0, 0.0, 1.0, 0.5, 0.5];
        let mono = to_mono(&samples, 2);
        assert_eq!(mono, vec![0.5, 0.5, 0.5]);
    }

    #[test]
    fn symphonia_ext_detection() {
        assert!(SYMPHONIA_EXTS.contains(&"wav"));
        assert!(SYMPHONIA_EXTS.contains(&"mp3"));
        assert!(SYMPHONIA_EXTS.contains(&"flac"));
        assert!(!SYMPHONIA_EXTS.contains(&"amr"));
    }

    #[test]
    fn ffmpeg_not_found_for_missing_file() {
        let result = decode_ffmpeg(std::path::Path::new("/nonexistent/file.amr"));
        assert!(result.is_err());
    }
}
