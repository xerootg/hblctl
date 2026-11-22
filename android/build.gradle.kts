// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
}

// `[workspace.package] version = "x.y.z"` from the repository's Cargo.toml. The app, the library and
// its Maven coordinates all carry it, so one release-plz bump moves everything together.
val workspaceVersion: String by extra(
    run {
        val cargo = rootProject.projectDir.parentFile.resolve("Cargo.toml").readText()
        val section = cargo.substringAfter("[workspace.package]")
        Regex("""(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"""").find(section)?.groupValues?.get(1)
            ?: error("workspace.package.version not found in Cargo.toml")
    },
)
