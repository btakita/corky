//! Search Console `urlInspection.index.inspect` — per-URL coverage.

use anyhow::{Result, bail};

pub type InspectResponse = serde_json::Value;

pub fn inspect_url(
    _token: &str,
    _site_url: &str,
    _inspection_url: &str,
) -> Result<InspectResponse> {
    bail!("gsc urlInspection.index.inspect not yet implemented")
}
