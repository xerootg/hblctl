// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

import java.io.ByteArrayInputStream
import java.util.zip.ZipFile
import java.util.zip.ZipInputStream

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

// The version under test defaults to the workspace version in Cargo.toml.
val hblctlVersion: String = providers.gradleProperty("hblctlVersion").getOrElse(
    run {
        val section = rootDir.resolve("../../Cargo.toml").readText().substringAfter("[workspace.package]")
        Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(section)?.groupValues?.get(1)
            ?: error("workspace.package.version not found in Cargo.toml")
    },
)
// A host (Linux x86_64) build of the same Rust crate, so the JVM test can call through the AAR's
// Kotlin bindings for real: `cargo build -p hblctl-ffi` produces it.
val hostLib: File = file(providers.gradleProperty("hostLib").getOrElse("../../target/debug/libhblctl_ffi.so"))

android {
    namespace = "org.hblctl.smoke"
    compileSdk = 35
    defaultConfig { minSdk = 29 }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    // The bindings read Build.VERSION.SDK_INT; on the JVM the android.jar stub returns 0 instead of throwing.
    testOptions { unitTests.isReturnDefaultValues = true }
}
kotlin { jvmToolchain(21) }
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
}

dependencies {
    implementation("org.hblctl:hblctl-ffi:$hblctlVersion")
    testImplementation(libs.junit)
    testImplementation(libs.json)
    // The JNA jar carries the desktop libjnidispatch the JVM test needs; the AAR flavour only has Android's.
    testImplementation(libs.jna)
}

// Checks the AAR itself: both ABIs' native libraries and the generated classes must be inside.
val verifyAar by tasks.registering {
    description = "Checks that the published hblctl-ffi AAR carries both ABIs and the generated bindings."
    val aar = configurations.detachedConfiguration(dependencies.create("org.hblctl:hblctl-ffi:$hblctlVersion@aar"))
    inputs.files(aar)
    doLast {
        ZipFile(aar.singleFile).use { zip ->
            val required = listOf("jni/arm64-v8a/libhblctl_ffi.so", "jni/x86_64/libhblctl_ffi.so", "classes.jar", "proguard.txt")
            val missing = required.filter { zip.getEntry(it) == null }
            check(missing.isEmpty()) { "${aar.singleFile.name} is missing $missing" }
            val classes = zip.getInputStream(zip.getEntry("classes.jar")).use { it.readBytes() }
            val names = mutableSetOf<String>()
            val jar = ZipInputStream(ByteArrayInputStream(classes))
            while (true) names += (jar.nextEntry ?: break).name
            jar.close()
            for (cls in listOf("Session", "Registry", "ByteStream", "StreamException")) {
                check("uniffi/hblctl_ffi/$cls.class" in names) { "classes.jar has no uniffi.hblctl_ffi.$cls" }
            }
        }
    }
}
tasks.withType<Test>().configureEach {
    dependsOn(verifyAar)
    inputs.file(hostLib)
    doFirst {
        check(hostLib.isFile) { "No host library at $hostLib: run `cargo build -p hblctl-ffi` first, or pass -PhostLib=/path/to/libhblctl_ffi.so" }
    }
    systemProperty("uniffi.component.hblctl_ffi.libraryOverride", hostLib.absolutePath)
}
