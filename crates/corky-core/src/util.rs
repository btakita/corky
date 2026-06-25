use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Command;

// #ckyunicodeslug: keep Unicode letters and digits (CJK, Cyrillic, accented
// Latin, …) instead of only ASCII `[a-z0-9]`, so non-Latin subjects produce a
// meaningful slug rather than all collapsing to `untitled`. Everything else
// (punctuation, whitespace, emoji) still becomes a hyphen separator.
static SLUG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^\p{Alphabetic}\p{Nd}]+").unwrap());
static THREAD_KEY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(re|fwd?):\s*").unwrap());

/// Generate a slug from text.
///
/// Lowercases (Unicode-aware), replaces runs of non-(letter/digit) characters
/// with hyphens, trims hyphens, truncates to 60 bytes on a char boundary.
/// Unicode letters/digits are preserved (CJK, Cyrillic, …); only subjects with
/// no letters or digits at all (e.g. emoji-only) fall back to "untitled".
pub fn slugify(text: &str) -> String {
    let lower = text.to_lowercase();
    let slugged = SLUG_RE.replace_all(&lower, "-");
    let trimmed = slugged.trim_matches('-');
    let truncated = if trimmed.len() > 60 {
        // Don't split in the middle of a multi-byte char
        let mut end = 60;
        while !trimmed.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &trimmed[..end]
    } else {
        trimmed
    };
    if truncated.is_empty() {
        "untitled".to_string()
    } else {
        truncated.to_string()
    }
}

/// Derive a thread key from a subject line.
///
/// Strips one `Re:` or `Fwd:` prefix (case-insensitive), then lowercases.
pub fn thread_key_from_subject(subject: &str) -> String {
    let trimmed = subject.trim().to_lowercase();
    THREAD_KEY_RE.replace(&trimmed, "").to_string()
}

/// Run a shell command, returning (stdout, stderr, exit_code).
pub fn run_cmd(args: &[&str]) -> anyhow::Result<(String, String, i32)> {
    let output = Command::new(args[0]).args(&args[1..]).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((stdout, stderr, code))
}

/// Run a shell command, printing it first. Returns Ok on success, Err on failure.
pub fn run_cmd_checked(args: &[&str]) -> anyhow::Result<String> {
    let cmd_str = args.join(" ");
    println!("  $ {}", cmd_str);
    let (stdout, stderr, code) = run_cmd(args)?;
    if code != 0 {
        anyhow::bail!(
            "Command failed (exit {}): {}\n{}",
            code,
            cmd_str,
            stderr.trim()
        );
    }
    Ok(stdout)
}

/// Resolve a secret value: inline string first, then shell command.
///
/// Returns `Ok(value)` if either source yields a non-empty string.
/// Returns `Err` with `context` message if both are empty.
pub fn resolve_secret(inline: &str, cmd: &str, context: &str) -> anyhow::Result<String> {
    if !inline.is_empty() {
        return Ok(inline.to_string());
    }
    if !cmd.is_empty() {
        let output = Command::new("sh").arg("-c").arg(cmd).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "{} command failed: {}",
                context,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            anyhow::bail!("{} command produced empty output", context);
        }
        return Ok(value);
    }
    anyhow::bail!("{}", context)
}

