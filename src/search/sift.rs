//! Sift backend — wraps the Sift CLI for local hybrid search.
//!
//! Sift is indexless — it searches a corpus directory directly on each query.
//! No separate index build step is needed.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

use super::backend::SearchBackend;
use super::result::{BackendStatus, Filters, SearchResult};
use crate::config::corky_config::SearchBackendConfig;

pub struct SiftBackend {
    name: String,
    corpus_path: Option<PathBuf>,
}

impl SiftBackend {
    pub fn from_config(name: &str, config: &SearchBackendConfig) -> Result<Self> {
        let corpus_path = config.index_path.as_ref().map(PathBuf::from);
        Ok(Self {
            name: name.to_string(),
            corpus_path,
        })
    }

    /// Resolve the corpus directory (config override or default conversations dir).
    fn corpus_dir(&self) -> PathBuf {
        self.corpus_path
            .clone()
            .unwrap_or_else(crate::resolve::conversations_dir)
    }

    /// Show sift status (check binary + corpus dir).
    pub fn run_status() -> Result<()> {
        let sift_version = Command::new("sift")
            .arg("--version")
            .output();

        match sift_version {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout);
                println!("Sift: {}", version.trim());
            }
            _ => {
                println!("Sift: not installed");
                println!("  Install: curl --proto '=https' --tlsv1.2 -LsSf https://github.com/rupurt/sift/releases/latest/download/sift-installer.sh | sh");
                return Ok(());
            }
        }

        let conversations_dir = crate::resolve::conversations_dir();
        if conversations_dir.exists() {
            let count = std::fs::read_dir(&conversations_dir)
                .map(|entries| entries.filter(|e| e.is_ok()).count())
                .unwrap_or(0);
            println!("Corpus: {} ({count} files)", conversations_dir.display());
        } else {
            println!("Corpus: {} (not found)", conversations_dir.display());
        }
        println!("Note: Sift is indexless — no index build needed.");
        Ok(())
    }

    /// Run a sift search and print results to stdout (for `corky sift index` placeholder).
    pub fn run_index(_watch: bool) -> Result<()> {
        println!("Sift is indexless — no index build step needed.");
        println!("Sift searches the corpus directory directly on each query.");
        println!("Use 'corky search <query>' to search your conversations.");
        Ok(())
    }
}

impl SearchBackend for SiftBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, query: &str, _filters: &Filters) -> Result<Vec<SearchResult>> {
        let corpus = self.corpus_dir();
        if !corpus.exists() {
            anyhow::bail!(
                "Corpus directory not found at {}.",
                corpus.display()
            );
        }

        let output = Command::new("sift")
            .args([
                "search",
                "--retrievers", "bm25",
                "--reranking", "none",
                "--json",
                "--limit", "20",
                &corpus.to_string_lossy(),
                query,
            ])
            .output()
            .context("Failed to run 'sift search'. Is sift installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("sift search failed: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout)
            .context("Failed to parse sift JSON output")?;

        let results_arr = json
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut results = Vec::new();
        for item in results_arr {
            let path = item
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let score = item
                .get("score")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let snippet = item
                .get("snippet")
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            results.push(SearchResult {
                path: PathBuf::from(path),
                score,
                snippet: snippet.to_string(),
                backend: self.name.clone(),
                metadata: std::collections::HashMap::new(),
            });
        }

        Ok(results)
    }

    fn index(&self, _paths: &[PathBuf]) -> Result<()> {
        println!("Sift is indexless — no index build needed.");
        Ok(())
    }

    fn status(&self) -> Result<BackendStatus> {
        let corpus = self.corpus_dir();
        let available = corpus.exists()
            && Command::new("sift")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

        let doc_count = if corpus.exists() {
            std::fs::read_dir(&corpus)
                .map(|entries| entries.filter(|e| e.is_ok()).count())
                .unwrap_or(0)
        } else {
            0
        };

        Ok(BackendStatus {
            name: self.name.clone(),
            available,
            document_count: doc_count,
            message: if available {
                format!("Corpus: {} ({doc_count} files, indexless)", corpus.display())
            } else {
                "Sift not installed or corpus not found".to_string()
            },
        })
    }
}
