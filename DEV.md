---
project_type: rust-maturin
publication_targets:
  - crates.io
  - pypi
  - github-release
secret_paths:
  crates_io: "btak/CARGO_REGISTRY_TOKEN"
  pypi: "pypi/token"
  gcp_client_id: "corky/gcp/client_id"
  gcp_client_secret: "corky/gcp/client_secret"
post_release:
  - "cargo install --path ."
  - "cargo build --release"
---

# corky — Release Notes

## GCP OAuth credentials baked at build time

The Makefile injects `CORKY_DEFAULT_GCP_CLIENT_ID` and `CORKY_DEFAULT_GCP_CLIENT_SECRET`
via `pass` at build time (using `env!()` in Rust). Release builds (`make release`)
automatically pick these up. Manual `cargo build --release` will too, as long as the
env vars are exported (the Makefile handles this).

For `cargo publish`, the crate compiles on crates.io without GCP creds — `env!()` uses
empty-string defaults so the published crate builds cleanly.

## Two-repo model

`mail/` is the private data repo (synced threads, drafts, contacts). corky is the
public tool repo. Never commit mail data into corky. The `mail` entry in `.gitignore`
keeps them separated.

## YouTube uploads default to public

When uploading to YouTube via corky, videos default to **public** visibility.
Always set `--visibility private` unless the user explicitly requests public.

## Version sync

`pyproject.toml` version must match `Cargo.toml`.
