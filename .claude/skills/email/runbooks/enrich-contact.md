# Enrich Contact

Update a contact's profile with new information from conversations, research, or user input.

## Steps

1. **Check if contact exists:**
   ```bash
   ls mail/contacts/<name>/
   ```
   If not, create directory and `AGENTS.md` (with `CLAUDE.md` symlink)

2. **Read existing profile:** `mail/contacts/<name>/CLAUDE.md` (symlink to AGENTS.md)

3. **Gather new information from:**
   - Recent conversations in `mail/conversations/` (grep for their email/name)
   - LinkedIn profile (via `chromium-bridge markdown <url>` if URL known)
   - User-provided context from the agent-doc session
   - Social profiles in `mail/social/` if available

4. **Update AGENTS.md** with new facts:
   - Role, company, title
   - Communication preferences
   - Relationship context (how you met, shared projects)
   - Platform/tools they use (verified, not inferred)
   - Last interaction date

5. **Cross-check:** Verify all claims against source material. Don't infer platform usage from ambiguous notes.

6. **Commit** the updated contact file

If you need `corky contact push google` while enriching a contact, remember the Google OAuth flow now binds the callback listener before browser launch and honors `CORKY_OAUTH_CALLBACK_PORT` for the current session.

## File Convention

- `AGENTS.md` is canonical (committed)
- `CLAUDE.md` is a symlink to `AGENTS.md`
- Personal overrides: `CLAUDE.local.md` / `AGENTS.local.md` (gitignored)
