//! Template generators for mailbox repos (AGENTS.md, README.md).

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().to_string() + c.as_str(),
    }
}

/// Generate AGENTS.md for a mailbox repo.
pub fn generate_agents_md(name: &str, owner_name: &str) -> String {
    let title_name = capitalize(name);
    format!(
        r#"# {owner_name}'s Mailbox for {title_name}

## Workflow

1. `git pull` to get the latest synced conversations
2. Read threads in `conversations/` for context
3. Draft replies in `drafts/`
4. `git add`, `git commit`, and `git push` when done

## Conversation format

Synced conversations live in `conversations/`. Each file is one thread:

```markdown
# Subject Line

**Labels**: label-name, other-label
**Accounts**: personal
**Thread ID**: thread-id
**Last updated**: Mon, 10 Feb 2025 11:00:00 +0000

---

## Sender Name <sender@example.com> — Mon, 10 Feb 2025 10:00:00 +0000

Message body text.

---

## Another Sender — Mon, 10 Feb 2025 11:00:00 +0000

Reply body text.
```

## Finding unanswered threads

Run the helper command to find threads awaiting a reply:

```sh
corky unanswered
corky unanswered --from "{owner_name}"
```

## Drafting a reply

Use the `draft new` command to scaffold a draft:

```sh
corky draft new --to recipient@example.com "Subject line"
corky draft new --to recipient@example.com --in-reply-to "<msg-id>" "Re: Thread subject"
```

Or create a file manually in `drafts/` named `YYYY-MM-DD-slug.md`:

```markdown
# Subject

**To**: recipient@example.com
**CC**: (optional)
**Status**: review
**Author**: {name}
**Account**: (optional -- account name for sending)
**From**: (optional -- email address for sending)
**In-Reply-To**: (optional -- message ID from thread)

---

Body text here.
```

### Required fields
- `# Subject` heading
- `**To**`: recipient email
- `**Status**`: set to `review` when ready for {owner_name}
- `**Author**`: your name (`{name}`)
- `---` separator before the body

### Replying to an existing thread
Set `**In-Reply-To**` to a message ID from the conversation thread. Message IDs
are not shown in the markdown files -- ask {owner_name} for the ID or leave it blank
and note which thread you're replying to in the body.

### Validating a draft

```sh
corky draft validate drafts/2026-02-19-example.md
```

### Status flow

`draft` -> `review` -> `approved` -> `sent`

- **draft**: work in progress (not ready for {owner_name})
- **review**: ready for {owner_name} to review
- **approved**: {owner_name} approved, ready to send
- **sent**: email has been sent (only {owner_name} sets this)

## Voice guidelines

See `voice.md` for {owner_name}'s writing voice. Match this style when drafting
on his behalf.

## What you can do

- Read conversations
- Create and edit drafts
- Run `corky unanswered` and `corky draft validate`
- Push changes to this repo

## What only {owner_name} can do

- Sync new emails into this repo
- Send emails (requires email credentials)
- Change draft Status to `sent`
"#,
        name = name,
        owner_name = owner_name,
    )
}

/// Generate README.md for a mailbox repo.
pub fn generate_readme_md(name: &str, owner_name: &str) -> String {
    let title_name = capitalize(name);
    format!(
        r#"# {owner_name}'s Mailbox for {title_name}

This repo contains email threads {owner_name} has shared with you and a place for you
to draft replies on his behalf.

## Quick start

```sh
git clone <this-repo-url>
cd <repo-name>
curl -sSf https://raw.githubusercontent.com/btakita/corky/main/install.sh | sh
```

### 1. Read conversations

Synced threads are in `conversations/`. Pull to get the latest:

```sh
git pull
```

### 2. Find threads that need a reply

```sh
corky unanswered
```

### 3. Draft a reply

Scaffold a draft with `corky draft new`:

```sh
corky draft new --to recipient@example.com "Subject line"
```

Or create a file manually in `drafts/` named `YYYY-MM-DD-slug.md`:

```markdown
# Subject

**To**: recipient@example.com
**Status**: review
**Author**: {name}

---

Your reply here.
```

Set **Status** to `review` when it's ready for {owner_name} to look at.

### 4. Validate and push

```sh
corky draft validate drafts/your-draft.md
git add drafts/
git commit -m "Draft reply to ..."
git push
```

{owner_name} will review your draft, and if approved, send it from his email account.

## Reference

See `AGENTS.md` for the full draft format (CC, In-Reply-To, Account, From),
conversation format, status flow, and voice guidelines.
"#,
        name = name,
        owner_name = owner_name,
    )
}
