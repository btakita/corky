//! Ragie backend — REST API client for cloud RAG.

use anyhow::{Context, Result};
use std::path::PathBuf;

use super::backend::SearchBackend;
use super::result::{BackendStatus, Filters, SearchResult};
use crate::config::corky_config::SearchBackendConfig;

pub struct RagieBackend {
    name: String,
    api_key_pass: String,
}

impl RagieBackend {
    pub fn from_config(name: &str, config: &SearchBackendConfig) -> Result<Self> {
        let api_key_pass = config
            .api_key_pass
            .clone()
            .unwrap_or_else(|| "ragie/api_key".to_string());
        Ok(Self {
            name: name.to_string(),
            api_key_pass,
        })
    }

    /// Resolve the API key via pass(1).
    fn api_key(&self) -> Result<String> {
        let output = std::process::Command::new("pass")
            .arg(&self.api_key_pass)
            .output()
            .context("Failed to run 'pass'. Is it installed?")?;
        if !output.status.success() {
            anyhow::bail!(
                "pass {} failed: {}",
                self.api_key_pass,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_string())
    }

    /// Push documents to Ragie.
    pub fn run_push(full: bool) -> Result<()> {
        let config = crate::config::corky_config::load_config(None)?;
        let search_config = config.search.unwrap_or_default();
        let backend_config = search_config
            .backends
            .get("ragie")
            .context("No [search.backends.ragie] section in .corky.toml")?;

        let backend = Self::from_config("ragie", backend_config)?;
        let api_key = backend.api_key()?;

        let conversations_dir = crate::resolve::conversations_dir();
        let pattern = conversations_dir.join("*.md");
        let files: Vec<PathBuf> = glob::glob(&pattern.to_string_lossy())
            .context("Failed to glob conversations")?
            .filter_map(|e| e.ok())
            .collect();

        println!(
            "Pushing {} files to Ragie{}...",
            files.len(),
            if full { " (full)" } else { "" }
        );

        for file in &files {
            let content = std::fs::read_to_string(file)?;
            let filename = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let body = ureq::json!({
                "name": filename,
                "content": content,
                "mode": if full { "hi_res" } else { "fast" },
            });

            let resp = ureq::post("https://api.ragie.ai/documents")
                .set("Authorization", &format!("Bearer {api_key}"))
                .set("Content-Type", "application/json")
                .send_json(body);

            match resp {
                Ok(_) => println!("  + {filename}"),
                Err(e) => eprintln!("  ! {filename}: {e}"),
            }
        }

        println!("Done.");
        Ok(())
    }

    /// Compare local vs Ragie-indexed documents.
    pub fn run_sync() -> Result<()> {
        let config = crate::config::corky_config::load_config(None)?;
        let search_config = config.search.unwrap_or_default();
        let backend_config = search_config
            .backends
            .get("ragie")
            .context("No [search.backends.ragie] section in .corky.toml")?;

        let backend = Self::from_config("ragie", backend_config)?;
        let api_key = backend.api_key()?;

        let resp = ureq::get("https://api.ragie.ai/documents")
            .set("Authorization", &format!("Bearer {api_key}"))
            .call()
            .context("Failed to list Ragie documents")?;

        let body: serde_json::Value = resp.into_json()?;
        let docs = body
            .get("documents")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        let conversations_dir = crate::resolve::conversations_dir();
        let local_count = std::fs::read_dir(&conversations_dir)
            .map(|entries| entries.filter(|e| e.is_ok()).count())
            .unwrap_or(0);

        println!("Local files:    {local_count}");
        println!("Ragie indexed:  {docs}");

        if local_count > docs {
            println!(
                "  {} file(s) not yet pushed to Ragie.",
                local_count - docs
            );
        } else if docs > local_count {
            println!(
                "  {} Ragie doc(s) with no local file.",
                docs - local_count
            );
        } else {
            println!("  In sync.");
        }
        Ok(())
    }

    /// Show Ragie account status.
    pub fn run_status() -> Result<()> {
        let config = crate::config::corky_config::load_config(None)?;
        let search_config = config.search.unwrap_or_default();
        let backend_config = search_config
            .backends
            .get("ragie")
            .context("No [search.backends.ragie] section in .corky.toml")?;

        let backend = Self::from_config("ragie", backend_config)?;
        let api_key = backend.api_key()?;

        let resp = ureq::get("https://api.ragie.ai/documents")
            .set("Authorization", &format!("Bearer {api_key}"))
            .call()
            .context("Failed to query Ragie API")?;

        let body: serde_json::Value = resp.into_json()?;
        let docs = body
            .get("documents")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        println!("Ragie status:");
        println!("  Documents indexed: {docs}");
        println!("  API: connected");
        Ok(())
    }
}

impl SearchBackend for RagieBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, query: &str, _filters: &Filters) -> Result<Vec<SearchResult>> {
        let api_key = self.api_key()?;

        let body = ureq::json!({
            "query": query,
            "rerank": true,
        });

        let resp = ureq::post("https://api.ragie.ai/retrievals")
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Content-Type", "application/json")
            .send_json(body)
            .context("Ragie retrieval request failed")?;

        let json: serde_json::Value = resp.into_json()?;
        let scored_chunks = json
            .get("scored_chunks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut results = Vec::new();
        for chunk in scored_chunks {
            let score = chunk.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let text = chunk
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let doc_name = chunk
                .get("document_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            results.push(SearchResult {
                path: PathBuf::from(doc_name),
                score,
                snippet: text.chars().take(200).collect(),
                backend: self.name.clone(),
                metadata: std::collections::HashMap::new(),
            });
        }

        Ok(results)
    }

    fn index(&self, _paths: &[PathBuf]) -> Result<()> {
        Self::run_push(false)
    }

    fn status(&self) -> Result<BackendStatus> {
        let api_key = self.api_key();
        let available = api_key.is_ok();
        Ok(BackendStatus {
            name: self.name.clone(),
            available,
            document_count: 0,
            message: if available {
                "API key configured".to_string()
            } else {
                format!("API key not found at pass path: {}", self.api_key_pass)
            },
        })
    }
}
