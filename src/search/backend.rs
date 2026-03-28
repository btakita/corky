//! SearchBackend trait and backend registry.

use anyhow::Result;
use std::collections::HashMap;

use super::result::{BackendStatus, Filters, SearchResult};
use crate::config::corky_config::SearchConfig;

/// Trait implemented by each search backend (Sift, Ragie, etc.).
pub trait SearchBackend: Send + Sync {
    /// Backend name (e.g. "sift", "ragie").
    fn name(&self) -> &str;

    /// Search for documents matching the query.
    fn search(&self, query: &str, filters: &Filters) -> Result<Vec<SearchResult>>;

    /// Index or push documents to the backend.
    fn index(&self, paths: &[std::path::PathBuf]) -> Result<()>;

    /// Report backend health and index status.
    fn status(&self) -> Result<BackendStatus>;
}

/// Registry of configured search backends.
pub struct BackendRegistry {
    backends: HashMap<String, Box<dyn SearchBackend>>,
}

impl BackendRegistry {
    /// Build a registry from the search config section.
    pub fn from_config(config: &SearchConfig) -> Result<Self> {
        let mut backends = HashMap::new();

        for (name, backend_config) in &config.backends {
            let backend: Box<dyn SearchBackend> = match backend_config.backend_type.as_str() {
                "sift" => Box::new(super::sift::SiftBackend::from_config(name, backend_config)?),
                "ragie" => Box::new(super::ragie::RagieBackend::from_config(name, backend_config)?),
                other => {
                    eprintln!("Unknown backend type: {other} (skipping {name})");
                    continue;
                }
            };
            backends.insert(name.clone(), backend);
        }

        Ok(Self { backends })
    }

    /// Get a backend by name.
    pub fn get(&self, name: &str) -> Option<&dyn SearchBackend> {
        self.backends.get(name).map(|b| b.as_ref())
    }

    /// List all registered backend names.
    pub fn all_names(&self) -> Vec<&str> {
        self.backends.keys().map(|s| s.as_str()).collect()
    }
}
