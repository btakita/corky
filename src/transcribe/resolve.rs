//! LLM-based speaker attribution for Unknown segments.
//!
//! Uses Claude API to attribute segments with speaker_id == 0 to known speakers
//! based on transcript context.

use anyhow::Result;
use super::diarize::MergedSegment;

/// Resolve Unknown (speaker_id 0) segments using Claude API.
///
/// Sends a batched prompt with surrounding context for each Unknown segment.
/// Updates speaker_id in-place on the merged segments.
pub fn resolve_unknown_speakers(
    merged: &mut [MergedSegment],
    speaker_labels: &std::collections::HashMap<usize, String>,
) -> Result<()> {
    // Collect indices of Unknown segments
    let unknown_indices: Vec<usize> = merged
        .iter()
        .enumerate()
        .filter(|(_, seg)| seg.speaker_id == 0)
        .map(|(i, _)| i)
        .collect();

    if unknown_indices.is_empty() {
        return Ok(());
    }

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| anyhow::anyhow!(
            "ANTHROPIC_API_KEY not set. Required for --resolve-unknown.\n\
             Set it or use --no-resolve-unknown to skip."
        ))?;

    eprintln!(
        "Resolving {} unknown segment(s) via Claude API...",
        unknown_indices.len()
    );

    // Build speaker reference
    let mut speaker_ref = String::new();
    for (id, name) in speaker_labels {
        // Find example text for this speaker
        let examples: Vec<&str> = merged
            .iter()
            .filter(|s| s.speaker_id == *id && !s.text.trim().is_empty())
            .take(3)
            .map(|s| s.text.trim())
            .collect();
        if !examples.is_empty() {
            speaker_ref.push_str(&format!(
                "- {} (examples: {})\n",
                name,
                examples.join(" | ")
            ));
        }
    }

    // Build unknown segments list
    let mut unknown_list = String::new();
    for (batch_idx, &seg_idx) in unknown_indices.iter().enumerate() {
        let seg = &merged[seg_idx];
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }

        // Context: up to 2 segments before and after
        let ctx_before: Vec<String> = merged[seg_idx.saturating_sub(2)..seg_idx]
            .iter()
            .filter(|s| s.speaker_id != 0)
            .map(|s| {
                let name = speaker_labels
                    .get(&s.speaker_id)
                    .map(|n| n.as_str())
                    .unwrap_or("Unknown");
                format!("[{}]: {}", name, s.text.trim())
            })
            .collect();
        let ctx_after: Vec<String> = merged[seg_idx + 1..std::cmp::min(seg_idx + 3, merged.len())]
            .iter()
            .filter(|s| s.speaker_id != 0)
            .map(|s| {
                let name = speaker_labels
                    .get(&s.speaker_id)
                    .map(|n| n.as_str())
                    .unwrap_or("Unknown");
                format!("[{}]: {}", name, s.text.trim())
            })
            .collect();

        unknown_list.push_str(&format!(
            "#{}: \"{}\"\n  Before: {}\n  After: {}\n\n",
            batch_idx + 1,
            text,
            if ctx_before.is_empty() { "(start of transcript)".to_string() } else { ctx_before.join(" → ") },
            if ctx_after.is_empty() { "(end of transcript)".to_string() } else { ctx_after.join(" → ") },
        ));
    }

    if unknown_list.is_empty() {
        return Ok(()); // All unknown segments were empty text
    }

    let prompt = format!(
        "You are analyzing a transcript with speaker diarization. Some segments could not be \
         attributed to a speaker. Based on context, attribute each unknown segment to the most \
         likely speaker.\n\n\
         Known speakers:\n{}\n\
         Unknown segments (with surrounding context):\n{}\n\
         For each numbered segment, respond with ONLY the speaker name, one per line.\n\
         Format: #N: Speaker Name\n\
         If genuinely uncertain, respond: #N: Unknown",
        speaker_ref, unknown_list
    );

    // Call Claude API via ureq
    let body = serde_json::json!({
        "model": "claude-sonnet-4-20250514",
        "max_tokens": 1024,
        "messages": [
            { "role": "user", "content": prompt }
        ]
    });

    let resp = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", &api_key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("Claude API request failed: {}", e))?;

    let resp_body: serde_json::Value = resp
        .into_json()
        .map_err(|e| anyhow::anyhow!("Failed to parse Claude API response: {}", e))?;

    let content = resp_body["content"][0]["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Unexpected Claude API response format"))?;

    // Parse response: "#N: Speaker Name" lines
    let reverse_labels: std::collections::HashMap<&str, usize> = speaker_labels
        .iter()
        .map(|(id, name)| (name.as_str(), *id))
        .collect();

    let mut resolved_count = 0;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('#')
            && let Some((num_str, name)) = rest.split_once(':')
        {
            let num_str = num_str.trim();
            let name = name.trim();
            if name == "Unknown" {
                continue;
            }
            if let Ok(batch_idx) = num_str.parse::<usize>()
                && batch_idx >= 1
                && batch_idx <= unknown_indices.len()
            {
                let seg_idx = unknown_indices[batch_idx - 1];
                // Find speaker ID by name (case-insensitive match)
                let matched_id = reverse_labels
                    .get(name)
                    .copied()
                    .or_else(|| {
                        let name_lower = name.to_lowercase();
                        reverse_labels
                            .iter()
                            .find(|(k, _)| k.to_lowercase() == name_lower)
                            .map(|(_, &id)| id)
                    });
                if let Some(id) = matched_id {
                    merged[seg_idx].speaker_id = id;
                    merged[seg_idx].confidence = 0.0;
                    resolved_count += 1;
                }
            }
        }
    }

    eprintln!(
        "Resolved {}/{} unknown segments via Claude API",
        resolved_count,
        unknown_indices.len()
    );

    Ok(())
}
