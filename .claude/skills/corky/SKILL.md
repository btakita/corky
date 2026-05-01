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

For email work, start with `corky unanswered`, `corky sync`, `corky draft new --to EMAIL "Subject"`, and `corky draft validate`. Use `corky contact add --from SLUG` or `corky contact info NAME` when thread context needs to become durable contact context.

For Google workflows, use `corky cal auth`, `corky cal list [--limit N] [--query Q]`, `corky cal create <SUMMARY> <START> <END>`, and `corky cal check <START> <END>` for calendar tasks. Document export stays on `corky doc build <FILE> [--format pdf|docx] [--template NAME]`.

For Gmail maintenance and project hygiene, use `corky filter check`, `corky filter push`, and `corky filter auth`, then finish with `corky watch`, `corky skill install`, or `corky audit-docs` as needed.

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
