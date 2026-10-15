#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
"$PWD/scripts/build.sh" --no-node
printf 'Built %s\n' "$PWD/target/release/libsweet_php.so"
printf 'Run with: php -d ffi.enable=1 examples/php-hello.php\n'
