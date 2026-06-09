# The repository sandbox image, also serving as the example for `aether eval`.
#
# The only hard requirement is that the `aether` binary is on the container's PATH —
# `aether eval` runs `aether headless` inside this image for every case. Install whatever
# toolchain your repo's evals need (python, node, a JDK, build tools, ...).
#
# Build it with `just build-sandbox`, or point an eval file at this file with
# `"docker": { "file": "Dockerfile", "context": "../../.." }` (paths relative to the eval file).


FROM archlinux:latest

RUN pacman -Syu --noconfirm ca-certificates git dbus \
    && pacman -Scc --noconfirm

# Install your project's toolchain here, e.g.:
# RUN apt-get update && apt-get install -y python3 python3-pip

# Provide the aether binary from the repository build output. Run
# `cargo build -p aether-agent-cli --bin aether` before this image builds.
COPY target/debug/aether /usr/local/bin/aether

WORKDIR /workspace

# `aether --sandbox-image` runs this image with its arguments appended to the entrypoint.
# Eval runs override the entrypoint, so this only serves the sandbox-CLI workflow.
ENTRYPOINT ["aether"]
