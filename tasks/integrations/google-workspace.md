# Google Workspace Integration — Spec

Extend corky to cover Google Workspace natively in Rust. No external `gws` plugin dependency — all API calls go through corky's existing OAuth infrastructure (`filter/gmail_auth.rs`).

## Current State

Already implemented:
- `corky doc read <doc>` — Google Docs read (HTML export → markdown)
- `corky doc write <doc> <file>` — Google Docs write (clear + insert)
- `corky doc upload <file>` — Google Drive upload (multipart)
- `corky doc info/export <file>` — Drive MIME detection plus Google Docs/Sheets/Slides/Drawings export and binary download
- `corky doc sheet <sheet>` — Google Sheets read (→ markdown table or CSV)
- `corky sheets pull <sheet> <tab> <csv>` / `corky sheets push <sheet> <tab> <csv>` — tab-level CSV sync
- `corky cal auth/create/list/delete/check` — Google Calendar full CRUD
- Gmail API sync (`sync/gmail_api_sync.rs`)
- Gmail send scope + draft creation (`GMAIL_SEND_SCOPE`)

OAuth infrastructure reused across all Google services:
- `filter/gmail_auth.rs` — shared token store, auth flow, scope constants
- Scopes defined: `GMAIL_SEND_SCOPE`, `DRIVE_FILE_SCOPE`, `DOCS_SCOPE`, `SHEETS_READONLY_SCOPE`

## Phase 1: Skill + Runbook Cleanup

Remove external gws plugin references from:
- `src/corky/.claude/skills/gws/SKILL.md` — rewrite to document `corky` CLI commands only
- `src/corky/.agent/runbooks/gws-gmail-send.md` — rewrite to use `corky draft send --attachment`
- `src/corky/.agent/runbooks/gws-docs-draft.md` — rewrite to use `corky doc read/write`
- `src/corky/AGENTS.md` — remove gws plugin install instructions; replace with corky-native docs

No code changes in this phase.

## Phase 2: Gmail Send with Attachments

**New command:** `corky draft send <draft-file> [--attachment <path>...]`

**Files:**
- `src/draft/send.rs` (new) — Gmail API send implementation
- `src/cli.rs` — add `DraftCommands::Send` variant
- `src/main.rs` — wire `DraftCommands::Send` to `draft::send::run`

**API:** `POST https://gmail.googleapis.com/gmail/v1/users/me/messages/send`

**Implementation:**
```
fn run(draft_file: &Path, attachments: &[PathBuf], account: Option<&str>) -> Result<()>
  1. Parse draft YAML (to, subject, body, in_reply_to, thread_id)
  2. Get token: gmail_auth::get_access_token_for_user(_, GMAIL_SEND_SCOPE, account)
  3. Build MIME message:
     - RFC 2822 headers (To, Subject, In-Reply-To, References)
     - multipart/mixed if attachments present
     - text/html part from draft body
     - one application/octet-stream part per attachment (base64)
  4. base64url-encode full MIME message
  5. POST { "raw": "<encoded>", "threadId": "<id>" }
  6. Print message ID on success
```

**OAuth scope:** `GMAIL_SEND_SCOPE` (already defined, `gmail.compose`)

**Draft YAML fields used:**
```yaml
to: recipient@example.com
subject: "..."
body: |   # markdown or HTML
in_reply_to: "<message-id>"   # optional
thread_id: "..."              # optional (Gmail thread attachment)
```

## Phase 3: Google Sheets Write

**New command:** `corky doc sheet-write <sheet> <range> <csv-file>`

**Files:**
- `src/doc/sheets.rs` — add `write()` function
- `src/cli.rs` — add `DocCommands::SheetWrite` variant
- `src/filter/gmail_auth.rs` — add `SHEETS_SCOPE` constant

**API:** `PUT https://sheets.googleapis.com/v4/spreadsheets/{id}/values/{range}?valueInputOption=USER_ENTERED`

