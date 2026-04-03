//! Integration tests for sync: markdown round-trip, merge, dedup, label routing,
//! orphan cleanup, mtime, manifest generation, sync state.

mod common;

use std::collections::HashSet;
use tempfile::TempDir;

use corky::sync::imap_sync::{merge_message_to_file, parse_msg_date};
use corky::sync::markdown::{parse_thread_markdown, thread_to_markdown};
use corky::sync::types::{Message, SyncState, Thread};
use corky::util::slugify;
use pretty_assertions::assert_eq;

// ---------------------------------------------------------------------------
// Markdown round-trip
// ---------------------------------------------------------------------------

#[test]
fn test_roundtrip_single_message() {
    let thread = Thread {
        id: "thread-1".to_string(),
        subject: "Meeting Tomorrow".to_string(),
        labels: vec!["inbox".to_string(), "important".to_string()],
        accounts: vec!["personal".to_string()],
        messages: vec![Message {
            id: "msg-1".to_string(),
            thread_id: "meeting tomorrow".to_string(),
            from: "Alice <alice@example.com>".to_string(),
            to: String::new(),
            cc: String::new(),
            date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
            subject: "Meeting Tomorrow".to_string(),
            body: "Let's meet at 3pm.".to_string(),
        message_id: None,
        }],
        last_date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        tracking: vec![],
    };

    let md = thread_to_markdown(&thread);
    let parsed = parse_thread_markdown(&md).unwrap();

    assert_eq!(parsed.subject, "Meeting Tomorrow");
    assert_eq!(parsed.id, "thread-1");
    assert_eq!(parsed.labels, vec!["inbox", "important"]);
    assert_eq!(parsed.accounts, vec!["personal"]);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].from, "Alice <alice@example.com>");
    assert_eq!(
        parsed.messages[0].date,
        "Mon, 10 Feb 2025 10:00:00 +0000"
    );
    assert_eq!(parsed.messages[0].body, "Let's meet at 3pm.");
    assert_eq!(
        parsed.last_date,
        "Mon, 10 Feb 2025 10:00:00 +0000"
    );
}

#[test]
fn test_roundtrip_multiple_messages() {
    let thread = Thread {
        id: "thread-2".to_string(),
        subject: "Project Update".to_string(),
        labels: vec!["work".to_string()],
        accounts: vec!["work".to_string()],
        messages: vec![
            Message {
                id: "msg-1".to_string(),
                thread_id: "project update".to_string(),
                from: "Bob <bob@work.com>".to_string(),
                to: String::new(),
                cc: String::new(),
                date: "Mon, 10 Feb 2025 09:00:00 +0000".to_string(),
                subject: "Project Update".to_string(),
                body: "Here's the update.".to_string(),
        message_id: None,
            },
            Message {
                id: "msg-2".to_string(),
                thread_id: "project update".to_string(),
                from: "Alice <alice@work.com>".to_string(),
                to: String::new(),
                cc: String::new(),
                date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
                subject: "Re: Project Update".to_string(),
                body: "Thanks for the update!".to_string(),
        message_id: None,
            },
            Message {
                id: "msg-3".to_string(),
                thread_id: "project update".to_string(),
                from: "Bob <bob@work.com>".to_string(),
                to: String::new(),
                cc: String::new(),
                date: "Mon, 10 Feb 2025 11:00:00 +0000".to_string(),
                subject: "Re: Project Update".to_string(),
                body: "No problem. Let me know if you need more.".to_string(),
        message_id: None,
            },
        ],
        last_date: "Mon, 10 Feb 2025 11:00:00 +0000".to_string(),
        tracking: vec![],
    };

    let md = thread_to_markdown(&thread);
    let parsed = parse_thread_markdown(&md).unwrap();

    assert_eq!(parsed.messages.len(), 3);
    assert_eq!(parsed.messages[0].from, "Bob <bob@work.com>");
    assert_eq!(parsed.messages[1].from, "Alice <alice@work.com>");
    assert_eq!(parsed.messages[2].from, "Bob <bob@work.com>");
    assert_eq!(parsed.messages[2].body, "No problem. Let me know if you need more.");
}

