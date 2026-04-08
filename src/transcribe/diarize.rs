//! Speaker diarization using pyannote-rs (ONNX Runtime).
//!
//! Requires the `diarize` feature flag.

use anyhow::Result;
use pyannote_rs::{EmbeddingExtractor, EmbeddingManager};
use std::collections::HashMap;

use super::model;

/// A labeled speaker segment from diarization.
#[derive(Debug, Clone)]
pub struct DiarizedSegment {
    pub start: f64,
    pub end: f64,
    pub speaker_id: usize,
    /// Cosine similarity confidence (0.0–1.0). 0.0 for segments assigned by proximity.
    pub confidence: f32,
}

/// ONNX model filenames (from pyannote-rs releases).
const SEGMENTATION_MODEL: &str = "segmentation-3.0.onnx";
const EMBEDDING_MODEL: &str = "wespeaker_en_voxceleb_CAM++.onnx";

/// Convert f32 audio samples ([-1.0, 1.0]) to i16 (pyannote-rs expects i16).
fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect()
}

/// Cosine similarity between two embedding vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Compute confidence (best cosine similarity) of an embedding against known speakers.
fn compute_confidence(embedding: &[f32], manager: &EmbeddingManager) -> (usize, f32) {
    let speakers = manager.get_all_speakers();
    let mut best_id = 0usize;
    let mut best_sim = 0.0f32;
    for (&id, stored) in speakers {
        let sim = cosine_similarity(embedding, stored.as_slice().unwrap_or(&[]));
        if sim > best_sim {
            best_sim = sim;
            best_id = id;
        }
    }
    (best_id, best_sim)
}

/// Run speaker diarization on audio samples.
///
/// Returns segments labeled with speaker IDs (1-based).
pub fn diarize(
    samples_f32: &[f32],
    sample_rate: u32,
    max_speakers: usize,
    cache_dir: Option<&str>,
) -> Result<Vec<DiarizedSegment>> {
    let samples_i16 = f32_to_i16(samples_f32);

    // Resolve ONNX model paths (download if needed)
    let seg_model = model::resolve_onnx_model(SEGMENTATION_MODEL, cache_dir)?;
    let emb_model = model::resolve_onnx_model(EMBEDDING_MODEL, cache_dir)?;

    eprintln!("Running speaker segmentation...");
    let segments = pyannote_rs::get_segments(&samples_i16, sample_rate, &seg_model)
        .map_err(|e| anyhow::anyhow!("Segmentation failed: {:?}", e))?;

    eprintln!("Extracting speaker embeddings...");
    let mut extractor = EmbeddingExtractor::new(&emb_model)
        .map_err(|e| anyhow::anyhow!("Failed to load embedding model: {:?}", e))?;
    let mut manager = EmbeddingManager::new(max_speakers);
    let threshold = 0.5;

    let mut result = Vec::new();

    for segment in segments {
        let segment = segment.map_err(|e| anyhow::anyhow!("Segment error: {:?}", e))?;
        match extractor.compute(&segment.samples) {
            Ok(embedding) => {
                let embedding_vec: Vec<f32> = embedding.collect();
                let speaker_id =
                    if manager.get_all_speakers().len() == max_speakers {
                        manager
                            .get_best_speaker_match(embedding_vec.clone())
                            .map_err(|e| anyhow::anyhow!("Speaker match error: {:?}", e))
                            .unwrap_or(0)
                    } else {
                        manager
                            .search_speaker(embedding_vec.clone(), threshold)
                            .unwrap_or(0)
                    };

                // Compute confidence score against stored speaker embeddings
                let (_, confidence) = compute_confidence(&embedding_vec, &manager);

                result.push(DiarizedSegment {
                    start: segment.start,
                    end: segment.end,
                    speaker_id,
                    confidence,
                });
            }
            Err(_) => {
                // Segment too short for embedding — mark for post-processing
                result.push(DiarizedSegment {
                    start: segment.start,
                    end: segment.end,
                    speaker_id: 0,
                    confidence: 0.0,
                });
            }
        }
    }

    // Post-process: reassign Unknown (speaker_id 0) segments to nearest speaker
    // by temporal proximity. There is no valid "Unknown" when speaker count is known.
    reassign_unknown_segments(&mut result);

    let unique_speakers: std::collections::HashSet<usize> =
        result.iter().map(|s| s.speaker_id).filter(|&id| id != 0).collect();
    eprintln!(
        "Diarization complete: {} segments, {} speakers detected",
        result.len(),
        unique_speakers.len()
    );

    Ok(result)
}