**Implementation:**
```
fn write(sheet: &str, range: &str, file: &Path, account: Option<&str>) -> Result<()>
  1. Parse CSV file into Vec<Vec<String>>
  2. Get token with SHEETS_SCOPE (write, not readonly)
  3. Build request body: { "values": [[...], ...] }
  4. PUT to sheets API
```

**New scope constant:**
```rust
pub const SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets";
```

## Phase 4: Google Chat

**New command:** `corky social chat send <space> <message>`

**Files:**
- `src/social/chat.rs` (new)
- `src/cli.rs` — extend `SocialCommands` or add `ChatCommands`
- `src/filter/gmail_auth.rs` — add `CHAT_SCOPE`

**API:** `POST https://chat.googleapis.com/v1/spaces/{name}/messages`

**Implementation:**
```
fn send(space: &str, message: &str, account: Option<&str>) -> Result<()>
  1. Normalize space name (strip "spaces/" prefix if present)
  2. Get token with CHAT_SCOPE
  3. POST { "text": message }
  4. Print message name on success
```

**New scope:**
```rust
pub const CHAT_SCOPE: &str = "https://www.googleapis.com/auth/chat.messages";
```

**Space name format:** `spaces/XXXXXXXXX` — users provide either the full path or just the ID.

## Phase 5: Google Tasks

**New command:** `corky tasks list/add/done`

**Files:**
- `src/tasks/` (new module directory)
  - `src/tasks/mod.rs`
  - `src/tasks/list.rs`
  - `src/tasks/add.rs`
  - `src/tasks/done.rs`
- `src/cli.rs` — add `TasksCommands`
- `src/filter/gmail_auth.rs` — add `TASKS_SCOPE`

**API base:** `https://tasks.googleapis.com/tasks/v1`

**Commands:**
```
corky tasks list [--tasklist <id>]        — list tasks (default: @default tasklist)
corky tasks add "<title>" [--due <date>]  — add task to default tasklist
corky tasks done <task-id>               — mark task complete
```

**Implementation:**
```
// list: GET /lists/{tasklist}/tasks?showCompleted=false
// add:  POST /lists/{tasklist}/tasks { "title": "...", "due": "..." }
// done: PATCH /lists/{tasklist}/tasks/{id} { "status": "completed" }
```

**New scope:**
```rust
pub const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";
```

## Phase 6: Update Skills + Runbooks

After Phases 2-5, revise corky skill/runbook files:

- `src/corky/.claude/skills/gws/SKILL.md` — document full `corky doc/cal/social chat/tasks` surface
- `src/corky/.agent/runbooks/gws-gmail-send.md` — use `corky draft send --attachment`
- `src/corky/.agent/runbooks/gws-docs-draft.md` — use `corky doc read/write`

## OAuth Scope Summary

| Service | Scope constant | Value |
|---------|---------------|-------|
| Gmail send | `GMAIL_SEND_SCOPE` | `gmail.compose` ✅ |
| Drive upload | `DRIVE_FILE_SCOPE` | `drive.file` ✅ |
| Docs read/write | `DOCS_SCOPE` | `documents` ✅ |
| Sheets read | `SHEETS_READONLY_SCOPE` | `spreadsheets.readonly` ✅ |
| Sheets write | `SHEETS_SCOPE` | `spreadsheets` (new) |
| Workspace document pre-auth | `GOOGLE_WORKSPACE_SCOPE` | `drive.file` + `drive.readonly` + `documents` + `spreadsheets` |
| Chat send | `CHAT_SCOPE` | `chat.messages` (new) |
| Tasks | `TASKS_SCOPE` | `tasks` (new) |

## Coverage After All Phases

| Service | Coverage |
|---------|---------|
| Gmail | sync + send + attachments |
| Drive | upload + metadata/export/download for document files |
| Calendar | full CRUD |
| Docs | read + write + Drive export |
| Sheets | read + write |
| Slides | text/PPTX/PDF export via Drive |
| Drawings | PNG/SVG/PDF export via Drive |
| Forms | detected; content export unsupported by Drive |
| Chat | send |
| Tasks | list + add + done |
| Meet | out of scope |
| Keep | out of scope |
