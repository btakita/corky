.PHONY: build release release-gpu require-local-gpu test clippy check precommit install install-hooks clean init-python wheel publish publish-crate publish-pypi

# GCP OAuth credentials (public desktop-app credentials, injected at build time)
export CORKY_DEFAULT_GCP_CLIENT_ID ?= $(shell pass corky/gcp/client_id 2>/dev/null)
export CORKY_DEFAULT_GCP_CLIENT_SECRET ?= $(shell pass corky/gcp/client_secret 2>/dev/null)

# Local binary builds must include GPU-accelerated transcription support.
UNAME_S := $(shell uname -s)
NVIDIA_SMI_OK := $(shell if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi >/dev/null 2>&1; then printf yes; fi)
ifeq ($(UNAME_S),Darwin)
CORKY_LOCAL_GPU_FEATURE ?= transcribe-metal
else ifeq ($(NVIDIA_SMI_OK),yes)
CORKY_LOCAL_GPU_FEATURE ?= transcribe-cuda
else
CORKY_LOCAL_GPU_FEATURE ?=
endif

require-local-gpu:
	@if [ -z "$(CORKY_LOCAL_GPU_FEATURE)" ]; then \
		echo "GPU support is required for local corky builds and installs."; \
		echo "Install/configure CUDA or Metal, or set CORKY_LOCAL_GPU_FEATURE=transcribe-cuda|transcribe-metal explicitly."; \
		exit 2; \
	fi

# Build debug binary with the required local GPU feature
build: require-local-gpu
	cargo build --features $(CORKY_LOCAL_GPU_FEATURE)

# Build release binary with the required local GPU feature and symlink to .bin/
release: require-local-gpu
	cargo build --release --features $(CORKY_LOCAL_GPU_FEATURE)
	@mkdir -p .bin
	@ln -sf ../target/release/corky .bin/corky
	@echo "Installed .bin/corky -> target/release/corky"

# Backward-compatible alias for explicit GPU release builds.
release-gpu: release

# Run tests with the required local GPU feature
test: require-local-gpu
	cargo test --workspace --features $(CORKY_LOCAL_GPU_FEATURE)

# Lint with the required local GPU feature
clippy: require-local-gpu
	cargo clippy --workspace --all-targets --features $(CORKY_LOCAL_GPU_FEATURE) -- -D warnings

# clippy + test
check: clippy test

# Pre-commit: clippy + test + audit-docs
precommit: check
	cargo run --quiet --features $(CORKY_LOCAL_GPU_FEATURE) -- audit-docs

# Install to ~/.cargo/bin with the required local GPU feature
install: require-local-gpu
	cargo install --path . --features $(CORKY_LOCAL_GPU_FEATURE)

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
	@echo "Venv ready. Use 'make wheel' to build/install into venv with the local GPU feature."

# Build wheel and install into venv for testing
wheel: require-local-gpu
	.venv/bin/maturin develop --release --features $(CORKY_LOCAL_GPU_FEATURE)

# Publish to crates.io
publish-crate:
	cargo publish -p corky-core
	cargo publish -p corky-transcribe
	cargo publish -p corky-google
	cargo publish -p corky-social
	cargo publish -p corky-mail
	cargo publish -p corky

# Publish to PyPI
publish-pypi:
	.venv/bin/maturin publish --skip-existing

# Publish to both crates.io and PyPI
publish: publish-crate publish-pypi
