//! Whisper-rs transcription engine.

use anyhow::{bail, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::audio;
use super::model;

/// Run transcription on an audio file.
///
/// Outputs timestamped text to stdout or a file.
/// If `speakers` is non-empty, uses whisper's speaker turn detection to label segments.
/// If `diarize` is true, uses pyannote-rs for speaker diarization (requires `diarize` feature).
#[allow(clippy::too_many_arguments)]
pub fn run(
    file: &Path,
    model_name: Option<&str>,
    language: Option<&str>,
    output: Option<&str>,
    speakers: &[String],
    diarize: bool,
    no_adaptive_chunk: bool,
    no_resolve_unknown: bool,
    no_confidence_retranscribe: bool,
) -> Result<()> {
    if !file.exists() {
        bail!("Audio file not found: {}", file.display());
    }

    // Load config for defaults
    let config = crate::config::corky_config::try_load_config(None);
    let tc = config.as_ref().and_then(|c| c.transcription.as_ref());

    let model_name = model_name
        .or(tc.map(|t| t.model.as_str()))
        .unwrap_or("large-v3-turbo");
    let language = language.or(tc.and_then(|t| {
        if t.language.is_empty() { None } else { Some(t.language.as_str()) }
    }));
    let cache_dir = tc.and_then(|t| {
        if t.model_path.is_empty() { None } else { Some(t.model_path.as_str()) }
    });

    // Resolve model
    let model_path = model::resolve_model_path(model_name, cache_dir)?;

    // Decode audio
    eprintln!("Decoding audio: {}", file.display());
    let samples = audio::decode_audio(file)?;
    let duration_secs = samples.len() as f64 / 16000.0;
    eprintln!(
        "Audio: {:.1}s ({} samples at 16kHz mono)",
        duration_secs,
        samples.len()
    );

    // Load whisper model
    eprintln!("Loading model: {}", model_path.display());
    let ctx = WhisperContext::new_with_params(
        model_path.to_str().unwrap_or(""),
        WhisperContextParameters::default(),
    )
    .map_err(|e| anyhow::anyhow!("Failed to load whisper model: {:?}", e))?;

    // Configure transcription
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if let Some(lang) = language {
        params.set_language(Some(lang));
    }
    params.set_print_progress(true);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    // Suppress hallucination: raise no-speech threshold, suppress blank/non-speech tokens
    params.set_no_speech_thold(0.8);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    // Enable tdrz speaker turn detection when speakers are provided
    if !speakers.is_empty() {
        params.set_tdrz_enable(true);
    }

    // Run inference
    eprintln!("Transcribing...");
    let start = std::time::Instant::now();

    let mut state = ctx
        .create_state()
        .map_err(|e| anyhow::anyhow!("Failed to create whisper state: {:?}", e))?;
    state
        .full(params, &samples)
        .map_err(|e| anyhow::anyhow!("Transcription failed: {:?}", e))?;

    let elapsed = start.elapsed();
    let n_segments = state.full_n_segments();

    eprintln!(
        "Done. {} segments in {:.1}s (speed: {:.1}x realtime)",
        n_segments,
        elapsed.as_secs_f64(),
        duration_secs / elapsed.as_secs_f64()
    );

    // Resolve config-level defaults for adaptive_chunk, resolve_unknown, confidence_retranscribe
    let adaptive_chunk = !no_adaptive_chunk
        && tc.map(|t| t.adaptive_chunk).unwrap_or(true);
    let resolve_unknown = !no_resolve_unknown
        && tc.map(|t| t.resolve_unknown).unwrap_or(true);
    let confidence_retranscribe = !no_confidence_retranscribe;
    let confidence_threshold = tc.map(|t| t.confidence_threshold).unwrap_or(0.4);

    // resolve_unknown implies diarize
    let diarize = diarize || resolve_unknown;

    // Format output
    let text = if diarize {
        #[cfg(feature = "diarize")]
        {
            format_diarized(
                &ctx, &state, n_segments, &samples, speakers, duration_secs, cache_dir,
                adaptive_chunk, resolve_unknown, confidence_retranscribe,
                confidence_threshold, language,
            )?
        }
        #[cfg(not(feature = "diarize"))]
        {
            bail!(
                "Diarization support not compiled.\n\
                 Rebuild with: cargo install corky --features diarize"
            );
        }
    } else if speakers.is_empty() {
        format_plain(&state, n_segments)?
    } else {
        format_speakers(&state, n_segments, speakers, duration_secs)?
    };

    // Output
    if let Some(out_path) = output {
        std::fs::write(out_path, &text)?;
        eprintln!("Written to: {}", out_path);
    } else {
        print!("{}", text);
    }

    Ok(())
}

/// Plain timestamped output (no speaker labels).
fn format_plain(state: &whisper_rs::WhisperState, n_segments: i32) -> Result<String> {
    let mut text = String::new();
    for i in 0..n_segments {
        let segment = state
            .get_segment(i)
            .ok_or_else(|| anyhow::anyhow!("Failed to get segment {}", i))?;

        let t0 = segment.start_timestamp();
        let t1 = segment.end_timestamp();
        let segment_text = segment
            .to_str()
            .map_err(|e| anyhow::anyhow!("Failed to get segment text: {:?}", e))?;

        text.push_str(&format!(
            "[{} --> {}] {}\n",
            format_timestamp(t0),
            format_timestamp(t1),
            segment_text.trim()
        ));
    }
    Ok(text)
}

/// Speaker-labeled markdown output using whisper's tdrz speaker turn detection.
fn format_speakers(
    state: &whisper_rs::WhisperState,
    n_segments: i32,
    speakers: &[String],
    duration_secs: f64,
) -> Result<String> {
    let mut text = String::new();

    // YAML frontmatter
    text.push_str("---\n");
    text.push_str(&format!("date: {}\n", chrono::Local::now().format("%Y-%m-%d")));
    text.push_str("type: phone-call\n");
    text.push_str("participants:\n");
    for s in speakers {
        text.push_str(&format!("  - {}\n", s));
    }
    let mins = (duration_secs / 60.0).floor() as u32;
    let secs = (duration_secs % 60.0).floor() as u32;
    text.push_str(&format!("duration: \"{}:{:02}\"\n", mins, secs));
    text.push_str("---\n\n");

    let mut current_speaker_idx: usize = 0;
    let mut block_start: i64 = 0;
    #[allow(unused_assignments)]
    let mut block_end: i64 = 0;
    let mut block_text = String::new();
    let mut block_active = false;

    for i in 0..n_segments {
        let segment = state
            .get_segment(i)
            .ok_or_else(|| anyhow::anyhow!("Failed to get segment {}", i))?;

        let t0 = segment.start_timestamp();
        let t1 = segment.end_timestamp();
        let segment_text = segment
            .to_str()
            .map_err(|e| anyhow::anyhow!("Failed to get segment text: {:?}", e))?;
        let is_turn = segment.next_segment_speaker_turn();

        if !block_active {
            block_start = t0;
            block_active = true;
        }
        block_end = t1;

        let trimmed = segment_text.trim();
        if !trimmed.is_empty() {
            if !block_text.is_empty() {
                block_text.push(' ');
            }
            block_text.push_str(trimmed);
        }

        // Flush block on speaker turn or last segment
        if is_turn || i == n_segments - 1 {
            if !block_text.is_empty() {
                let speaker = &speakers[current_speaker_idx % speakers.len()];
                text.push_str(&format!(
                    "**{}** [{} → {}]\n{}\n\n",
                    speaker,
                    format_timestamp(block_start),
                    format_timestamp(block_end),
                    block_text
                ));
            }

            if is_turn {
                current_speaker_idx += 1;
            }
            block_active = false;
            block_text.clear();
        }
    }

    Ok(text)
}

/// A whisper segment with timestamps, text, and confidence score.
#[cfg(feature = "diarize")]
#[derive(Debug, Clone)]
struct WhisperSeg {
    t0: i64,
    t1: i64,
    text: String,
    confidence: f32,
}

/// Collect whisper segments as structured data (centisecond timestamps + text + confidence).
///
/// Confidence is the average token probability for the segment (0.0-1.0).
#[cfg(feature = "diarize")]
fn collect_whisper_segments(
    state: &whisper_rs::WhisperState,
    n_segments: i32,
) -> Result<Vec<WhisperSeg>> {
    let mut result = Vec::new();
    for i in 0..n_segments {
        let segment = state
            .get_segment(i)
            .ok_or_else(|| anyhow::anyhow!("Failed to get segment {}", i))?;
        let t0 = segment.start_timestamp();
        let t1 = segment.end_timestamp();
        let text = segment
            .to_str()
            .map_err(|e| anyhow::anyhow!("Failed to get segment text: {:?}", e))?
            .to_string();

        // Compute avg token probability as confidence
        let n_tokens = segment.n_tokens();
        let confidence = if n_tokens > 0 {
            let mut sum = 0.0f32;
            for j in 0..n_tokens {
                if let Some(token) = segment.get_token(j) {
                    sum += token.token_probability();
                }
            }
            sum / n_tokens as f32
        } else {
            0.0
        };

        result.push(WhisperSeg { t0, t1, text, confidence });
    }
    // Remove whisper repetition hallucinations
    result = filter_repetitions(result);
    Ok(result)
}

/// Filter out whisper repetition hallucinations within each segment's text.
///
/// Handles two patterns:
/// 1. Exact n-gram repeats: "call her tomorrow. call her tomorrow. call her tomorrow."
/// 2. Escalating repeats: "call her tomorrow tomorrow tomorrow tomorrow..."
#[cfg(feature = "diarize")]
fn filter_repetitions(segments: Vec<WhisperSeg>) -> Vec<WhisperSeg> {
    let mut cleaned_count = 0;
    let result: Vec<_> = segments
        .into_iter()
        .map(|mut seg| {
            let cleaned = collapse_repeated_phrases(&seg.text);
            if cleaned.len() < seg.text.len() {
                cleaned_count += 1;
            }
            seg.text = cleaned;
            seg
        })
        .collect();
    if cleaned_count > 0 {
        eprintln!("Cleaned repetitions in {} segment(s)", cleaned_count);
    }
    result
}

/// Collapse repeated phrases within a single text segment.
///
/// Two-pass approach:
/// 1. Exact n-gram: phrases of 4-15 words repeating 3+ times consecutively
/// 2. Word frequency spike: any word appearing 10+ times in a 100-word window
///    triggers truncation at the start of the repetitive region
#[cfg(feature = "diarize")]
fn collapse_repeated_phrases(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 12 {
        return text.to_string();
    }

    // Pass 1: Exact n-gram collapse (phrases of 4-15 words repeating 3+ times)
    let mut result_words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    for phrase_len in (4..=15).rev() {
        let mut collapsed: Vec<String> = Vec::new();
        let mut i = 0;
        while i < result_words.len() {
            if i + phrase_len * 3 <= result_words.len() {
                let phrase: Vec<&str> = result_words[i..i + phrase_len]
                    .iter()
                    .map(|s| s.as_str())
                    .collect();
                let mut repeat_count = 1;
                let mut j = i + phrase_len;
                while j + phrase_len <= result_words.len() {
                    let next: Vec<&str> = result_words[j..j + phrase_len]
                        .iter()
                        .map(|s| s.as_str())
                        .collect();
                    if next
                        .iter()
                        .zip(phrase.iter())
                        .all(|(a, b)| a.to_lowercase() == b.to_lowercase())
                    {
                        repeat_count += 1;
                        j += phrase_len;
                    } else {
                        break;
                    }
                }
                if repeat_count >= 3 {
                    collapsed.extend(result_words[i..i + phrase_len].iter().cloned());
                    i = j;
                    continue;
                }
            }
            collapsed.push(result_words[i].clone());
            i += 1;
        }
        result_words = collapsed;
    }

    // Pass 2: Word frequency spike detection (escalating patterns)
    // Scan with a sliding window; if any non-stopword appears 15+ times in 100 words,
    // that region is hallucinated. Keep text up to where the spike starts.
    // Stopwords are excluded because they naturally appear at high frequency in speech.
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of",
        "with", "by", "from", "as", "is", "was", "are", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could", "should",
        "may", "might", "shall", "can", "that", "this", "these", "those", "it", "its",
        "i", "you", "he", "she", "we", "they", "me", "him", "her", "us", "them",
        "my", "your", "his", "our", "their", "not", "so", "if", "up", "out", "about",
        "just", "like", "very", "all", "also", "into", "than", "then", "there", "what",
        "which", "who", "how", "when", "well", "yeah", "know", "really", "think",
    ];
    let window_size = 100;
    let spike_threshold = 15;
    let mut truncate_at: Option<usize> = None;

    for start in 0..result_words.len().saturating_sub(window_size) {
        let end = (start + window_size).min(result_words.len());
        let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for w in &result_words[start..end] {
            let lower = w.to_lowercase();
            // Strip trailing punctuation for stopword matching
            let stripped = lower.trim_end_matches(|c: char| !c.is_alphabetic());
            if !STOPWORDS.contains(&stripped) {
                *freq.entry(lower).or_insert(0) += 1;
            }
        }
        if freq.values().any(|&count| count >= spike_threshold) {
            truncate_at = Some(start);
            break;
        }
    }

    if let Some(trunc) = truncate_at {
        // Find the sentence boundary just before the spike
        // Walk back to find a period, question mark, or capital letter start
        let mut cut = trunc;
        for i in (0..trunc).rev() {
            let w = &result_words[i];
            if w.ends_with('.') || w.ends_with('?') || w.ends_with('!') {
                cut = i + 1;
                break;
            }
        }
        result_words.truncate(cut);
    }

    result_words.join(" ")
}

