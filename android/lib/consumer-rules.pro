# SPDX-FileCopyrightText: 2026 Haley Grandle
# SPDX-License-Identifier: LGPL-3.0-or-later
# Applied to apps that minify with R8. JNA finds the bindings' native methods and structures by
# reflection, so neither JNA nor the generated bindings may be renamed or removed.
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class uniffi.hblctl_ffi.** { *; }
