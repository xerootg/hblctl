// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

// A stand-alone consumer of the published AAR. It resolves org.hblctl:hblctl-ffi from a Maven
// repository exactly as the companion app would, so it catches packaging mistakes the in-tree app
// (which uses project(":lib")) cannot. Run it from android/:
//   ./gradlew :lib:publishReleasePublicationToLocalRepository
//   ./gradlew -p consumer-smoke test
pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
val hblctlRepo: String = providers.gradleProperty("hblctlRepo").getOrElse(file("../lib/build/repo").path)
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        maven {
            name = "hblctl"
            url = uri(hblctlRepo)
            content { includeGroup("org.hblctl") }
        }
        google()
        mavenCentral()
    }
    versionCatalogs { create("libs") { from(files("../gradle/libs.versions.toml")) } }
}
rootProject.name = "hblctl-consumer-smoke"
