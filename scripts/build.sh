#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_NODE=1
BUILD_PHP=1

for option in "$@"; do
  case "$option" in
    --no-node) BUILD_NODE=0 ;;
    --no-php) BUILD_PHP=0 ;;
    *) echo "Unknown build option: $option" >&2; exit 2 ;;
  esac
done

if (( BUILD_NODE )); then
  cargo build --manifest-path "$ROOT/Cargo.toml" --release -p sweet-node
  node "$ROOT/scripts/copy-native.js"
fi

if (( BUILD_PHP )); then
  cargo build --manifest-path "$ROOT/Cargo.toml" --release -p sweet-php
fi
