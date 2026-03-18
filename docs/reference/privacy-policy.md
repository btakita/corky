# Privacy Policy

**Corky — Correspondence Kit**
**Last updated:** 2026-03-18

## What Corky Does

Corky is an open-source, self-hosted email management tool that runs entirely on your local machine. When using the public Corky OAuth app to connect to Gmail, the following applies:

## Data Collection

**Corky does not collect, store, or transmit any of your data to external servers.** All email data remains on your local machine.

Specifically:

- **Email content** is fetched from Gmail and stored locally in your configured mail directory. It never leaves your machine.
- **OAuth tokens** are stored locally on your filesystem (typically `~/.config/corky/`). They are never sent to any server other than Google's OAuth endpoints.
- **No analytics, telemetry, or tracking** of any kind is performed.
- **No third-party services** receive your data.

## Google API Usage

Corky requests the following Gmail API scopes:

- `gmail.readonly` — Reading email messages and metadata, listing labels and mailboxes
- `gmail.send` — Sending emails (saved as drafts for your review first)
- `gmail.settings.basic` — Managing Gmail filters and labels

Corky may also request:

- `youtube.readonly` — Reading YouTube channel data and video metadata

Corky does **not** request permission to:

- Delete emails
- Access contacts or calendar data
- Modify YouTube content

## Data Storage

All data is stored locally on your machine:

- Email messages in your configured mail directory
- OAuth credentials in your local config directory
- No cloud storage, no remote databases, no server-side processing

## Data Sharing

Corky does not share your data with anyone. Period.

- No advertising partners
- No analytics providers
- No third-party integrations that receive your email data

## Your Rights

Since all data is local to your machine:

- **Delete your data** at any time by removing your mail directory and config files
- **Revoke access** at any time via your Google Account security settings
- **Inspect the code** — Corky is open source

## Self-Hosted Credentials

You may use your own Google Cloud project credentials instead of the public Corky app. In that case, this privacy policy does not apply — your data flows directly between your machine and your own Google Cloud project.

## Changes to This Policy

Changes will be published to this repository. Since Corky collects no data, policy changes are unlikely to affect you.

## Contact

For questions about this privacy policy, open an issue on the [Corky GitHub repository](https://github.com/btakita/corky).
