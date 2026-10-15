# GStreamer media dependencies

Sweet uses GStreamer for local container demuxing, synchronized audio/video
playback, and video decoding. The native Rust path remains available as a
fallback if GStreamer cannot open a file.

## Desktop

Install GStreamer development files and runtime plugins before building
`sweet-node`. On Debian or Ubuntu:

```sh
sudo apt install libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-libav
```

The base and good plugins provide `playbin`, audio output, and common parsers.
The bad and libav plugins add codecs and container support. Codec availability
depends on the installed plugin set and platform.

## Android APK

Download and extract the official GStreamer Android universal SDK, then set
`GSTREAMER_ANDROID_ROOT` to its extracted root. The APK build script selects
the requested ABI, uses its `.pc` files for cross-compilation, and bundles the
ABI's GStreamer shared libraries and plugins into the APK. For example:

```sh
export GSTREAMER_ANDROID_ROOT="$HOME/opt/gstreamer-1.0-android-universal-1.28.8"
ANDROID_ABI=arm64-v8a scripts/build-android-apk.sh
```

The script supports `arm64-v8a` and `x86_64`. GStreamer initializes its plugin
search path from the app's native library directory and stores its plugin
registry in the app cache.
