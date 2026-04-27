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

Use the command family that matches the user intent rather than memorizing a long flat list:

- **Email:** `corky unanswered`, `corky draft new`, `corky draft validate`, `corky draft push`, `corky draft send`, `corky sync`, `corky contact add --from`, `corky contact info`
- **Google Workspace / connector surfaces:** `corky doctor gmail --json`, `corky sync refetch THREAD_ID --json`, `corky docs ...`, `corky sheets ...`, `corky cal ...`, `corky chat ...`, `corky tasks ...`
- **Filters and system:** `corky filter check`, `corky filter push`, `corky filter auth`, `corky watch`, `corky skill install`, `corky audit-docs`

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
