---
description: Draft and send emails with HTML formatting via corky or browser paste
user-invocable: true
argument-hint: "<command> [args]"
---

# email

Draft, format, and send emails from agent-doc sessions. Handles HTML formatting, Gmail threading, and multiple delivery paths.

## Invocation

```
/email draft <recipient> <subject>   — draft a new email or reply
/email send <draft-file>             — send a draft via corky or browser
/email html <draft-file>             — save HTML version for browser paste
```

## Commands

### `draft` — Create or Reply to Email

Draft a new email or reply to an existing thread.

**Steps:**
1. If replying, search `mail/conversations/` for the thread
2. Extract thread ID and subject for threading
3. Create draft via `corky draft new --to <recipient> --in-reply-to <thread-id> "<subject>"`
4. Write email content to the draft file
5. Set status to `approved`

**Reply threading:**
- Always search `mail/conversations/` for existing threads with the recipient
- Extract Thread ID from the conversation file
- Use `--in-reply-to` with the thread ID
- Preserve the original subject with `Re:` prefix (Gmail threading requires matching subjects)

### `send` — Send a Draft

**IMPORTANT: Never send automatically.** See `runbooks/email-send.md` for full workflow.

**Browser paste workflow (fallback when corky is unavailable):**
1. Save HTML version: `file:///tmp/<slug>.html`
2. Instruct user: Open in browser -> Select all (Ctrl+A) -> Copy -> Paste in Gmail
3. Always use `file:///` protocol paths

### `html` — Generate HTML for Browser Paste

Save a formatted HTML file from a draft for the browser paste workflow.

**Steps:**
1. Read the draft markdown file
2. Convert to styled HTML with Gmail-compatible inline CSS:
   - `font-family: Arial, sans-serif; font-size: 14px; color: #333; line-height: 1.6`
   - Headings: `color: #1a1a1a; border-bottom: 1px solid #ddd`
   - Subheadings: `color: #444`
3. Save to `/tmp/<slug>.html`
4. Print `file:///tmp/<slug>.html` for the user to open

## Gmail Account Configuration

### btak.dev@gmail.com (gmail-api provider)

Uses Gmail API for both sync and send. No SMTP password needed.

```toml
[accounts."btak.dev"]
provider = "gmail-api"
user = "btak.dev@gmail.com"
labels = ["INBOX"]
sync_days = 30
```

- **Sync:** `gmail.readonly` scope (token key: `gmail:btak.dev`)
- **Send/Draft:** `gmail.compose` scope (token key: `gmail:btak.dev:send`)
- First use opens browser for OAuth consent; tokens auto-refresh after that

### SMTP Accounts (personal, proton-dev)

```toml
[accounts.personal]
provider = "gmail"
user = "brian.takita@gmail.com"
password_cmd = "pass email/gmail"
```

Requires password for both IMAP draft push and SMTP send.

## Conventions

- **Never send without approval** — always push to Gmail Drafts first, wait for user to say "send"
- Always use `file:///` protocol for browser-openable file paths
- Always search for existing threads before creating standalone emails
- Gmail threading requires matching subjects — use `Re: <original subject>`
- HTML emails use inline CSS (Gmail strips `<style>` tags)
- Draft content comes from agent-doc session context — read correspondence, resume, and contact files as needed
- Verify facts against `mail/contacts/<name>/CLAUDE.md` before sending
- Attachment paths must be absolute (tilde `~` not expanded for attachments in corky)

## Runbooks

When executing these operations, read and follow the linked runbook:

- `send email` — [runbooks/email-send.md](runbooks/email-send.md)
- `review inbox` — [runbooks/review-inbox.md](runbooks/review-inbox.md)
- `draft reply` — [runbooks/draft-reply.md](runbooks/draft-reply.md)
- `enrich contact` — [runbooks/enrich-contact.md](runbooks/enrich-contact.md)
