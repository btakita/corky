# Versions

Corky is alpha software. Expect breaking changes between minor versions.

Use `BREAKING CHANGE:` prefix in version entries to flag incompatible changes.

## 0.25.1

- **Skill runbook extraction**: Extract on-demand runbooks from SKILL.md files to reduce always-loaded context (21% reduction). 4 new runbooks: gmail-config, browser-paste, imports, transcription. Email SKILL.md trimmed from 107 to 66 lines, corky SKILL.md from 71 to 61 lines.

## 0.25.0

- **Multi-skill install**: `corky skill install` now writes 3 skills (corky, email, linkedin) plus runbooks. Each skill gets its own `SKILL.md` under `.claude/skills/{name}/`. Email skill includes 4 runbooks (`draft-reply`, `email-send`, `enrich-contact`, `review-inbox`). LinkedIn skill includes 1 runbook (`linkedin-post`). Corky `SKILL.md` trimmed to general corky commands only.

## 0.24.0

- **Gmail API threading fix (Bug 1)**: Extract `Message-ID` header during Gmail sync and store as `**Message-ID**` metadata in conversation markdown files. Previously `in_reply_to` in draft YAML accepted Gmail thread IDs instead of RFC 2822 Message-IDs, breaking the `In-Reply-To` email header.
- **Gmail API threading fix (Bug 2)**: Add `thread_id` field to draft YAML frontmatter. `send_via_gmail_api()` and `push_to_drafts_via_gmail_api()` now include `threadId` in the Gmail API request body, threading replies into existing conversations.
- **Account-based doc auth**: `--account` flag on `doc upload`, `doc read`, `doc write`, `doc sheet` commands. Uses login hint for OAuth to target specific Google accounts.
- **Contact delete**: `corky contact delete <RESOURCE_NAME>...` — delete contacts from Google Contacts via People API.
- **Transcribe resolve**: Model resolution module for transcription.

## 0.23.0

- **Single-thread refetch**: `corky sync refetch <THREAD_ID>` — re-fetch all messages in a Gmail thread via the Threads API, bypassing historyId state. Rebuilds the conversation file with fresh body content. Useful when body extraction failed on initial sync (e.g., base64 padding, attachmentId issues).
- **HTML-first email body extraction**: Prefer `text/html` (converted to markdown via `htmd`) over `text/plain` for email bodies. Falls back to plain text when no HTML part exists. Previously HTML-only emails produced empty bodies.
- **Markdown cleanup**: Post-conversion cleanup strips `@media` CSS blocks, tracking pixel images, inline `style` attributes, and excessive blank lines from converted email bodies.
- **Tracking pixel detection**: Detect tracking pixel domains in email bodies and record them in a `**Tracking**` metadata line in conversation file headers. Supports `/wf/open`, `/analytics/open`, 1x1 images, and other common patterns.
- **META_RE regex fix**: Fixed `\s*` matching `\n` in metadata regex, causing metadata lines with empty values to swallow subsequent lines.

## 0.22.2

- **Gmail API body decode fix**: Handle padded URL-safe base64 in message bodies. Gmail sometimes sends base64 with padding characters; `URL_SAFE_NO_PAD` decoder silently rejected these, producing empty bodies. Now falls back to `URL_SAFE` decoder.
- **Gmail API attachment body fix**: Fetch message body via attachments API when Gmail returns `attachmentId` instead of inline `data` (common for `multipart/related` emails with embedded images).

## 0.22.1

- **Contact push CLAUDE.md fallback**: `corky contact push google` now reads `CLAUDE.md` when `AGENTS.md` is absent, fixing silent field omission for contacts using the symlink convention.
- **Diarize default feature**: Diarization is now a default Cargo feature — no `--features diarize` needed.

## 0.22.0

- **Search backends**: Unified `corky search` command with pluggable backends (sift local, ragie cloud).
- **Sift indexing**: `corky sift index/status` for local full-text search.
- **Ragie cloud search**: `corky ragie push/sync/search/status` for cloud-hosted search via Ragie API.
- **SearchConfig**: Backend configuration in `.corky.toml`.

