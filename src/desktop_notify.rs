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
        let (program, args) = linux_notify_command(title, body);
        let _ = std::process::Command::new(program).args(args).output();
    }
    #[cfg(target_os = "windows")]
    {
        let script = windows_notify_script(title, body);
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command"])
            .arg(script)
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

#[cfg(target_os = "linux")]
fn linux_notify_command(title: &str, body: &str) -> (&'static str, Vec<String>) {
    ("notify-desktop", vec![title.to_string(), body.to_string()])
}

#[cfg(target_os = "windows")]
fn windows_notify_script(title: &str, body: &str) -> String {
    format!(
        "[reflection.assembly]::LoadWithPartialName('System.Windows.Forms') | Out-Null; \
         [reflection.assembly]::LoadWithPartialName('System.Drawing') | Out-Null; \
         $n = New-Object System.Windows.Forms.NotifyIcon; \
         $n.Icon = [System.Drawing.SystemIcons]::Information; \
         $n.BalloonTipTitle = '{}'; \
         $n.BalloonTipText = '{}'; \
         $n.Visible = $true; \
         $n.ShowBalloonTip(5000); \
         Start-Sleep -Seconds 6; \
         $n.Dispose()",
        escape_powershell_single_quoted(title),
        escape_powershell_single_quoted(body),
    )
}

#[cfg(target_os = "windows")]
fn escape_powershell_single_quoted(input: &str) -> String {
    input.replace('\'', "''")
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_oauth_notifications_use_notify_desktop() {
        let (program, args) = super::linux_notify_command("Title", "Body");
        assert_eq!(program, "notify-desktop");
        assert_eq!(args, vec!["Title".to_string(), "Body".to_string()]);
    }
}
