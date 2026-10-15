#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}"
ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
NODE_MOBILE_ROOT="${NODE_MOBILE_ROOT:?Set NODE_MOBILE_ROOT to the extracted nodejs-mobile Android release}"
YTDLP_ANDROID_ROOT="${YTDLP_ANDROID_ROOT:-$HOME/prog/yt-dlp-android}"
PYTHON_BUILD="${PYTHON_BUILD:-$(command -v python3)}"
ANDROID_ABI="${ANDROID_ABI:-arm64-v8a}"
ANDROID_API="${ANDROID_API:-26}"
ANDROID_BUILD_TOOLS="${ANDROID_BUILD_TOOLS:-35.0.0}"
ANDROID_BUILD_TYPE="${ANDROID_BUILD_TYPE:-debug}"
OUT="${ANDROID_APK_OUT:-$ROOT/target/android-apk}"
APP_PROJECT="$ROOT/packaging/android/gradle-app"
NDK_TOOLCHAIN="${ANDROID_NDK_HOME:?Set ANDROID_NDK_HOME to an installed Android NDK}/toolchains/llvm/prebuilt/linux-x86_64/bin"

case "$ANDROID_ABI" in
  arm64-v8a) TARGET="aarch64-linux-android"; NDK_TRIPLE="aarch64-linux-android" ;;
  x86_64) TARGET="x86_64-linux-android"; NDK_TRIPLE="x86_64-linux-android" ;;
  *) echo "Supported Android ABIs: arm64-v8a, x86_64" >&2; exit 2 ;;
esac

for file in "$ANDROID_SDK_ROOT/build-tools/$ANDROID_BUILD_TOOLS/aapt2" \
            "$ANDROID_SDK_ROOT/platforms/android-35/android.jar" \
            "$NODE_MOBILE_ROOT/bin/$ANDROID_ABI/libnode.so" \
            "$YTDLP_ANDROID_ROOT/library/src/main/python/ytdlp_runner.py"; do
  [[ -e "$file" ]] || { echo "Missing Android build input: $file" >&2; exit 2; }
done

export ANDROID_SDK_ROOT ANDROID_NDK_HOME ANDROID_ABI NODE_MOBILE_ROOT
export LOCAL_PYTHON="$PYTHON_BUILD"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0
TARGET_ENV="${TARGET^^}"
TARGET_ENV="${TARGET_ENV//-/_}"
export "CARGO_TARGET_${TARGET_ENV}_LINKER=$NDK_TOOLCHAIN/${NDK_TRIPLE}${ANDROID_API}-clang"
export "CC_${TARGET//-/_}=$NDK_TOOLCHAIN/${NDK_TRIPLE}${ANDROID_API}-clang"
export "CXX_${TARGET//-/_}=$NDK_TOOLCHAIN/${NDK_TRIPLE}${ANDROID_API}-clang++"

cargo build --manifest-path "$ROOT/Cargo.toml" --release -p sweet-node \
  --target "$TARGET" --no-default-features --features sweet-node/android-apk

MAIN="$APP_PROJECT/app/src/main"
rm -rf "$MAIN/assets/sweet-app" "$MAIN/jniLibs"
mkdir -p "$MAIN/assets/sweet-app/djgrok" "$MAIN/jniLibs/$ANDROID_ABI"
cp "$ROOT/index.js" "$ROOT/dom.js" "$ROOT/dominant.js" "$ROOT/package.json" "$MAIN/assets/sweet-app/"
cp "$ROOT/djgrok/"*.js "$ROOT/djgrok/"*.mjs "$ROOT/djgrok/"*.ttf "$ROOT/djgrok/"*.png \
   "$ROOT/djgrok/BOO-COPYING" "$MAIN/assets/sweet-app/djgrok/"
cp -R "$ROOT/djgrok/covers" "$MAIN/assets/sweet-app/djgrok/"
cp "$ROOT/target/$TARGET/release/libsweet.so" "$MAIN/jniLibs/$ANDROID_ABI/libsweet.so"
cp "$NODE_MOBILE_ROOT/bin/$ANDROID_ABI/libnode.so" "$MAIN/jniLibs/$ANDROID_ABI/libnode.so"
cp "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$TARGET/libc++_shared.so" \
  "$MAIN/jniLibs/$ANDROID_ABI/libc++_shared.so"

KEYSTORE="${ANDROID_KEYSTORE:-$OUT/djgrok-debug.keystore}"
mkdir -p "$OUT"
if [[ ! -e "$KEYSTORE" ]]; then
  keytool -genkeypair -noprompt -keystore "$KEYSTORE" -storepass android -keypass android \
    -alias androiddebugkey -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA \
    -keysize 2048 -validity 10000 >/dev/null
fi

export JAVA_HOME="${JAVA_HOME:-/tmp/android-jdk17}"
export PATH="$JAVA_HOME/bin:$PATH"
case "$ANDROID_BUILD_TYPE" in
  debug|release) BUILD_TASK="assemble${ANDROID_BUILD_TYPE^}" ;;
  *) echo "ANDROID_BUILD_TYPE must be debug or release" >&2; exit 2 ;;
esac
"$APP_PROJECT/gradlew" -p "$APP_PROJECT" \
  -PytdlpAndroidRoot="$YTDLP_ANDROID_ROOT" \
  -Dandroid.injected.testOnly=false \
  -PandroidKeystore="$KEYSTORE" \
  "$BUILD_TASK"

APK="$APP_PROJECT/app/build/outputs/apk/$ANDROID_BUILD_TYPE/app-${ANDROID_BUILD_TYPE}.apk"
if [[ ! -f "$APK" ]]; then
  APK="$APP_PROJECT/app/build/intermediates/apk/$ANDROID_BUILD_TYPE/app-${ANDROID_BUILD_TYPE}.apk"
fi
if [[ "$ANDROID_BUILD_TYPE" == debug ]]; then
  FINAL_APK="$OUT/DJ-Grok.apk"
else
  FINAL_APK="$OUT/DJ-Grok-release.apk"
fi
cp "$APK" "$FINAL_APK"
"$ANDROID_SDK_ROOT/build-tools/$ANDROID_BUILD_TOOLS/apksigner" verify --verbose "$FINAL_APK"
echo "$FINAL_APK"
