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
  if ! command -v pkg-config >/dev/null 2>&1; then
    echo "Cannot build sweet-node: pkg-config is required to locate GStreamer development files." >&2
    echo "Install pkg-config and the GStreamer development packages, then rerun npm run build." >&2
    exit 1
  fi

  if ! pkg-config --exists 'gstreamer-1.0 >= 1.14' gstreamer-app-1.0 gstreamer-video-1.0; then
    echo "Cannot build sweet-node: GStreamer development files were not found by pkg-config." >&2
    echo "On Debian/Ubuntu, install them with:" >&2
    echo "  sudo apt install pkg-config libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev" >&2
    echo "If GStreamer is installed in a custom location, add its pkgconfig directory to PKG_CONFIG_PATH." >&2
    exit 1
  fi

  cargo build --manifest-path "$ROOT/Cargo.toml" --release -p sweet-node
  node "$ROOT/scripts/copy-native.js"
fi

if (( BUILD_PHP )); then
  cargo build --manifest-path "$ROOT/Cargo.toml" --release -p sweet-php
fi