/// Chunk duration cascade for adaptive chunking (seconds).
#[cfg(feature = "diarize")]
const CHUNK_CASCADE: &[f64] = &[600.0, 120.0, 30.0];

/// Diarized speaker-labeled output using pyannote-rs.
#[cfg(feature = "diarize")]
#[allow(clippy::too_many_arguments)]
fn format_diarized(
    ctx: &WhisperContext,
    state: &whisper_rs::WhisperState,
    n_segments: i32,
    samples: &[f32],
    speakers: &[String],
    duration_secs: f64,
    cache_dir: Option<&str>,
    adaptive_chunk: bool,
    resolve_unknown: bool,
    confidence_retranscribe: bool,
    confidence_threshold: f32,
    language: Option<&str>,
) -> Result<String> {
    use super::diarize;

    // Collect whisper transcript segments
    let whisper_segs = collect_whisper_segments(state, n_segments)?;

    // Run diarization (with adaptive chunking if enabled)
    let max_speakers = if speakers.is_empty() { 6 } else { speakers.len() };
    let expected_min = if speakers.is_empty() { 2 } else { speakers.len() };
    let diarized = if adaptive_chunk {
        run_adaptive_diarization(samples, max_speakers, expected_min, cache_dir)?
    } else {
        diarize::diarize(samples, 16000, max_speakers, cache_dir)?
    };

    // Convert WhisperSeg to the tuple format expected by merge_speakers
    let whisper_tuples: Vec<(i64, i64, String)> = whisper_segs
        .iter()
        .map(|s| (s.t0, s.t1, s.text.clone()))
        .collect();

    // Merge whisper text with diarization speaker labels
    let mut merged = diarize::merge_speakers(&whisper_tuples, &diarized, 16000);

    // Inject whisper-level confidence into merged segments
    // (merge_speakers sets confidence from diarization overlap; override with whisper token prob)
    for (ms, ws) in merged.iter_mut().zip(whisper_segs.iter()) {
        ms.confidence = ws.confidence;
    }

    // Confidence-based re-transcription pass
    if confidence_retranscribe {
        retranscribe_low_confidence(
            &mut merged, ctx, samples, confidence_threshold, language,
        )?;
    }

    // Determine speaker names
    let speaker_labels = if speakers.is_empty() {
        let excerpts = diarize::get_speaker_excerpts(&merged);
        if excerpts.is_empty() {
            std::collections::HashMap::new()
        } else {
            diarize::interactive_label(&excerpts)?
        }
    } else {
        let mut seen_order: Vec<usize> = Vec::new();
        for seg in &merged {
            if seg.speaker_id != 0 && !seen_order.contains(&seg.speaker_id) {
                seen_order.push(seg.speaker_id);
            }
        }
        let mut labels = std::collections::HashMap::new();
        for (i, &id) in seen_order.iter().enumerate() {
            let name = speakers.get(i).cloned().unwrap_or_else(|| format!("Speaker {}", id));
            labels.insert(id, name);
        }
        labels
    };

    // Resolve unknown speakers via Claude API
    if resolve_unknown {
        use super::resolve;
        if let Err(e) = resolve::resolve_unknown_speakers(&mut merged, &speaker_labels) {
            eprintln!("Warning: resolve-unknown failed: {}", e);
        }
    }

    // Build output
    format_merged_output(&merged, &speaker_labels, duration_secs)
}

