#!/usr/bin/env python3
"""Scale assets/icon.png into the launcher densities the Android build copies.

dx writes the stock green robot into the generated project and does not read
an app icon, so these files are what actually lands on the device. The
adaptive foreground is inset to the 66dp safe circle: a round mask would
otherwise clip the serifs of the wordmark.
"""

from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
MASTER = ROOT / "assets" / "icon.png"
OUT = ROOT / "assets" / "android"
NAVY = (0x1B, 0x24, 0x36, 255)
# Farthest ink on the 512 master is ~186px from center. The safe zone is a
# 66dp circle on the 108dp adaptive canvas, 156px at that size.
SAFE_SCALE = 0.82

LEGACY = {
    "mipmap-mdpi": 48,
    "mipmap-hdpi": 72,
    "mipmap-xhdpi": 96,
    "mipmap-xxhdpi": 144,
    "mipmap-xxxhdpi": 192,
}
ADAPTIVE = {
    "mipmap-mdpi": 108,
    "mipmap-hdpi": 162,
    "mipmap-xhdpi": 216,
    "mipmap-xxhdpi": 324,
    "mipmap-xxxhdpi": 432,
}


def main() -> None:
    master = Image.open(MASTER).convert("RGBA")
    if master.size != (512, 512):
        raise SystemExit(f"{MASTER} must be 512×512, got {master.size}")

    fitted = master.resize(
        (round(512 * SAFE_SCALE), round(512 * SAFE_SCALE)),
        Image.Resampling.LANCZOS,
    )
    foreground = Image.new("RGBA", (512, 512), NAVY)
    foreground.paste(
        fitted,
        ((512 - fitted.width) // 2, (512 - fitted.height) // 2),
        fitted,
    )

    for folder, size in LEGACY.items():
        dest = OUT / folder
        dest.mkdir(parents=True, exist_ok=True)
        master.resize((size, size), Image.Resampling.LANCZOS).save(
            dest / "ic_launcher.png", optimize=True
        )
    for folder, size in ADAPTIVE.items():
        dest = OUT / folder
        dest.mkdir(parents=True, exist_ok=True)
        foreground.resize((size, size), Image.Resampling.LANCZOS).save(
            dest / "ic_launcher_foreground.png", optimize=True
        )

    anydpi = OUT / "mipmap-anydpi-v26"
    anydpi.mkdir(parents=True, exist_ok=True)
    (anydpi / "ic_launcher.xml").write_text(
        """<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/dust_icon_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
</adaptive-icon>
"""
    )
    values = OUT / "values"
    values.mkdir(parents=True, exist_ok=True)
    (values / "dust_icon.xml").write_text(
        """<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="dust_icon_background">#1B2436</color>
</resources>
"""
    )


if __name__ == "__main__":
    main()