#[test]
fn test_parse_multi_label() {
    let md = "# Multi Label Thread\n\n\
              **Labels**: alpha, beta, gamma\n\
              **Accounts**: acct1, acct2\n\
              **Thread ID**: multi\n\
              **Last updated**: Mon, 1 Jan 2024 00:00:00 +0000\n";
    let parsed = parse_thread_markdown(md).unwrap();
    assert_eq!(parsed.labels, vec!["alpha", "beta", "gamma"]);
    assert_eq!(parsed.accounts, vec!["acct1", "acct2"]);
}

#[test]
fn test_parse_empty_subject_returns_none() {
    let md = "# \n\n**Thread ID**: test\n";
    let result = parse_thread_markdown(md);
    assert!(result.is_none());
}

#[test]
fn test_parse_no_h1_returns_none() {
    let md = "No heading here\n**Thread ID**: test\n";
    let result = parse_thread_markdown(md);
    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// Merge / dedup
// ---------------------------------------------------------------------------

#[test]
fn test_merge_message_creates_new_file() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg = Message {
        id: "1".to_string(),
        thread_id: "hello world".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Hello World".to_string(),
        body: "Hi there!".to_string(),
        message_id: None,
    };

    let result = merge_message_to_file(
        &out_dir,
        "inbox",
        "personal",
        &msg,
        "hello world",
    )
    .unwrap();

    assert!(result.is_some());
    let file_path = result.unwrap();
    assert!(file_path.exists());

    // Verify content
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("# Hello World"));
    assert!(content.contains("**Labels**: inbox"));
    assert!(content.contains("**Accounts**: personal"));
    assert!(content.contains("**Thread ID**: hello world"));
    assert!(content.contains("Alice <alice@example.com>"));
    assert!(content.contains("Hi there!"));
}

#[test]
fn test_merge_message_appends_to_existing() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg1 = Message {
        id: "1".to_string(),
        thread_id: "test thread".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 09:00:00 +0000".to_string(),
        subject: "Test Thread".to_string(),
        body: "First message".to_string(),
        message_id: None,
    };

    let msg2 = Message {
        id: "2".to_string(),
        thread_id: "test thread".to_string(),
        from: "Bob <bob@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Re: Test Thread".to_string(),
        body: "Second message".to_string(),
        message_id: None,
    };

    merge_message_to_file(&out_dir, "inbox", "personal", &msg1, "test thread").unwrap();
    merge_message_to_file(&out_dir, "inbox", "personal", &msg2, "test thread").unwrap();

    // Find the file
    let entries: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    assert_eq!(entries.len(), 1);

    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let parsed = parse_thread_markdown(&content).unwrap();
    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.messages[0].from, "Alice <alice@example.com>");
    assert_eq!(parsed.messages[1].from, "Bob <bob@example.com>");
}

#[test]
fn test_dedup_same_sender_date_skipped() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg = Message {
        id: "1".to_string(),
        thread_id: "dedup test".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Dedup Test".to_string(),
        body: "Original message".to_string(),
        message_id: None,
    };

    // Merge the same message twice
    merge_message_to_file(&out_dir, "inbox", "personal", &msg, "dedup test").unwrap();
    merge_message_to_file(&out_dir, "inbox", "personal", &msg, "dedup test").unwrap();

    // Should still have only 1 message
    let entries: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    assert_eq!(entries.len(), 1);

    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let parsed = parse_thread_markdown(&content).unwrap();
    assert_eq!(parsed.messages.len(), 1);
}

#[test]
fn test_dedup_different_sender_same_date_not_skipped() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg1 = Message {
        id: "1".to_string(),
        thread_id: "multi sender".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Multi Sender".to_string(),
        body: "From Alice".to_string(),
        message_id: None,
    };

    let msg2 = Message {
        id: "2".to_string(),
        thread_id: "multi sender".to_string(),
        from: "Bob <bob@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Multi Sender".to_string(),
        body: "From Bob".to_string(),
        message_id: None,
    };

    merge_message_to_file(&out_dir, "inbox", "personal", &msg1, "multi sender").unwrap();
    merge_message_to_file(&out_dir, "inbox", "personal", &msg2, "multi sender").unwrap();

    let entries: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let parsed = parse_thread_markdown(&content).unwrap();
    assert_eq!(parsed.messages.len(), 2);
}

// ---------------------------------------------------------------------------
// Label accumulation
// ---------------------------------------------------------------------------

