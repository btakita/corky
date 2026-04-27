//! Sync contact CLAUDE.md files between root contacts/ and mailbox contacts/.
//!
//! v2 — conversation-aware: only syncs contacts to mailboxes where they have
//! conversations or are explicitly shared via .corky.toml.
//!
//! Eligibility rules (root → mailbox):
//! - Conversation match: sender name in mailbox conversations slugifies to the contact name
//! - Explicit sharing: `[contacts.{name}].shared_with` includes the mailbox name
//! - Alias matching: `[contacts.{name}].aliases` provides alternative sender names
//!
//! Mailbox → root sync is always allowed.
//! Resolution: 3-way merge via `.sync-state.json` content hashes. If only one
//! side changed since last sync, that side wins. If both changed (conflict),
//! falls back to newest-wins by mtime. First sync (no base hash) also uses mtime.
//! Only CLAUDE.md is synced; CLAUDE.local.md and other files are skipped.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::contact::Contact;
use crate::resolve;
use crate::sync::types::ContactSyncState;

/// A discovered contact CLAUDE.md with its metadata.
#[derive(Debug)]
struct ContactFile {
    path: PathBuf,
    mtime: SystemTime,
}

/// Regex for extracting sender names from conversation `## ` headers.
static SENDER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^## (.+?)(?:\s+—\s+.+)?$").unwrap());

/// Regex for stripping " via {Service}" suffix from sender names.
static VIA_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+via\s+.+$").unwrap());

/// Regex for stripping email addresses in angle brackets.
static EMAIL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*<[^>]+>").unwrap());

/// FNV-1a hash of content, returned as a 16-char hex string.
fn content_hash(content: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001B3;
    let mut hash = FNV_OFFSET;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}", hash)
}

/// Get the stored base hash for a contact-mailbox pair.
fn get_base_hash<'a>(
    contact_state: &'a HashMap<String, ContactSyncState>,
    contact_name: &str,
    mb_name: &str,
) -> Option<&'a str> {
    contact_state
        .get(contact_name)
        .and_then(|cs| cs.mailboxes.get(mb_name))
        .map(|s| s.as_str())
}

/// Store the base hash after a successful sync.
fn set_base_hash(
    contact_state: &mut HashMap<String, ContactSyncState>,
    contact_name: &str,
    mb_name: &str,
    hash: &str,
) {
    contact_state
        .entry(contact_name.to_string())
        .or_default()
        .mailboxes
        .insert(mb_name.to_string(), hash.to_string());
}

/// Run the contact sync.
pub fn run() -> Result<()> {
    let data_dir = resolve::data_dir();
    let root_contacts_dir = data_dir.join("contacts");
    let mailboxes_dir = data_dir.join("mailboxes");

    // Load contact config from .corky.toml (optional — missing config means no explicit sharing)
    let contacts_config = crate::config::contact::load_contacts(None).unwrap_or_default();

    // Load sync state for 3-way merge tracking
    let mut state = crate::sync::load_state()?;

    sync_contacts(
        &root_contacts_dir,
        &mailboxes_dir,
        &contacts_config,
        &mut state.contacts,
    )?;

    // Persist updated sync state
    crate::sync::save_state(&state)?;

    Ok(())
}

