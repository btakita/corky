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
        if let Some((program, args)) = linux_notify_command(title, body) {
            let _ = std::process::Command::new(program).args(args).output();
        }
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
fn linux_notify_command(title: &str, body: &str) -> Option<(&'static str, Vec<String>)> {
    linux_notify_command_with(title, body, command_exists)
}

#[cfg(target_os = "linux")]
fn linux_notify_command_with(
    title: &str,
    body: &str,
    mut is_available: impl FnMut(&str) -> bool,
) -> Option<(&'static str, Vec<String>)> {
    if is_available("notify-desktop") {
        Some(("notify-desktop", vec![title.to_string(), body.to_string()]))
    } else if is_available("notify-send") {
        Some(("notify-send", vec![title.to_string(), body.to_string()]))
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn command_exists(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        match std::fs::metadata(candidate) {
            Ok(metadata) if metadata.is_file() => is_executable(&metadata),
            _ => false,
        }
    })
}

#[cfg(target_os = "linux")]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
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
    fn linux_oauth_notifications_prefer_notify_desktop() {
        let (program, args) = super::linux_notify_command_with("Title", "Body", |program| {
            program == "notify-desktop"
        })
        .unwrap();
        assert_eq!(program, "notify-desktop");
        assert_eq!(args, vec!["Title".to_string(), "Body".to_string()]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_oauth_notifications_fall_back_to_notify_send() {
        let (program, args) =
            super::linux_notify_command_with("Title", "Body", |program| program == "notify-send")
                .unwrap();
        assert_eq!(program, "notify-send");
        assert_eq!(args, vec!["Title".to_string(), "Body".to_string()]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_oauth_notifications_skip_when_no_command_exists() {
        assert!(super::linux_notify_command_with("Title", "Body", |_| false).is_none());
    }
}