## 0.21.0

- **YouTube playlists**: `corky youtube playlist create/add/list/remove` — full playlist management via Data API v3. Create playlists, add/remove videos, list all playlists. Uses existing `youtube.force-ssl` OAuth scope.
- **Sorted YouTube commands**: CLI `--help` output and docs now list YouTube subcommands alphabetically.

## 0.20.0

- **YouTube comments**: `corky youtube comment <file> "text"` posts top-level comments on published videos via the Data API v3 `commentThreads.insert` endpoint. Prints comment ID and direct `&lc=` link for manual pinning.

## 0.19.3

- **Metal GPU support**: `transcribe-metal` feature for macOS Metal acceleration. `make release-gpu` and CI release workflow auto-detect CUDA (Linux) vs Metal (macOS).
- **Audio decode resilience**: Video formats (mov, mkv, webm, avi, ts, mts) go straight to ffmpeg. Symphonia failures now fall back to ffmpeg instead of erroring.
- **CI GPU builds**: Release workflow builds CUDA (Linux) and Metal (macOS) binary variants alongside CPU builds.

## 0.19.2

- **Write destination display**: `Wrote:` output now shows routing destination (e.g., `file.md → mailboxes/lucas/conversations/`) so fan-out writes don't look like duplicates.
- **Pull error handling**: Mailbox sync checks for git remote before attempting pull; shows actual error message on failure instead of generic "continuing with push".
- **SPEC.md**: Added §6.8 Gmail API Sync, §6.9 Doctor, §18 Self-Hosted Deployment.

## 0.19.1

- **Watch bug fixes**: Three fixes for `corky watch` reliability:
  - **Scoped routing**: `[routing]` entries now support `account:label` syntax to prevent label leakage across accounts. `for-lucas` route scoped to `personal` account only.
  - **Gmail API 404 resilience**: Messages deleted between `messages.list` and `messages.get` are skipped with a warning instead of aborting the entire account sync.
  - **IMAP shutdown signal**: Ctrl+C now interrupts IMAP syncs between message fetches (previously only Gmail API syncs respected the shutdown signal).
- **GPU auto-detection**: `make install` and `install.sh` auto-detect NVIDIA GPU and attempt CUDA-accelerated transcription build with graceful fallback.

## 0.19.0

- **`corky linkedin comment`**: Post comments on published LinkedIn posts via API. Takes a published draft file (with `post_id` in frontmatter) and comment text. Uses v2 API path (`/v2/socialActions/{urn}/comments`) — the versioned `/rest/` endpoint requires partner-level permissions that personal OAuth tokens don't have.

## 0.18.1

- **Gmail API email sending**: `provider = "gmail-api"` accounts now use the Gmail API for both draft push (`drafts.create`) and direct send (`messages.send`), bypassing SMTP entirely. Dedicated OAuth token key (`gmail:<account>:send`) avoids scope collision with sync tokens. Password no longer required for gmail-api accounts in draft resolution. One-time browser OAuth consent grants `gmail.send` scope, then auto-refreshes.

## 0.18.0

- **Built-in GCP OAuth credentials**: Zero-config Gmail API sync. `DEFAULT_GCP_CLIENT_ID` and `DEFAULT_GCP_CLIENT_SECRET` constants for corky's public GCP desktop OAuth app. Credential resolution order: config > cmd > env > built-in default. Injected at build time via `env!()` macro (avoids GitHub secret scanning).
- **`corky doctor`**: Runtime environment validation command. Per-provider health checks — config loading, mail directory, Gmail (filter auth, API sync tokens, credential resolution), LinkedIn (OAuth, profile URN, token validity), YouTube (OAuth, channel IDs, token validity). 11 integration tests.
- **`gmail-api` as init provider**: `corky init` now accepts `gmail-api` as a provider with provider-specific guidance.
- **Self-hosted Gmail setup guide**: Step-by-step docs for users who want their own GCP project instead of corky's public OAuth app.
- **Privacy policy and terms of service**: Documents for the public Google OAuth app.

