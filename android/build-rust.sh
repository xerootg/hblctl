#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Haley Grandle
# SPDX-License-Identifier: LGPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-/opt/android-sdk/ndk/29.0.14206865}"
out="$PWD/android/app/build/generated"
mkdir -p "$out/uniffi" "$out/jniLibs"
cargo build --offline -p hblctl-ffi
cargo run --offline -p hblctl-ffi --features cli -- generate --library target/debug/libhblctl_ffi.so --language kotlin --no-format --config android/uniffi.toml --out-dir "$out/uniffi"
cargo ndk -t arm64-v8a -t x86_64 --platform 29 -o "$out/jniLibs" build --offline -p hblctl-ffi
