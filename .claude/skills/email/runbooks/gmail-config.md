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
- Browser auth binds the loopback listener before opening the browser, defaults to `127.0.0.1:8484`, and honors `CORKY_OAUTH_CALLBACK_PORT` for a one-session override

## SMTP Accounts (personal, proton-dev)

```toml
[accounts.personal]
provider = "gmail"
user = "brian.takita@gmail.com"
password_cmd = "pass email/gmail"
```

Requires password for both IMAP draft push and SMTP send.