## 0.17.0

- **Gmail API sync provider**: New `provider = "gmail-api"` option for accounts using OAuth instead of IMAP. Full Gmail REST API sync via `messages.list` and `messages.get`. `historyId`-based incremental sync (`GmailLabelState`). Scope-aware OAuth (`get_access_token_with_scope`, `login_hint`). Watch and sync dispatch based on provider type. Added `base64 = "0.22"` dependency.

## 0.16.0

- **HTML email sending**: `multipart/alternative` with pulldown-cmark markdown-to-HTML conversion. Works for both direct sends and IMAP draft pushes.
- **Inline images**: `images:` YAML frontmatter + `--image` flag on `draft new`. Content-ID (CID) references in `multipart/related`.
- **Clipboard attachment**: `corky draft attach --clipboard` captures clipboard content via `wl-paste`/`xclip`/`pngpaste`. Also adds `--file` and `--inline` flags.
- **`corky sync imports`**: Config-driven import dispatcher. Reads `[[imports]]` from `.corky.toml` and dispatches to existing sms_import, telegram_import, or slack_import handlers. Tilde expansion, default label/account, skip-on-missing-path.
- **Rust 2024 let-chains refactor**: Collapsed 61 nested if statements across 26 files. Removed `collapsible_if = "allow"` lint override.

## 0.15.3

- **CUDA transcription build fix**: Updated `whisper-rs` to 0.16 to fix CUDA build compatibility.

## 0.15.2

- **YouTube embeddable flag**: Sets `embeddable=true` for YouTube uploads and edits, allowing videos to be embedded on external sites.

## 0.15.1

- **YouTube default visibility changed to public**: Was "private", now defaults to "public".
- **`corky youtube edit`**: New command for updating published video metadata (title, description, tags, visibility).
- **YouTube error handling**: Better error messages for empty body and upload failures.
- **Test suite**: 8 YouTube tests + 5 LinkedIn/YouTube draft round-trip tests.

## 0.15.0

- **YouTube upload**: `corky youtube auth/draft/publish` commands for uploading videos to YouTube via Data API v3. Resumable upload with 8MB chunks, SRT captions upload, draft frontmatter with video/captions/title fields.
- **LinkedIn post editing**: `corky linkedin edit <file> [--body TEXT]` updates published LinkedIn posts via PARTIAL_UPDATE API. Text-only updates (images/visibility immutable post-publish).
- **Profiles consolidation**: Profile loading now checks `.corky.toml [profiles]` section first, with fallback to standalone `profiles.toml`. Configuration convention: all corky config belongs in `.corky.toml`.
- **AGENTS.md configuration convention**: Added `## Configuration` section documenting single-config-file convention.

## 0.14.1

- **Docs update**: Updated README and SPEC for transcribe enabled by default.

## 0.14.0

- **CPU transcription by default**: `default = ["transcribe"]` — `cargo install corky` now includes CPU transcription out of the box. GPU stays opt-in via `--features transcribe-cuda`.

## 0.13.4

- **Unified corky skill via agent-kit**: Bundle `SKILL.md` via `include_str!`, replacing separate `.claude/skills/email/` and `.claude/skills/agent-doc/` directories. `corky skill install/check` commands wired via `agent_kit::skill::SkillConfig`. Added `agent-kit` dependency.

## 0.13.3

- **Calendar event creation**: `corky cal create` creates calendar events with summary, start, end, optional description and location. Supports RFC 3339 datetime and `YYYY-MM-DD` all-day events.
- **Calendar availability check**: `corky cal check` checks calendar availability for a time range. Shows busy periods and free time summary.

## 0.13.2

- **Non-interactive OAuth in watch mode**: `check_filter_drift()` uses `get_access_token_noninteractive()` — never opens a browser. Logs actionable "Run `corky filter auth`" message when token is expired.
- **OAuth callback timeout**: Increased from 120s to 300s (5 minutes) for manual `corky filter auth` flow.

