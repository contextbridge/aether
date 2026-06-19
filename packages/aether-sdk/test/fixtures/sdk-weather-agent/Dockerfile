# syntax=docker/dockerfile:1

# Build aether for the image platform. The host's target/debug/aether is built for the host OS

FROM rust:1-bookworm AS aether

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /repo

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY packages packages

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/repo/target \
    cargo build -p aether-agent-cli \
    && cp target/debug/aether /usr/local/bin/aether

FROM node:24-bookworm

# Shared libs the aether binary links at runtime (D-Bus + OpenSSL).
RUN apt-get update && apt-get install -y --no-install-recommends \
    libdbus-1-3 libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN corepack enable

WORKDIR /repo
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages/aether-sdk/package.json packages/aether-sdk/package.json
COPY packages/aether-sdk/tsconfig.json packages/aether-sdk/tsconfig.json
COPY packages/aether-sdk/tsconfig.build.json packages/aether-sdk/tsconfig.build.json
COPY packages/aether-sdk/scripts packages/aether-sdk/scripts
COPY packages/aether-sdk/src packages/aether-sdk/src
RUN pnpm install --frozen-lockfile
RUN pnpm --filter @aether-agent/sdk compile

COPY --from=aether /usr/local/bin/aether /usr/local/bin/aether

WORKDIR /app
COPY packages/aether-sdk/test/fixtures/sdk-weather-agent/package.json ./package.json
COPY packages/aether-sdk/test/fixtures/sdk-weather-agent/tsconfig.json ./tsconfig.json
COPY packages/aether-sdk/test/fixtures/sdk-weather-agent/eval-agent.ts ./eval-agent.ts

RUN pnpm install --frozen-lockfile=false --ignore-scripts
RUN pnpm exec tsc -p tsconfig.json

CMD ["node", "/app/dist/eval-agent.js"]
