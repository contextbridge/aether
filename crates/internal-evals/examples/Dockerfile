# The repository sandbox image, also serving as the example for `aether eval`.
#
# The eval command must be on the container's PATH and emit newline-delimited
# AgentEvent JSON on stdout. Install whatever toolchain your repo's evals need
# (python, node, a JDK, build tools, ...).
#
# Build it with `just build-sandbox`, or point an eval file at this file with
# `"docker": { "file": "Dockerfile", "context": "../../.." }` (paths relative to the eval file).

FROM rust:bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake libdbus-1-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /repo
COPY . .
RUN cargo build -p aether-agent-cli --bin aether

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git dbus \
    && rm -rf /var/lib/apt/lists/*

# Install your project's toolchain here, e.g.:
# RUN apt-get update && apt-get install -y python3 python3-pip

COPY --from=builder /repo/target/debug/aether /usr/local/bin/aether
COPY .aether /repo/.aether
COPY AGENTS.md /repo/AGENTS.md
COPY mcp.json /repo/mcp.json
COPY crates/internal-evals/examples/aether-eval-agent /usr/local/bin/aether-eval-agent
RUN chmod +x /usr/local/bin/aether-eval-agent

WORKDIR /workspace

# `aether --sandbox-image` runs this image with its arguments appended to the entrypoint.
# Eval runs override the entrypoint, so this only serves the sandbox-CLI workflow.
ENTRYPOINT ["aether"]
