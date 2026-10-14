#!/usr/bin/env bash
# Gradle init hook that replaces the launcher icon dx generates.
#
# dx always writes Android's stock green robot (an adaptive icon under
# mipmap-anydpi-v26, which is what Android 8+ shows) and never reads
# bundle.icon. Source this and call install_android_icon_hook before
# `dx bundle --platform android`. The caller removes $ANDROID_ICON_HOOK
# when the build exits; Gradle loads every script in ~/.gradle/init.d.

install_android_icon_hook() {
  local root=$1
  local gradle_init="${GRADLE_USER_HOME:-$HOME/.gradle}/init.d"
  if [[ ! -d "$root/assets/android" ]]; then
    echo "Launcher icon resources missing at $root/assets/android" >&2
    echo "Run scripts/render-android-icons.py after updating assets/icon.png" >&2
    return 1
  fi
  mkdir -p "$gradle_init"
  ANDROID_ICON_HOOK="$gradle_init/dust-android-icon.gradle"
  cat > "$ANDROID_ICON_HOOK" << EOF
gradle.beforeProject { project ->
    project.afterEvaluate {
        def android = project.extensions.findByName("android")
        if (android == null) return
        if (android.defaultConfig.applicationId != "dev.dust.morse") return
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
}
