// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}
android {
    namespace = "org.hblctl.android"
    compileSdk = 35
    ndkVersion = libs.versions.ndk.get()
    defaultConfig {
        applicationId = "org.hblctl.android"
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    buildFeatures { compose = true }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    sourceSets["main"].java.srcDir(layout.buildDirectory.dir("generated/uniffi"))
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("generated/jniLibs"))
    packaging { resources.excludes += listOf("META-INF/AL2.0", "META-INF/LGPL2.1") }
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