/// Reassign Unknown (speaker_id 0) segments to the nearest known speaker by time.
fn reassign_unknown_segments(segments: &mut [DiarizedSegment]) {
    let len = segments.len();
    for i in 0..len {
        if segments[i].speaker_id != 0 {
            continue;
        }
        let mid = (segments[i].start + segments[i].end) / 2.0;

        // Search backward for nearest known speaker
        let mut prev_id = 0usize;
        let mut prev_dist = f64::MAX;
        for j in (0..i).rev() {
            if segments[j].speaker_id != 0 {
                prev_id = segments[j].speaker_id;
                prev_dist = (mid - segments[j].end).abs();
                break;
            }
        }

        // Search forward for nearest known speaker
        let mut next_id = 0usize;
        let mut next_dist = f64::MAX;
        for seg in &segments[(i + 1)..] {
            if seg.speaker_id != 0 {
                next_id = seg.speaker_id;
                next_dist = (seg.start - mid).abs();
                break;
            }
        }

        // Assign to nearest neighbor (prefer previous on tie)
        let assigned = if prev_id != 0 && (prev_dist <= next_dist || next_id == 0) {
            prev_id
        } else if next_id != 0 {
            next_id
        } else {
            continue; // No known speakers at all — leave as 0
        };

        segments[i].speaker_id = assigned;
        // confidence stays 0.0 to indicate proximity-assigned
    }
}

/// Diarize audio in chunks, then merge speaker IDs across chunks using embedding similarity.
///
/// Each chunk is diarized independently, then speaker IDs are unified across chunks
/// by comparing representative embeddings.
pub fn diarize_chunked(
    chunks: &[(f64, Vec<f32>)],
    sample_rate: u32,
    max_speakers: usize,
    cache_dir: Option<&str>,
) -> Result<Vec<DiarizedSegment>> {
    let seg_model = model::resolve_onnx_model(SEGMENTATION_MODEL, cache_dir)?;
    let emb_model = model::resolve_onnx_model(EMBEDDING_MODEL, cache_dir)?;

    // Per-chunk: diarized segments + representative embeddings per speaker
    let mut all_segments: Vec<Vec<DiarizedSegment>> = Vec::new();
    let mut chunk_embeddings: Vec<HashMap<usize, Vec<f32>>> = Vec::new();

    for (chunk_idx, (offset, samples)) in chunks.iter().enumerate() {
        eprintln!(
            "Diarizing chunk {}/{} (offset {:.0}s, {:.1}s)...",
            chunk_idx + 1,
            chunks.len(),
            offset,
            samples.len() as f64 / sample_rate as f64,
        );

        let samples_i16 = f32_to_i16(samples);
        let segments = pyannote_rs::get_segments(&samples_i16, sample_rate, &seg_model)
            .map_err(|e| anyhow::anyhow!("Segmentation failed on chunk {}: {:?}", chunk_idx, e))?;

        let mut extractor = EmbeddingExtractor::new(&emb_model)
            .map_err(|e| anyhow::anyhow!("Failed to load embedding model: {:?}", e))?;
        let mut manager = EmbeddingManager::new(max_speakers);
        let threshold = 0.5;

        let mut chunk_segs = Vec::new();
        let mut embeddings_by_speaker: HashMap<usize, Vec<f32>> = HashMap::new();

        for segment in segments {
            let segment = segment.map_err(|e| anyhow::anyhow!("Segment error: {:?}", e))?;
            match extractor.compute(&segment.samples) {
                Ok(embedding) => {
                    let embedding_vec: Vec<f32> = embedding.collect();
                    let speaker_id =
                        if manager.get_all_speakers().len() == max_speakers {
                            manager
                                .get_best_speaker_match(embedding_vec.clone())
                                .map_err(|e| anyhow::anyhow!("Speaker match error: {:?}", e))
                                .unwrap_or(0)
                        } else {
                            manager
                                .search_speaker(embedding_vec.clone(), threshold)
                                .unwrap_or(0)
                        };

                    let (_, confidence) = compute_confidence(&embedding_vec, &manager);

                    // Store first embedding per speaker as representative
                    embeddings_by_speaker
                        .entry(speaker_id)
                        .or_insert_with(|| embedding_vec.clone());

                    chunk_segs.push(DiarizedSegment {
                        start: segment.start + offset,
                        end: segment.end + offset,
                        speaker_id,
                        confidence,
                    });
                }
                Err(_) => {
                    chunk_segs.push(DiarizedSegment {
                        start: segment.start + offset,
                        end: segment.end + offset,
                        speaker_id: 0,
                        confidence: 0.0,
                    });
                }
            }
        }

        reassign_unknown_segments(&mut chunk_segs);
        all_segments.push(chunk_segs);
        chunk_embeddings.push(embeddings_by_speaker);
    }

    // Merge speaker IDs across chunks using embedding similarity
    let merged = merge_cross_chunk_speakers(all_segments, &chunk_embeddings, max_speakers);

    let unique_speakers: std::collections::HashSet<usize> =
        merged.iter().map(|s| s.speaker_id).filter(|&id| id != 0).collect();
    eprintln!(
        "Chunked diarization complete: {} segments, {} speakers across {} chunks",
        merged.len(),
        unique_speakers.len(),
        chunks.len()
    );

    Ok(merged)
}