/// A contiguous block of low-confidence segments identified for re-transcription.
#[cfg(feature = "diarize")]
#[derive(Debug, Clone)]
struct LowConfidenceBlock {
    /// Index of the first segment in the block.
    start_idx: usize,
    /// Index one past the last segment in the block.
    end_idx: usize,
    /// Average confidence of segments in this block.
    avg_confidence: f32,
}

/// Scan merged segments and find contiguous blocks where average confidence is below threshold.
///
/// Adjacent low-confidence segments are grouped together. Isolated high-confidence segments
/// between two low-confidence regions don't break the block (gap tolerance of 1).
#[cfg(feature = "diarize")]
fn find_low_confidence_blocks(
    merged: &[super::diarize::MergedSegment],
    threshold: f32,
) -> Vec<LowConfidenceBlock> {
    if merged.is_empty() {
        return Vec::new();
    }

    let mut blocks = Vec::new();
    let mut block_start: Option<usize> = None;

    for (i, seg) in merged.iter().enumerate() {
        let is_low = seg.confidence < threshold;
        if is_low {
            if block_start.is_none() {
                block_start = Some(i);
            }
        } else if let Some(start) = block_start {
            // Allow a gap of 1 high-confidence segment between low-confidence regions
            let next_is_low = merged.get(i + 1).map(|s| s.confidence < threshold).unwrap_or(false);
            if !next_is_low {
                // End the block
                let end = i;
                let count = end - start;
                if count > 0 {
                    let avg = merged[start..end]
                        .iter()
                        .map(|s| s.confidence as f64)
                        .sum::<f64>() / count as f64;
                    blocks.push(LowConfidenceBlock {
                        start_idx: start,
                        end_idx: end,
                        avg_confidence: avg as f32,
                    });
                }
                block_start = None;
            }
        }
    }

    // Close any trailing block
    if let Some(start) = block_start {
        let end = merged.len();
        let count = end - start;
        if count > 0 {
            let avg = merged[start..end]
                .iter()
                .map(|s| s.confidence as f64)
                .sum::<f64>() / count as f64;
            blocks.push(LowConfidenceBlock {
                start_idx: start,
                end_idx: end,
                avg_confidence: avg as f32,
            });
        }
    }

    blocks
}

