# Commands

All commands are available through the `corky` CLI.

## General

```sh
corky --help                    # Show all commands
corky help [FILTER]             # Show command reference (optional filter)
corky init --user EMAIL        # Initialize in current directory
corky init --user EMAIL /path # Initialize at specific path
corky install-skill email     # Install the email agent skill
```

### init

```
corky init --user EMAIL [PATH] [--provider PROVIDER]
           [--password-cmd CMD] [--labels LABEL,...] [--github-user USER]
           [--name NAME] [--mailbox-name NAME] [--sync] [--force]
```

Creates `{path}/mail/` with directory structure, `.corky.toml`, and `voice.md`. If inside a git repo, adds `mail` to `.gitignore`. Registers the project dir as a named mailbox in app config.

- `--provider`: `gmail` (default), `protonmail-bridge`, `imap`
- `--labels`: default `correspondence` (comma-separated)
- `--force`: overwrite existing config
- `--sync`: run sync after init

### install-skill

```
corky install-skill NAME
```

Install an agent skill into the current directory. Currently supported: `email`.
Mailbox repos ship the skill automatically via `mb add`/`mb reset`.

## Sync

```sh
corky sync                     # Incremental IMAP sync (all accounts)
corky sync full                # Full re-sync (ignore saved state)
corky sync account personal    # Sync one account
corky sync routes              # Apply routing rules to existing conversations
corky sync mailbox [NAME]      # Push/pull shared mailboxes
```

### sync-auth

```sh
corky sync-auth
```

Gmail OAuth setup. Requires `credentials.json` from Google Cloud Console.

## Import

### Telegram

```sh
corky sync telegram-import FILE            # Import Telegram Desktop JSON export
corky sync telegram-import DIR             # Import from directory containing result.json
corky sync telegram-import FILE --label personal --account tg-personal
```

Import Telegram Desktop JSON exports into corky conversations. Each chat becomes a thread with ID `tg:{chat_id}`. Export from Telegram Desktop: Settings > Advanced > Export Telegram data > JSON format.

### Slack

```sh
corky slack import FILE.zip                # Import Slack workspace export ZIP
corky slack import FILE.zip --label work --account slack-work
```

Import Slack workspace export ZIPs. Messages are grouped by `thread_ts` into threads with ID `slack:{channel_id}:{thread_ts}`. Export from Slack: Workspace admin > Settings > Import/Export Data > Export.

Both commands support `--label` (default: provider name) and `--account` (default: provider name) flags.

## Email

```sh
corky list-folders [ACCOUNT]   # List IMAP folders for an account
corky add-label LABEL --account NAME         # Add a label to sync config
corky unanswered                             # Find threads awaiting a reply (all scopes)
corky unanswered .                           # Root conversations only
corky unanswered NAME                        # Specific mailbox only
```

## Google Workspace

```sh
corky auth --email you@gmail.com --scope docs             # Pre-authenticate Google Docs
corky auth --email you@gmail.com --scope sheets-readonly  # Pre-authenticate read-only Sheets
corky docs read DOC_URL [-o FILE]                         # Read Google Doc text
corky docs write DOC_URL FILE                             # Replace Google Doc text from markdown
corky sheets read SHEET_URL --range A1:D10 --format csv   # Read Google Sheet range
corky sheets write SHEET_URL RANGE CSV                    # Write CSV to Google Sheet range
corky sheets pull SHEET_URL TAB CSV                       # Sync a tab down to CSV
corky sheets push SHEET_URL TAB CSV                       # Clear/create tab, then sync local CSV
corky sheets delete-tab SHEET_URL TAB                     # Delete a Google Sheet tab
```

## Drafts

```sh
corky draft new "Subject" --to EMAIL         # Scaffold a new draft file
corky draft new "Subject" --to EMAIL --mailbox NAME  # Create in mailbox drafts/
corky draft validate                         # Validate all drafts (root + mailboxes)
corky draft validate .                       # Validate root drafts only
corky draft validate NAME                    # Validate drafts in a mailbox
corky draft validate FILE [FILE...]          # Validate specific files
corky draft push mail/drafts/FILE.md         # Save a draft via IMAP
corky draft push mail/drafts/FILE.md --send  # Send via SMTP
```

### draft new

```
corky draft new SUBJECT --to EMAIL [--cc EMAIL] [--account NAME]
               [--from EMAIL] [--in-reply-to MSG-ID] [--mailbox NAME]
```

Scaffolds a new draft file with pre-filled metadata. Creates `drafts/YYYY-MM-DD-slug.md` and prints the path. Author resolved from `[owner] name` in `.corky.toml`.

### Reply Threading Heuristics

When drafting an email, determine whether to thread as a reply or start a new thread:

| User intent | Action |
|---|---|
| Follow-up, correction, or reply to a recent email | **Threaded reply** — find the original in `mail/conversations/`, set `in_reply_to` to its Message-ID, derive subject as `Re: <original subject>` |
| New topic to the same person | **New thread** — no `in_reply_to`, fresh subject |
| Ambiguous | Default to reply if there's a recent conversation with the same contact; ask if unclear |

**Gmail threading requirements:**
- `In-Reply-To` and `References` headers must reference the original Message-ID
- Subject must match (adding `Re:` prefix is fine; changing the subject entirely breaks threading)
- Mismatched subjects cause Gmail to create a new thread even with correct headers

### draft push

Default: creates a draft via IMAP APPEND to the drafts folder.
`--send`: sends via SMTP. Requires Status to be `review` or `approved`. After sending, updates Status to `sent`.

Account resolution:
1. `**Account**` field → match by name in `.corky.toml`
2. `**From**` field → match by email address
3. Fall back to default account
4. Credential bubbling: if the draft is inside a mailbox, walk parent directories for a `.corky.toml` with matching credentials

## Contacts

```sh
corky contact add NAME --email EMAIL [--email EMAIL2]   # Add a contact manually
corky contact add --from SLUG [NAME]                    # Create from a conversation
corky contact info NAME                                 # Show contact details + threads
```

### contact add

Manual mode: creates `mail/contacts/{name}/` with `AGENTS.md` template and `CLAUDE.md` symlink. Updates `.corky.toml`.

From-conversation mode (`--from`): finds the conversation, extracts non-owner participants from From/To/CC headers, and creates an enriched contact with pre-filled AGENTS.md (Topics, Formality, Tone, Research sections).

### contact info

Aggregates contact information: emails from config, AGENTS.md content, matching threads from manifest.toml (root and mailboxes), and a summary with thread count and last activity.

## Mailboxes

```sh
corky mailbox add NAME --label LABEL [--name NAME] [--github] [--pat]
corky mailbox sync [NAME]                   # Push/pull shared mailboxes
corky mailbox status                        # Check mailbox status
corky mailbox list                          # List registered mailboxes
corky mailbox remove NAME [--delete-repo]   # Remove a mailbox
corky mailbox rename OLD NEW [--rename-repo] # Rename a mailbox
corky mailbox reset [NAME] [--no-sync]      # Regenerate templates
```

All mailbox commands accept the `mb` alias (e.g. `corky mb add`).

## Watch

```sh
corky watch                    # Poll IMAP and sync on an interval
corky watch --interval 60      # Override poll interval (seconds)
```

## Global flags

```sh
corky --mailbox NAME <command>  # Use a specific mailbox for any command
```

## Development

```sh
corky audit-docs               # Audit instruction files for staleness
```
