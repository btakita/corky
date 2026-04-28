//! Best-effort desktop notifications for interactive CLI flows.

/// Show a desktop notification if the current platform supports it.
///
/// Failures are intentionally ignored so notification support never blocks the
/// primary CLI action.
#[allow(unused_variables)]
pub fn notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let title = escape_osascript(title);
        let body = escape_osascript(body);
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"{}\" with title \"{}\"",
                body, title
            ))
            .output();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .arg(title)
            .arg(body)
            .output();
    }
}

/// Notify the user that an OAuth flow needs interactive attention.
pub fn notify_oauth(service: &str) {
    notify(
        "corky OAuth required",
        &format!(
            "{} authorization needs browser approval. Check your browser or terminal.",
            service
        ),
    );
}

#[cfg(target_os = "macos")]
fn escape_osascript(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn escapes_osascript_quotes_and_backslashes() {
        let escaped = super::escape_osascript(r#"a "quote" and \ slash"#);
        assert_eq!(escaped, r#"a \"quote\" and \\ slash"#);
    }

    #[test]
    fn oauth_notification_mentions_browser_approval() {
        let body = format!(
            "{} authorization needs browser approval. Check your browser or terminal.",
            "Gmail"
        );
        assert!(body.contains("browser approval"));
        assert!(body.contains("Gmail"));
    }
}
