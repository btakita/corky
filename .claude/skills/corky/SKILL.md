# Corky — Correspondence Kit

Manage email, calendar, documents, and communications from the command line.

## Core Principles
- **Draft only** — never send email directly; always save as a draft for human review
- **Match voice** — follow the Writing Voice guidelines in voice.md
- **Use context** — read relevant threads in `conversations/` before drafting

## Data Paths
- `conversations/` — synced email threads as Markdown
- `contacts/{name}/AGENTS.md` — per-contact context
- `manifest.toml` — thread index
- `drafts/` — outgoing email drafts

## Commands

Auth tip: Google-backed browser OAuth flows bind the loopback listener before opening the browser, default to `127.0.0.1:8484`, and honor `CORKY_OAUTH_CALLBACK_PORT` for a single-session override.

### Email
Use `corky unanswered` to find threads awaiting reply, `corky draft new --to EMAIL "Subject"` to scaffold a draft, `corky draft validate` to check format, `corky sync` to refresh IMAP threads, and `corky contact add --from SLUG` / `corky contact info NAME` to manage contact context.

### Calendar
Use `corky cal auth` for Google Calendar OAuth2, `corky cal list [--limit N] [--query Q]` for upcoming events, `corky cal create <SUMMARY> <START> <END> [--description] [--location]` to create events, and `corky cal check` / `corky cal delete` for availability and cleanup.

### Documents
Use `corky doc build <FILE> [--format pdf|docx] [--template NAME]` to convert markdown to PDF or DOCX.

### Filters
Use `corky filter check` to compare local vs Gmail filters, `corky filter push` for the destructive/manual sync path, and `corky filter auth` for Gmail OAuth2.

### System
Use `corky watch` for IMAP polling plus filter drift detection, `corky skill install` to install bundled Claude Code skills, and `corky audit-docs` to audit instruction files.

## Related Skills

Email drafting, LinkedIn posting, and their runbooks are installed as separate skills:
- `.claude/skills/email/` — email drafting, sending, inbox review
- `.claude/skills/linkedin/` — LinkedIn post drafting and publishing

## Runbooks

- `imports` — [runbooks/imports.md](runbooks/imports.md)
- `transcription` — [runbooks/transcription.md](runbooks/transcription.md)

## Success Criteria
- Drafts sound like the user wrote them
- No email sent without explicit approval
- Threads read in full before drafting
- Calendar queries answered without asking the user to check manually