## 0.13.1

- **Google Calendar management**: `corky cal auth` (OAuth2), `corky cal list` (upcoming events with `--query`/`--limit`), `corky cal delete` (by search, `--all` deletes entire recurring series, `--dry-run`). Reuses Gmail OAuth credentials from `.corky.toml`.
- **SMS import**: `corky sync sms-import` imports SMS Backup & Restore XML files into conversation markdown.

## 0.13.0

- **Document building**: `corky doc build` converts markdown to PDF (pandoc + weasyprint) or DOCX (pandoc native). CSS template support via `--template` flag or `template:` YAML frontmatter field. Templates loaded from `{data_dir}/templates/{name}.css`.

## 0.12.4

- **Rust 2024 edition migration**: `set_var`/`remove_var` wrapped in `unsafe {}`, removed explicit `ref mut` patterns (implicit borrow in 2024), added `collapsible_if = "allow"` clippy lint. instruction-files bumped to v0.1.4.

## 0.12.3

- **Filter drift detection**: `corky filter check` compares local `.corky.toml` filters against live Gmail filters (read-only, 2 API calls). `corky watch` runs this hourly and warns on drift.

## 0.12.2

- **BREAKING CHANGE: `corky social` → `corky linkedin`**: All social media commands scoped under platform-specific subcommand. `corky social draft linkedin` becomes `corky linkedin draft`, etc. Internal `social/` module unchanged. Convention: each platform gets its own top-level subcommand when added.

## 0.12.1

- **Email attachments**: `--attach` flag on `corky draft new` and `attachments:` YAML frontmatter field. Sends as `multipart/mixed` with auto-detected content types via `mime_guess`. Files validated at send time.

## 0.12.0

- **`corky transcribe`**: whisper-rs speech-to-text with optional speaker diarization. Feature-gated (`transcribe`/`transcribe-cuda`), auto-downloads models. Multi-format audio support (symphonia + ffmpeg fallback). `--speakers` flag for tdrz speaker turn detection, markdown output.
- **`corky filter`**: Gmail filter management — `filter build` (generate from config), `filter push` (sync to Gmail), `filter pull` (download from Gmail). Gmail OAuth2 for filter API access.
- **Topics sync**: Sync keywords from `mail/topics/` directories to `.corky.toml`.
- **Watch improvements**: Improved Ctrl+C handling, scheduled publishing integrated into poll loop.

## 0.11.5

- **`corky label clear`**: Bulk IMAP label removal. Removes a specified Gmail label from all messages that carry it. Uses IMAP STORE to remove label flags.

## 0.11.4

- **BREAKING CHANGE: `SPECS.md` renamed to `SPEC.md`**: Singular naming convention. All internal references updated.
- Gitignore `.agent-doc/` directory.
- Dependency sync: instruction-files bumped to 0.1.2.

## 0.11.3

- **Watch Ctrl+C fix**: Fixed unresponsiveness in `corky watch` using `tokio::select!` + watch channel. Enabled tokio `sync` feature.
- **Test coverage expansion**: 22 new tests (357 total) — watch helpers (8), schedule dispatch (3), LinkedIn API mocks (11).
- **LinkedIn API testability**: Refactored `linkedin.rs` with `_at` variants for configurable base URL (HTTP mocking). Added `mockito` dev-dependency.

## 0.11.2

- **OAuth URL encoding fix**: Proper percent-encoding for OAuth form body params (fixes potential 401 errors).
- **Better OAuth error reporting**: Shows LinkedIn response body on token exchange failure.
- **CWD-as-data-dir**: `data_dir()` detects `.corky.toml` in CWD (supports running from inside `mail/`).
- **Schedule improvements**: Dry-run flag for schedule and publish commands (`--dry-run`).

## 0.11.1