/// Core sync logic, factored out for testability.
fn sync_contacts(
    root_contacts_dir: &Path,
    mailboxes_dir: &Path,
    contacts_config: &BTreeMap<String, Contact>,
    contact_state: &mut HashMap<String, ContactSyncState>,
) -> Result<()> {
    let root_contacts = discover_contacts(root_contacts_dir)?;
    let mut synced = 0u32;

    if mailboxes_dir.is_dir() {
        for mb_path in iter_mailbox_dirs(mailboxes_dir)? {
            let mb_name = mb_path.file_name().unwrap().to_string_lossy().to_string();
            let mb_contacts_dir = mb_path.join("contacts");

            // Skip mailboxes without a contacts/ dir
            if !mb_contacts_dir.is_dir() {
                continue;
            }

            // Build eligible set for this mailbox
            let eligible = build_eligible_set(&root_contacts, contacts_config, &mb_path, &mb_name)?;

            // Discover existing mailbox contacts
            let mb_contacts = discover_contacts(&mb_contacts_dir)?;

            // Process eligible root contacts → mailbox (bidirectional with 3-way merge)
            for name in &eligible {
                if let Some(root_cf) = root_contacts.get(name) {
                    if let Some(mb_cf) = mb_contacts.get(name) {
                        // Both exist + eligible: 3-way merge
                        let root_content = std::fs::read_to_string(&root_cf.path)
                            .with_context(|| format!("reading {}", root_cf.path.display()))?;
                        let mb_content = std::fs::read_to_string(&mb_cf.path)
                            .with_context(|| format!("reading {}", mb_cf.path.display()))?;
                        let root_hash = content_hash(&root_content);
                        let mb_hash = content_hash(&mb_content);

                        if root_hash == mb_hash {
                            // Already in sync — record hash
                            set_base_hash(contact_state, name, &mb_name, &root_hash);
                            continue;
                        }

                        let base =
                            get_base_hash(contact_state, name, &mb_name).map(|s| s.to_string());

                        let take_root = match base.as_deref() {
                            Some(b) if root_hash == b => false, // root unchanged, take mb
                            Some(b) if mb_hash == b => true,    // mb unchanged, take root
                            Some(_) => {
                                // Both changed — conflict, fall back to mtime
                                eprintln!(
                                    "  warning: conflict for contact {} in {}, using newest",
                                    name, mb_name
                                );
                                root_cf.mtime > mb_cf.mtime
                            }
                            None => {
                                // No base hash (first sync) — mtime wins
                                root_cf.mtime > mb_cf.mtime
                            }
                        };

                        if take_root {
                            write_and_set_mtime(&mb_cf.path, &root_content, root_cf.mtime)?;
                            println!("  {} <- {}", mb_cf.path.display(), root_cf.path.display());
                            set_base_hash(contact_state, name, &mb_name, &root_hash);
                        } else {
                            write_and_set_mtime(&root_cf.path, &mb_content, mb_cf.mtime)?;
                            println!("  {} <- {}", root_cf.path.display(), mb_cf.path.display());
                            set_base_hash(contact_state, name, &mb_name, &mb_hash);
                        }
                        synced += 1;
                    } else {
                        // Only in root + eligible: copy to mailbox
                        let content = std::fs::read_to_string(&root_cf.path)
                            .with_context(|| format!("reading {}", root_cf.path.display()))?;
                        let hash = content_hash(&content);
                        let dest_dir = mb_contacts_dir.join(name);
                        let dest = dest_dir.join("CLAUDE.md");
                        std::fs::create_dir_all(&dest_dir)?;
                        write_and_set_mtime(&dest, &content, root_cf.mtime)?;
                        println!("  {} (new from root)", dest.display());
                        set_base_hash(contact_state, name, &mb_name, &hash);
                        synced += 1;
                    }
                }
            }

            // Mailbox → Root: always allowed
            for (name, mb_cf) in &mb_contacts {
                if !root_contacts.contains_key(name) {
                    // Only in mailbox: copy to root
                    let content = std::fs::read_to_string(&mb_cf.path)
                        .with_context(|| format!("reading {}", mb_cf.path.display()))?;
                    let hash = content_hash(&content);
                    let dest_dir = root_contacts_dir.join(name);
                    let dest = dest_dir.join("CLAUDE.md");
                    std::fs::create_dir_all(&dest_dir)?;
                    write_and_set_mtime(&dest, &content, mb_cf.mtime)?;
                    println!("  {} (new from mailbox)", dest.display());
                    set_base_hash(contact_state, name, &mb_name, &hash);
                    synced += 1;
                } else if !eligible.contains(name) {
                    // Both exist but NOT eligible: 3-way merge, mailbox→root only
                    let root_cf = &root_contacts[name];
                    let root_content = std::fs::read_to_string(&root_cf.path)
                        .with_context(|| format!("reading {}", root_cf.path.display()))?;
                    let mb_content = std::fs::read_to_string(&mb_cf.path)
                        .with_context(|| format!("reading {}", mb_cf.path.display()))?;
                    let root_hash = content_hash(&root_content);
                    let mb_hash = content_hash(&mb_content);

                    if root_hash == mb_hash {
                        set_base_hash(contact_state, name, &mb_name, &root_hash);
                        continue;
                    }

                    let base = get_base_hash(contact_state, name, &mb_name).map(|s| s.to_string());

                    let should_sync = match base.as_deref() {
                        Some(b) if mb_hash == b => false,       // mb unchanged, skip
                        Some(b) if root_hash == b => true,      // root unchanged, mb changed
                        Some(_) => mb_cf.mtime > root_cf.mtime, // both changed, mtime
                        None => mb_cf.mtime > root_cf.mtime,    // no base, mtime
                    };

                    if should_sync {
                        write_and_set_mtime(&root_cf.path, &mb_content, mb_cf.mtime)?;
                        println!("  {} <- {}", root_cf.path.display(), mb_cf.path.display());
                        set_base_hash(contact_state, name, &mb_name, &mb_hash);
                        synced += 1;
                    }
                }
            }
        }
    }

    if synced == 0 {
        println!("contacts: already in sync");
    } else {
        println!("contacts: synced {} file(s)", synced);
    }

    Ok(())
}

