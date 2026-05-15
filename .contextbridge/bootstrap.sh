#!/bin/sh
set -eu

pnpm install --frozen-lockfile
cargo fetch