- **`_cmd` fields for social OAuth credentials**: `client_id_cmd` and `client_secret_cmd` in `[social.linkedin]` for credential managers (pass, op, etc.). Resolution order: inline value > `_cmd` shell command > env var.
- **Shared `resolve_secret()` utility**: Extracted from `resolve_password()` into `util.rs`. Both email password and social credential resolution use the same code path.
- **Fix: credential resolution error propagation**: When `[social.linkedin]` is configured but the `_cmd` command fails, the error is now propagated instead of silently falling through to the env var check.

## 0.11.0

- **Scheduling**: Unified scheduler scans both social and email drafts, dispatches to existing publish paths. `schedule_tick()` integrated into watch loop.
- **Topics management**: `corky topics list/add/info/suggest` commands with `.corky.toml [topics.*]` config. Bidirectional topic sync between `mail/topics/` and `mailboxes/*/topics/` (mtime-based).
- **Draft YAML migration**: `corky draft migrate` converts legacy `**Key**: value` drafts to YAML frontmatter. `EmailDraftMeta` serde struct with dual-parser (YAML frontmatter + legacy format).
- SPEC.md updated with Topic Sync, Scheduling, and Draft Lifecycle sections.

## 0.10.0

- **Social media posting with LinkedIn**: OAuth2 authentication (`corky social auth linkedin`), draft scaffolding (`corky social draft linkedin`), publish (`corky social publish`), profile management via `profiles.toml`. Token store with JSON persistence.
- SPEC.md updated with Social Media Posting section.

## 0.9.5

- **Extract audit-docs to instruction-files crate**: `src/audit_docs.rs` reduced to a thin wrapper around the new `instruction-files` crate dependency.

## 0.9.4

- **`corky upgrade` self-update**: Downloads prebuilt binary from GitHub Releases as the primary upgrade strategy. Falls back to `cargo install`, then `pip install --upgrade`, then manual instructions including `curl | sh`.

## 0.9.3

- **Upgrade check**: Queries crates.io for latest version with a 24h cache. Prints a one-line stderr warning on startup if outdated.
- **`corky upgrade`**: New subcommand tries `cargo install` then `pip install --upgrade`, or prints manual instructions.

## 0.9.2

Contact sync v2: conversation-aware eligibility + 3-way merge.

- **`corky contact sync`**: Only syncs contacts to mailboxes where they have conversations (sender name matched via slugification) or are explicitly shared via `shared_with` in `.corky.toml`. Mailbox→root always allowed.
- **3-way merge resolution**: Content hashes (FNV-1a) stored per contact-mailbox pair in `.sync-state.json`. If only one side changed since last sync, that side wins. Conflicts fall back to newest-wins by mtime with a warning.
- **`Contact` struct extended**: `shared_with` and `aliases` fields added to `[contacts.*]` in `.corky.toml`. `aliases` match sender names that don't slugify to the contact directory name.
- 26 contact sync tests (19 eligibility + 7 three-way merge).

## 0.9.1

Telegram HTML export support.

- **`corky sync telegram-import`**: Auto-detect JSON vs HTML by file extension. Parse Telegram Desktop HTML exports (chat name, sender, date with timezone, message text). HTML entity decoding and link tag stripping. Chat ID derived from name hash.

## 0.9.0

Telegram + Slack import.

- **`corky sync telegram-import`**: Import Telegram Desktop JSON exports into mailbox threads. Supports single-chat and multi-chat formats. `--account` flag for account routing.
- **`corky slack import`**: Import Slack workspace export ZIPs into mailbox threads. Resolves user mentions, channels, and mrkdwn formatting. `--account` flag for account routing.
- CLI integration tests for both import commands.
- mdbook documentation for Telegram and Slack import guides.

## 0.8.0

Contact enrichment, To/CC sync, credential bubbling.