/// Build the set of contact names eligible for sync to a specific mailbox.
fn build_eligible_set(
    root_contacts: &HashMap<String, ContactFile>,
    contacts_config: &BTreeMap<String, Contact>,
    mb_path: &Path,
    mb_name: &str,
) -> Result<HashSet<String>> {
    let mut eligible = HashSet::new();

    // 1. Scan conversations for sender slugs
    let conversations_dir = mb_path.join("conversations");
    let sender_slugs = extract_sender_slugs(&conversations_dir)?;

    // Match sender slugs against root contact names
    for name in root_contacts.keys() {
        if sender_slugs.contains(name) {
            eligible.insert(name.clone());
            continue;
        }
        // Check aliases from config
        if let Some(config) = contacts_config.get(name) {
            for alias in &config.aliases {
                if sender_slugs.contains(&slugify_sender(alias)) {
                    eligible.insert(name.clone());
                    break;
                }
            }
        }
    }

    // 2. Check explicit sharing
    for (name, config) in contacts_config {
        if config.shared_with.iter().any(|s| s == mb_name) && root_contacts.contains_key(name) {
            eligible.insert(name.clone());
        }
    }

    Ok(eligible)
}

/// Extract unique slugified sender names from all conversations in a directory.
fn extract_sender_slugs(conversations_dir: &Path) -> Result<HashSet<String>> {
    let mut slugs = HashSet::new();
    if !conversations_dir.is_dir() {
        return Ok(slugs);
    }
    for entry in std::fs::read_dir(conversations_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        for line in content.lines() {
            if let Some(caps) = SENDER_RE.captures(line) {
                let name = caps.get(1).unwrap().as_str();
                let slug = slugify_sender(name);
                if !slug.is_empty() {
                    slugs.insert(slug);
                }
            }
        }
    }
    Ok(slugs)
}

