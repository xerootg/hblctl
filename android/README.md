<!-- SPDX-FileCopyrightText: 2026 Haley Grandle -->
<!-- SPDX-License-Identifier: LGPL-3.0-or-later -->

# Android

The LGPL-3.0-or-later app uses the built-in registry through UniFFI 0.29. Every operation,
its safety badge, documentation and form comes from registry data; `SchemaForm` uses the
pure Kotlin `SchemaModel`. Calls go exclusively through Rust `dispatch::invoke`.

Install JDK 21, Gradle 9, Android SDK platform 35/build tools 35.0.0, NDK
29.0.14206865, Rust targets `aarch64-linux-android` and `x86_64-linux-android`,
and `cargo-ndk`. No Gradle wrapper binary is vendored. From the repository root:

```sh
export ANDROID_HOME=/opt/android-sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/29.0.14206865
export JAVA_HOME=/path/to/jdk-21
bash android/build-rust.sh
gradle -p android :app:testDebugUnitTest :app:assembleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

`preBuild` invokes `build-rust.sh`, building both ABIs and generating Kotlin into
`app/build/generated/uniffi`, with native libraries in `app/build/generated/jniLibs`.
The exact standalone generation commands are:

```sh
cargo build --offline -p hblctl-ffi
cargo run --offline -p hblctl-ffi --features cli -- generate --library target/debug/libhblctl_ffi.so --language kotlin --no-format --config android/uniffi.toml --out-dir android/app/build/generated/uniffi
```

The script assumes a Linux host and the default Cargo target directory. Rust dependencies
must already be fetched (`cargo fetch --locked` on a network-enabled machine). Gradle
also requires its dependencies cached or network access on the reviewer's machine.

Connect defaults to `192.168.42.2:30303` with a read-only ceiling and a five-second
connection/result timeout. The ceiling stays fixed for the connection. Destructive calls
require typing `command.function` each time. Only one camera connection is supported.
Blocking FFI calls run on `Dispatchers.IO`; operations are serialized inside Rust.
Progress and notifications are included in the completed result and status callback.
Connection state is the last known state; peer closure is detected during invocation.

Sessions live under the app's private `files/sessions` directory. Each connection attempt
creates a unique directory with metadata, a TRACE JSON event log, and a binary capture.
Sessions can decode their own captures and share a ZIP using Android's system share sheet
(including save-to-files targets). Disconnect before exporting for a complete snapshot.
The app disables Android backup. The release workflow uploads a debug-signed APK;
it is intended for testing, with no production signing keys required.