- **`corky contact add --from SLUG`**: Create a contact from a conversation. Extracts non-owner participants from From/To/CC headers, generates enriched AGENTS.md with pre-filled Topics, Formality, Tone, and Research sections. Auto-derives contact name from display name; positional `NAME` overrides.
- **`corky contact info NAME`**: Aggregates contact config, AGENTS.md content, and matching threads from all manifest.toml files. Shows thread count and last activity.
- **`corky contact` subcommand group**: `contact add` and `contact info` under a new subcommand group. `contact-add` kept as hidden backward-compatible alias.
- **BREAKING CHANGE: `Contact` struct simplified**: `labels` and `account` fields removed from `[contacts.*]` in `.corky.toml`. Existing files with these fields are silently ignored on parse. `--label` and `--account` flags on `contact-add` alias are accepted but ignored.
- **To/CC in sync pipeline**: Messages now store To and CC headers. Conversation markdown includes per-message `**To**:`/`**CC**:` lines. Manifest matches contacts by from, to, and cc. Old files without these lines parse correctly.
- **Credential bubbling**: `draft push` walks parent directories for `.corky.toml` with matching account credentials when drafting from child mailboxes.
- **Release workflow fix**: Vendor OpenSSL for aarch64-linux cross builds, update macOS runner to `macos-14`, add `fail-fast: false` to build matrix.

## 0.7.3

Always install the email skill.

- **BREAKING CHANGE: `--with-skill` removed from `corky init`**: The email skill is now installed unconditionally during `init`. The `--with-skill` flag is removed.
- **`mb add` / `mb reset` ship email skill**: Mailbox creation and reset now install `.claude/skills/email/` automatically.
- **Email skill files updated**: Paths simplified (no `mail/` prefix), added `draft new` command reference, removed legacy `find_unanswered.py`.

## 0.7.2

New `draft new` command for scaffolding draft files.

- **`corky draft new SUBJECT --to EMAIL`**: Scaffolds a new draft file with pre-filled metadata (To, Status, Author, etc.). Supports `--cc`, `--account`, `--from`, `--in-reply-to`, and `--mailbox` flags. Author resolved from `[owner] name` in `.corky.toml`. Slug collisions handled with `-2`, `-3` suffix. Also available as `corky mailbox draft new`.

## 0.7.1

CLI reorganization: draft subcommand group, unanswered scoping.

- **`corky draft validate` / `corky draft push`**: Draft commands moved under `draft` subcommand group. `validate-draft` and `push-draft` kept as hidden aliases for backwards compatibility. Also available under `corky mailbox draft`.
- **`corky draft validate` scope discovery**: When called with no files, discovers and validates all drafts across root `drafts/` and all `mailboxes/*/drafts/`. Supports `.` (root only) and mailbox name scoping, same pattern as `unanswered`.
- **`corky unanswered`**: Renamed from `find-unanswered` with scope argument support (`.` for root, mailbox name, omit for all). `find-unanswered` kept as hidden alias.
- **Mailbox scoping for `unanswered`**: Available as both `corky unanswered [SCOPE]` and `corky mailbox unanswered [SCOPE]`.

## 0.7.0

BREAKING CHANGE: Rust rewrite. Complete port from Python to Rust.

