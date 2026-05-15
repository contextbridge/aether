# syntax=docker/dockerfile:1
# Docker image for cloud agents
# Only contains the toolchain needed for agents to run this repo's just targets.

ARG RUST_VERSION=1.95.0
ARG DEBIAN_VERSION=12.12

FROM rust:${RUST_VERSION}-slim-bookworm AS rust-toolchain

ARG RUST_VERSION
ARG RUST_TOOLCHAIN=stable

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN rustup component add --toolchain "${RUST_VERSION}" rustfmt clippy rust-analyzer \
  && rust_host="$(rustc -vV | awk '/host:/ { print $2 }')" \
  && ln -s "${RUSTUP_HOME}/toolchains/${RUST_VERSION}-${rust_host}" "${RUSTUP_HOME}/toolchains/${RUST_TOOLCHAIN}-${rust_host}" \
  && rustup default "${RUST_TOOLCHAIN}" \
  && chmod -R go-w "${RUSTUP_HOME}" "${CARGO_HOME}"

FROM debian:${DEBIAN_VERSION}-slim

ARG AGENT_UID=1000
ARG AGENT_GID=1000
ARG MISE_VERSION=v2026.5.9
ARG TYPESCRIPT_VERSION=6.0.3
ARG TYPESCRIPT_LANGUAGE_SERVER_VERSION=5.2.0

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ENV RUSTUP_HOME=/usr/local/rustup \
    MISE_INSTALL_PATH=/usr/local/bin/mise \
    MISE_DATA_DIR=/mise \
    MISE_CONFIG_DIR=/mise \
    MISE_CACHE_DIR=/mise/cache \
    PATH=/mise/shims:/home/node/.cargo/bin:/usr/local/cargo/bin:/usr/local/bin:$PATH \
    COREPACK_ENABLE_DOWNLOAD_PROMPT=0 \
    NPM_CONFIG_UPDATE_NOTIFIER=false

RUN groupadd -g "${AGENT_GID}" node \
  && useradd -m -u "${AGENT_UID}" -g "${AGENT_GID}" -s /bin/bash node

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    gnupg \
    jq \
    file \
    procps \
    xz-utils \
    unzip \
    build-essential \
    pkg-config \
    libdbus-1-dev \
    libdbus-1-3 \
  && install -m 0755 -d /etc/apt/keyrings \
  && curl -fsSL https://download.docker.com/linux/debian/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg \
  && chmod a+r /etc/apt/keyrings/docker.gpg \
  && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/debian bookworm stable" > /etc/apt/sources.list.d/docker.list \
  && apt-get update \
  && apt-get install -y --no-install-recommends docker-ce-cli docker-compose-plugin \
  && rm -rf /var/lib/apt/lists/*

COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup

RUN curl -fsSL https://mise.run | MISE_VERSION="${MISE_VERSION}" MISE_INSTALL_PATH="${MISE_INSTALL_PATH}" sh

COPY .tool-versions /tmp/aether-tooling/.tool-versions

WORKDIR /tmp/aether-tooling
RUN --mount=type=secret,id=github_token,required=false \
  if [ -f /run/secrets/github_token ]; then export GITHUB_TOKEN="$(cat /run/secrets/github_token)"; fi; \
  mise install --yes \
  && awk 'NF >= 2 && $1 !~ /^#/ { print $1 "@" $2 }' .tool-versions | (cd / && xargs -r mise use -g) \
  && npm install -g "typescript@${TYPESCRIPT_VERSION}" "typescript-language-server@${TYPESCRIPT_LANGUAGE_SERVER_VERSION}" \
  && chmod -R a+rX /mise

RUN printf '%s\n' 'export PATH="/home/node/.cargo/bin:/usr/local/cargo/bin:$PATH"' 'eval "$(mise activate bash --shims)"' > /etc/profile.d/aether-tooling.sh

RUN mkdir -p /workspace /home/node/.cargo \
  && chown -R node:node /workspace /home/node/.cargo

USER node
ENV HOME=/home/node \
    CARGO_HOME=/home/node/.cargo
WORKDIR /workspace