/// Take the first `max` characters of `s` without splitting a multi-byte
/// char boundary.
///
/// Unlike byte slicing (`&s[..max]`), this never panics: it is safe on short
/// strings (returns the whole string) and on multi-byte input (CJK, emoji,
/// Cyrillic). `max` counts Unicode scalar values, not bytes.
pub fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Truncate a string for preview display, adding "..." if truncated.
pub fn truncate_preview(s: &str, max: usize) -> String {
    let first_line = s.lines().next().unwrap_or("").trim();
    if first_line.len() <= max {
        first_line.to_string()
    } else {
        let mut end = max.saturating_sub(3);
        while !first_line.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &first_line[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn test_slugify_special_chars() {
        assert_eq!(slugify("Re: My Important Email!"), "re-my-important-email");
    }

    #[test]
    fn test_slugify_truncation() {
        let long = "a".repeat(100);
        assert_eq!(slugify(&long).len(), 60);
    }

    #[test]
    fn test_slugify_empty() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("!!!"), "untitled");
    }

    #[test]
    fn test_slugify_unicode_letters_preserved() {
        // #ckyunicodeslug: non-Latin subjects must not collapse to "untitled".
        assert_eq!(slugify("你好 世界"), "你好-世界");
        assert_eq!(slugify("Привет мир"), "привет-мир");
        assert_eq!(slugify("Café déjà vu"), "café-déjà-vu");
        // Mixed scripts + digits.
        assert_eq!(slugify("Q3 报告 2026"), "q3-报告-2026");
        // Emoji are not letters/digits → become separators; with surrounding
        // text the text survives; emoji-only still falls back.
        assert_eq!(slugify("hi 👍 there"), "hi-there");
        assert_eq!(slugify("👍🏽🎉"), "untitled");
    }

    #[test]
    fn test_truncate_chars_ascii() {
        assert_eq!(truncate_chars("hello world", 5), "hello");
        // Shorter than max returns the whole string (no panic, no padding).
        assert_eq!(truncate_chars("hi", 10), "hi");
        assert_eq!(truncate_chars("", 10), "");
    }

    #[test]
    fn test_truncate_chars_multibyte_no_panic() {
        // Byte-slicing &s[..10] here would panic mid-codepoint or on a short
        // byte length; char-based truncation must not.
        let cjk = "你好世界你好世界你好世界"; // 12 CJK chars, 36 bytes
        assert_eq!(truncate_chars(cjk, 4), "你好世界");
        let emoji = "👍🏽👍🏽👍🏽"; // multi-byte grapheme clusters
        // Takes 2 scalar values without splitting a UTF-8 boundary.
        assert_eq!(truncate_chars(emoji, 2).chars().count(), 2);
        // due-string style: a date shorter than 10 chars must not panic.
        assert_eq!(truncate_chars("2026", 10), "2026");
        // RFC3339 date truncated to the YYYY-MM-DD prefix.
        assert_eq!(truncate_chars("2026-06-24T10:00:00Z", 10), "2026-06-24");
    }

    #[test]
    fn test_thread_key_strips_re() {
        assert_eq!(thread_key_from_subject("Re: Hello World"), "hello world");
    }

    #[test]
    fn test_thread_key_strips_fwd() {
        assert_eq!(thread_key_from_subject("Fwd: Hello World"), "hello world");
    }

    #[test]
    fn test_thread_key_strips_fw() {
        assert_eq!(thread_key_from_subject("Fw: Hello World"), "hello world");
    }

    #[test]
    fn test_thread_key_case_insensitive() {
        assert_eq!(thread_key_from_subject("RE: Hello World"), "hello world");
    }

    #[test]
    fn test_thread_key_no_prefix() {
        assert_eq!(thread_key_from_subject("Hello World"), "hello world");
    }

    #[test]
    fn test_resolve_secret_inline() {
        let result = resolve_secret("my-secret", "", "unused context");
        assert_eq!(result.unwrap(), "my-secret");
    }

    #[test]
    fn test_resolve_secret_inline_wins_over_cmd() {
        let result = resolve_secret("inline-val", "echo cmd-val", "unused");
        assert_eq!(result.unwrap(), "inline-val");
    }

    #[test]
    fn test_resolve_secret_cmd() {
        let result = resolve_secret("", "echo hello-from-cmd", "unused");
        assert_eq!(result.unwrap(), "hello-from-cmd");
    }

    #[test]
    fn test_resolve_secret_cmd_strips_whitespace() {
        let result = resolve_secret("", "printf '  padded  \n'", "unused");
        assert_eq!(result.unwrap(), "padded");
    }

    #[test]
    fn test_resolve_secret_both_empty() {
        let result = resolve_secret("", "", "no secret configured");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no secret configured")
        );
    }

    #[test]
    fn test_resolve_secret_cmd_empty_output() {
        let result = resolve_secret("", "echo -n ''", "empty cmd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty output"));
    }

    #[test]
    fn test_resolve_secret_cmd_failure() {
        let result = resolve_secret("", "false", "bad cmd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bad cmd"));
    }
}
