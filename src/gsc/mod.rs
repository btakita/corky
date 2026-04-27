//! Google Search Console integration.
//!
//! Supports service-account auth (preferred, no browser) and user-OAuth
//! fallback using the same client as Gmail/Calendar. See `auth.rs` for
//! selection logic.

pub mod auth;
pub mod inspect;
pub mod query;
pub mod sites;