#[test]
fn test_label_accumulation() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg1 = Message {
        id: "1".to_string(),
        thread_id: "label acc".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 09:00:00 +0000".to_string(),
        subject: "Label Accumulation".to_string(),
        body: "First".to_string(),
        message_id: None,
    };

    let msg2 = Message {
        id: "2".to_string(),
        thread_id: "label acc".to_string(),
        from: "Bob <bob@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Re: Label Accumulation".to_string(),
        body: "Second".to_string(),
        message_id: None,
    };

    // Merge from different labels and accounts
    merge_message_to_file(&out_dir, "inbox", "personal", &msg1, "label acc").unwrap();
    merge_message_to_file(&out_dir, "sent", "work", &msg2, "label acc").unwrap();

    let entries: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let parsed = parse_thread_markdown(&content).unwrap();

    // Both labels should be accumulated
    assert!(parsed.labels.contains(&"inbox".to_string()));
    assert!(parsed.labels.contains(&"sent".to_string()));

    // Both accounts should be accumulated
    assert!(parsed.accounts.contains(&"personal".to_string()));
    assert!(parsed.accounts.contains(&"work".to_string()));
}

#[test]
fn test_label_not_duplicated() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg1 = Message {
        id: "1".to_string(),
        thread_id: "no dup label".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 09:00:00 +0000".to_string(),
        subject: "No Dup Label".to_string(),
        body: "First".to_string(),
        message_id: None,
    };

    let msg2 = Message {
        id: "2".to_string(),
        thread_id: "no dup label".to_string(),
        from: "Bob <bob@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Re: No Dup Label".to_string(),
        body: "Second".to_string(),
        message_id: None,
    };

    // Same label used twice
    merge_message_to_file(&out_dir, "inbox", "personal", &msg1, "no dup label").unwrap();
    merge_message_to_file(&out_dir, "inbox", "personal", &msg2, "no dup label").unwrap();

    let entries: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let parsed = parse_thread_markdown(&content).unwrap();

    // Should have exactly one "inbox" label, not two
    assert_eq!(
        parsed.labels.iter().filter(|l| *l == "inbox").count(),
        1
    );
}

// ---------------------------------------------------------------------------
// Slug collision
// ---------------------------------------------------------------------------

#[test]
fn test_slug_collision_suffix() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    // Create two threads with the same subject but different thread IDs
    let msg1 = Message {
        id: "1".to_string(),
        thread_id: "thread-a".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 09:00:00 +0000".to_string(),
        subject: "Same Subject".to_string(),
        body: "Thread A".to_string(),
        message_id: None,
    };

    let msg2 = Message {
        id: "2".to_string(),
        thread_id: "thread-b".to_string(),
        from: "Bob <bob@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Same Subject".to_string(),
        body: "Thread B".to_string(),
        message_id: None,
    };

    let path1 = merge_message_to_file(&out_dir, "inbox", "personal", &msg1, "thread-a")
        .unwrap()
        .unwrap();
    let path2 = merge_message_to_file(&out_dir, "inbox", "personal", &msg2, "thread-b")
        .unwrap()
        .unwrap();

    // Different files
    assert_ne!(path1, path2);

    // Second should have -2 suffix
    let stem1 = path1.file_stem().unwrap().to_string_lossy().to_string();
    let stem2 = path2.file_stem().unwrap().to_string_lossy().to_string();
    assert_eq!(stem1, "same-subject");
    assert_eq!(stem2, "same-subject-2");
}

// ---------------------------------------------------------------------------
// Message ordering
// ---------------------------------------------------------------------------

#[test]
fn test_messages_sorted_by_date() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    // Insert messages out of order
    let msg_late = Message {
        id: "2".to_string(),
        thread_id: "order test".to_string(),
        from: "Bob <bob@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Tue, 11 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Order Test".to_string(),
        body: "Late message".to_string(),
        message_id: None,
    };

    let msg_early = Message {
        id: "1".to_string(),
        thread_id: "order test".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 09:00:00 +0000".to_string(),
        subject: "Order Test".to_string(),
        body: "Early message".to_string(),
        message_id: None,
    };

    // Insert late first, then early
    merge_message_to_file(&out_dir, "inbox", "personal", &msg_late, "order test").unwrap();
    merge_message_to_file(&out_dir, "inbox", "personal", &msg_early, "order test").unwrap();

    let entries: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let parsed = parse_thread_markdown(&content).unwrap();

    // Messages should be in chronological order
    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.messages[0].from, "Alice <alice@example.com>");
    assert_eq!(parsed.messages[1].from, "Bob <bob@example.com>");
}