/// Re-transcribe low-confidence blocks with smaller audio windows.
///
/// For each block, extracts the audio for that time range (with 2s padding),
/// runs whisper on the chunk, and replaces the original segments if the new
/// transcription has higher average confidence.
#[cfg(feature = "diarize")]
fn retranscribe_low_confidence(
    merged: &mut Vec<super::diarize::MergedSegment>,
    ctx: &WhisperContext,
    samples: &[f32],
    threshold: f32,
    language: Option<&str>,
) -> Result<()> {
    let blocks = find_low_confidence_blocks(merged, threshold);
    if blocks.is_empty() {
        return Ok(());
    }

    eprintln!(
        "Found {} low-confidence block(s) below threshold {:.2}",
        blocks.len(),
        threshold
    );

    let sample_rate = 16000_u32;
    let total_samples = samples.len();
    let mut replaced_count = 0;
    let mut total_improvement = 0.0f64;

    // Process blocks in reverse order so index shifts don't affect earlier blocks
    for block in blocks.iter().rev() {
        // Get time range from merged segments (centisecond timestamps)
        let block_t0_cs = merged[block.start_idx].t0;
        let block_t1_cs = merged[block.end_idx - 1].t1;

        // Convert to sample indices with 2s padding
        let padding_samples = 2 * sample_rate as usize;
        let start_sample = ((block_t0_cs as f64 * 0.01 * sample_rate as f64) as usize)
            .saturating_sub(padding_samples);
        let end_sample = ((block_t1_cs as f64 * 0.01 * sample_rate as f64) as usize + padding_samples)
            .min(total_samples);

        if end_sample <= start_sample {
            continue;
        }

        let chunk = &samples[start_sample..end_sample];
        let chunk_offset_cs = (start_sample as f64 / sample_rate as f64 * 100.0) as i64;

        eprintln!(
            "  Re-transcribing block [{} -> {}] ({} segments, avg conf {:.2})",
            format_timestamp(block_t0_cs),
            format_timestamp(block_t1_cs),
            block.end_idx - block.start_idx,
            block.avg_confidence
        );

        // Create a new whisper state and transcribe the chunk
        let mut state = ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create whisper state for retranscription: {:?}", e))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if let Some(lang) = language {
            params.set_language(Some(lang));
        }
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_speech_thold(0.8);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);

        state
            .full(params, chunk)
            .map_err(|e| anyhow::anyhow!("Re-transcription failed: {:?}", e))?;

        let n_new = state.full_n_segments();
        if n_new == 0 {
            continue;
        }

        // Collect new segments with confidence
        let mut new_segs = Vec::new();
        for i in 0..n_new {
            let seg = state
                .get_segment(i)
                .ok_or_else(|| anyhow::anyhow!("Failed to get re-transcribed segment {}", i))?;

            let n_tokens = seg.n_tokens();
            let confidence = if n_tokens > 0 {
                let mut sum = 0.0f32;
                for j in 0..n_tokens {
                    if let Some(token) = seg.get_token(j) {
                        sum += token.token_probability();
                    }
                }
                sum / n_tokens as f32
            } else {
                0.0
            };

            let text = seg
                .to_str()
                .map_err(|e| anyhow::anyhow!("Failed to get segment text: {:?}", e))?
                .to_string();

            // Adjust timestamps: chunk-relative -> absolute (add chunk offset)
            let t0 = seg.start_timestamp() + chunk_offset_cs;
            let t1 = seg.end_timestamp() + chunk_offset_cs;

            new_segs.push((t0, t1, text, confidence));
        }

        // Calculate new average confidence
        let new_avg_conf = if new_segs.is_empty() {
            0.0
        } else {
            new_segs.iter().map(|s| s.3 as f64).sum::<f64>() / new_segs.len() as f64
        };

        // Only replace if the new transcription is better
        if new_avg_conf as f32 > block.avg_confidence {
            let improvement = new_avg_conf - block.avg_confidence as f64;
            total_improvement += improvement;
            replaced_count += 1;

            // Build replacement MergedSegments, preserving original speaker labels
            let mut replacements: Vec<super::diarize::MergedSegment> = Vec::new();
            for (t0, t1, text, confidence) in &new_segs {
                // Find the best matching original speaker for this time range
                let mid_cs = (t0 + t1) / 2;
                let speaker_id = merged[block.start_idx..block.end_idx]
                    .iter()
                    .min_by_key(|s| {
                        let s_mid = (s.t0 + s.t1) / 2;
                        (s_mid - mid_cs).unsigned_abs()
                    })
                    .map(|s| s.speaker_id)
                    .unwrap_or(0);

                replacements.push(super::diarize::MergedSegment {
                    t0: *t0,
                    t1: *t1,
                    text: text.clone(),
                    speaker_id,
                    confidence: *confidence,
                });
            }

            // Replace the block in merged
            merged.splice(block.start_idx..block.end_idx, replacements);

            eprintln!(
                "    Replaced: confidence {:.2} -> {:.2} (+{:.2})",
                block.avg_confidence, new_avg_conf, improvement
            );
        } else {
            eprintln!(
                "    Kept original: new confidence {:.2} <= original {:.2}",
                new_avg_conf, block.avg_confidence
            );
        }
    }

    if replaced_count > 0 {
        eprintln!(
            "Re-transcription: replaced {}/{} block(s), avg improvement +{:.2}",
            replaced_count,
            blocks.len(),
            if replaced_count > 0 { total_improvement / replaced_count as f64 } else { 0.0 }
        );
    }

    Ok(())
}

