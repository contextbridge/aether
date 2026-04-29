# List available recipes
default:
    @just --list

run:
    cargo run -p wisp -- -a 'cargo run -p aether-agent-cli acp'

# Build the workspace
build:
    cargo build

# Check the workspace
check:
    cargo check

# Run tests with nextest
test *PKGS:
    cargo nextest run --all-features {{ if PKGS == "" { "--workspace" } else { PKGS } }}

# Check formatting
fmt-check *PKGS:
    cargo fmt --check {{ if PKGS == "" { "--all" } else { PKGS } }}

# Format Rust code
fmt:
    cargo fmt --all

# Run clippy
lint *PKGS:
    cargo clippy --all-targets --all-features {{ if PKGS == "" { "--workspace" } else { PKGS } }} -- -D warnings

# Check documentation builds without warnings
doc-check *PKGS:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --all-features {{ if PKGS == "" { "--workspace --examples" } else { PKGS } }}

# Install Node dependencies for the TypeScript SDK
sdk-install:
    pnpm install --frozen-lockfile

# End-to-end probe: build aether + SDK, then run one prompt through the real binary.
# Forwards extra args to the script (e.g. `just sdk-e2e -- --model anthropic:claude-sonnet-4-5`).
sdk-e2e *ARGS:
    cargo build -p aether-agent-cli
    pnpm sdk:build
    pnpm sdk:e2e {{ARGS}}

# Run all CI checks
ci: fmt-check lint test doc-check
    pnpm fmt-check
    pnpm sdk:generate-config-types:check
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

# Update packages/llm/models.json from models.dev
update-models:
    ./packages/llm/scripts/fetch-models.sh

# Sweep build artifacts older than N days (default: 7)
sweep DAYS="7":
    cargo sweep --time {{DAYS}}

# Sweep artifacts not used by the current toolchain
sweep-installed:
    cargo sweep --time 1

# Build the workspace sandbox image
build-sandbox TAG="aether-sandbox:latest":
    docker build -t {{TAG}} -f Dockerfile.sandbox .

# Run wisp + aether agent inside the sandbox
run-sandbox:
    cargo run -p wisp -- -a 'cargo run -p aether-agent-cli -- --sandbox-image aether-sandbox:latest acp'

# Install aether-cli and wisp binaries locally
install:
    cargo install --path packages/aether-cli --force
    cargo install --path packages/wisp --force
    cargo sweep --installed

# Preview the release PR release-plz would open against main
release-pr-preview:
    release-plz release-pr --dry-run

# Clean everything
clean:
    cargo clean
