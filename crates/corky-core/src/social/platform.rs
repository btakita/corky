//! Social media platform enum.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    LinkedIn,
    Bluesky,
    Mastodon,
    Twitter,
    Youtube,
}

impl Platform {
    pub const ALL: &'static [Platform] = &[
        Platform::LinkedIn,
        Platform::Bluesky,
        Platform::Mastodon,
        Platform::Twitter,
        Platform::Youtube,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::LinkedIn => "linkedin",
            Platform::Bluesky => "bluesky",
            Platform::Mastodon => "mastodon",
            Platform::Twitter => "twitter",
            Platform::Youtube => "youtube",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "linkedin" => Ok(Platform::LinkedIn),
            "bluesky" => Ok(Platform::Bluesky),
            "mastodon" => Ok(Platform::Mastodon),
            "twitter" => Ok(Platform::Twitter),
            "youtube" => Ok(Platform::Youtube),
            _ => {
                let supported: Vec<&str> = Platform::ALL.iter().map(|p| p.as_str()).collect();
                anyhow::bail!(
                    "Unknown platform '{}'. Supported: {}",
                    s,
                    supported.join(", ")
                )
            }
        }
    }
}