/// Run adaptive diarization: try full audio first, cascade to smaller chunks if quality is poor.
#[cfg(feature = "diarize")]
fn run_adaptive_diarization(
    samples: &[f32],
    max_speakers: usize,
    expected_min: usize,
    cache_dir: Option<&str>,
) -> Result<Vec<super::diarize::DiarizedSegment>> {
    use super::diarize;
    use super::audio;

    // Maximum fraction of total duration a single speaker block may span.
    // If any block exceeds this, the diarization missed speaker changes and we cascade.
    const MAX_SPAN_RATIO: f64 = 0.6;

    let is_good = |segs: &[diarize::DiarizedSegment]| -> bool {
        if !diarize::quality_ok(segs, expected_min) {
            return false;
        }
        let ratio = diarize::max_speaker_span_ratio(segs);
        if ratio > MAX_SPAN_RATIO {
            eprintln!(
                "Diarization span check failed: largest block is {:.0}% of audio (max {:.0}%)",
                ratio * 100.0,
                MAX_SPAN_RATIO * 100.0,
            );
            return false;
        }
        true
    };

    // Try full audio first
    eprintln!("Attempting full-audio diarization...");
    let result = diarize::diarize(samples, 16000, max_speakers, cache_dir)?;

    if is_good(&result) {
        eprintln!("Full-audio diarization quality OK");
        return Ok(result);
    }

    // Cascade through chunk sizes
    for &chunk_secs in CHUNK_CASCADE {
        let duration_secs = samples.len() as f64 / 16000.0;
        if chunk_secs >= duration_secs {
            continue; // Chunk size >= total duration, skip
        }

        eprintln!(
            "Full-audio quality poor, trying {:.0}s chunks...",
            chunk_secs,
        );
        let chunks = audio::chunk_audio(samples, chunk_secs, 16000);
        let result = diarize::diarize_chunked(&chunks, 16000, max_speakers, cache_dir)?;

        if is_good(&result) {
            eprintln!("Chunked diarization ({:.0}s) quality OK", chunk_secs);
            return Ok(result);
        }
    }

    // Fall back to smallest chunk result even if quality isn't great
    let smallest = CHUNK_CASCADE.last().copied().unwrap_or(30.0);
    eprintln!(
        "Using {:.0}s chunks (best available quality)",
        smallest,
    );
    let chunks = super::audio::chunk_audio(samples, smallest, 16000);
    diarize::diarize_chunked(&chunks, 16000, max_speakers, cache_dir)
}

