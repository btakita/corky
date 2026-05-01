//! Unified search subsystem with pluggable backends.
//!
//! Provides `corky search` (unified), `corky sift` (local hybrid),
//! and `corky ragie` (cloud RAG) commands.

pub mod backend;
pub mod ragie;
pub mod result;
pub mod sift;

use anyhow::Result;

use crate::config::corky_config;
use backend::BackendRegistry;

/// Run a unified search across configured backends.
pub fn run(query: &str, backend_filter: Option<&str>, all: bool) -> Result<()> {
    let config = corky_config::load_config(None)?;
    let search_config = config.search.unwrap_or_default();
    let registry = BackendRegistry::from_config(&search_config)?;

    let backends: Vec<&str> = if all {
        registry.all_names()
    } else if let Some(filter) = backend_filter {
        filter.split(',').collect()
    } else {
        search_config
            .default_backends
            .iter()
            .map(|s| s.as_str())
            .collect()
    };

    if backends.is_empty() {
        println!("No search backends configured. Add a [search] section to .corky.toml.");
        return Ok(());
    }

    let mut all_results = Vec::new();

    for name in &backends {
        let b = match registry.get(name) {
            Some(b) => b,
            None => {
                eprintln!("Unknown backend: {name}");
                continue;
            }
        };
        match b.search(query, &result::Filters::default()) {
            Ok(results) => all_results.extend(results),
            Err(e) => eprintln!("[{name}] search error: {e}"),
        }
    }

    // Deduplicate and sort by score (descending)
    all_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if all_results.is_empty() {
        println!("No results found.");
    } else {
        for r in &all_results {
            println!("[{:.2}] [{}] {}", r.score, r.backend, r.path.display());
            if !r.snippet.is_empty() {
                println!("  {}", r.snippet);
            }
        }
        println!("\n{} result(s)", all_results.len());
    }

    Ok(())
}
