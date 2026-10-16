#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET=x86_64-pc-windows-gnu
OUT="$ROOT/target/sweet-surprise"
TMP="$ROOT/target/sweet-surprise-tmp"
CACHE="$ROOT/target/php-windows-cache"
if [[ -n "${PHP_WINDOWS_URL:-}" ]]; then
  PHP_URL="$PHP_WINDOWS_URL"
  PHP_ARCHIVE="${PHP_URL##*/}"
else
  if [[ -z "${PHP_VERSION:-}" ]]; then
    CACHED_ARCHIVE="$(find "$CACHE" -maxdepth 1 -type f -name 'php-8.*-Win32-vs17-x64.zip' -printf '%f\n' 2>/dev/null | sort -V | tail -n 1)"
    if [[ -n "$CACHED_ARCHIVE" ]]; then
      PHP_VERSION="${CACHED_ARCHIVE#php-}"
      PHP_VERSION="${PHP_VERSION%-Win32-vs17-x64.zip}"
    else
      PHP_VERSION="$(curl --fail --silent --show-error --location https://windows.php.net/downloads/releases/ | grep -oE 'php-8\.[0-9]+\.[0-9]+-Win32-vs17-x64\.zip' | sort -Vu | tail -n 1 | sed -E 's/^php-([0-9]+\.[0-9]+\.[0-9]+)-.*/\1/')"
    fi
  fi
  PHP_ARCHIVE="php-${PHP_VERSION}-Win32-vs17-x64.zip"
  PHP_URL="https://windows.php.net/downloads/releases/${PHP_ARCHIVE}"
fi

if ! command -v x86_64-w64-mingw32-gcc >/dev/null; then
  echo 'Missing x86_64-w64-mingw32-gcc (install the MinGW-w64 x86_64 toolchain).' >&2
  exit 1
fi
if ! command -v 7z >/dev/null; then
  echo 'Missing 7z (install p7zip-full).' >&2
  exit 1
fi

mkdir -p "$ROOT/target" "$CACHE"
rm -rf "$OUT" "$TMP"
mkdir -p "$OUT" "$TMP/php"

if [[ ! -x "$HOME/.cargo/bin/rustup" ]]; then
  echo 'rustup is required to install the Windows Rust target.' >&2
  exit 1
fi
"$HOME/.cargo/bin/rustup" target add "$TARGET"

if [[ ! -f "$CACHE/$PHP_ARCHIVE" ]]; then
  curl --fail --location --retry 3 "$PHP_URL" -o "$CACHE/$PHP_ARCHIVE"
fi
7z x -y "$CACHE/$PHP_ARCHIVE" "-o$TMP/php" >/dev/null

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
cargo build --manifest-path "$ROOT/Cargo.toml" --release --target "$TARGET" -p sweet-php

cp "$ROOT/target/$TARGET/release/sweet_php.dll" "$OUT/sweet.dll"
cp "$TMP/php/php.exe" "$OUT/php-runtime.exe"
cp "$TMP/php/php8ts.dll" "$OUT/"
mkdir -p "$OUT/ext"
cp "$TMP/php/ext/php_ffi.dll" "$OUT/ext/"
cp "$ROOT/php/Sweet.php" "$OUT/Sweet.php"
sed "s#require_once __DIR__ . '/php/Sweet.php';#require_once __DIR__ . '/Sweet.php';#" "$ROOT/sweet-surprise.php" > "$OUT/main.php"
cp "$ROOT/bg.mp4" "$OUT/bg.mp4"
mkdir -p "$OUT/fonts"
cp /usr/share/fonts/truetype/lato/Lato-{Regular,Bold}.ttf "$OUT/fonts/"
cat > "$OUT/php.ini" <<'INI'
extension_dir="${SWEET_APP_DIR}ext"
extension=php_ffi.dll
ffi.enable=true
INI

x86_64-w64-mingw32-gcc -O2 -municode "$ROOT/scripts/php-windows-launcher.c" -o "$OUT/sweet_surprise.exe"

cd "$ROOT/target"
rm -f sweet_surprise.zip
mv sweet-surprise sweet_surprise
7z a -tzip -mx=1 sweet_surprise.zip sweet_surprise >/dev/null
mv sweet_surprise sweet-surprise
rm -rf "$TMP" "$ROOT/target/php-hellobimbo-windows-tmp" "$ROOT/target/php-hellobimbo-windows-x64"
rm -f "$ROOT/target/php-hellobimbo-windows-x64.zip"
printf 'Created %s\n' "$ROOT/target/sweet_surprise.zip"