/// Format merged segments into markdown output.
#[cfg(feature = "diarize")]
fn format_merged_output(
    merged: &[super::diarize::MergedSegment],
    speaker_labels: &std::collections::HashMap<usize, String>,
    duration_secs: f64,
) -> Result<String> {
    let mut text = String::new();

    // YAML frontmatter
    text.push_str("---\n");
    text.push_str(&format!("date: {}\n", chrono::Local::now().format("%Y-%m-%d")));
    text.push_str("type: phone-call\n");
    text.push_str("participants:\n");
    let mut participant_names: Vec<&String> = speaker_labels.values().collect();
    participant_names.sort();
    for name in &participant_names {
        text.push_str(&format!("  - {}\n", name));
    }
    let mins = (duration_secs / 60.0).floor() as u32;
    let secs = (duration_secs % 60.0).floor() as u32;
    text.push_str(&format!("duration: \"{}:{:02}\"\n", mins, secs));
    text.push_str("---\n\n");

    // Group consecutive segments by speaker into blocks
    let mut current_speaker: Option<usize> = None;
    let mut block_start: i64 = 0;
    let mut block_end: i64 = 0;
    let mut block_text = String::new();
    let mut block_confidence_sum: f64 = 0.0;
    let mut block_confidence_count: usize = 0;

    for (i, seg) in merged.iter().enumerate() {
        let speaker_changed = current_speaker != Some(seg.speaker_id);
        let is_last = i == merged.len() - 1;

        if speaker_changed && !block_text.is_empty() {
            // Clean repetition hallucinations from merged speaker block
            let cleaned_block = collapse_repeated_phrases(&block_text);
            let speaker_name = current_speaker
                .and_then(|id| speaker_labels.get(&id))
                .map(|s| s.as_str())
                .unwrap_or("Unknown");
            let avg_conf = if block_confidence_count > 0 {
                block_confidence_sum / block_confidence_count as f64
            } else {
                0.0
            };
            text.push_str(&format!(
                "**{}** ({:.2}) [{} → {}]\n{}\n\n",
                speaker_name,
                avg_conf,
                format_timestamp(block_start),
                format_timestamp(block_end),
                cleaned_block.trim()
            ));
            block_text.clear();
            block_confidence_sum = 0.0;
            block_confidence_count = 0;
        }

        if speaker_changed {
            current_speaker = Some(seg.speaker_id);
            block_start = seg.t0;
        }
        block_end = seg.t1;
        block_confidence_sum += seg.confidence as f64;
        block_confidence_count += 1;

        let trimmed = seg.text.trim();
        if !trimmed.is_empty() {
            if !block_text.is_empty() {
                block_text.push(' ');
            }
            block_text.push_str(trimmed);
        }

        if is_last && !block_text.is_empty() {
            let cleaned_block = collapse_repeated_phrases(&block_text);
            let speaker_name = current_speaker
                .and_then(|id| speaker_labels.get(&id))
                .map(|s| s.as_str())
                .unwrap_or("Unknown");
            let avg_conf = if block_confidence_count > 0 {
                block_confidence_sum / block_confidence_count as f64
            } else {
                0.0
            };
            text.push_str(&format!(
                "**{}** ({:.2}) [{} → {}]\n{}\n\n",
                speaker_name,
                avg_conf,
                format_timestamp(block_start),
                format_timestamp(block_end),
                cleaned_block.trim()
            ));
        }
    }

    Ok(text)
}

