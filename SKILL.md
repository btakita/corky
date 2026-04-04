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
- `corky unanswered` — threads awaiting reply
- `corky draft new --to EMAIL "Subject"` — scaffold a new draft
- `corky draft validate` — validate draft format
- `corky sync` — sync threads from IMAP
- `corky contact add --from SLUG` — create contact from conversation
- `corky contact info NAME` — show contact details

### Calendar
- `corky cal auth` — Google Calendar OAuth2
- `corky cal list [--limit N] [--query Q]` — upcoming events
- `corky cal create <SUMMARY> <START> <END> [--description] [--location]` — create event
- `corky cal check <START> <END>` — check availability
- `corky cal delete <QUERY> [--all] [--dry-run]` — delete events

### Documents
- `corky doc build <FILE> [--format pdf|docx] [--template NAME]` — convert markdown to PDF/DOCX

### Filters
- `corky filter check` — compare local vs Gmail filters (read-only)
- `corky filter push` — push local filters to Gmail (destructive, manual only)
- `corky filter auth` — Gmail OAuth2 for filter management

### Imports
- `corky sync sms-import <FILE.xml>` — import SMS Backup & Restore XML
- `corky sync telegram-import <DIR>` — import Telegram Desktop JSON export
- `corky slack import <FILE.zip>` — import Slack workspace export

### Transcription
- `corky transcribe <FILE> [--diarize] [--speakers "Name1,Name2"]` — transcribe audio/video

### System
- `corky watch` — IMAP polling + filter drift detection daemon
- `corky skill install` — install Claude Code skills
- `corky audit-docs` — audit instruction files

## Workflows

### Check schedule
1. Run `corky cal list` to see upcoming events
2. For availability questions, use `corky cal check <START> <END>`
3. To create meetings, use `corky cal create`

## Related Skills

Email drafting, LinkedIn posting, and their runbooks are installed as separate skills:
- `.claude/skills/email/` — email drafting, sending, inbox review
- `.claude/skills/linkedin/` — LinkedIn post drafting and publishing

## Success Criteria
- Drafts sound like the user wrote them
- No email sent without explicit approval
- Threads read in full before drafting
- Calendar queries answered without asking the user to check manually
