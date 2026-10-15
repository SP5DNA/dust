#!/usr/bin/env bash
# Gradle init hook for the Android project dx generates.
#
# dx 0.7 leaves three things out for this app, and has a setting for none:
#
# - It always writes Android's stock green robot as the launcher icon (an
#   adaptive icon under mipmap-anydpi-v26, which is what Android 8+ shows),
#   and never reads bundle.icon.
# - It always writes versionCode = 1. Play turns away an upload whose
#   version code is not above every one uploaded before it, so every build
#   after the first would be refused.
# - It asks for no native debug symbols, so Play has nothing to name the
#   functions in a crash with. The release profile keeps the symbol table
#   (Cargo.toml strips only the debug info), and the Android plugin can pack
#   it into the bundle for Play, given the NDK to read it with.
#
# Source this and call install_android_gradle_hook before
# `dx bundle --platform android`. The caller removes $ANDROID_GRADLE_HOOK
# when the build exits; Gradle loads every script in ~/.gradle/init.d.

# The workspace version in the Cargo.toml under `root`, e.g. 0.14.4.
dust_version() {
  awk '
    /^\[workspace.package\]/ { in_pkg = 1; next }
    in_pkg && /^\[/ { in_pkg = 0 }
    in_pkg && /^version *=/ { gsub(/[" ]/, "", $3); print $3; exit }
  ' "$1/Cargo.toml"
}

# The Android version code for a MAJOR.MINOR.PATCH version:
# MAJOR * 1000000 + MINOR * 1000 + PATCH, so 0.14.4 is 14004. A later version
# is always a higher code, as Play requires, while MINOR and PATCH stay under
# 1000.
dust_version_code() {
  local version=$1
  if [[ ! "$version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    echo "Version '$version' is not MAJOR.MINOR.PATCH" >&2
    return 1
  fi
  local major=$((10#${BASH_REMATCH[1]}))
  local minor=$((10#${BASH_REMATCH[2]}))
  local patch=$((10#${BASH_REMATCH[3]}))
  if ((minor > 999 || patch > 999)); then
    echo "Version '$version' has no version code: MINOR and PATCH must stay under 1000" >&2
    return 1
  fi
  echo $((major * 1000000 + minor * 1000 + patch))
}

install_android_gradle_hook() {
  local root=$1
  local gradle_init="${GRADLE_USER_HOME:-$HOME/.gradle}/init.d"
  if [[ ! -d "$root/assets/android" ]]; then
    echo "Launcher icon resources missing at $root/assets/android" >&2
    echo "Run scripts/render-android-icons.py after updating assets/icon.png" >&2
    return 1
  fi
  local version code
  version="$(dust_version "$root")"
  code="$(dust_version_code "$version")" || return 1
  # The NDK the Rust code is built with. Left alone, the Android plugin looks
  # only for its own pinned NDK version, and without that one it can neither
  # read the symbols out nor strip the libraries it packages.
  local ndk_path='// No ANDROID_NDK_HOME: the Android plugin looks for its own NDK.'
  if [[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]]; then
    ndk_path="android.ndkPath = \"${ANDROID_NDK_HOME}\""
  fi
  mkdir -p "$gradle_init"
  ANDROID_GRADLE_HOOK="$gradle_init/dust-android.gradle"
  cat > "$ANDROID_GRADLE_HOOK" << EOF
gradle.beforeProject { project ->
    // Registered before the project's build script runs, so it runs before
    // the Android plugin's own afterEvaluate, the one that makes the
    // variants from the DSL: the DSL can still be changed here, and what is
    // set here is what they are made from.
    project.afterEvaluate {
        def android = project.extensions.findByName("android")
        if (android == null) return
        if (android.defaultConfig.applicationId != "dev.dust.morse") return

        // The version, from Cargo.toml, in place of the versionCode = 1 dx
        // writes.
        android.defaultConfig.versionCode = ${code}
        android.defaultConfig.versionName = "${version}"
        println "Dust: version ${version}, version code ${code}"

        // The native symbol table, packed into the bundle where Play reads
        // it: crashes and ANRs in the Rust code then name their functions.
        // The libraries shipped in the app are stripped of it.
        android.defaultConfig.ndk.debugSymbolLevel = "SYMBOL_TABLE"
        ${ndk_path}

        def iconRes = new File("${root}/assets/android")
        if (!iconRes.isDirectory()) {
            throw new GradleException("Dust launcher icon dir missing: " + iconRes)
        }
        def res = project.file("src/main/res")
        project.delete(
            new File(res, "drawable/ic_launcher_background.xml"),
            new File(res, "drawable-v24/ic_launcher_foreground.xml")
        )
        ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"].each { density ->
            project.delete(new File(res, "mipmap-" + density + "/ic_launcher.webp"))
        }
        project.copy {
            from iconRes
            into res
        }
        println "Dust: installed launcher icon from " + iconRes
    }
}
EOF
  echo "Android: version ${version}, version code ${code}"
}

# The native debug symbols packed into the app bundle at `aab`, one entry a
# line: what Play reads to name the functions in a crash. Read with the
# JDK's jar, which every Android build already needs.
android_bundle_symbols() {
  local listing
  listing="$(jar tf "$1")" || return 1
  grep '^BUNDLE-METADATA/com.android.tools.build.debugsymbols/.' <<< "$listing" || true
}

# Fail unless the APK at `apk` carries the version and version code the
# Cargo.toml under `root` asks for: proof the hook reached the build.
check_android_apk_version() {
  local root=$1 apk=$2
  local version code aapt2 badging
  version="$(dust_version "$root")"
  code="$(dust_version_code "$version")" || return 1
  aapt2="$(find "${ANDROID_HOME:?ANDROID_HOME is not set}/build-tools" -name aapt2 -type f | sort -V | tail -1)"
  if [[ -z "$aapt2" ]]; then
    echo "No aapt2 under $ANDROID_HOME/build-tools" >&2
    return 1
  fi
  # The first line only, cut here rather than by `head`: under pipefail,
  # aapt2 writing on into a closed pipe would fail the whole check.
  badging="$("$aapt2" dump badging "$apk")" || return 1
  badging="${badging%%$'\n'*}"
  echo "$badging"
  if [[ "$badging" != *"versionCode='${code}' versionName='${version}'"* ]]; then
    echo "$apk is not version ${version} with version code ${code}" >&2
    return 1
  fi
}
