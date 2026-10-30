// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// Read `[workspace.package] version = "x.y.z"` from the repository's Cargo.toml.
val workspaceVersion: String = run {
    val cargo = rootProject.projectDir.parentFile.resolve("Cargo.toml").readText()
    val section = cargo.substringAfter("[workspace.package]")
    Regex("""(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"""").find(section)?.groupValues?.get(1)
        ?: error("workspace.package.version not found in Cargo.toml")
}
val workspaceVersionCode: Int = workspaceVersion.split(".").map { it.toInt() }.let { (major, minor, patch) ->
    major * 1_000_000 + minor * 1_000 + patch
}

android {
    namespace = "org.hblctl.android"
    compileSdk = 35
    ndkVersion = libs.versions.ndk.get()
    defaultConfig {
        applicationId = "org.hblctl.android"
        minSdk = 29
        targetSdk = 35
        // The app carries the workspace version from Cargo.toml so releases, APK names and the
        // Android versionCode (which must increase for in-place updates) all move together.
        versionName = workspaceVersion
        versionCode = workspaceVersionCode
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    buildFeatures { compose = true }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    sourceSets["main"].java.srcDir(layout.buildDirectory.dir("generated/uniffi"))
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("generated/jniLibs"))
    packaging { resources.excludes += listOf("META-INF/AL2.0", "META-INF/LGPL2.1") }
    // Release signing comes from the environment (CI decodes the keystore from a secret). Without it the
    // release build type stays unsigned, so PR builds and local builds need nothing extra.
    val keystorePath = System.getenv("ANDROID_KEYSTORE_PATH")
    if (keystorePath != null) {
        signingConfigs.create("release") {
            storeFile = file(keystorePath)
            storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
            keyAlias = System.getenv("ANDROID_KEY_ALIAS") ?: "hblctl"
            keyPassword = System.getenv("ANDROID_KEY_PASSWORD") ?: System.getenv("ANDROID_KEYSTORE_PASSWORD")
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            if (keystorePath != null) signingConfig = signingConfigs.getByName("release")
        }
    }
}
kotlin { jvmToolchain(21) }
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
}
val buildRust by tasks.registering(Exec::class) {
    workingDir(rootProject.projectDir.parentFile)
    commandLine("bash", "android/build-rust.sh")
    inputs.dir("../../crates")
    inputs.file("../../Cargo.lock")
    inputs.file(rootProject.file("uniffi.toml"))
    inputs.file(rootProject.file("build-rust.sh"))
    outputs.dir(layout.buildDirectory.dir("generated/uniffi"))
    outputs.dir(layout.buildDirectory.dir("generated/jniLibs"))
}
tasks.named("preBuild") { dependsOn(buildRust) }
tasks.configureEach { if (name.startsWith("compile") && name.endsWith("Kotlin")) dependsOn(buildRust) }
dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.material3)
    implementation(libs.compose.ui)
    implementation(libs.activity.compose)
    implementation(libs.coroutines)
    implementation(libs.json)
    implementation(libs.core)
    implementation(libs.jna) { artifact { type = "aar" } }
    testImplementation(libs.junit)
}
