#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
curl -o "$SCRIPT_DIR/../models.json" https://models.dev/api.json
echo "Downloaded models.json"
