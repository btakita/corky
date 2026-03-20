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

## Key features

- **Flat conversations** — one Markdown file per thread, all sources merged
- **Sandboxed sharing** — label-based routing gives collaborators only the threads you choose
- **AI-native** — files, CLI, and git work the same for humans and agents
- **Multi-account** — Gmail, Protonmail Bridge, generic IMAP, all in one directory
- **Social posting** — draft and publish to LinkedIn (and future platforms) via OAuth
- **Scheduling** — schedule email and social drafts for timed publishing
- **Topics** — organize conversations with shared topic context across mailboxes
- **Transcription** — whisper-rs audio transcription with speaker diarization via pyannote-rs
- **Watch daemon** — poll IMAP and run scheduled publishing with `corky watch`

## Usage

```sh
corky sync                      # Incremental IMAP sync
corky unanswered                # Threads awaiting a reply
corky draft push FILE           # Save as email draft
corky mailbox add NAME --label LABEL  # Share threads
corky contact sync              # Sync contact CLAUDE.md between root and mailboxes
corky filter push               # Push Gmail filters from .corky.toml
corky filter push --dry-run     # Preview filter changes
corky filter pull               # Show current Gmail filters
corky filter auth               # Authenticate for Gmail filter API
corky linkedin draft              # Create LinkedIn draft
corky linkedin publish FILE      # Publish to LinkedIn
corky linkedin comment FILE TEXT # Comment on a published post
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

**Feature flags:** Transcription (CPU) is enabled by default. GPU acceleration is auto-detected — both `make install` and `install.sh` check for an NVIDIA GPU and attempt to enable `transcribe-cuda` automatically, falling back to CPU-only if the GPU build fails. For manual control: `cargo install --path . --features transcribe-cuda`. Diarization requires `--features diarize`.

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

This opens your browser for OAuth authorization. Authorize with the correct Google account. The token is cached at `~/.config/corky/tokens.json` for future syncs.

**Notes:**
- If your OAuth app is in testing mode, add the Gmail account as a test user in the Cloud Console
- `login_hint` pre-selects the configured email in the consent screen
- Post-auth verification ensures the token matches the configured account
- Incremental sync uses Gmail's `historyId` for efficient polling after initial sync

## Development

```sh
cp .corky.toml.example mail/.corky.toml
make check    # clippy + test
make release  # build + symlink to .bin/corky
```

See [building](https://btakita.github.io/corky/development/building.html) and [conventions](https://btakita.github.io/corky/development/conventions.html).

## AI agent instructions

Project instructions live in `AGENTS.md` (symlinked as `CLAUDE.md`). Personal overrides go in `CLAUDE.local.md` / `AGENTS.local.md` (gitignored).