/// Slugify a sender name to match against contact directory names.
///
/// Strips " via {Service}" suffixes and email addresses in angle brackets,
/// then lowercases and replaces non-alphanumeric chars with hyphens.
pub fn slugify_sender(name: &str) -> String {
    // Strip " via ..." suffix
    let name = VIA_RE.replace(name, "");
    // Strip email in <...>
    let name = EMAIL_RE.replace_all(&name, "");
    // Lowercase, replace non-alnum with hyphens, collapse runs
    let mut slug = String::new();
    let mut prev_hyphen = true; // start true to skip leading hyphens
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    // Trim trailing hyphen
    if slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Discover CLAUDE.md files under `contacts_dir/{name}/CLAUDE.md`.
fn discover_contacts(contacts_dir: &Path) -> Result<HashMap<String, ContactFile>> {
    let mut map = HashMap::new();
    if !contacts_dir.is_dir() {
        return Ok(map);
    }
    for entry in std::fs::read_dir(contacts_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let claude_md = entry.path().join("CLAUDE.md");
        if claude_md.exists() {
            let mtime = std::fs::metadata(&claude_md)?.modified()?;
            map.insert(
                name,
                ContactFile {
                    path: claude_md,
                    mtime,
                },
            );
        }
    }
    Ok(map)
}

/// Iterate directories directly under `mailboxes_dir`.
fn iter_mailbox_dirs(mailboxes_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    if !mailboxes_dir.is_dir() {
        return Ok(dirs);
    }
    for entry in std::fs::read_dir(mailboxes_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Write content to a file and set its mtime to match the source.
fn write_and_set_mtime(path: &Path, content: &str, mtime: SystemTime) -> Result<()> {
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    let ft = filetime::FileTime::from_system_time(mtime);
    filetime::set_file_mtime(path, ft)
        .with_context(|| format!("setting mtime on {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Helper: sync with no prior state (first sync scenario).
    fn sync_no_state(
        root: &Path,
        mailboxes: &Path,
        config: &BTreeMap<String, Contact>,
    ) -> Result<()> {
        let mut cs = HashMap::new();
        sync_contacts(root, mailboxes, config, &mut cs)
    }

    /// Helper: create a contact CLAUDE.md with given content under `base/name/CLAUDE.md`.
    fn create_contact(base: &Path, name: &str, content: &str) -> PathBuf {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("CLAUDE.md");
        std::fs::write(&file, content).unwrap();
        file
    }

    /// Helper: set a file's mtime to a specific offset from now.
    fn set_mtime_offset(path: &Path, offset: Duration) {
        let t = SystemTime::now() - offset;
        let ft = filetime::FileTime::from_system_time(t);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    /// Helper: create a conversation file with sender headers.
    fn create_conversation(conversations_dir: &Path, filename: &str, senders: &[&str]) {
        std::fs::create_dir_all(conversations_dir).unwrap();
        let mut content = String::from("# Test Conversation\n\n---\n\n");
        for sender in senders {
            content.push_str(&format!(
                "## {} — Mon, 01 Jan 2024 00:00:00 +0000\n\nHello\n\n---\n\n",
                sender
            ));
        }
        std::fs::write(conversations_dir.join(filename), content).unwrap();
    }

    /// Helper: build a BTreeMap config with shared_with entries.
    fn config_with_sharing(entries: &[(&str, &[&str])]) -> BTreeMap<String, Contact> {
        let mut config = BTreeMap::new();
        for (name, mailboxes) in entries {
            config.insert(
                name.to_string(),
                Contact {
                    shared_with: mailboxes.iter().map(|s| s.to_string()).collect(),
                    ..Default::default()
                },
            );
        }
        config
    }

    // --- Slugify tests ---

    #[test]
    fn test_slugify_basic_name() {
        assert_eq!(slugify_sender("Eric Yang"), "eric-yang");
    }

    #[test]
    fn test_slugify_via_suffix() {
        assert_eq!(
            slugify_sender("Connie Lai via Wellfound Match"),
            "connie-lai"
        );
    }

    #[test]
    fn test_slugify_email_in_brackets() {
        assert_eq!(
            slugify_sender("Wellfound <noreply@wellfound.com>"),
            "wellfound"
        );
    }

    #[test]
    fn test_slugify_name_with_email() {
        assert_eq!(
            slugify_sender("Alice Smith <alice@example.com>"),
            "alice-smith"
        );
    }

    #[test]
    fn test_slugify_empty() {
        assert_eq!(slugify_sender(""), "");
    }

    // --- Conversation scanning tests ---

    #[test]
    fn test_extract_sender_slugs() {
        let tmp = TempDir::new().unwrap();
        let conv_dir = tmp.path().join("conversations");
        create_conversation(
            &conv_dir,
            "thread.md",
            &["Connie Lai via Wellfound Match", "Eric Yang"],
        );

        let slugs = extract_sender_slugs(&conv_dir).unwrap();
        assert!(slugs.contains("connie-lai"));
        assert!(slugs.contains("eric-yang"));
        assert_eq!(slugs.len(), 2);
    }

    #[test]
    fn test_extract_sender_slugs_no_dir() {
        let tmp = TempDir::new().unwrap();
        let conv_dir = tmp.path().join("nonexistent");
        let slugs = extract_sender_slugs(&conv_dir).unwrap();
        assert!(slugs.is_empty());
    }

    // --- Eligibility tests ---

    #[test]
    fn test_eligible_via_conversation() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        // Create root contact and conversation with that contact
        create_contact(&root_contacts, "bob", "content");
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        // bob should be synced to alice's mailbox (conversation match)
        let mb_file = mb_contacts.join("bob").join("CLAUDE.md");
        assert!(mb_file.exists(), "eligible contact should sync to mailbox");
    }

    #[test]
    fn test_ineligible_not_synced() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        // Create root contact but NO conversation in the mailbox
        create_contact(&root_contacts, "bob", "content");

        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        // bob should NOT be synced to alice's mailbox (no conversation, no explicit sharing)
        let mb_file = mb_contacts.join("bob").join("CLAUDE.md");
        assert!(
            !mb_file.exists(),
            "ineligible contact should NOT sync to mailbox"
        );
    }

    #[test]
    fn test_eligible_via_explicit_sharing() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        // Create root contact, no conversation, but explicit sharing
        create_contact(&root_contacts, "bob", "content");
        let config = config_with_sharing(&[("bob", &["alice"])]);

        sync_no_state(&root_contacts, &mailboxes, &config).unwrap();

        let mb_file = mb_contacts.join("bob").join("CLAUDE.md");
        assert!(
            mb_file.exists(),
            "explicitly shared contact should sync to mailbox"
        );
    }

    #[test]
    fn test_eligible_via_alias() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        // Create root contact and conversation with alias name
        create_contact(&root_contacts, "connie-lai", "content");
        create_conversation(
            &mb.join("conversations"),
            "thread.md",
            &["Connie Lai via Wellfound Match"],
        );

        // Without alias config, slugify("Connie Lai via Wellfound Match") = "connie-lai"
        // which matches the contact name directly. But let's test the alias path
        // with a name that doesn't match directly.
        let tmp2 = TempDir::new().unwrap();
        let root2 = tmp2.path().join("contacts");
        let mbs2 = tmp2.path().join("mailboxes");
        let mb2 = mbs2.join("alice");
        let mb2_contacts = mb2.join("contacts");
        std::fs::create_dir_all(&mb2_contacts).unwrap();

        create_contact(&root2, "cl", "content");
        create_conversation(&mb2.join("conversations"), "thread.md", &["Connie Lai"]);

        // "Connie Lai" slugifies to "connie-lai", not "cl". Without alias, no match.
        let empty_config = BTreeMap::new();
        sync_no_state(&root2, &mbs2, &empty_config).unwrap();
        assert!(
            !mb2_contacts.join("cl").join("CLAUDE.md").exists(),
            "should not match without alias"
        );

        // With alias, should match.
        let mut config = BTreeMap::new();
        config.insert(
            "cl".to_string(),
            Contact {
                aliases: vec!["Connie Lai".to_string()],
                ..Default::default()
            },
        );

        // Re-create to reset state
        let tmp3 = TempDir::new().unwrap();
        let root3 = tmp3.path().join("contacts");
        let mbs3 = tmp3.path().join("mailboxes");
        let mb3 = mbs3.join("alice");
        let mb3_contacts = mb3.join("contacts");
        std::fs::create_dir_all(&mb3_contacts).unwrap();

        create_contact(&root3, "cl", "content");
        create_conversation(&mb3.join("conversations"), "thread.md", &["Connie Lai"]);

        sync_no_state(&root3, &mbs3, &config).unwrap();
        assert!(
            mb3_contacts.join("cl").join("CLAUDE.md").exists(),
            "alias should enable sync"
        );
    }

    // --- Sync direction tests ---

    #[test]
    fn test_mailbox_to_root_always_allowed() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb_contacts = mailboxes.join("alice").join("contacts");

        // Contact only in mailbox (no root copy, no conversation match needed)
        create_contact(&mb_contacts, "bob", "mailbox content");

        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        let root_file = root_contacts.join("bob").join("CLAUDE.md");
        assert!(root_file.exists(), "mailbox→root should always be allowed");
        assert_eq!(
            std::fs::read_to_string(&root_file).unwrap(),
            "mailbox content"
        );
    }

    #[test]
    fn test_both_exist_eligible_newer_root_wins() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "newer root");
        let mb_file = create_contact(&mb_contacts, "bob", "older mailbox");

        // Make root newer
        set_mtime_offset(&mb_file, Duration::from_secs(100));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&root_file, "newer root").unwrap();

        // Create conversation to make eligible
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        assert_eq!(std::fs::read_to_string(&mb_file).unwrap(), "newer root");
    }

    #[test]
    fn test_both_exist_eligible_newer_mailbox_wins() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "older root");
        let mb_file = create_contact(&mb_contacts, "bob", "newer mailbox");

        // Make mailbox newer
        set_mtime_offset(&root_file, Duration::from_secs(100));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&mb_file, "newer mailbox").unwrap();

        // Create conversation to make eligible
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        assert_eq!(
            std::fs::read_to_string(&root_file).unwrap(),
            "newer mailbox"
        );
    }

    #[test]
    fn test_both_exist_ineligible_mailbox_newer_syncs_to_root() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "older root");
        let mb_file = create_contact(&mb_contacts, "bob", "newer mailbox");

        // Make mailbox newer
        set_mtime_offset(&root_file, Duration::from_secs(100));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&mb_file, "newer mailbox").unwrap();

        // No conversation — ineligible
        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        // Mailbox→root should still happen (always allowed)
        assert_eq!(
            std::fs::read_to_string(&root_file).unwrap(),
            "newer mailbox"
        );
    }

    #[test]
    fn test_both_exist_ineligible_root_newer_no_sync() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "newer root");
        let mb_file = create_contact(&mb_contacts, "bob", "older mailbox");

        // Make root newer
        set_mtime_offset(&mb_file, Duration::from_secs(100));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&root_file, "newer root").unwrap();

        // No conversation — ineligible
        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        // Root→mailbox should NOT happen (ineligible)
        assert_eq!(std::fs::read_to_string(&mb_file).unwrap(), "older mailbox");
    }

    #[test]
    fn test_claude_local_md_not_synced() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let bob_root = root_contacts.join("bob");
        std::fs::create_dir_all(&bob_root).unwrap();
        std::fs::write(bob_root.join("CLAUDE.md"), "public").unwrap();
        std::fs::write(bob_root.join("CLAUDE.local.md"), "private").unwrap();

        // Make eligible via conversation
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        let mb_bob = mb_contacts.join("bob");
        assert!(mb_bob.join("CLAUDE.md").exists());
        assert!(!mb_bob.join("CLAUDE.local.md").exists());
    }

    #[test]
    fn test_no_contacts_dir_no_error() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();
    }

    #[test]
    fn test_selective_mailbox_sync() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");

        // Two mailboxes
        let mb1 = mailboxes.join("alice");
        let mb1_contacts = mb1.join("contacts");
        let mb2 = mailboxes.join("charlie");
        let mb2_contacts = mb2.join("contacts");
        std::fs::create_dir_all(&mb1_contacts).unwrap();
        std::fs::create_dir_all(&mb2_contacts).unwrap();

        create_contact(&root_contacts, "bob", "root content");

        // Conversation with bob only in alice's mailbox
        create_conversation(&mb1.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        sync_no_state(&root_contacts, &mailboxes, &empty_config).unwrap();

        // bob should sync to alice (has conversation) but NOT to charlie
        assert!(mb1_contacts.join("bob").join("CLAUDE.md").exists());
        assert!(!mb2_contacts.join("bob").join("CLAUDE.md").exists());
    }

    // --- 3-way merge tests ---

    #[test]
    fn test_3way_only_root_changed() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        // Initial sync: both sides get "v1"
        let root_file = create_contact(&root_contacts, "bob", "v1");
        let mb_file = create_contact(&mb_contacts, "bob", "v1");
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        let mut cs = HashMap::new();
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Base hash recorded
        assert!(cs.contains_key("bob"));
        let base_hash = cs["bob"].mailboxes["alice"].clone();

        // Root changes to "v2", mailbox stays at "v1"
        std::fs::write(&root_file, "v2").unwrap();
        // Make root older than mailbox (3-way should still pick root because mb unchanged)
        set_mtime_offset(&root_file, Duration::from_secs(200));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&mb_file, "v1").unwrap(); // rewrite to set newer mtime

        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Root changed, mailbox didn't → take root side (even though mtime says mb is newer)
        assert_eq!(std::fs::read_to_string(&mb_file).unwrap(), "v2");
        assert_ne!(cs["bob"].mailboxes["alice"], base_hash);
    }

    #[test]
    fn test_3way_only_mailbox_changed() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "v1");
        let mb_file = create_contact(&mb_contacts, "bob", "v1");
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        let mut cs = HashMap::new();
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Mailbox changes to "v2", root stays at "v1"
        std::fs::write(&mb_file, "v2").unwrap();
        // Make mailbox older than root (3-way should still pick mb because root unchanged)
        set_mtime_offset(&mb_file, Duration::from_secs(200));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&root_file, "v1").unwrap();

        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Mailbox changed, root didn't → take mailbox side
        assert_eq!(std::fs::read_to_string(&root_file).unwrap(), "v2");
    }

    #[test]
    fn test_3way_both_changed_conflict_mtime_fallback() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "v1");
        let mb_file = create_contact(&mb_contacts, "bob", "v1");
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        let mut cs = HashMap::new();
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Both change to different content
        set_mtime_offset(&mb_file, Duration::from_secs(200));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&root_file, "v2-root").unwrap(); // root is newer

        std::fs::write(&mb_file, "v2-mb").unwrap();
        set_mtime_offset(&mb_file, Duration::from_secs(300));

        // Root is newer by mtime → should win on conflict
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        assert_eq!(std::fs::read_to_string(&mb_file).unwrap(), "v2-root");
    }

    #[test]
    fn test_3way_already_in_sync_records_hash() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        create_contact(&root_contacts, "bob", "same content");
        create_contact(&mb_contacts, "bob", "same content");
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        let mut cs = HashMap::new();
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Hash should be recorded even when already in sync
        assert!(cs.contains_key("bob"));
        assert!(cs["bob"].mailboxes.contains_key("alice"));
    }

    #[test]
    fn test_3way_new_file_records_hash() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        // Only root has the contact
        create_contact(&root_contacts, "bob", "root content");
        create_conversation(&mb.join("conversations"), "thread.md", &["Bob"]);

        let empty_config = BTreeMap::new();
        let mut cs = HashMap::new();
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Hash should be recorded for the new copy
        assert!(cs.contains_key("bob"));
        assert_eq!(cs["bob"].mailboxes["alice"], content_hash("root content"));
    }

    #[test]
    fn test_3way_ineligible_only_mb_changed_syncs_to_root() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "v1");
        let mb_file = create_contact(&mb_contacts, "bob", "v1");
        // No conversation — ineligible

        let empty_config = BTreeMap::new();
        let mut cs = HashMap::new();
        // First sync records base (both same → in sync)
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Mailbox changes, root doesn't
        std::fs::write(&mb_file, "v2-mb").unwrap();
        // Make mb older (3-way should still sync since only mb changed)
        set_mtime_offset(&mb_file, Duration::from_secs(200));
        thread::sleep(Duration::from_millis(50));
        std::fs::write(&root_file, "v1").unwrap();

        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // mb changed, root didn't → take mb side (even though mtime says root newer)
        assert_eq!(std::fs::read_to_string(&root_file).unwrap(), "v2-mb");
    }

    #[test]
    fn test_3way_ineligible_only_root_changed_no_sync() {
        let tmp = TempDir::new().unwrap();
        let root_contacts = tmp.path().join("contacts");
        let mailboxes = tmp.path().join("mailboxes");
        let mb = mailboxes.join("alice");
        let mb_contacts = mb.join("contacts");
        std::fs::create_dir_all(&mb_contacts).unwrap();

        let root_file = create_contact(&root_contacts, "bob", "v1");
        let mb_file = create_contact(&mb_contacts, "bob", "v1");
        // No conversation — ineligible

        let empty_config = BTreeMap::new();
        let mut cs = HashMap::new();
        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Root changes, mailbox doesn't
        std::fs::write(&root_file, "v2-root").unwrap();

        sync_contacts(&root_contacts, &mailboxes, &empty_config, &mut cs).unwrap();

        // Ineligible: root→mb not allowed, mb unchanged → no sync
        assert_eq!(std::fs::read_to_string(&mb_file).unwrap(), "v1");
        assert_eq!(std::fs::read_to_string(&root_file).unwrap(), "v2-root");
    }
}
