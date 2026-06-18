# The repository sandbox image, also serving as the example for `aether eval`.
#
# The eval command must be on the container's PATH and emit newline-delimited
# AgentMessage JSON on stdout. Install whatever toolchain your repo's evals need
# (python, node, a JDK, build tools, ...).
#
# Build it with `just build-sandbox`, or point an eval file at this file with
# `"docker": { "file": "Dockerfile", "context": "../../.." }` (paths relative to the eval file).

FROM archlinux:latest

RUN pacman -Syu --noconfirm ca-certificates git dbus \
    && pacman -Scc --noconfirm

# Install your project's toolchain here, e.g.:
# RUN apt-get update && apt-get install -y python3 python3-pip

# Provide the aether binary and an example direct eval wrapper from the repository
# build output. Run `cargo build -p aether-agent-cli --bin aether` before this image builds.
COPY target/debug/aether /usr/local/bin/aether
COPY .aether /repo/.aether
COPY packages/internal-evals/examples/aether-eval-agent /usr/local/bin/aether-eval-agent
RUN chmod +x /usr/local/bin/aether-eval-agent

WORKDIR /workspace

# `aether --sandbox-image` runs this image with its arguments appended to the entrypoint.
# Eval runs override the entrypoint, so this only serves the sandbox-CLI workflow.
ENTRYPOINT ["aether"]
