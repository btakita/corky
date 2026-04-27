//! Search Console `searchAnalytics.query` — Performance API.

use anyhow::{Result, bail};

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct DimensionFilter {
    pub dimension: String,
    pub operator: String,
    pub expression: String,
}

pub struct QueryParams<'a> {
    pub site_url: &'a str,
    pub start_date: &'a str,
    pub end_date: &'a str,
    pub dimensions: &'a [&'a str],
    pub row_limit: u32,
    pub start_row: u32,
    pub filters: Vec<DimensionFilter>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct QueryResponse {
    #[serde(default)]
    pub rows: Option<Vec<Row>>,
    #[serde(
        rename = "responseAggregationType",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub aggregation: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub keys: Vec<String>,
    pub clicks: f64,
    pub impressions: f64,
    pub ctr: f64,
    pub position: f64,
}

pub fn run_query(_token: &str, _params: &QueryParams) -> Result<QueryResponse> {
    bail!("gsc searchAnalytics.query not yet implemented")
}
