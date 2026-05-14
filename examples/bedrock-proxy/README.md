# Bedrock SigV4 proxy example

This example starts the AWS SigV4 proxy locally and runs Aether headless against Bedrock through that proxy. It exercises the CLI provider overrides:

```bash
--provider bedrock.url=http://127.0.0.1:8080 --provider bedrock.auth=none
```

The proxy signs requests with AWS credentials; Aether deliberately sends unsigned Bedrock requests to the proxy.

## Setup

```bash
cp examples/bedrock-proxy/.env.example examples/bedrock-proxy/.env
$EDITOR examples/bedrock-proxy/.env
```

Set `AETHER_BEDROCK_MODEL` to a Bedrock model ID or application inference profile ARN that your AWS account can invoke.

## Run

```bash
just bedrock-proxy-run
```

The recipe:

1. Refreshes/exports AWS credentials for Docker Compose.
2. Starts `public.ecr.aws/aws-observability/aws-sigv4-proxy` on `127.0.0.1:8080`.
3. Runs `cargo run -p aether-agent-cli -- headless ...` with `bedrock.url` pointed at the proxy and `bedrock.auth=none`.
4. Stops the proxy when Aether exits.

Override the prompt without editing `.env`:

```bash
AETHER_BEDROCK_PROMPT="Reply with exactly five words." just bedrock-proxy-run
```

If your SSO profile is not `Development-PowerUser`, pass a different one:

```bash
AWS_PROFILE=MyProfile just bedrock-proxy-run
```

## Manual commands

```bash
set -a
. examples/bedrock-proxy/.env
set +a

docker compose -f examples/bedrock-proxy/docker-compose.yml --env-file examples/bedrock-proxy/.env up -d bedrock-proxy
cargo run -p aether-agent-cli -- headless \
  --model "bedrock:${AETHER_BEDROCK_MODEL}" \
  --provider "bedrock.url=http://127.0.0.1:${BEDROCK_PROXY_PORT:-8080}" \
  --provider bedrock.auth=none \
  "${AETHER_BEDROCK_PROMPT}"
docker compose -f examples/bedrock-proxy/docker-compose.yml --env-file examples/bedrock-proxy/.env down
```