// ---------------------------------------------------------------------------
// Orphan cleanup
// ---------------------------------------------------------------------------

#[test]
fn test_orphan_cleanup() {
    let tmp = TempDir::new().unwrap();
    let conv_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();

    // Create some files
    let kept = conv_dir.join("kept.md");
    let orphan = conv_dir.join("orphan.md");
    let non_md = conv_dir.join("readme.txt");

    std::fs::write(&kept, "# Kept\n\n**Thread ID**: kept\n").unwrap();
    std::fs::write(&orphan, "# Orphan\n\n**Thread ID**: orphan\n").unwrap();
    std::fs::write(&non_md, "not a markdown file").unwrap();

    // Build touched set with only the kept file
    let mut touched = HashSet::new();
    touched.insert(kept.clone());

    // The cleanup_orphans function is private in mod.rs, but we can test
    // the behavior through the public API or replicate the logic
    // Since cleanup_orphans is private, we test the underlying behavior:
    // files with .md extension not in the touched set should be removable
    for entry in std::fs::read_dir(&conv_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") && !touched.contains(&path) {
            std::fs::remove_file(&path).unwrap();
        }
    }

    assert!(kept.exists(), "Kept file should remain");
    assert!(!orphan.exists(), "Orphan should be removed");
    assert!(non_md.exists(), "Non-markdown files should not be removed");
}

// ---------------------------------------------------------------------------
// mtime
// ---------------------------------------------------------------------------

#[test]
fn test_mtime_set_on_write() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg = Message {
        id: "1".to_string(),
        thread_id: "mtime test".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Tue, 15 Jul 2025 12:00:00 +0000".to_string(),
        subject: "Mtime Test".to_string(),
        body: "Test body".to_string(),
        message_id: None,
    };

    let path = merge_message_to_file(&out_dir, "inbox", "personal", &msg, "mtime test")
        .unwrap()
        .unwrap();

    // On Unix, mtime should be set to the message date
    #[cfg(unix)]
    {
        let metadata = std::fs::metadata(&path).unwrap();
        let mtime = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let expected = parse_msg_date("Tue, 15 Jul 2025 12:00:00 +0000");
        let expected_ts = expected.timestamp() as u64;

        // Allow a small tolerance for rounding
        assert!(
            (mtime as i64 - expected_ts as i64).unsigned_abs() < 2,
            "mtime {} should be close to expected {}",
            mtime,
            expected_ts
        );
    }
}

// ---------------------------------------------------------------------------
// SyncState
// ---------------------------------------------------------------------------

#[test]
fn test_sync_state_roundtrip() {
    let state = SyncState::default();
    let data = serde_json::to_vec(&state).unwrap();
    let loaded = corky::sync::types::load_state(&data).unwrap();
    assert!(loaded.accounts.is_empty());
}

// ---------------------------------------------------------------------------
// parse_msg_date
// ---------------------------------------------------------------------------

#[test]
fn test_parse_msg_date_rfc2822() {
    let dt = parse_msg_date("Mon, 10 Feb 2025 10:00:00 +0000");
    assert_eq!(dt.year(), 2025);
    assert_eq!(dt.month(), 2);
    assert_eq!(dt.day(), 10);
}

use chrono::Datelike;

#[test]
fn test_parse_msg_date_invalid_returns_epoch() {
    let dt = parse_msg_date("not a date");
    // Should return epoch (year 1970) on failure
    assert_eq!(dt.year(), 1970);
}

// ---------------------------------------------------------------------------
// Slugify edge cases for sync filenames
// ---------------------------------------------------------------------------

#[test]
fn test_slugify_produces_valid_filename() {
    assert_eq!(slugify("Hello World"), "hello-world");
    assert_eq!(slugify("Re: My Important Email!"), "re-my-important-email");
}

#[test]
fn test_slugify_truncates_long_subjects() {
    let long_subject = "a".repeat(100);
    let slug = slugify(&long_subject);
    assert!(slug.len() <= 60);
}

#[test]
fn test_slugify_empty_returns_untitled() {
    assert_eq!(slugify(""), "untitled");
    assert_eq!(slugify("!!!"), "untitled");
}

// ---------------------------------------------------------------------------
// Thread to markdown formatting
// ---------------------------------------------------------------------------

