#!/usr/bin/env bash
set -euo pipefail

cargo test -p rss-hub-reader-api
cargo clippy -p rss-hub-reader-api -- -D warnings
npm run typecheck:web
npm run build:web
