//! Social media draft parsing and rendering (YAML frontmatter).

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::platform::Platform;

/// Status of a social draft — the publish lifecycle state machine.
///
/// `Publishing` is a crash-safe transient marker persisted *before* the platform
/// API call so an interrupted publish reconciles by `post_id` on retry instead of
/// re-creating (double-posting) the post. `Published`/`Failed` are terminal: the
/// scheduler skips them (see [`DraftStatus::is_terminal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftStatus {
    Draft,
    Ready,
    Publishing,
    Published,
    Failed,
}

/// Events that drive the [`DraftStatus`] publish state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishEvent {
    /// A manual or scheduled publish attempt begins (persist `Publishing`).
    Start,
    /// The platform API created the post (or a dry-run finished) → `Published`.
    Succeeded,
    /// The platform API call failed → `Failed`.
    Failed,
}

impl DraftStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DraftStatus::Draft => "draft",
            DraftStatus::Ready => "ready",
            DraftStatus::Publishing => "publishing",
            DraftStatus::Published => "published",
            DraftStatus::Failed => "failed",
        }
    }

    /// Terminal states the scheduler must never auto-publish.
    pub fn is_terminal(self) -> bool {
        matches!(self, DraftStatus::Published | DraftStatus::Failed)
    }

    /// PB1 gate: whether a publish attempt may begin from this state.
    ///
    /// `ready` folds in PB1's "status is Ready, or `scheduled_at` is set, or this
    /// is a dry-run" — any of which makes a `Draft` publishable. `Publishing` is
    /// allowed so an interrupted attempt can be resumed/reconciled in [`publish`].
    ///
    /// [`publish`]: super::publish::publish
    pub fn can_publish(self, ready: bool) -> Result<()> {
        match self {
            DraftStatus::Published => bail!(
                "Draft has already been published. Nothing to do."
            ),
            DraftStatus::Failed => bail!(
                "A previous publish attempt failed.\n\
                 Set status to 'ready' to retry."
            ),
            DraftStatus::Publishing => Ok(()),
            DraftStatus::Ready => Ok(()),
            DraftStatus::Draft if ready => Ok(()),
            DraftStatus::Draft => bail!(
                "Draft is not ready for publishing (status: draft).\n\
                 Set status to 'ready' or add scheduled_at to the frontmatter."
            ),
        }
    }

    /// Advance the lifecycle for a [`PublishEvent`], rejecting invalid transitions.
    pub fn transition(self, event: PublishEvent) -> Result<DraftStatus> {
        let next = match (self, event) {
            (DraftStatus::Draft | DraftStatus::Ready | DraftStatus::Publishing, PublishEvent::Start) => {
                DraftStatus::Publishing
            }
            (DraftStatus::Publishing, PublishEvent::Succeeded) => DraftStatus::Published,
            (DraftStatus::Publishing, PublishEvent::Failed) => DraftStatus::Failed,
            (from, ev) => bail!("Invalid publish transition: {from} -> {ev:?}"),
        };
        Ok(next)
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
            "publishing" => Ok(DraftStatus::Publishing),
            "published" => Ok(DraftStatus::Published),
            "failed" => Ok(DraftStatus::Failed),
            _ => bail!(
                "Invalid status '{}'. Valid: draft, ready, publishing, published, failed",
                s
            ),
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
    /// Set when a publish attempt begins (status → `Publishing`), cleared on the
    /// terminal write. A non-None value with `status: publishing` on disk means a
    /// prior attempt was interrupted and must be reconciled, not blindly retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_started_at: Option<DateTime<Utc>>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn status_roundtrips_through_str() {
        for s in [
            DraftStatus::Draft,
            DraftStatus::Ready,
            DraftStatus::Publishing,
            DraftStatus::Published,
            DraftStatus::Failed,
        ] {
            assert_eq!(DraftStatus::from_str(s.as_str()).unwrap(), s);
        }
        assert!(DraftStatus::from_str("bogus").is_err());
    }

    #[test]
    fn only_published_and_failed_are_terminal() {
        assert!(DraftStatus::Published.is_terminal());
        assert!(DraftStatus::Failed.is_terminal());
        assert!(!DraftStatus::Draft.is_terminal());
        assert!(!DraftStatus::Ready.is_terminal());
        assert!(!DraftStatus::Publishing.is_terminal());
    }

    #[test]
    fn can_publish_enforces_pb1() {
        // Ready is always publishable.
        assert!(DraftStatus::Ready.can_publish(false).is_ok());
        // Draft only when `ready` folds in (scheduled/dry-run).
        assert!(DraftStatus::Draft.can_publish(false).is_err());
        assert!(DraftStatus::Draft.can_publish(true).is_ok());
        // Terminal/transient gating.
        assert!(DraftStatus::Published.can_publish(true).is_err());
        assert!(DraftStatus::Failed.can_publish(true).is_err());
        // Publishing is allowed so an interrupted attempt can reconcile.
        assert!(DraftStatus::Publishing.can_publish(false).is_ok());
    }

    #[test]
    fn valid_transitions_advance_state() {
        assert_eq!(
            DraftStatus::Ready.transition(PublishEvent::Start).unwrap(),
            DraftStatus::Publishing
        );
        assert_eq!(
            DraftStatus::Draft.transition(PublishEvent::Start).unwrap(),
            DraftStatus::Publishing
        );
        // Re-Start while Publishing (e.g. a scheduled re-scan) stays Publishing.
        assert_eq!(
            DraftStatus::Publishing
                .transition(PublishEvent::Start)
                .unwrap(),
            DraftStatus::Publishing
        );
        assert_eq!(
            DraftStatus::Publishing
                .transition(PublishEvent::Succeeded)
                .unwrap(),
            DraftStatus::Published
        );
        assert_eq!(
            DraftStatus::Publishing
                .transition(PublishEvent::Failed)
                .unwrap(),
            DraftStatus::Failed
        );
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        // Cannot succeed/fail without first entering Publishing.
        assert!(DraftStatus::Ready.transition(PublishEvent::Succeeded).is_err());
        assert!(DraftStatus::Draft.transition(PublishEvent::Failed).is_err());
        // Terminal states do not advance.
        assert!(DraftStatus::Published.transition(PublishEvent::Start).is_err());
        assert!(DraftStatus::Failed.transition(PublishEvent::Start).is_err());
        assert!(
            DraftStatus::Published
                .transition(PublishEvent::Succeeded)
                .is_err()
        );
    }

    #[test]
    fn publishing_marker_survives_render_roundtrip() {
        // A draft left mid-publish must reload as `Publishing` with its post_id so
        // `publish()` reconciles instead of re-posting (#ckypubsm crash-safety).
        let meta = SocialDraftMeta {
            platform: Platform::LinkedIn,
            author: "alex".into(),
            visibility: "public".into(),
            status: DraftStatus::Publishing,
            tags: vec![],
            scheduled_at: None,
            published_at: None,
            publish_started_at: Some(Utc::now()),
            post_id: Some("urn:li:share:123".into()),
            post_url: Some("https://www.linkedin.com/feed/update/urn:li:share:123".into()),
            images: vec![],
            video: None,
            captions: None,
            title: None,
            first_comment: None,
        };
        let draft = SocialDraft::new(meta, "body".into());
        let reparsed = SocialDraft::parse(&draft.render().unwrap()).unwrap();
        assert_eq!(reparsed.meta.status, DraftStatus::Publishing);
        assert_eq!(reparsed.meta.post_id.as_deref(), Some("urn:li:share:123"));
        assert!(reparsed.meta.publish_started_at.is_some());
    }
}
