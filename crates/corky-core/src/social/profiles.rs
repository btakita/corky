//! Social media profile registry (profiles.toml).
//!
//! Maps human profile names to platform handles and URNs.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::platform::Platform;
use crate::config::corky_config;
use crate::resolve;

/// A single platform entry within a profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformEntry {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urn: Option<String>,
}

/// A named profile containing one or more platform entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linkedin: Option<PlatformEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bluesky: Option<PlatformEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mastodon: Option<PlatformEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twitter: Option<PlatformEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube: Option<PlatformEntry>,
}

impl Profile {
    /// Get the platform entry for a given platform.
    pub fn get_platform(&self, platform: Platform) -> Option<&PlatformEntry> {
        match platform {
            Platform::LinkedIn => self.linkedin.as_ref(),
            Platform::Bluesky => self.bluesky.as_ref(),
            Platform::Mastodon => self.mastodon.as_ref(),
            Platform::Twitter => self.twitter.as_ref(),
            Platform::Youtube => self.youtube.as_ref(),
        }
    }

    /// Returns all platform entries this profile has.
    fn platform_entries(&self) -> Vec<(Platform, &PlatformEntry)> {
        let mut entries = Vec::new();
        if let Some(e) = &self.linkedin {
            entries.push((Platform::LinkedIn, e));
        }
        if let Some(e) = &self.bluesky {
            entries.push((Platform::Bluesky, e));
        }
        if let Some(e) = &self.mastodon {
            entries.push((Platform::Mastodon, e));
        }
        if let Some(e) = &self.twitter {
            entries.push((Platform::Twitter, e));
        }
        if let Some(e) = &self.youtube {
            entries.push((Platform::Youtube, e));
        }
        entries
    }
}

/// The full profiles.toml file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfilesFile {
    #[serde(flatten)]
    pub profiles: HashMap<String, Profile>,
}

/// Validation diagnostics.
#[derive(Debug, Default)]
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub info: Vec<String>,
}

impl ValidationResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl ProfilesFile {
    /// Load profiles from .corky.toml [profiles] section, falling back to profiles.toml.
    pub fn load() -> Result<Self> {
        // Prefer [profiles] section in .corky.toml
        if let Some(config) = corky_config::try_load_config(None)
            && !config.profiles.is_empty()
        {
            return Ok(ProfilesFile {
                profiles: config.profiles,
            });
        }
        // Fallback to standalone profiles.toml
        let path = resolve::profiles_toml();
        Self::load_from(&path)
    }

    /// Load profiles.toml from a specific path.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            bail!(
                "profiles.toml not found at {}.\n\
                 Create it or run `corky linkedin check` for guidance.",
                path.display()
            );
        }
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Parse profiles from a TOML string.
    pub fn parse(content: &str) -> Result<Self> {
        let file: ProfilesFile = toml::from_str(content)?;
        Ok(file)
    }

    /// Validate the profiles file. Returns errors, warnings, and info messages.
    pub fn validate(&self) -> ValidationResult {
        let mut result = ValidationResult::default();

        // P4: Profile with no platform entries
        for (name, profile) in &self.profiles {
            if profile.platform_entries().is_empty() {
                result
                    .warnings
                    .push(format!("Profile '{}' has no platform entries", name));
            }
        }

        // P1: Duplicate handle within same platform
        // P2: Duplicate URN within same platform
        for platform in Platform::ALL {
            let mut handles: HashMap<&str, &str> = HashMap::new(); // handle -> profile_name
            let mut urns: HashMap<&str, &str> = HashMap::new(); // urn -> profile_name

            for (name, profile) in &self.profiles {
                if let Some(entry) = profile.get_platform(*platform) {
                    // Check duplicate handle
                    if let Some(existing) = handles.get(entry.handle.as_str()) {
                        if *existing != name {
                            result.errors.push(format!(
                                "{}: handle '{}' already mapped to profile '{}' (duplicate in '{}')",
                                platform, entry.handle, existing, name
                            ));
                        }
                    } else {
                        handles.insert(&entry.handle, name);
                    }

                    // Check duplicate URN within platform
                    if let Some(urn) = &entry.urn {
                        if let Some(existing) = urns.get(urn.as_str()) {
                            if *existing != name {
                                result.errors.push(format!(
                                    "{}: URN '{}' used by profiles '{}' and '{}'",
                                    platform, urn, existing, name
                                ));
                            }
                        } else {
                            urns.insert(urn, name);
                        }
                    }
                }
            }
        }

        // P3: Same URN across different profiles (cross-profile URN conflict)
        let mut global_urns: HashMap<String, (&str, Platform)> = HashMap::new(); // urn -> (profile, platform)
        for (name, profile) in &self.profiles {
            for (platform, entry) in profile.platform_entries() {
                if let Some(urn) = &entry.urn {
                    let key = format!("{}:{}", platform, urn);
                    if let Some((existing_name, _)) = global_urns.get(&key) {
                        if *existing_name != name.as_str() {
                            // Already reported in P2
                        }
                    } else {
                        global_urns.insert(key, (name, platform));
                    }

                    // Also check if the same URN appears for a different profile on same platform
                    // (already handled above), but check cross-platform same URN different profile
                    let urn_key = urn.clone();
                    for (other_name, other_profile) in &self.profiles {
                        if other_name == name {
                            continue;
                        }
                        for (other_platform, other_entry) in other_profile.platform_entries() {
                            if other_platform == platform {
                                continue; // Same platform duplicate already caught
                            }
                            if let Some(other_urn) = &other_entry.urn
                                && *other_urn == urn_key
                            {
                                result.errors.push(format!(
                                    "URN '{}' used by profile '{}' ({}) and '{}' ({})",
                                    urn, name, platform, other_name, other_platform
                                ));
                            }
                        }
                    }
                }
            }
        }

        // P5: Cross-platform coherence (informational)
        for (name, profile) in &self.profiles {
            let entries = profile.platform_entries();
            if entries.len() > 1 {
                let platforms: Vec<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();
                result.info.push(format!(
                    "Profile '{}' has entries on {}: verify same person",
                    name,
                    platforms.join(", ")
                ));
            }
        }

        // Deduplicate errors (cross-profile URN check can produce dupes)
        let mut seen = HashSet::new();
        result.errors.retain(|e| seen.insert(e.clone()));

        result
    }

    /// Resolve a profile name to its URN for a given platform.
    pub fn resolve_urn(&self, profile_name: &str, platform: Platform) -> Result<String> {
        let profile = self.profiles.get(profile_name).ok_or_else(|| {
            let available: Vec<&String> = self.profiles.keys().collect();
            anyhow::anyhow!(
                "Profile '{}' not found in profiles.toml. Available: {}",
                profile_name,
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )
        })?;

        let entry = profile.get_platform(platform).ok_or_else(|| {
            anyhow::anyhow!("Profile '{}' has no {} entry", profile_name, platform)
        })?;

        entry.urn.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Profile '{}' has no URN for {}. Run `corky linkedin auth` to set it up.",
                profile_name,
                platform
            )
        })
    }

    /// Resolve a handle to its profile name for a given platform.
    pub fn resolve_handle(&self, handle: &str, platform: Platform) -> Option<String> {
        for (name, profile) in &self.profiles {
            if let Some(entry) = profile.get_platform(platform)
                && entry.handle == handle
            {
                return Some(name.clone());
            }
        }
        None
    }
}