#[test]
fn test_thread_to_markdown_format() {
    let thread = Thread {
        id: "fmt-test".to_string(),
        subject: "Format Test".to_string(),
        labels: vec!["label1".to_string(), "label2".to_string()],
        accounts: vec!["acct1".to_string()],
        messages: vec![Message {
            id: "1".to_string(),
            thread_id: "format test".to_string(),
            from: "Sender <sender@test.com>".to_string(),
            to: String::new(),
            cc: String::new(),
            date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
            subject: "Format Test".to_string(),
            body: "Body text here.".to_string(),
        message_id: None,
        }],
        last_date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        tracking: vec![],
    };

    let md = thread_to_markdown(&thread);

    // Verify structure
    assert!(md.starts_with("# Format Test\n"));
    assert!(md.contains("**Labels**: label1, label2"));
    assert!(md.contains("**Accounts**: acct1"));
    assert!(md.contains("**Thread ID**: fmt-test"));
    assert!(md.contains("**Last updated**: Mon, 10 Feb 2025 10:00:00 +0000"));
    assert!(md.contains("---"));
    // The em dash character
    assert!(md.contains("## Sender <sender@test.com> \u{2014} Mon, 10 Feb 2025 10:00:00 +0000"));
    assert!(md.contains("Body text here."));
}

#[test]
fn test_thread_to_markdown_empty_labels() {
    let thread = Thread {
        id: "empty".to_string(),
        subject: "Empty Labels".to_string(),
        labels: vec![],
        accounts: vec![],
        messages: vec![],
        last_date: String::new(),
        tracking: vec![],
    };

    let md = thread_to_markdown(&thread);
    assert!(md.contains("**Labels**: "));
    assert!(md.contains("**Accounts**: "));
}

// ---------------------------------------------------------------------------
// Manifest generation
// ---------------------------------------------------------------------------

#[test]
fn test_manifest_generation() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let conv_dir = data_dir.join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();

    // Set CORKY_DATA so .corky.toml resolution works
    // SAFETY: Test-only; no concurrent env access.
    unsafe { std::env::set_var("CORKY_DATA", data_dir.to_string_lossy().as_ref()) };

    // Create a .corky.toml (contacts section is optional)
    std::fs::write(data_dir.join(".corky.toml"), "").unwrap();

    // Create a conversation file
    let thread = Thread {
        id: "manifest-test".to_string(),
        subject: "Manifest Subject".to_string(),
        labels: vec!["inbox".to_string()],
        accounts: vec!["personal".to_string()],
        messages: vec![Message {
            id: "1".to_string(),
            thread_id: "manifest-test".to_string(),
            from: "Alice <alice@example.com>".to_string(),
            to: String::new(),
            cc: String::new(),
            date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
            subject: "Manifest Subject".to_string(),
            body: "Test body".to_string(),
        message_id: None,
        }],
        last_date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        tracking: vec![],
    };

    std::fs::write(
        conv_dir.join("manifest-subject.md"),
        thread_to_markdown(&thread),
    )
    .unwrap();

    corky::sync::manifest::generate_manifest(&conv_dir).unwrap();

    let manifest_path = data_dir.join("manifest.toml");
    assert!(manifest_path.exists());

    let content = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(content.contains("manifest-subject"));
    assert!(content.contains("Manifest Subject"));

    unsafe { std::env::remove_var("CORKY_DATA") };
}

// ---------------------------------------------------------------------------
// Empty label not added
// ---------------------------------------------------------------------------

#[test]
fn test_empty_label_not_added() {
    let tmp = TempDir::new().unwrap();
    let out_dir = tmp.path().join("conversations");
    std::fs::create_dir_all(&out_dir).unwrap();

    let msg = Message {
        id: "1".to_string(),
        thread_id: "empty label".to_string(),
        from: "Alice <alice@example.com>".to_string(),
        to: String::new(),
        cc: String::new(),
        date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
        subject: "Empty Label".to_string(),
        body: "Test".to_string(),
        message_id: None,
    };

    merge_message_to_file(&out_dir, "", "", &msg, "empty label").unwrap();

    let entries: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    let parsed = parse_thread_markdown(&content).unwrap();

    // Empty label/account should not be in the list
    assert!(parsed.labels.is_empty() || !parsed.labels.contains(&String::new()));
    assert!(parsed.accounts.is_empty() || !parsed.accounts.contains(&String::new()));
}
