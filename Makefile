.PHONY: build release test clippy check precommit install install-hooks clean init-python wheel publish publish-crate publish-pypi

# GCP OAuth credentials (public desktop-app credentials, injected at build time)
export CORKY_DEFAULT_GCP_CLIENT_ID ?= $(shell pass corky/gcp/client_id 2>/dev/null)
export CORKY_DEFAULT_GCP_CLIENT_SECRET ?= $(shell pass corky/gcp/client_secret 2>/dev/null)

# Build debug binary
build:
	cargo build

# Build release binary and symlink to .bin/
release:
	cargo build --release
	@mkdir -p .bin
	@ln -sf ../target/release/corky .bin/corky
	@echo "Installed .bin/corky -> target/release/corky"

# Build release binary with GPU support (CUDA on Linux, Metal on macOS)
release-gpu:
	@if [ "$$(uname)" = "Darwin" ]; then \
		echo "Building with Metal (macOS)..."; \
		cargo build --release --features transcribe-metal; \
	elif command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi >/dev/null 2>&1; then \
		echo "Building with CUDA (Linux)..."; \
		cargo build --release --features transcribe-cuda; \
	else \
		echo "No GPU detected — building CPU-only."; \
		cargo build --release; \
	fi
	@mkdir -p .bin
	@ln -sf ../target/release/corky .bin/corky
	@echo "Installed .bin/corky -> target/release/corky"

# Run tests
test:
	cargo test

# Lint
clippy:
	cargo clippy -- -D warnings

# clippy + test
check: clippy test

# Pre-commit: clippy + test + audit-docs
precommit: check
	cargo run --quiet -- audit-docs

# Install to ~/.cargo/bin (auto-detects GPU for transcribe-cuda)
install:
	@FEATURES=""; \
	if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi >/dev/null 2>&1; then \
		echo "GPU detected — attempting install with transcribe-cuda..."; \
		if cargo install --path . --features transcribe-cuda 2>/dev/null; then \
			echo "Installed with GPU support (transcribe-cuda)."; \
			exit 0; \
		else \
			echo "GPU build failed — falling back to CPU-only install."; \
		fi; \
	else \
		echo "No GPU detected — installing CPU-only."; \
	fi; \
	cargo install --path .

# Install git hooks
install-hooks:
	@mkdir -p .git/hooks
	@printf '#!/bin/sh\nmake precommit\n' > .git/hooks/pre-commit
	@chmod +x .git/hooks/pre-commit
	@echo "Installed .git/hooks/pre-commit"

# Remove build artifacts
clean:
	cargo clean
	rm -f .bin/corky

# Set up Python venv with maturin
init-python: PY_VERSION = $(shell [ -f .python-version ] && \
	cat .python-version || echo "3.14")
init-python:
	@echo "Setting up Python $(PY_VERSION) venv..."
	@if command -v mise >/dev/null 2>&1; then \
		mise install; \
	fi
	uv venv .venv --python "$(PY_VERSION)" --no-project --clear --seed $(VENV_ARGS)
	uv pip install maturin
	@echo "Venv ready. Use 'make wheel' to build, or '.venv/bin/maturin develop --release' to install into venv."

# Build wheel and install into venv for testing
wheel:
	.venv/bin/maturin develop --release

# Publish to crates.io
publish-crate:
	cargo publish

# Publish to PyPI
publish-pypi:
	.venv/bin/maturin publish --skip-existing

# Publish to both crates.io and PyPI
publish: publish-crate publish-pypi
