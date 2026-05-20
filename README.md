<p align="center">
  <img src="assets/logo.svg" width="128" alt="Corky logo">
</p>

# Corky

> **Alpha software.** Expect breaking changes between minor versions. See [VERSIONS.md](VERSIONS.md) for migration notes.

**Full documentation: https://btakita.github.io/corky**

Sync email threads from IMAP to Markdown. Draft replies with AI assistance. Share scoped threads with collaborators via git.

Corky syncs threads from any IMAP provider (Gmail, Protonmail Bridge, self-hosted) into `mail/conversations/` — one file per thread, regardless of source. A thread that arrives via both Gmail and Protonmail merges into one file. Labels, accounts, and contacts are metadata, not directory structure.

## Install

```sh
pip install corky        # or: pipx install corky
```

Or via shell installer:

```sh
curl -sSf https://raw.githubusercontent.com/btakita/corky/main/install.sh | sh
```

Or from source: `cargo install --path .`

## Quick start

```sh
corky init --user you@gmail.com
# Edit mail/.corky.toml with credentials
corky sync
```

See the [getting started guide](https://btakita.github.io/corky/getting-started/quick-start.html) for full setup instructions.

When corky needs browser OAuth, it binds the local callback listener before opening the browser. Google-backed flows default to `127.0.0.1:8484`, honor `CORKY_OAUTH_CALLBACK_PORT` for a one-session override, and only fall back to another loopback port when you explicitly set `CORKY_OAUTH_ALLOW_EPHEMERAL_PORT=1` for a client that supports wildcard loopback redirects. Browser-auth prompts also raise a best-effort desktop notification via `notify-desktop` on Linux, falling back to `notify-send` when `notify-desktop` is unavailable; macOS uses `osascript`, and Windows uses PowerShell NotifyIcon.
If `[gsc]` service-account auth is configured, that Search Console token is only cached in-process and is scoped by the resolved account/config fingerprint, so changing mailbox roots or configs does not reuse the wrong SA token.

## Key features

- **Flat conversations** — one Markdown file per thread, all sources merged
- **Sandboxed sharing** — label-based routing gives collaborators only the threads you choose
- **AI-native** — files, CLI, and git work the same for humans and agents
- **Multi-account** — Gmail, Protonmail Bridge, generic IMAP, all in one directory
- **Social posting** — draft and publish to LinkedIn and YouTube via OAuth
- **Google Workspace** — Gmail send with attachments, Docs/Sheets read-write, Chat, Tasks
- **Scheduling** — schedule email and social drafts for timed publishing
- **Topics** — organize conversations with shared topic context across mailboxes
- **Transcription** — whisper-rs audio transcription with speaker diarization via pyannote-rs
- **Watch daemon** — poll IMAP and Gmail API accounts, sync shared mailboxes, notify on new messages, and run scheduled publishing with `corky watch`. Ctrl+C cleanly interrupts both sync paths

## Usage

```sh
corky sync                      # Incremental IMAP sync
corky sync refetch THREAD_ID    # Re-fetch a single Gmail thread
corky unanswered                # Threads awaiting a reply
corky draft push FILE           # Save as email draft (thread_id in YAML for threading)
corky draft send FILE           # Send via Gmail API (supports --attachment FILE)
corky mailbox add NAME --label LABEL  # Share threads
corky contact push [google]     # Push contacts to Google Contacts (People API)
corky contact sync              # Sync contact CLAUDE.md between root and mailboxes
corky contact delete RES_NAME   # Delete contact from Google Contacts
corky filter push               # Push Gmail filters from .corky.toml
corky filter push --dry-run     # Preview filter changes
corky filter pull               # Show current Gmail filters
corky auth --email you@gmail.com --scope sync  # Pre-authenticate Gmail API sync
corky auth --account my-gmail --email you@gmail.com --scope send  # Pre-authenticate Gmail compose
corky filter auth               # Authenticate for Gmail filter API
corky linkedin draft              # Create LinkedIn draft
corky linkedin publish FILE      # Publish to LinkedIn
corky linkedin comment FILE TEXT # Comment on a published post
corky youtube comment FILE TEXT      # Comment on a published video
corky youtube playlist add PL VID   # Add video to playlist
corky youtube playlist create TITLE # Create a playlist
corky youtube playlist list         # List your playlists
corky doc upload FILE --account a@gmail.com  # Google Drive upload (account-targeted OAuth)
corky docs read DOC_URL                    # Read Google Doc text
corky docs write DOC_URL FILE              # Replace Google Doc text from markdown
corky doc sheet SHEET_URL                   # Read Google Sheet as markdown table
corky doc sheet-write SHEET_URL RANGE CSV  # Write CSV to Google Sheet range
corky sheets read SHEET_URL --range A1:D10 # Read Google Sheet range
corky sheets write SHEET_URL RANGE CSV     # Write CSV to Google Sheet range
corky sheets pull SHEET_URL TAB CSV        # Sync a Google Sheet tab to local CSV
corky sheets push SHEET_URL TAB CSV        # Clear/create tab, then sync local CSV
corky sheets delete-tab SHEET_URL TAB      # Delete a Google Sheet tab
corky chat send SPACE_ID "message"          # Send Google Chat message
corky tasks list                            # List pending Google Tasks
corky tasks add "Task title" --due 2026-04-20  # Add a task
corky tasks done TASK_ID                    # Mark task complete
corky schedule run              # Publish due scheduled items
corky topics list               # Show configured topics
corky watch                     # Poll, sync, and publish scheduled
corky transcribe FILE            # Transcribe audio to text
corky transcribe FILE --diarize  # With speaker diarization
corky --help                    # All commands
```

### Transcription & speaker diarization

Transcribe audio files with optional speaker diarization. Supports WAV, MP3, FLAC, OGG, M4A, AMR, and more.

```sh
# Basic transcription
corky transcribe call.amr -o transcript.md

# With speaker diarization (interactive labeling)
corky transcribe call.amr --diarize -o transcript.md

# With pre-assigned speaker names
corky transcribe call.amr --diarize --speakers "Alice,Bob" -o transcript.md
```

Diarization uses [pyannote-rs](https://github.com/thewh1teagle/pyannote-rs) (ONNX Runtime) to detect and label speakers. When run without `--speakers`, corky shows text excerpts per speaker and prompts you to assign names interactively. ONNX models auto-download on first use — no gated HuggingFace access required.

**Feature flags:** Transcription (CPU) is enabled by default. GPU acceleration is auto-detected — `make install` and `install.sh` detect NVIDIA CUDA (Linux) or Apple Metal (macOS) and build with the appropriate GPU backend, falling back to CPU-only if the GPU build fails. For manual control: `cargo install --path . --features transcribe-cuda` (Linux) or `--features transcribe-metal` (macOS). Diarization is enabled by default. Video formats (mov, mkv, webm, etc.) are transcribed via ffmpeg; audio formats use symphonia with ffmpeg fallback.

> This feature was designed collaboratively using [agent-doc](https://github.com/btakita/agent-doc) interactive document sessions.

### Telegram import

Import Telegram Desktop JSON exports into corky conversations:

```sh
corky sync telegram-import ~/Downloads/telegram-export/result.json
corky sync telegram-import ~/Downloads/telegram-export/           # directory of exports
corky sync telegram-import result.json --label personal --account tg-personal
```

Export from Telegram Desktop: Settings > Advanced > Export Telegram data > JSON format.

### Slack import

Import Slack workspace export ZIPs:

```sh
corky slack import ~/Downloads/my-workspace-export.zip
corky slack import export.zip --label work --account slack-work
```

Export from Slack: Workspace admin > Settings > Import/Export Data > Export.

See the [command reference](https://btakita.github.io/corky/guide/commands.html) for details.

### Gmail API sync

Sync Gmail accounts without IMAP or app passwords using the Gmail REST API with OAuth2.

**1. Configure `.corky.toml`:**

Corky ships with built-in GCP OAuth credentials, so no Google Cloud Console setup is needed for most users. Just configure your account:

```toml
# [gmail] section is optional — built-in credentials are used by default
# To use your own OAuth app, uncomment and configure:
# [gmail]
# client_id_cmd = "pass corky/gmail/client_id"       # or inline: client_id = "..."
# client_secret_cmd = "pass corky/gmail/client_secret" # or inline: client_secret = "..."

[accounts.my-gmail]
provider = "gmail-api"
user = "you@gmail.com"
labels = ["INBOX"]
sync_days = 30           # optional, default 3650
```

**2. First sync:**

```sh
corky sync account my-gmail
```

This opens your browser for OAuth authorization. Authorize with the correct Google account. The token is cached at `~/.config/corky/tokens.json` for future syncs; corky now writes that shared token store with a lock + atomic replace so concurrent auth flows do not clobber unrelated entries.

You can also authenticate before running sync: `corky auth --email you@gmail.com --scope sync` stores the Gmail read-only token under the same `gmail:<email>` key used by `gmail-api` sync. For Gmail API draft/send, use `corky auth --account my-gmail --email you@gmail.com --scope send` so the compose token is stored under `gmail:<account>:send`.

`corky draft send` uses a separate `gmail.compose` token under `gmail:<account>:send`, so attachment sends and Gmail API draft sends do not overwrite the read-only sync token. If Gmail returns 401 on the send path, corky clears that cached send token and asks you to re-run `corky draft send`, which triggers compose-scope re-auth.

The IMAP/Gmail sync cursor file (`mail/.sync-state.json`) uses the same lock + atomic-write pattern, with merge-on-save so contact sync and message sync preserve each other's unrelated sections.
Search Console's optional `[gsc]` service-account fallback is separate from that shared token store: it is cached only in-process and keyed by the resolved account/config fingerprint before corky reuses it.

**Threading:** Synced conversations include a `**Message-ID**` metadata line per message. Draft YAML supports a `thread_id` field (Gmail thread ID) so replies thread correctly via the Gmail API.

**Notes:**
- If your OAuth app is in testing mode, add the Gmail account as a test user in the Cloud Console
- `login_hint` pre-selects the configured email in the consent screen
- Google access tokens are short-lived by design; Corky stores refresh tokens and refreshes access tokens automatically when the original authorization is still valid
- Post-auth verification ensures the token matches the configured account
- Incremental sync uses Gmail's `historyId` for efficient polling after initial sync
- Messages deleted between listing and fetch (404) are skipped gracefully — sync continues with remaining messages

**Connector / Codex-friendly JSON surfaces:**

```sh
corky doctor gmail --json
corky sync refetch THREAD_ID --json
corky draft push drafts/reply.md --json
corky draft push drafts/reply.md --send --json
corky draft send drafts/reply.md --attachment /tmp/file.pdf --json
```

These commands keep the normal human-readable output by default. `--json` emits stable machine-readable summaries for auth state, single-thread refetches, draft creation, and sends.

## Development

```sh
cp .corky.toml.example mail/.corky.toml
make check    # clippy + test
make release  # build + symlink to .bin/corky
```

See [building](https://btakita.github.io/corky/development/building.html) and [conventions](https://btakita.github.io/corky/development/conventions.html).

## AI agent instructions

Project instructions live in `AGENTS.md` (symlinked as `CLAUDE.md`). Personal overrides go in `CLAUDE.local.md` / `AGENTS.local.md` (gitignored).
