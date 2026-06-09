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
  - "make install"
  - "make release"
---

# corky — Release Notes

## GCP OAuth credentials baked at build time

The Makefile injects `CORKY_DEFAULT_GCP_CLIENT_ID` and `CORKY_DEFAULT_GCP_CLIENT_SECRET`
via `pass` at build time (using `env!()` in Rust). Release builds (`make release`)
automatically pick these up. Manual `cargo build --release --features transcribe-cuda`
will too, as long as the env vars are exported (the Makefile handles this).

## Local GPU build requirement

Local corky binaries must be built and installed with GPU-accelerated transcription.
Use `make build`, `make release`, `make install`, or `make wheel`; they require
`CORKY_LOCAL_GPU_FEATURE` (`transcribe-cuda` on this Linux/CUDA workstation,
`transcribe-metal` on macOS). If a direct cargo/maturin command is unavoidable,
include `--features transcribe-cuda` locally and stop on GPU build failure.

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