/// Format centisecond timestamp as HH:MM:SS.mmm
fn format_timestamp(cs: i64) -> String {
    let total_ms = cs * 10;
    let hours = total_ms / 3_600_000;
    let minutes = (total_ms % 3_600_000) / 60_000;
    let seconds = (total_ms % 60_000) / 1_000;
    let ms = total_ms % 1_000;
    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_formatting() {
        assert_eq!(format_timestamp(0), "00:00:00.000");
        assert_eq!(format_timestamp(100), "00:00:01.000");
        assert_eq!(format_timestamp(6000), "00:01:00.000");
        assert_eq!(format_timestamp(360000), "01:00:00.000");
        assert_eq!(format_timestamp(365432), "01:00:54.320");
    }

    #[cfg(feature = "diarize")]
    mod confidence_tests {
        use super::super::*;
        use crate::transcribe::diarize::MergedSegment;

        fn seg(t0: i64, t1: i64, confidence: f32) -> MergedSegment {
            MergedSegment {
                t0,
                t1,
                text: "test".to_string(),
                speaker_id: 1,
                confidence,
            }
        }

        #[test]
        fn no_blocks_when_all_high_confidence() {
            let segs = vec![seg(0, 100, 0.9), seg(100, 200, 0.8), seg(200, 300, 0.7)];
            let blocks = find_low_confidence_blocks(&segs, 0.4);
            assert!(blocks.is_empty());
        }

        #[test]
        fn single_low_confidence_block() {
            let segs = vec![
                seg(0, 100, 0.9),
                seg(100, 200, 0.2),
                seg(200, 300, 0.3),
                seg(300, 400, 0.85),
            ];
            let blocks = find_low_confidence_blocks(&segs, 0.4);
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0].start_idx, 1);
            assert_eq!(blocks[0].end_idx, 3);
            assert!((blocks[0].avg_confidence - 0.25).abs() < 0.01);
        }

        #[test]
        fn multiple_low_confidence_blocks() {
            let segs = vec![
                seg(0, 100, 0.1),   // low
                seg(100, 200, 0.2), // low
                seg(200, 300, 0.9), // high
                seg(300, 400, 0.95),// high
                seg(400, 500, 0.1), // low
            ];
            let blocks = find_low_confidence_blocks(&segs, 0.4);
            assert_eq!(blocks.len(), 2);
            assert_eq!(blocks[0].start_idx, 0);
            assert_eq!(blocks[0].end_idx, 2);
            assert_eq!(blocks[1].start_idx, 4);
            assert_eq!(blocks[1].end_idx, 5);
        }

        #[test]
        fn empty_segments_no_blocks() {
            let blocks = find_low_confidence_blocks(&[], 0.4);
            assert!(blocks.is_empty());
        }

        #[test]
        fn all_low_confidence_single_block() {
            let segs = vec![seg(0, 100, 0.1), seg(100, 200, 0.2), seg(200, 300, 0.15)];
            let blocks = find_low_confidence_blocks(&segs, 0.4);
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0].start_idx, 0);
            assert_eq!(blocks[0].end_idx, 3);
        }

        #[test]
        fn gap_tolerance_bridges_single_high_segment() {
            // A single high-confidence segment between two low ones should be bridged
            let segs = vec![
                seg(0, 100, 0.1),   // low
                seg(100, 200, 0.5), // high (but sandwiched)
                seg(200, 300, 0.2), // low
            ];
            let blocks = find_low_confidence_blocks(&segs, 0.4);
            // The gap tolerance allows bridging when the next segment after the high one is low
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0].start_idx, 0);
            assert_eq!(blocks[0].end_idx, 3);
        }
    }
}
