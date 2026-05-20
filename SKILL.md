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

### Email
Use `corky unanswered` for threads awaiting reply, `corky sync` for IMAP sync, `corky draft new --to EMAIL "Subject"` and `corky draft validate` for draft work, and `corky contact add --from SLUG` / `corky contact info NAME` for contact context.

### Calendar
Use `corky cal auth` for OAuth, `corky cal list [--limit N] [--query Q]` for upcoming events, `corky cal create <SUMMARY> <START> <END>` for event creation, `corky cal check <START> <END>` for availability, and `corky cal delete <QUERY> [--all] [--dry-run]` for deletion.

### Documents
Use `corky doc build <FILE> [--format pdf|docx] [--template NAME]` to convert markdown, `corky docs read <DOC> [-o FILE]` and `corky docs write <DOC> <FILE>` for Google Docs text sync, `corky sheets read <SHEET> [--range RANGE] [--format table|csv]` for Google Sheets reads, `corky sheets pull <SHEET> <TAB> <CSV>` to sync a Google Sheet tab to CSV, and `corky sheets push <SHEET> <TAB> <CSV>` to clear/create a tab and sync CSV into it.

### Filters
Use `corky filter check` for read-only drift detection, `corky filter push` for manual Gmail filter updates, and `corky filter auth` for filter OAuth.

### System
Use `corky watch` for polling/filter drift, `corky skill install` for Claude Code skills, and `corky audit-docs` for instruction audits.

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
