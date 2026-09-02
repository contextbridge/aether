# List available recipes
default:
    @just --list

run:
    cargo run -p aether-wisp -- -a 'cargo run -p aether-agent-cli acp'

# Build the workspace
build:
    cargo build

# Check the workspace
check:
    cargo check

# Run tests with nextest
test *PKGS:
    cargo nextest run --no-tests pass --all-features {{ if PKGS == "" { "--workspace --exclude internal-evals" } else { PKGS } }}

# Run tests with nextest's CI profile and JUnit output
test-ci *PKGS:
    cargo nextest run --no-tests pass --profile ci --all-features {{ if PKGS == "" { "--workspace --exclude internal-evals" } else { PKGS } }}

# Run real LLM evals from the dedicated eval crate against a fresh sandbox image
evals *ARGS: build-sandbox
    cargo nextest run -p internal-evals --ignore-default-filter -E 'group(evals)' {{ARGS}}

# List eval test names without running them
evals-list *ARGS:
    cargo nextest list -p internal-evals --ignore-default-filter -E 'group(evals)' {{ARGS}}

# Check formatting
fmt-check *PKGS:
    cargo fmt --check {{ if PKGS == "" { "--all" } else { PKGS } }}

# Format Rust code
fmt:
    cargo fmt --all

# Run clippy
lint *PKGS:
    cargo clippy --all-targets --all-features {{ if PKGS == "" { "--workspace" } else { PKGS } }} -- -D warnings

# Verify checked SQL queries match the session-index schema
sqlx-check:
    #!/usr/bin/env bash
    set -euo pipefail
    db="$(mktemp /tmp/aether-sessions.XXXXXX)"
    trap 'rm -f "$db" "$db-wal" "$db-shm"' EXIT
    cd crates/aether-sessions
    DATABASE_URL="sqlite://$db" cargo sqlx migrate run
    DATABASE_URL="sqlite://$db" cargo sqlx prepare --check -- --features analytics

# Check documentation builds without warnings
doc-check *PKGS:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --all-features {{ if PKGS == "" { "--workspace --examples" } else { PKGS } }}

# Regenerate the AetherSettings JSON Schema consumed by the website and SDK
gen-schema:
    cargo run -q -p aether-project --bin aether-settings-schema > packages/website/src/data/aether-settings.schema.json

# Install Node dependencies for the TypeScript SDK
sdk-install:
    pnpm install --frozen-lockfile

# Build the cli debug binary, then run SDK tests (incl. the real-binary stdio e2e test).
sdk-test:
    cargo build -p aether-agent-cli
    pnpm sdk:test

# End-to-end probe: build aether + SDK, then run one prompt through the real binary.
# Forwards extra args to the script (e.g. `just sdk-e2e -- --model anthropic:claude-sonnet-4-5`).
sdk-e2e *ARGS:
    cargo build -p aether-agent-cli
    pnpm sdk:build
    pnpm sdk:e2e {{ARGS}}

# Validate the GitHub-hosted Aether agent automation
automation-check:
    jq empty .aether/settings.json .aether/github-agent/mcp.json
    bash -n scripts/aether-agent/*

# Run all CI checks
ci: fmt-check lint test-ci doc-check sqlx-check automation-check
    pnpm fmt-check
    pnpm sdk:typecheck
    pnpm sdk:test

# Initialize or update cargo-dist configuration and CI workflows
dist-init:
    dist init

# Preview what cargo-dist will build in CI
dist-plan:
    dist plan

# Build distributable artifacts for the current platform
dist-build:
    dist build

# Smoke test dist release workflow locally with act (optional)
act-dist-plan:
    act pull_request -W .github/workflows/release.yml -j plan -P ubuntu-22.04=catthehacker/ubuntu:act-22.04

# Preview unreleased changelog
changelog:
    git cliff --unreleased

# Generate full CHANGELOG.md
changelog-gen:
    git cliff -o CHANGELOG.md

# Update crates/llm/models.json from models.dev
update-models:
    ./crates/llm/scripts/fetch-models.sh

# Sweep build artifacts older than N days (default: 7)
sweep DAYS="7":
    cargo sweep --time {{DAYS}}

# Sweep artifacts not used by the current toolchain
sweep-installed:
    cargo sweep --time 1

# Build the sandbox image from the eval example Dockerfile
build-sandbox TAG="aether-sandbox:latest":
    docker build -t {{TAG}} -f crates/internal-evals/examples/Dockerfile .

# Run wisp + aether agent inside the sandbox
run-sandbox:
    cargo run -p aether-wisp -- -a 'cargo run -p aether-agent-cli -- --sandbox-image aether-sandbox:latest acp'

# Install aether-cli and wisp binaries locally
install:
    cargo install --path crates/aether-cli --force
    cargo install --path crates/wisp --force
    cargo sweep --installed

# Preview the release PR release-plz would open against main
release-pr-preview:
    release-plz release-pr --dry-run

# Clean everything
clean:
    cargo clean
