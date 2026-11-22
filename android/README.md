<!-- SPDX-FileCopyrightText: 2026 Haley Grandle -->
<!-- SPDX-License-Identifier: LGPL-3.0-or-later -->

# Android

Two Gradle modules live here:

- **`lib`**: the hblctl Android library. It packages the UniFFI 0.29 Kotlin bindings for
  `hblctl-ffi` and the native library for `arm64-v8a` and `x86_64` as an AAR, published as
  `org.hblctl:hblctl-ffi`. Apps use it without installing Rust.
- **`app`**: the LGPL-3.0-or-later reference app, built on `lib`. Every operation, its safety badge,
  documentation and form comes from registry data; `SchemaForm` uses the pure Kotlin `SchemaModel`.
  Calls go exclusively through Rust `dispatch::invoke`.

`consumer-smoke` is a separate Gradle build that uses the published AAR the way an outside app does.

## Use hblctl from your own app

Each release from **v0.4.0** carries the library in two forms. Pick one.

### Option 1: Maven, from GitHub Packages

GitHub Packages needs a token **even to download public packages**. Create a classic personal access
token with only the `read:packages` scope at <https://github.com/settings/tokens/new?scopes=read:packages>,
then give it to Gradle without committing it. Either put it in `~/.gradle/gradle.properties`:

```properties
gpr.user=your-github-username
gpr.key=ghp_the_token
```

or set the environment variables `GITHUB_ACTOR` and `GITHUB_TOKEN` (the usual choice in CI, from a
masked secret). Then in the app's `settings.gradle.kts`:

```kotlin
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
        maven {
            name = "hblctl"
            url = uri("https://maven.pkg.github.com/xerootg/hblctl")
            credentials {
                username = providers.gradleProperty("gpr.user").orElse(providers.environmentVariable("GITHUB_ACTOR")).orNull
                password = providers.gradleProperty("gpr.key").orElse(providers.environmentVariable("GITHUB_TOKEN")).orNull
            }
            content { includeGroup("org.hblctl") }
        }
    }
}
```

and in the module's `build.gradle.kts`:

```kotlin
dependencies {
    implementation("org.hblctl:hblctl-ffi:0.4.0")
}
```

JNA (the Android AAR flavour) and `androidx.annotation` come with it. The app needs
`android.useAndroidX=true` in `gradle.properties` (new projects already have it).

If the build says `Could not GET 'https://maven.pkg.github.com/...'. Received status code 401`, Gradle
sent no token or a token without `read:packages`: check the two properties above.

### Option 2: the plain AAR from the release page

No token needed. Download `hblctl-ffi-v0.4.0.aar` from
<https://github.com/xerootg/hblctl/releases/tag/v0.4.0> into the app's `libs/` folder, then:

```kotlin
dependencies {
    implementation(files("libs/hblctl-ffi-v0.4.0.aar"))
    // A file dependency brings no dependencies of its own, so add the library's two by hand.
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("androidx.annotation:annotation:1.9.1")
}
```

### What is in it

Everything is in the Kotlin package `uniffi.hblctl_ffi`:

- `Registry()`: the built-in command registry. `commandsJson()` lists every command with its safety class
  and documentation; `inputSchema(command, func)` gives the JSON Schema of its arguments.
- `Session.connect(host, port, timeoutMs, allow, sessionDir)`: opens TCP to the camera, usually
  `192.168.42.2` port `30303`. The library declares the `INTERNET` permission for this.
- `Session.connectStream(stream, label, timeoutMs, allow, sessionDir)`: the same session over a byte
  stream the app supplies (next section).
- `session.invoke(command, func, argsJson)` runs one operation and returns its result as JSON.
  `allow` (a `Safety`) is the highest safety class the session will send, fixed for its lifetime.
- Errors arrive as `uniffi.hblctl_ffi.Exception` subclasses: `Refused`, `Arguments`, `Transport`,
  `Session`, each with a `detail` string to show the user.
- Every session writes `meta.json`, a TRACE event log and a byte-exact capture into `sessionDir`, which
  must not already hold a session. `dump(path)` decodes a capture.

Calls block: run them off the main thread (for example on `Dispatchers.IO`). R8 keep rules for JNA
and the bindings ship inside the AAR, so minified apps need nothing extra.

### Bring your own byte stream