/// Merge speaker IDs across chunks by matching representative embeddings.
///
/// Speakers in chunk 0 keep their IDs. For subsequent chunks, each speaker's
/// representative embedding is compared against all known speakers. If a match
/// exceeds threshold, the ID is remapped; otherwise a new global ID is assigned.
fn merge_cross_chunk_speakers(
    chunk_segments: Vec<Vec<DiarizedSegment>>,
    chunk_embeddings: &[HashMap<usize, Vec<f32>>],
    max_speakers: usize,
) -> Vec<DiarizedSegment> {
    let threshold = 0.5;

    // Global speaker embeddings: global_id -> embedding
    let mut global_embeddings: HashMap<usize, Vec<f32>> = HashMap::new();
    let mut next_global_id: usize = 1;

    // Per-chunk ID remapping: chunk_local_id -> global_id
    let mut all_remaps: Vec<HashMap<usize, usize>> = Vec::new();

    for embs in chunk_embeddings {
        let mut remap: HashMap<usize, usize> = HashMap::new();

        for (&local_id, embedding) in embs {
            if local_id == 0 {
                continue;
            }

            // Find best matching global speaker
            let mut best_global = 0usize;
            let mut best_sim = 0.0f32;
            for (&gid, gemb) in &global_embeddings {
                let sim = cosine_similarity(embedding, gemb);
                if sim > best_sim {
                    best_sim = sim;
                    best_global = gid;
                }
            }

            if best_sim >= threshold && best_global != 0 {
                remap.insert(local_id, best_global);
            } else if global_embeddings.len() < max_speakers {
                // New speaker (only if under max_speakers limit)
                let gid = next_global_id;
                next_global_id += 1;
                global_embeddings.insert(gid, embedding.clone());
                remap.insert(local_id, gid);
            } else {
                // Max speakers reached — force-assign to best match
                if best_global != 0 {
                    remap.insert(local_id, best_global);
                } else if let Some(&fallback) = global_embeddings.keys().next() {
                    remap.insert(local_id, fallback);
                }
            }
        }

        all_remaps.push(remap);
    }

    // Apply remapping
    let mut result = Vec::new();
    for (chunk_idx, segments) in chunk_segments.into_iter().enumerate() {
        let remap = &all_remaps[chunk_idx];
        for mut seg in segments {
            if let Some(&global_id) = remap.get(&seg.speaker_id) {
                seg.speaker_id = global_id;
            }
            result.push(seg);
        }
    }

    result
}

/// Evaluate diarization quality. Returns true if the result is acceptable.
pub fn quality_ok(segments: &[DiarizedSegment], expected_min_speakers: usize) -> bool {
    if segments.is_empty() {
        return false;
    }

    let unique: std::collections::HashSet<usize> = segments
        .iter()
        .map(|s| s.speaker_id)
        .filter(|&id| id != 0)
        .collect();

    // Fail if fewer speakers than expected
    if unique.len() < expected_min_speakers && expected_min_speakers > 1 {
        return false;
    }

    // Fail if >30% unknown
    let unknown_count = segments.iter().filter(|s| s.speaker_id == 0).count();
    let unknown_ratio = unknown_count as f64 / segments.len() as f64;
    if unknown_ratio > 0.3 {
        return false;
    }

    true
}

/// Return the largest contiguous same-speaker block as a fraction of total covered duration.
///
/// Groups consecutive segments with the same non-zero speaker_id and finds the longest
/// run. Returns 0.0 if segments is empty or total duration is zero.
pub fn max_speaker_span_ratio(segments: &[DiarizedSegment]) -> f64 {
    if segments.is_empty() {
        return 0.0;
    }

    let total = segments.last().map(|s| s.end).unwrap_or(0.0)
        - segments.first().map(|s| s.start).unwrap_or(0.0);
    if total <= 0.0 {
        return 0.0;
    }

    let mut max_span = 0.0f64;
    let mut block_start: Option<f64> = None;
    let mut block_end = 0.0f64;
    let mut current_speaker = 0usize;

    for seg in segments {
        if seg.speaker_id == 0 {
            // Unknown segment — end current block
            if let Some(bs) = block_start.take() {
                max_span = max_span.max(block_end - bs);
            }
            current_speaker = 0;
            continue;
        }

        if seg.speaker_id == current_speaker {
            block_end = seg.end;
        } else {
            if let Some(bs) = block_start.take() {
                max_span = max_span.max(block_end - bs);
            }
            current_speaker = seg.speaker_id;
            block_start = Some(seg.start);
            block_end = seg.end;
        }
    }
    if let Some(bs) = block_start {
        max_span = max_span.max(block_end - bs);
    }

    max_span / total
}

