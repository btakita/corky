//! Google Search Console integration.
//!
//! Prefers user OAuth with a real Google account that already has Search
//! Console access. Service-account credentials can be attempted as a best-effort
//! fallback, but Search Console does not reliably accept service-account
//! identities as property users. See `auth.rs` for selection logic.

pub mod auth;
pub mod inspect;
pub mod query;
pub mod sites;
