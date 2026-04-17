# Correspondence Kit

## Two Repos

**corky** is the tool — Rust source, tests, config templates. It is a public repo.

**mail** is the data — synced threads, drafts, contacts, mailboxes.
It is a separate, private repo. corky accesses it via a `mail/` path in the
working directory, which can be either:

- A **symlink** to an external clone (e.g. `mail -> ~/data/mail`)
- A **subdirectory** or nested clone inside the corky checkout

**Developer workflow:** `mail/` exists at the working directory root (symlink or subdirectory).
The `mail` entry in `.gitignore` keeps the data repo out of corky's git history.

**General user workflow:** `corky init --user EMAIL` creates `mail/` in the current
directory with config inside it, and registers the project dir as a named mailbox.
Commands find the data dir via the resolution order in `src/resolve.rs`: local `mail/`,
`CORKY_DATA` env, app config mailbox, `~/Documents` fallback. Use `--mailbox NAME` to select a
specific mailbox.

## Writing Voice

See `voice.md` (committed) for tone, style, and formatting guidelines.

## Safety Rules

- **Never send email directly.** Always save as a Gmail draft for review first.
- **Never guess at intent.** If the right response is unclear, ask rather than assume.
- **Never share conversation content** outside this local environment (no third-party APIs) unless explicitly instructed.

## Drafting Emails

- **Reply vs new thread:** Follow-ups, corrections, and replies to recent emails → threaded reply. New topics → new thread. When ambiguous, default to reply and ask.
- **Threading a reply:** Find the original email in `mail/conversations/`, extract its Message-ID and the `**Thread ID**` line. Set both `in_reply_to` (RFC 2822 Message-ID) **and** `thread_id` (Gmail internal ID) in the draft YAML — Gmail requires both to attach the draft to the existing thread. Derive subject as `Re: <original subject>`. Pass via CLI: `corky draft new --in-reply-to "<msg-id>" --thread-id "<thread-id>" ...`
- **Verify facts:** Before sending, cross-check claims about a contact's platform, tools, or status against their `mail/contacts/<name>/CLAUDE.md`. Don't infer from ambiguous notes — confirm explicitly.
- **Gmail threading:** Subject changes break threads. Always use `Re: <original>` for replies, never a new subject.

## Environment Setup

**New user (quick install):**
```sh
curl -sSf https://raw.githubusercontent.com/btakita/corky/main/install.sh | sh
corky init --user you@gmail.com
```

**New user (from source):**
```sh
cargo install --path .
corky init --user you@gmail.com
```

**Developer (from repo checkout):**
```sh
cp .corky.toml.example mail/.corky.toml   # configure your email accounts
make release                                              # build + symlink to .bin/corky
```

See README.md for full config reference (.corky.toml, Gmail OAuth).

## Google Workspace Integration

Corky has native support for all Google Workspace services. No external plugin required.

Authenticate once:

```bash
corky filter auth   # Gmail, Drive, Docs, Sheets, Chat, Tasks (shared token flow)
corky cal auth      # Google Calendar (separate token)
```

**Command reference:**
- Gmail send (with attachments) → `corky draft send --attachment <file>`
- Gmail draft → `corky draft push`
- Google Docs → `corky docs read <url>` / `corky docs write <url> <file>`
- Google Drive → `corky doc upload`
- Google Sheets → `corky sheets read <url>` / `corky sheets write <url> <range> <file>`
- Google Chat → `corky chat send <space> <message>`
- Google Tasks → `corky tasks list/add/done`
- Google Calendar → `corky cal list/create/delete/check`

## Sync Behavior

- **Immutable filenames**: Slug derived from subject on first write, never changes.
  Thread identity tracked by `**Thread ID**` metadata inside the file.
- **File mtime**: Set to last message date via `libc::utime()`.
- **Multi-label accumulation**: Thread fetched from multiple labels/accounts accumulates all in metadata.
- **Incremental by default**: Tracks IMAP UIDs per-account in `.sync-state.json`. `sync full` re-fetches everything.
- **Streaming writes**: Each message merged immediately. If sync crashes, state is not saved; next run re-fetches.
- **Shared label routing**: Labels in `[routing]` section of `.corky.toml` route to `mail/mailboxes/{name}/conversations/`.
  One label can fan-out to multiple mailboxes.
- **Dedup**: Messages deduplicated by `(sender, date)` tuple when merging into existing files.
- **Slug collisions**: Different threads with same slug get `-2`, `-3` suffix.
- **Orphan cleanup**: On `sync full`, files not touched during sync are deleted.
- **HTML-first body extraction**: Prefers `text/html` (converted to markdown via `htmd`) over `text/plain`. Falls back to plain text when no HTML part exists.

## File Formats

See README.md for conversation markdown format, draft format, and status values.

## Mailbox Config

Use `[routing]` for label-to-mailbox mapping and `[mailboxes.*]` for per-mailbox settings in `.corky.toml`:

```toml
[routing]
for-alex = ["mailboxes/alex"]
shared = ["mailboxes/alice", "mailboxes/bob"]

[mailboxes.alex]
auto_send = false

[mailboxes.bob]
auto_send = true
```

