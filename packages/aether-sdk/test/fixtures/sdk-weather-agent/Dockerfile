FROM node:24-bookworm

RUN corepack enable

WORKDIR /repo
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages/aether-sdk/package.json packages/aether-sdk/package.json
COPY packages/aether-sdk/tsconfig.json packages/aether-sdk/tsconfig.json
COPY packages/aether-sdk/tsconfig.build.json packages/aether-sdk/tsconfig.build.json
COPY packages/aether-sdk/scripts packages/aether-sdk/scripts
COPY packages/aether-sdk/src packages/aether-sdk/src
RUN pnpm install --frozen-lockfile
RUN pnpm --filter @aether-agent/sdk build

COPY target/debug/aether /usr/local/bin/aether

WORKDIR /app
COPY packages/aether-sdk/test/fixtures/sdk-weather-agent/package.json ./package.json
COPY packages/aether-sdk/test/fixtures/sdk-weather-agent/tsconfig.json ./tsconfig.json
COPY packages/aether-sdk/test/fixtures/sdk-weather-agent/eval-agent.ts ./eval-agent.ts
RUN pnpm install --frozen-lockfile=false
RUN pnpm exec tsc -p tsconfig.json

CMD ["node", "/app/dist/eval-agent.js"]
