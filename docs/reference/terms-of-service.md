# Terms of Service

**Corky — Correspondence Kit**
**Last updated:** 2026-03-18

## Acceptance

By using Corky with the public OAuth app credentials, you agree to these terms.

## Description of Service

Corky is an open-source, self-hosted email management tool. It fetches email from providers like Gmail via API and stores it locally on your machine. Corky runs entirely on your hardware — there is no hosted service.

## Use of the Public OAuth App

The public Corky OAuth app is provided as a convenience so you don't need to create your own Google Cloud project. When using the public app:

- You authorize Corky to access your Gmail (read, send, manage filters) and optionally YouTube (read-only)
- All data remains on your local machine
- The OAuth app credentials are shared across all Corky users, but your tokens and data are private to your machine

## Destructive Actions Warning

Corky can perform actions that are **irreversible or difficult to reverse**:

- **Sending emails:** Once sent via the Gmail API, emails cannot be unsent. Corky saves emails as drafts for your review before sending — always review drafts before confirming delivery.
- **Filter management:** Corky can create, modify, and overwrite Gmail filters. Applying filter changes may replace your existing filters. Always review filter changes before pushing them to Gmail (`corky filter check` shows the diff).
- **Label management:** Corky can create and modify labels.

**You are solely responsible for reviewing and confirming all destructive actions.** Corky provides preview and diff tools to help you verify changes before applying them, but the final decision to execute is yours.

## No Warranty

Corky is provided "as is" without warranty of any kind, express or implied. This includes but is not limited to:

- No guarantee of uninterrupted access to Gmail or other services
- No guarantee of data accuracy or completeness
- No liability for data loss (your local backups are your responsibility)
- **No liability for emails sent, filters overwritten, or any other actions performed through Corky** — you are responsible for verifying all operations before confirming them

## Limitations

- Corky is subject to Google's API usage policies and rate limits
- Google may revoke or restrict the public OAuth app at any time
- You are responsible for compliance with any applicable laws regarding your email data

## Self-Hosted Alternative

You may use your own Google Cloud project credentials at any time. This removes any dependency on the public Corky OAuth app and these terms.

## Termination

You may stop using the public OAuth app at any time by:

1. Revoking access in your Google Account security settings
2. Deleting your local OAuth tokens
3. Optionally switching to self-hosted credentials

## Changes

These terms may be updated. Continued use of the public OAuth app constitutes acceptance of updated terms.

## Contact

For questions, open an issue on the [Corky GitHub repository](https://github.com/btakita/corky).
