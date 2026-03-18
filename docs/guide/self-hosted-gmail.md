# Self-Hosted Gmail Setup

By default, `corky init` uses corky's public OAuth app for Gmail access — zero config required. If you prefer full control over your credentials, you can create your own GCP project instead.

## Why self-host?

- **Privacy.** Your OAuth tokens never touch a third-party client ID.
- **Control.** You own the GCP project — revoke access, audit logs, rotate secrets on your schedule.
- **No dependency.** The public corky app could change scopes, hit quota limits, or disappear. Your own project is yours.

For most users, the public app is fine. Self-hosting is for those who want the extra layer of control.

## Prerequisites

- A Google account
- Basic comfort with the command line
- corky installed (`corky --version` to confirm)

## Step 1: Create a GCP project

1. Go to [console.cloud.google.com](https://console.cloud.google.com/)
2. Click the project dropdown (top bar) → **New Project**
3. Name it something like `corky-personal`
4. Click **Create**
5. Select the new project from the dropdown

## Step 2: Enable the Gmail API

1. In the sidebar, go to **APIs & Services → Library**
2. Search for **Gmail API**
3. Click **Gmail API** → **Enable**

## Step 3: Configure the OAuth consent screen

1. Go to **APIs & Services → OAuth consent screen**
2. Select **External** user type → **Create**
3. Fill in the required fields:
   - **App name:** `corky` (or whatever you like)
   - **User support email:** your email
   - **Developer contact:** your email
4. Click **Save and Continue**
5. On the **Scopes** page, click **Add or Remove Scopes** and add:
   - `https://www.googleapis.com/auth/gmail.readonly`
   - `https://www.googleapis.com/auth/gmail.send`
   - `https://www.googleapis.com/auth/gmail.settings.basic`
6. Click **Save and Continue** through the remaining steps

Your app will stay in "testing" mode. This limits access to 100 test users — more than enough for personal use. No Google verification needed.

## Step 4: Create an OAuth client

1. Go to **APIs & Services → Credentials**
2. Click **Create Credentials → OAuth client ID**
3. Application type: **Desktop app**
4. Name: `corky` (or anything)
5. Click **Create**
6. Copy the **Client ID** and **Client Secret**

## Step 5: Configure corky

Add a `[gmail]` section to your `.corky.toml`:

```toml
[gmail]
client_id = "123456789-abcdef.apps.googleusercontent.com"
client_secret = "GOCSPX-your-secret-here"
```

To avoid storing secrets in plaintext, use command substitution:

```toml
[gmail]
client_id_cmd = "pass corky/gmail/client_id"
client_secret_cmd = "pass corky/gmail/client_secret"
```

The `_cmd` variants run the command at runtime and use its stdout as the value. Any secret manager that prints to stdout works (`pass`, `op read`, `security find-generic-password`, etc.).

## Step 6: Verify

```sh
corky doctor gmail
```

This checks that the Gmail API is reachable with your credentials. Fix any errors before proceeding.

## Step 7: First sync

```sh
corky sync
```

On first run, corky opens a browser for OAuth consent. Authorize the app, and sync begins.

## Notes

- **Testing mode is fine.** Self-hosted apps in testing mode support up to 100 users. For personal use, you never need to submit for Google verification.
- **Token storage.** OAuth tokens are cached locally in your data directory. They refresh automatically.
- **Switching back.** To return to the public app, remove the `[gmail]` section from `.corky.toml`. corky falls back to the built-in client ID.