`Session.connect` can only reach the camera over a network route Android already has. When the app
reaches the camera another way, for example an ADB `tcp:30303` stream over USB or a USB network
interface the app configured itself, it implements `ByteStream` and hands it over:

```kotlin
class AdbStream(private val input: InputStream, private val output: OutputStream, private val socket: Closeable) : ByteStream {
    override fun send(bytes: ByteArray) {
        try { output.write(bytes); output.flush() }
        catch (e: IOException) { throw StreamException.Io("writing to the camera failed: ${e.message}") }
    }
    override fun recv(timeoutMs: ULong): ByteArray? {
        // readWithTimeout is your own helper: wait at most timeoutMs, give null on timeout, -1 at end of stream.
        val buffer = ByteArray(64 * 1024)
        val n = readWithTimeout(input, buffer, timeoutMs.toLong()) ?: throw StreamException.Timeout()
        return if (n < 0) null else buffer.copyOf(n)
    }
    override fun close() { socket.close() }
}

val session = Session.connectStream(AdbStream(input, output, socket), "adb tcp:30303", 5_000uL, Safety.READ, dir)
```

The rules, which the library relies on:

- **`recv` must return within `timeoutMs`.** Rust cannot interrupt a blocked call. Throw
  `StreamException.Timeout` when nothing arrived; the session then reports
  `transport: ... timed out after 5s` and disconnects, exactly as it does over TCP.
  `timeoutMs` is what is left of the session timeout since the last `send` returned, so a reply
  that trickles in still has to arrive within one timeout.
- Return `null` only when the camera closed the stream. Never return an empty array.
- Throw `StreamException.Io(detail)` for a broken stream; `detail` appears in the error the user sees.
- `close()` is called exactly once: on `disconnect()`, when the link fails, when the session is closed
  or garbage-collected, and also if `connectStream` itself fails. Do not close the stream yourself.
- Calls happen one at a time on the thread running `invoke`, inside the session's lock, so they must not
  call back into the `Session`.

The bytes are the plain 30303 protocol, so captures and logs look the same as over TCP; `meta.json`
records `"transport": "stream"` and the label.

## Build it yourself

Install JDK 21, Android SDK platform 35 and build tools 35.0.0, NDK 29.0.14206865, Rust targets
`aarch64-linux-android` and `x86_64-linux-android`, and `cargo-ndk`. Gradle 9 comes from the wrapper.
From `android/`:

```sh
export ANDROID_HOME=/opt/android-sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/29.0.14206865
export JAVA_HOME=/path/to/jdk-21
./gradlew :app:testDebugUnitTest :app:assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

The library's `preBuild` runs `build-rust.sh`, which builds both ABIs and generates the Kotlin into
`lib/build/generated/uniffi`, with native libraries in `lib/build/generated/jniLibs`. Native libraries
are built with Cargo's `dev` profile; `HBLCTL_RUST_PROFILE=release` (set by the release workflow)
builds optimised ones. To generate the bindings by hand instead, run these from the **repository
root** (not `android/`):

```sh
cargo build --offline -p hblctl-ffi
cargo run --offline -p hblctl-ffi --features cli -- generate --library target/debug/libhblctl_ffi.so --language kotlin --no-format --config android/uniffi.toml --out-dir android/lib/build/generated/uniffi
```

The script assumes a Linux host and the default Cargo target directory. Rust dependencies
must already be fetched (`cargo fetch --locked` on a network-enabled machine). Gradle
also requires its dependencies cached or network access.

### Build and check the AAR

From `android/`:

```sh
./gradlew :lib:publishReleasePublicationToLocalRepository
./gradlew -p consumer-smoke testDebugUnitTest
```

The first command writes the AAR, POM and sources jar to `lib/build/repo` (a Maven layout). The second
resolves `org.hblctl:hblctl-ffi` from there, checks that the AAR carries both ABIs and the generated
classes, and runs JVM tests that call through the AAR's Kotlin bindings into a host build of the same
Rust code (`target/debug/libhblctl_ffi.so`, left there by `build-rust.sh`; pass `-PhostLib=` for
another path). Both run in CI on every pull request, and the release workflow runs them before it
publishes.

## The reference app

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
The app disables Android backup. Releases attach an APK signed with the project's release key,
so it updates in place.
