//! Shared types for search results, filters, and backend status.

use std::collections::HashMap;
use std::path::PathBuf;

/// A single search result from any backend.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Path to the source file.
    pub path: PathBuf,
    /// Relevance score, normalized 0.0–1.0.
    pub score: f64,
    /// Matched text excerpt.
    pub snippet: String,
    /// Which backend produced this result.
    pub backend: String,
    /// Metadata: labels, contacts, dates, etc.
    pub metadata: HashMap<String, String>,
}

/// Filters for narrowing search results.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    /// Filter by label(s).
    pub labels: Vec<String>,
    /// Filter by contact name(s).
    pub contacts: Vec<String>,
    /// Filter by date range (RFC 3339 strings).
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

/// Backend health and index status.
#[derive(Debug, Clone)]
pub struct BackendStatus {
    /// Backend name.
    pub name: String,
    /// Whether the backend is reachable / index exists.
    pub available: bool,
    /// Number of indexed documents.
    pub document_count: usize,
    /// Human-readable status message.
    pub message: String,
}
