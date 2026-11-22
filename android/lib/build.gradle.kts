// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

// The hblctl Android library: UniFFI-generated Kotlin for hblctl-ffi plus its native libraries for
// arm64-v8a and x86_64, packaged as an AAR so apps can use hblctl without a Rust toolchain.
// Published as org.hblctl:hblctl-ffi:<workspace version>; see android/README.md for consumers.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    `maven-publish`
}

val workspaceVersion: String by rootProject.extra
group = "org.hblctl"
version = workspaceVersion

android {
    namespace = "org.hblctl.ffi"
    compileSdk = 35
    ndkVersion = libs.versions.ndk.get()
    defaultConfig {
        minSdk = 29
        consumerProguardFiles("consumer-rules.pro")
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    sourceSets["main"].java.srcDir(layout.buildDirectory.dir("generated/uniffi"))
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("generated/jniLibs"))
    publishing { singleVariant("release") { withSourcesJar() } }
}
kotlin { jvmToolchain(21) }
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions.jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
}

val rustProfile: String = providers.environmentVariable("HBLCTL_RUST_PROFILE").getOrElse("dev")
val buildRust by tasks.registering(Exec::class) {
    description = "Builds hblctl-ffi for Android and generates its Kotlin bindings (android/build-rust.sh)."
    workingDir(rootProject.projectDir.parentFile)
    commandLine("bash", "android/build-rust.sh")
    environment("HBLCTL_RUST_PROFILE", rustProfile)
    inputs.property("rustProfile", rustProfile)
    inputs.dir("../../crates")
    inputs.file("../../Cargo.lock")
    inputs.file("../../Cargo.toml")
    inputs.file(rootProject.file("uniffi.toml"))
    inputs.file(rootProject.file("build-rust.sh"))
    outputs.dir(layout.buildDirectory.dir("generated/uniffi"))
    outputs.dir(layout.buildDirectory.dir("generated/jniLibs"))
    // The host build the consumer smoke test loads; declaring it makes Gradle rebuild it if it is deleted.
    outputs.file("../../target/debug/libhblctl_ffi.so")
}
tasks.named("preBuild") { dependsOn(buildRust) }
tasks.configureEach {
    if ((name.startsWith("compile") && name.endsWith("Kotlin")) || name.endsWith("SourcesJar")) dependsOn(buildRust)
}

dependencies {
    // The generated bindings call JNA (Android AAR flavour, which carries libjnidispatch per ABI) and
    // annotate API-level checks with androidx.annotation. Both reach consumers through the POM.
    api(libs.jna) { artifact { type = "aar" } }
    implementation(libs.annotation)
}

publishing {
    publications {
        register<MavenPublication>("release") {
            groupId = "org.hblctl"
            artifactId = "hblctl-ffi"
            version = workspaceVersion
            afterEvaluate { from(components["release"]) }
            pom {
                name.set("hblctl-ffi")
                description.set("Kotlin bindings and native libraries for hblctl, the Hasselblad X2D diagnostic-port client.")
                url.set("https://github.com/xerootg/hblctl")
                licenses {
                    license {
                        name.set("LGPL-3.0-or-later")
                        url.set("https://spdx.org/licenses/LGPL-3.0-or-later.html")
                    }
                }
                scm { url.set("https://github.com/xerootg/hblctl") }
            }
        }
    }
    repositories {
        // A plain directory, for the consumer smoke test and for inspecting what would be published.
        maven {
            name = "local"
            url = uri(layout.buildDirectory.dir("repo"))
        }
        // GitHub Packages. The release workflow passes GITHUB_ACTOR and GITHUB_TOKEN.
        maven {
            name = "GitHubPackages"
            url = uri("https://maven.pkg.github.com/xerootg/hblctl")
            credentials {
                username = providers.environmentVariable("GITHUB_ACTOR").orNull
                password = providers.environmentVariable("GITHUB_TOKEN").orNull
            }
        }
    }
}
