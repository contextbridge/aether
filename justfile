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

# Run tests with nextest's CI profile and JUnit output
test-ci *PKGS:
    cargo nextest run --profile ci --all-features {{ if PKGS == "" { "--workspace" } else { PKGS } }}

# Run real LLM evals from the dedicated eval crate
# e.g. `just evals anthropic:claude-sonnet-4-5`
evals MODEL *ARGS:
    AETHER_EVAL_MODEL={{MODEL}} cargo nextest run -p aether-evals --ignore-default-filter -E 'group(evals)' {{ARGS}}

# List eval test names without running them
evals-list *ARGS:
    cargo nextest list -p aether-evals --ignore-default-filter -E 'group(evals)' {{ARGS}}

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
ci: fmt-check lint test-ci doc-check
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

# Run Aether headless through the local Bedrock SigV4 proxy
bedrock-proxy-run:
    #!/usr/bin/env bash
    set -euo pipefail
    example_dir="examples/bedrock-proxy"
    env_file="$example_dir/.env"
    if [ ! -f "$env_file" ]; then
        echo "Missing $env_file. Copy $example_dir/.env.example to $env_file and fill in AETHER_BEDROCK_MODEL." >&2
        exit 1
    fi

    prompt_override_set=false
    if [ "${AETHER_BEDROCK_PROMPT+x}" = x ]; then
        prompt_override_set=true
        prompt_override="$AETHER_BEDROCK_PROMPT"
    fi

    set -a
    . "$env_file"
    set +a

    if [ "$prompt_override_set" = true ]; then
        export AETHER_BEDROCK_PROMPT="$prompt_override"
    fi

    if [ -z "${AETHER_BEDROCK_MODEL:-}" ] || [[ "$AETHER_BEDROCK_MODEL" == *REPLACE_ME* ]]; then
        echo "Set AETHER_BEDROCK_MODEL in $env_file to a Bedrock model ID or inference profile ARN." >&2
        exit 1
    fi

    if [ -z "${AWS_ACCESS_KEY_ID:-}" ] && [ -z "${AWS_SESSION_TOKEN:-}" ]; then
        export AWS_PROFILE="${AWS_PROFILE:-Development-PowerUser}"
        if ! aws sts get-caller-identity --profile "$AWS_PROFILE" >/dev/null 2>&1; then
            echo "SSO session expired for $AWS_PROFILE — logging in..." >&2
            aws sso login --profile "$AWS_PROFILE"
        fi
        eval "$(aws configure export-credentials --profile "$AWS_PROFILE" --format env)"
    fi

    export AWS_REGION="${AWS_REGION:-us-west-2}"
    export BEDROCK_PROXY_PORT="${BEDROCK_PROXY_PORT:-8080}"
    compose=(docker compose -f "$example_dir/docker-compose.yml" --env-file "$env_file")
    cleanup() {
        "${compose[@]}" down >/dev/null 2>&1 || true
    }
    trap cleanup EXIT

    "${compose[@]}" up -d bedrock-proxy

    model="$AETHER_BEDROCK_MODEL"
    if [[ "$model" != bedrock:* ]]; then
        model="bedrock:$model"
    fi

    cargo run -p aether-agent-cli -- headless \
        --model "$model" \
        --provider "bedrock.url=http://127.0.0.1:$BEDROCK_PROXY_PORT" \
        --provider bedrock.auth=none \
        "${AETHER_BEDROCK_PROMPT:-Say hello in one sentence.}"

# Stop the local Bedrock SigV4 proxy example
bedrock-proxy-down:
    docker compose -f examples/bedrock-proxy/docker-compose.yml --env-file examples/bedrock-proxy/.env down

# Clean everything
clean:
    cargo clean
