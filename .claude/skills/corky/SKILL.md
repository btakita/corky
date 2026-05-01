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
Use these for inbox and draft work:
```text
corky unanswered
corky draft new --to EMAIL "Subject"
corky draft validate
corky sync
corky contact add --from SLUG
corky contact info NAME
```

### Calendar
```text
corky cal auth
corky cal list [--limit N] [--query Q]
corky cal create <SUMMARY> <START> <END> [--description] [--location]
corky cal check <START> <END>
corky cal delete <QUERY> [--all] [--dry-run]
```

### Documents
```text
corky doc build <FILE> [--format pdf|docx] [--template NAME]
```

### Filters
```text
corky filter check
corky filter push
corky filter auth
```

### System
```text
corky watch
corky skill install
corky audit-docs
```

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