**Label scoping**: `account:label` syntax binds a label to one account (e.g. `"proton-dev:INBOX"`).
Plain labels match all accounts.

## Package-Level Instruction Files

Each module directory can contain its own `AGENTS.md` with package-specific conventions.
Keep the root `AGENTS.md` focused on cross-cutting concerns.

**Dual-name convention:** `AGENTS.md` is canonical (committed). `CLAUDE.md` is a symlink.
Personal overrides: `CLAUDE.local.md` / `AGENTS.local.md` (gitignored).

**Actionable over informational.** Instruction files contain the minimum needed to generate
correct code. Reference material belongs in `README.md`.

**Update with the code.** When a change affects patterns, conventions, or module boundaries,
update instruction files as part of the same change.

**Stay concise.** Combined root + package files should stay well under 1000 lines.

## Workflow

Follow a research → plan → implement cycle. Never write code until the plan is reviewed.
([Reference](https://boristane.com/blog/how-i-use-claude-code/))

1. **Research** — Read the relevant code deeply. Document findings in `research.md`.
   Iterate with the user until misunderstandings are resolved.
2. **Plan** — Write a detailed implementation plan in `plan.md` with file paths and
   code snippets. Reference existing patterns in the codebase as guides. Iterate with
   the user — they add inline notes, reject approaches, inject domain knowledge.
   Repeat until the plan is solid ("don't implement yet").
3. **Todo** — Produce a granular todo list from the approved plan before writing any code.
4. **Implement** — Execute the plan. Mark completed tasks, maintain strict typing,
   and continuously run checks (`make check`). Terse single-sentence feedback is fine
   during this phase.
5. **Precommit** — Run `make precommit` and `corky audit-docs` before committing.

## Review Pipeline

Three-step quality gate before committing. Run in order — later steps assume earlier ones pass.

1. **`make check`** — clippy + tests. Catches compilation errors, lint violations, and regressions.
2. **`corky audit-docs`** — instruction file and SPEC.md consistency. Ensures docs match code.
3. **`/simplify`** — Claude Code built-in. Reviews changed files for reuse, quality, and efficiency.

Steps 1–2 are automated (CI-runnable). Step 3 requires Claude Code. Skip gracefully if a tool is unavailable.

## PR Process

- Feature work happens on a branch with a PR.
- `research.md` and `plan.md` are committed to the branch for iteration (they are
  gitignored on main). Before the squash merge, remove them from the branch so they
  never land on main.
- PRs use **squash merge** — the branch history (including research/plan commits)
  collapses into one clean commit on main.
- Keep `SPEC.md` up to date on each commit — don't defer spec updates to a final
  cleanup pass. Every commit that changes behavior should include the corresponding
  spec change.

## Release Process

1. Bump version in `Cargo.toml` + `pyproject.toml` (keep in sync)
2. `make check` (clippy + test)
3. Branch → PR → squash merge to main
4. Tag: `git tag v<version> && git push origin v<version>`
5. `cargo publish` (crates.io)
6. `maturin publish` (PyPI)
7. `gh release create v<version> --generate-notes` with prebuilt binary (GitHub Release)
8. Install binary: `cargo install --path .`

Publish order when instruction-files changes: instruction-files → agent-doc → corky.

## Configuration

**Single config file:** All corky configuration belongs in `.corky.toml`. Do not create separate `*.toml` files for new features (e.g., no `profiles.toml`, `filters.toml`, etc.). Nest new config sections under `.corky.toml` using TOML tables (e.g., `[profiles.btakita.linkedin]`, `[youtube]`).

**Existing exception:** `profiles.toml` currently exists as a separate file — it should be consolidated into `.corky.toml` under a `[profiles]` section.

## Conventions

- Use `make check` (clippy + test), `make release` (build + .bin symlink) for development
- Use `serde` derive for all data types
- Use `anyhow` for application errors, `thiserror` for domain errors
- Use `toml_edit` for format-preserving TOML edits (add-label)
- Use `std::process::Command` for git operations (not `git2`)
- Use `regex` + `once_cell::Lazy` for compiled regex patterns
- Keep sync, draft, mailbox, contact, social, schedule, topics, filter, cal logic in separate modules
- Do not commit `.env`, `.corky.toml`, `CLAUDE.local.md` / `AGENTS.local.md`, or `mail`
- Never bump versions automatically — the user will bump versions explicitly
- Commits that include a version change should include the version number in the commit message
- Use `BREAKING CHANGE:` prefix in VERSIONS.md entries for incompatible changes
- Update `SPEC.md` in the same commit as the code change (see PR Process)
- Commits must be clean — no dangling unstaged files. When splitting work across commits, stage all related files (including `Cargo.lock`)



## Library Context Policy

This library follows the agent-loop library-context policy. Contributors
authoring `AGENTS.md`, `SKILL.md`, or runbooks in this repo must read:

[Library Context Policy](../instruction-files/LIBRARY_CONTEXT_POLICY.md)

before making changes.
