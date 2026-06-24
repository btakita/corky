//! Social media draft parsing and rendering (YAML frontmatter).

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::platform::Platform;

/// Status of a social draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftStatus {
    Draft,
    Ready,
    Published,
}

impl DraftStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DraftStatus::Draft => "draft",
            DraftStatus::Ready => "ready",
            DraftStatus::Published => "published",
        }
    }
}

impl std::fmt::Display for DraftStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DraftStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "draft" => Ok(DraftStatus::Draft),
            "ready" => Ok(DraftStatus::Ready),
            "published" => Ok(DraftStatus::Published),
            _ => bail!("Invalid status '{}'. Valid: draft, ready, published", s),
        }
    }
}

/// YAML frontmatter metadata for a social draft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialDraftMeta {
    pub platform: Platform,
    pub author: String,
    #[serde(default = "default_visibility")]
    pub visibility: String,
    #[serde(default = "default_status")]
    pub status: DraftStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_comment: Option<String>,
}

fn default_visibility() -> String {
    "public".to_string()
}

fn default_status() -> DraftStatus {
    DraftStatus::Draft
}

/// A social draft: metadata + body text.
#[derive(Debug, Clone)]
pub struct SocialDraft {
    pub meta: SocialDraftMeta,
    pub body: String,
}

impl SocialDraft {
    /// Parse a social draft from file content (YAML frontmatter + body).
    pub fn parse(content: &str) -> Result<Self> {
        let content = content.trim_start_matches('\u{feff}'); // Strip BOM
        if !content.starts_with("---") {
            bail!("Missing YAML frontmatter delimiter `---` at start of file");
        }

        let after_first = &content[3..];
        let end = after_first
            .find("\n---")
            .ok_or_else(|| anyhow::anyhow!("Missing closing YAML frontmatter delimiter `---`"))?;

        let yaml_str = &after_first[..end];
        let body_start = end + 4; // skip \n---
        let body = if body_start < after_first.len() {
            after_first[body_start..]
                .trim_start_matches('\n')
                .to_string()
        } else {
            String::new()
        };

        let meta: SocialDraftMeta = serde_yaml::from_str(yaml_str)?;
        Ok(SocialDraft { meta, body })
    }

    /// Render the draft back to file content.
    pub fn render(&self) -> Result<String> {
        let yaml = serde_yaml::to_string(&self.meta)?;
        Ok(format!("---\n{}---\n{}", yaml, self.body))
    }

    /// Update the metadata, preserving the body.
    pub fn update_meta(&mut self, meta: SocialDraftMeta) {
        self.meta = meta;
    }

    /// Create a new draft with the given metadata and body.
    pub fn new(meta: SocialDraftMeta, body: String) -> Self {
        SocialDraft { meta, body }
    }
}
