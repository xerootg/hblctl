#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Haley Grandle
# SPDX-License-Identifier: LGPL-3.0-or-later
# Builds hblctl-ffi for Android and generates its Kotlin bindings into android/lib/build/generated.
# HBLCTL_RUST_PROFILE=release builds optimised native libraries (the release workflow sets it); the
# default, dev, compiles fastest for local work.
set -euo pipefail
cd "$(dirname "$0")/.."
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-/opt/android-sdk/ndk/29.0.14206865}"
profile="${HBLCTL_RUST_PROFILE:-dev}"
case "$profile" in
  dev | release) ;;
  *) echo "HBLCTL_RUST_PROFILE must be dev or release, not '$profile'" >&2; exit 2 ;;
esac
out="$PWD/android/lib/build/generated"
rm -rf "$out/uniffi" "$out/jniLibs"
mkdir -p "$out/uniffi" "$out/jniLibs"
# The bindings are generated from a host build; they depend only on the exported interface, not the profile.
cargo build --offline -p hblctl-ffi
cargo run --offline -p hblctl-ffi --features cli -- generate --library target/debug/libhblctl_ffi.so --language kotlin --no-format --config android/uniffi.toml --out-dir "$out/uniffi"
cargo ndk -t arm64-v8a -t x86_64 --platform 29 -o "$out/jniLibs" build --offline -p hblctl-ffi --profile "$profile"
