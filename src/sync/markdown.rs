//! Thread ↔ Markdown serialization/parsing.

use once_cell::sync::Lazy;
use regex::Regex;

use super::types::{Message, Thread};
use crate::util::thread_key_from_subject;

static META_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\*\*(.+?)\*\*:[ ]*(.+)$").unwrap());
static MSG_HEADER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^## (.+?) \u{2014} (.+)$").unwrap());

/// Serialize a Thread to Markdown.
pub fn thread_to_markdown(thread: &Thread) -> String {
    let labels_str = thread.labels.join(", ");
    let accounts_str = thread.accounts.join(", ");
    let mut lines = vec![
        format!("# {}", thread.subject),
        String::new(),
        format!("**Labels**: {}", labels_str),
        format!("**Accounts**: {}", accounts_str),
        format!("**Thread ID**: {}", thread.id),
        format!("**Last updated**: {}", thread.last_date),
    ];
    if !thread.tracking.is_empty() {
        lines.push(format!("**Tracking**: {}", thread.tracking.join(", ")));
    }
    lines.push(String::new());
    for msg in &thread.messages {
        lines.push("---".to_string());
        lines.push(String::new());
        lines.push(format!("## {} \u{2014} {}", msg.from, msg.date));
        lines.push(String::new());
        if !msg.to.is_empty() {
            lines.push(format!("**To**: {}", msg.to));
        }
        if !msg.cc.is_empty() {
            lines.push(format!("**CC**: {}", msg.cc));
        }
        if !msg.to.is_empty() || !msg.cc.is_empty() {
            lines.push(String::new());
        }
        lines.push(msg.body.trim().to_string());
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Parse a conversation markdown file back into a Thread.
pub fn parse_thread_markdown(text: &str) -> Option<Thread> {
    let lines: Vec<&str> = text.split('\n').collect();

    // Extract subject from first H1
    let subject = lines.iter().find_map(|line| {
        line.strip_prefix("# ")
            .map(|s| s.trim().to_string())
    })?;

    if subject.is_empty() {
        return None;
    }

    // Extract metadata
    let mut meta = std::collections::HashMap::new();
    for cap in META_RE.captures_iter(text) {
        meta.insert(
            cap[1].to_string(),
            cap[2].trim().to_string(),
        );
    }

    let thread_id = meta.get("Thread ID").cloned().unwrap_or_default();
    let last_date = meta.get("Last updated").cloned().unwrap_or_default();

    let labels = meta.get("Labels")
        .map(|s| s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    // Parse accounts
    let accounts = meta
        .get("Accounts")
        .map(|s| {
            s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let tracking = meta
        .get("Tracking")
        .map(|s| s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    // Split into message sections on "## Sender — Date"
    let mut messages: Vec<Message> = Vec::new();
    let mut current_from = String::new();
    let mut current_date = String::new();
    let mut current_to = String::new();
    let mut current_cc = String::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_message = false;
    let mut in_msg_meta = false; // after header, before body

    for line in &lines {
        if let Some(cap) = MSG_HEADER_RE.captures(line) {
            // Save previous message
            if in_message {
                messages.push(Message {
                    id: String::new(),
                    thread_id: thread_key_from_subject(&subject),
                    from: current_from.clone(),
                    to: current_to.clone(),
                    cc: current_cc.clone(),
                    date: current_date.clone(),
                    subject: subject.clone(),
                    body: body_lines.join("\n").trim().to_string(),
                });
            }
            current_from = cap[1].to_string();
            current_date = cap[2].to_string();
            current_to = String::new();
            current_cc = String::new();
            body_lines.clear();
            in_message = true;
            in_msg_meta = true;
        } else if in_message {
            if in_msg_meta {
                if let Some(cap) = META_RE.captures(line) {
                    match cap[1].to_string().as_str() {
                        "To" => current_to = cap[2].trim().to_string(),
                        "CC" => current_cc = cap[2].trim().to_string(),
                        _ => {} // ignore other per-message metadata
                    }
                } else if line.trim().is_empty() {
                    // blank lines between metadata and body are consumed
                } else {
                    // first non-metadata, non-blank line — switch to body
                    in_msg_meta = false;
                    if line.trim() != "---" {
                        body_lines.push(line);
                    }
                }
            } else if line.trim() != "---" {
                body_lines.push(line);
            }
        }
    }

    // Save last message
    if in_message {
        messages.push(Message {
            id: String::new(),
            thread_id: thread_key_from_subject(&subject),
            from: current_from,
            to: current_to,
            cc: current_cc,
            date: current_date,
            subject: subject.clone(),
            body: body_lines.join("\n").trim().to_string(),
        });
    }

    Some(Thread {
        id: thread_id,
        subject,
        labels,
        accounts,
        messages,
        last_date,
        tracking,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let thread = Thread {
            id: "test-thread".to_string(),
            subject: "Hello World".to_string(),
            labels: vec!["inbox".to_string()],
            accounts: vec!["personal".to_string()],
            messages: vec![Message {
                id: "1".to_string(),
                thread_id: "hello world".to_string(),
                from: "Alice <alice@example.com>".to_string(),
                to: String::new(),
                cc: String::new(),
                date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
                subject: "Hello World".to_string(),
                body: "Hello there!".to_string(),
            }],
            last_date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
            tracking: vec![],
        };

        let md = thread_to_markdown(&thread);
        let parsed = parse_thread_markdown(&md).unwrap();

        assert_eq!(parsed.subject, "Hello World");
        assert_eq!(parsed.id, "test-thread");
        assert_eq!(parsed.labels, vec!["inbox"]);
        assert_eq!(parsed.accounts, vec!["personal"]);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].from, "Alice <alice@example.com>");
        assert_eq!(parsed.messages[0].body, "Hello there!");
    }

    #[test]
    fn test_roundtrip_with_to_cc() {
        let thread = Thread {
            id: "test-thread".to_string(),
            subject: "Hello World".to_string(),
            labels: vec!["inbox".to_string()],
            accounts: vec!["personal".to_string()],
            messages: vec![Message {
                id: "1".to_string(),
                thread_id: "hello world".to_string(),
                from: "Alice <alice@example.com>".to_string(),
                to: "Bob <bob@example.com>, Charlie <charlie@example.com>".to_string(),
                cc: "Dave <dave@example.com>".to_string(),
                date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
                subject: "Hello World".to_string(),
                body: "Hello there!".to_string(),
            }],
            last_date: "Mon, 10 Feb 2025 10:00:00 +0000".to_string(),
            tracking: vec![],
        };

        let md = thread_to_markdown(&thread);
        assert!(md.contains("**To**: Bob <bob@example.com>, Charlie <charlie@example.com>"));
        assert!(md.contains("**CC**: Dave <dave@example.com>"));

        let parsed = parse_thread_markdown(&md).unwrap();
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].to, "Bob <bob@example.com>, Charlie <charlie@example.com>");
        assert_eq!(parsed.messages[0].cc, "Dave <dave@example.com>");
        assert_eq!(parsed.messages[0].body, "Hello there!");
    }

    #[test]
    fn test_parse_old_format_no_to_cc() {
        // Old format without To/CC lines should parse with empty to/cc
        let md = "# Subject\n\n**Labels**: inbox\n**Thread ID**: test\n**Last updated**: Mon, 1 Jan 2024 00:00:00 +0000\n\n---\n\n## Alice <alice@example.com> \u{2014} Mon, 1 Jan 2024 00:00:00 +0000\n\nHello there!\n";
        let parsed = parse_thread_markdown(md).unwrap();
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].to, "");
        assert_eq!(parsed.messages[0].cc, "");
        assert_eq!(parsed.messages[0].body, "Hello there!");
    }

    #[test]
    fn test_parse_multi_label() {
        let md = "# Subject\n\n**Labels**: label1, label2\n**Thread ID**: test\n**Last updated**: Mon, 1 Jan 2024 00:00:00 +0000\n";
        let parsed = parse_thread_markdown(md).unwrap();
        assert_eq!(parsed.labels, vec!["label1", "label2"]);
    }

    #[test]
    fn test_tracking_roundtrip() {
        let thread = Thread {
            id: "track-test".to_string(),
            subject: "Tracked Email".to_string(),
            labels: vec!["inbox".to_string()],
            accounts: vec!["test".to_string()],
            messages: vec![],
            last_date: String::new(),
            tracking: vec!["sendgrid.net".to_string(), "outlier.ai".to_string()],
        };

        let md = thread_to_markdown(&thread);
        assert!(md.contains("**Tracking**: sendgrid.net, outlier.ai"));

        let parsed = parse_thread_markdown(&md).unwrap();
        assert_eq!(parsed.tracking, vec!["sendgrid.net", "outlier.ai"]);
    }

    #[test]
    fn test_no_tracking_line_when_empty() {
        let thread = Thread {
            id: "no-track".to_string(),
            subject: "Clean Email".to_string(),
            labels: vec![],
            accounts: vec![],
            messages: vec![],
            last_date: String::new(),
            tracking: vec![],
        };

        let md = thread_to_markdown(&thread);
        assert!(!md.contains("**Tracking**"));
    }
}
