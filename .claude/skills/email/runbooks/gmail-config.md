# Gmail Account Configuration

## btak.dev@gmail.com (gmail-api provider)

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
- Browser OAuth binds `127.0.0.1:8484` by default; use `CORKY_OAUTH_CALLBACK_PORT` for a registered override, or `CORKY_OAUTH_ALLOW_EPHEMERAL_PORT=1` only with wildcard-capable Google OAuth clients.

## SMTP Accounts (personal, proton-dev)

```toml
[accounts.personal]
provider = "gmail"
user = "brian.takita@gmail.com"
password_cmd = "pass email/gmail"
```

Requires password for both IMAP draft push and SMTP send.

<!-- Refreshed for the fixed 127.0.0.1:8484 Google OAuth callback default with opt-in arbitrary-port fallback. -->
