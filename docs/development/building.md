# Building

## Developer setup

```sh
git clone https://github.com/btakita/corky.git
cd corky
cp .corky.toml.example mail/.corky.toml   # configure your email accounts
make release                               # GPU build + symlink to .bin/corky
```

Local source builds and installs require GPU-accelerated transcription support. The
Makefile selects `transcribe-cuda` on NVIDIA Linux or `transcribe-metal` on macOS
through `CORKY_LOCAL_GPU_FEATURE` and fails if no GPU backend is available.

## Make targets

```sh
make build        # Debug build with local GPU feature
make release      # Release build with local GPU feature + symlink to .bin/corky
make test         # Run tests with local GPU feature
make clippy       # Lint with local GPU feature
make check        # Lint + test with local GPU feature
make install      # Install to ~/.cargo/bin with local GPU feature
make precommit    # Full pre-commit checks
```

## .gitignore

The following are gitignored:

```
.env
.corky.toml
credentials.json
*.credentials.json
CLAUDE.local.md
AGENTS.local.md
mail
.idea/
tmp/
target/
.bin/
```

Config files (`.corky.toml`, `voice.md`) live inside `mail/` which is already gitignored. `credentials.json` is also gitignored in `mail/.gitignore`.
