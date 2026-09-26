#!/usr/bin/env bash
set -euo pipefail

cargo test -p infofetcher-api
cargo clippy -p infofetcher-api -- -D warnings
npm run typecheck:web
npm run build:web