- **Rust rewrite**: All ~3,750 LOC of Python replaced with ~6,700 LOC of Rust. Single crate, no workspace. All 25 CLI commands ported with identical behavior and file formats.
- **BREAKING CHANGE: Python removal**: `pyproject.toml`, `uv.lock`, and all `.py` files removed. Install via `cargo install corky` instead of `pip install corky`.
- **Dependencies**: clap (CLI), serde/toml/toml_edit (config), imap/native-tls (IMAP), mailparse (email parsing), lettre (SMTP), chrono (dates), directories (platform dirs), tokio (watch daemon), anyhow/thiserror (errors).
- **Format-preserving TOML**: `add-label` now uses `toml_edit` crate (cleaner than Python's regex approach).
- **Gmail OAuth**: Stub that prints instructions to use Python `corky sync-auth` for one-time setup. Full OAuth port deferred.
- **No behavioral changes**: All file formats, sync algorithms, collaborator workflows, and CLI interfaces are identical to 0.6.1. Existing `mail/` data repos are fully compatible.

**Migration from 0.6.x**: Uninstall Python corky (`pip uninstall corky`), install Rust binary (`cargo install corky` or download from releases). No data migration needed — all file formats are unchanged.

## 0.6.1

Lower Python requirement to 3.11+, app config with multi-space support.

- **Python 3.11+**: Lowered minimum from 3.12. No 3.12-specific features were used; `tomllib` and `datetime.UTC` (3.11+) are the actual floor.
- **App config (`src/app_config.py`)**: New module for persistent configuration via `platformdirs`. Stores named spaces in `~/.config/corky/config.toml` (Linux), `~/Library/Application Support/corky/config.toml` (macOS), `%APPDATA%/corky/config.toml` (Windows).
- **Spaces**: `corky spaces` lists configured spaces. `corky init` registers a space automatically. `corky --space NAME <cmd>` selects a specific space for any command.
- **Data dir resolution updated**: New 4-step order: `mail/` in cwd → `CORKY_DATA` env → app config space → `~/Documents/mail` fallback.

## 0.6.0

Path resolution, `corky init`, and functional specification.

- **`corky init`**: New command to initialize a data directory for general users. `corky init --user you@gmail.com` creates `~/Documents/mail` with directory structure, `accounts.toml`, and empty config files. Supports `--provider`, `--password-cmd`, `--labels`, `--github-user`, `--name`, `--sync`, `--force`, `--data-dir` flags.
- **Path resolution (`src/resolve.py`)**: New module centralizes all path resolution. Data directory resolves in order: `mail/` in cwd (developer), `CORKY_DATA` env, `~/Documents/mail` (general user). Config directory: `.` if local `mail/` exists, otherwise same as data dir.
- **BREAKING CHANGE: Removed module-level path constants**: `CONFIG_PATH` removed from `accounts.py`, `collab/__init__.py`, `contact/__init__.py`. `CONTACTS_DIR` removed from `contact/add.py`. `STATE_FILE` and `CONVERSATIONS_DIR` removed from `sync/imap.py`. `CREDENTIALS_FILE` removed from `sync/auth.py`. `VOICE_FILE` removed from `collab/add.py`, `collab/sync.py`, `collab/reset.py`. `_DIR_PREFIXES` removed from `collab/rename.py`. All replaced by `resolve.*()` function calls. Tests that monkeypatched these constants must now patch `resolve.<function>` instead.
- **`SPEC.md`**: Language-independent functional specification for an eventual Rust port. Covers file formats, algorithms (slugify, thread key, dedup, label routing), all 18 commands, sync algorithm, collaborator lifecycle, provider presets.

**Migration from 0.5.x**: If your code or tests monkeypatch `CONFIG_PATH`, `CONTACTS_DIR`, `STATE_FILE`, `CONVERSATIONS_DIR`, `CREDENTIALS_FILE`, `VOICE_FILE`, or `_DIR_PREFIXES`, switch to patching `resolve.accounts_toml`, `resolve.contacts_dir`, `resolve.sync_state_file`, `resolve.conversations_dir`, `resolve.credentials_json`, `resolve.voice_md`, or `resolve.data_dir` respectively.

## 0.5.0

Directional collaborator repos, nested CLI, owner identity, GitHub username keys.

- **Rename `shared/` to `for/{gh-user}/`**: Collaborator submodules now live under `for/{github_user}/` instead of `shared/{name}/`. Repo naming: `{owner}/to-{collab-gh}`. This supports multi-user corky setups where each party has directional directories (`for/` outgoing, `by/` incoming).
- **BREAKING CHANGE: `collab-*` → `for *` / `by *`**: Flat `collab-add`, `collab-sync`, `collab-remove`, `collab-rename`, `collab-reset`, `collab-status` replaced with nested `for add`, `for sync`, `for remove`, `for rename`, `for reset`, `for status`. `find-unanswered` and `validate-draft` moved to `by find-unanswered` and `by validate-draft`. Standalone `find-unanswered` and `validate-draft` entry points kept for `uvx` use.
- **Owner identity**: New `[owner]` section in `accounts.toml` with `github_user` and optional `name`. Required for collaborator features.
- **GitHub username as collaborator key**: `collaborators.toml` section keys are now GitHub usernames (was display names). `github_user` is derived from the key. Optional `name` field stores display name.
- **Auto-derived repo**: `repo` field in `collaborators.toml` is auto-derived as `{owner_gh}/to-{collab_gh}` if omitted.
- **`for add` CLI change** (was `collab-add`): Positional arg is now `GITHUB_USER` (was `NAME`). `--github-user` flag removed (redundant). Added `--name` flag for display name.
- **Parameterized templates**: AGENTS.md and README.md templates use owner name from config instead of hardcoded "Brian".
- Help output now groups commands into sections: corky commands, collaborator commands (for/by), dev commands.
- `.gitignore`: `shared/` replaced with `for/` and `by/`.

**Migration from 0.4.x**: Remove existing `shared/` submodules (`git submodule deinit -f shared/{name} && git rm -f shared/{name}`), update `collaborators.toml` keys to GitHub usernames, add `[owner]` section to `accounts.toml`, re-add collaborators with `corky for add`. Replace `collab-*` with `for *` and `find-unanswered` / `validate-draft` with `by find-unanswered` / `by validate-draft` in scripts and aliases. Run `for reset` to update shared repo templates.

## 0.4.1

Add-label command and audit-docs fixes.

- `corky add-label LABEL --account NAME`: Add a label to an account's sync config via text-level TOML edit (preserves comments).
- `contact-add` integration: `--label` + `--account` automatically adds label to account sync config.
- audit-docs: Fix tree parser for symlink-to-directory entries.
- SKILL.md: Updated to reflect flat conversation directory, contacts, manifest.

## 0.4.0

Flat conversation directory, contacts, manifest.

- **Flat conversations**: All threads in `mail/conversations/` as `[slug].md`. No account or label subdirectories. Consolidates correspondence across multiple email accounts into one directory.
- **Immutable filenames**: Slug derived from subject on first write, never changes. Thread identity tracked by `**Thread ID**` metadata.
- **File mtime**: Set to last message date via `os.utime()`. `ls -t` sorts by thread activity.
- **Multi-source accumulation**: Threads fetched from multiple labels or accounts accumulate all sources in `**Labels**` and `**Accounts**` metadata.
- **Orphan cleanup**: `--full` sync deletes files not touched during the run.
- **manifest.toml**: Generated after sync. Indexes threads by labels, accounts, contacts, and last-updated date.
- **Contacts**: `contacts.toml` maps contacts to email addresses. Per-contact `AGENTS.md` in `mail/contacts/{name}/` provides drafting context. `corky contact-add` scaffolds new contacts.
- **tomli-w**: Added as dependency for TOML writing.
- Backward-compatible parsing of legacy `**Label**` format.

## 0.3.0

IMAP polling daemon.

- `corky watch` polls IMAP on an interval and syncs automatically.
- Configurable poll interval and desktop notifications via `accounts.toml` `[watch]` section.
- systemd and launchd service templates.

## 0.2.1

Maintenance release.

- CI workflow: ty, ruff, pytest on push and PR.

## 0.2.0

Collaborator tooling and multi-account support.

- `collab-reset` command (now `for reset`): pull, regenerate templates, commit and push.
- Reframed docs as human-and-agent friendly.
- `account:label` scoped routing for collaborators.
- `list-folders` command and self-signed cert support.
- Multi-account IMAP support via `accounts.toml` with provider presets.
- Collaborator tooling: `collab-add` (now `for add`), `collab-sync` (now `for sync`), `collab-remove` (now `for remove`), `find-unanswered` (now `by find-unanswered`), `validate-draft` (now `by validate-draft`).
- Multi-collaborator architecture with submodule-based sharing.

## 0.1.0

Renamed to corky. Unified CLI dispatcher.

- `corky` CLI with subcommands.
- `push-draft` command to create drafts or send emails from markdown.
- Incremental IMAP sync with `--full` option.
- Gmail sync workspace with drafting support.