/// A merged segment: whisper text + speaker ID + confidence.
#[derive(Debug, Clone)]
pub struct MergedSegment {
    pub t0: i64,
    pub t1: i64,
    pub text: String,
    pub speaker_id: usize,
    pub confidence: f32,
}

/// Merge whisper transcript segments with diarization speaker labels.
///
/// For each whisper segment, find the diarized speaker whose segment overlaps most.
pub fn merge_speakers(
    whisper_segments: &[(i64, i64, String)],
    diarized: &[DiarizedSegment],
    sample_rate: u32,
) -> Vec<MergedSegment> {
    whisper_segments
        .iter()
        .map(|(t0, t1, text)| {
            // Convert whisper centisecond timestamps to seconds
            let w_start = *t0 as f64 * 0.01;
            let w_end = *t1 as f64 * 0.01;

            // Find diarized segment with most overlap + its confidence
            let (speaker, confidence) =
                best_overlapping_speaker(w_start, w_end, diarized, sample_rate);
            MergedSegment {
                t0: *t0,
                t1: *t1,
                text: text.clone(),
                speaker_id: speaker,
                confidence,
            }
        })
        .collect()
}

/// Find the speaker ID with the most temporal overlap for a given time range.
/// Returns (speaker_id, weighted_confidence).
/// Falls back to nearest segment by temporal proximity when no overlap exists.
fn best_overlapping_speaker(
    start: f64,
    end: f64,
    diarized: &[DiarizedSegment],
    _sample_rate: u32,
) -> (usize, f32) {
    let mut speaker_overlap: HashMap<usize, f64> = HashMap::new();
    let mut speaker_conf_sum: HashMap<usize, f64> = HashMap::new();

    for seg in diarized {
        let overlap_start = start.max(seg.start);
        let overlap_end = end.min(seg.end);
        let overlap = (overlap_end - overlap_start).max(0.0);
        if overlap > 0.0 {
            *speaker_overlap.entry(seg.speaker_id).or_default() += overlap;
            *speaker_conf_sum.entry(seg.speaker_id).or_default() +=
                overlap * seg.confidence as f64;
        }
    }

    if let Some((&id, &total_overlap)) = speaker_overlap
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        let weighted_conf = if total_overlap > 0.0 {
            (speaker_conf_sum.get(&id).copied().unwrap_or(0.0) / total_overlap) as f32
        } else {
            0.0
        };
        return (id, weighted_conf);
    }

    // No overlap — fall back to nearest diarized segment by temporal proximity
    let mid = (start + end) / 2.0;
    diarized
        .iter()
        .filter(|seg| seg.speaker_id != 0)
        .min_by(|a, b| {
            let dist_a = (mid - (a.start + a.end) / 2.0).abs();
            let dist_b = (mid - (b.start + b.end) / 2.0).abs();
            dist_a.partial_cmp(&dist_b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|seg| (seg.speaker_id, 0.0)) // 0.0 confidence for proximity fallback
        .unwrap_or((0, 0.0))
}

/// Get representative text excerpts for each speaker (for interactive labeling).
pub fn get_speaker_excerpts(
    merged: &[MergedSegment],
) -> HashMap<usize, Vec<String>> {
    let mut excerpts: HashMap<usize, Vec<String>> = HashMap::new();

    for seg in merged {
        let trimmed = seg.text.trim();
        if trimmed.is_empty() || seg.speaker_id == 0 {
            continue;
        }
        let entry = excerpts.entry(seg.speaker_id).or_default();
        if entry.len() < 3 {
            entry.push(trimmed.to_string());
        }
    }

    excerpts
}

/// Prompt the user to assign names to speaker IDs via stdin.
pub fn interactive_label(
    excerpts: &HashMap<usize, Vec<String>>,
) -> Result<HashMap<usize, String>> {
    use std::io::{self, BufRead, Write};

    let mut labels = HashMap::new();
    let mut speaker_ids: Vec<usize> = excerpts.keys().copied().collect();
    speaker_ids.sort();

    eprintln!("\n--- Speaker Identification ---");
    for &id in &speaker_ids {
        eprintln!("\nSpeaker {} excerpts:", id);
        if let Some(texts) = excerpts.get(&id) {
            for (i, text) in texts.iter().enumerate() {
                let preview = if text.len() > 120 { &text[..120] } else { text };
                eprintln!("  {}. \"{}\"", i + 1, preview);
            }
        }
        eprint!("Who is Speaker {}? (name or enter to skip): ", id);
        io::stderr().flush()?;

        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        let name = line.trim().to_string();
        if !name.is_empty() {
            labels.insert(id, name);
        } else {
            labels.insert(id, format!("Speaker {}", id));
        }
    }
    eprintln!("---\n");

    Ok(labels)
}
