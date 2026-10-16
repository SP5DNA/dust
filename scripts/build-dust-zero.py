#!/usr/bin/env python3
"""Cut the slashed zero out of IBM Plex Mono and embed it in the stylesheet.

Copy puts 0 next to O, and Plex draws its zero with a dot that is lost at the
size of an alignment cell. Plex has a slashed zero too, behind the OpenType
`zero` feature, but Google Fonts strips that feature from the files it serves,
so `font-variant-numeric: slashed-zero` does nothing. This takes the slashed
glyph from IBM's own release, maps it to plain U+0030, and keeps nothing else:
a face of one character, "Dust Zero", that the group-character rules put
first in their stack. Every other character falls through to Plex.

The faces go into assets/styles.css as data URIs, between the markers below,
because the sheet is inlined on desktop and mobile, where a font file beside
it would not resolve; and inlined, the zero is there offline and on the first
frame. IBM Plex is under the SIL Open Font License 1.1: the derived face takes
another name, as the licence's Reserved Font Name requires, and carries the
licence whole in its name table.

Needs fonttools and brotli: `pip install fonttools brotli`.
"""

import base64
import hashlib
import io
import re
import tarfile
import urllib.request
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parents[1]
STYLESHEET = ROOT / "assets" / "styles.css"

PLEX_VERSION = "2.5.0"
TARBALL = f"https://registry.npmjs.org/@ibm/plex-mono/-/plex-mono-{PLEX_VERSION}.tgz"
TARBALL_SHA256 = "55b5ffcfcd5e9db36ef2070b2b07a573434ca02c6f3be2b921c0b88fff29acad"
LICENSE = "package/LICENSE.txt"

FAMILY = "Dust Zero"
ZERO = 0x30
# The stylesheet asks for 400 and 700, and Plex is loaded at 500 and 700.
# Each face takes half of the weight axis, so a request never falls between.
FACES = [
    ("Medium", "package/fonts/complete/woff2/IBMPlexMono-Medium.woff2", "100 599"),
    ("Bold", "package/fonts/complete/woff2/IBMPlexMono-Bold.woff2", "600 900"),
]

BEGIN = "/* dust-zero:begin */"
END = "/* dust-zero:end */"


def fetch() -> tarfile.TarFile:
    with urllib.request.urlopen(TARBALL) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != TARBALL_SHA256:
        raise SystemExit(f"{TARBALL} has sha256 {digest}, expected {TARBALL_SHA256}")
    return tarfile.open(fileobj=io.BytesIO(data), mode="r:gz")


def slashed_glyph(font: TTFont) -> str:
    """The glyph the `zero` feature puts in place of the plain zero."""
    plain = font.getBestCmap()[ZERO]
    gsub = font["GSUB"].table
    for record in gsub.FeatureList.FeatureRecord:
        if record.FeatureTag != "zero":
            continue
        for index in record.Feature.LookupListIndex:
            for table in gsub.LookupList.Lookup[index].SubTable:
                mapping = getattr(table, "mapping", {})
                if plain in mapping:
                    return mapping[plain]
    raise SystemExit("the font has no `zero` substitution for the plain zero")


def build_face(source: bytes, style: str, license_text: str) -> bytes:
    # Keep Plex's own timestamp, so the same release builds the same bytes.
    font = TTFont(io.BytesIO(source), recalcTimestamp=False)
    glyph = slashed_glyph(font)
    for table in font["cmap"].tables:
        if ZERO in table.cmap:
            table.cmap[ZERO] = glyph

    options = subset.Options()
    options.layout_features = []
    options.hinting = False
    options.notdef_outline = False
    options.glyph_names = False
    options.name_IDs = [0, 5, 7, 14]
    options.name_languages = [0x409]
    options.flavor = "woff2"
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=[ZERO])
    subsetter.subset(font)

    names = font["name"]
    postscript = f"{FAMILY.replace(' ', '')}-{style}"
    for name_id, value in [
        (1, f"{FAMILY} {style}" if style != "Bold" else FAMILY),
        (2, "Bold" if style == "Bold" else "Regular"),
        (3, f"{postscript};IBM Plex Mono {PLEX_VERSION}"),
        (4, f"{FAMILY} {style}"),
        (6, postscript),
        (10, f"The slashed zero of IBM Plex Mono {PLEX_VERSION}, alone."),
        (13, license_text),
        (16, FAMILY),
        (17, style),
    ]:
        names.setName(value, name_id, 3, 1, 0x409)

    out = io.BytesIO()
    font.save(out)
    return out.getvalue()


def css(faces: list[tuple[str, bytes]]) -> str:
    rules = [BEGIN]
    for weights, data in faces:
        encoded = base64.b64encode(data).decode("ascii")
        rules.append(
            "@font-face {\n"
            f'  font-family: "{FAMILY}";\n'
            "  font-style: normal;\n"
            f"  font-weight: {weights};\n"
            "  font-display: swap;\n"
            "  unicode-range: U+0030;\n"
            f'  src: url("data:font/woff2;base64,{encoded}") format("woff2");\n'
            "}"
        )
    rules.append(END)
    return "\n".join(rules)


def main() -> None:
    with fetch() as tar:
        license_text = tar.extractfile(LICENSE).read().decode("utf-8").strip()
        faces = [
            (weights, build_face(tar.extractfile(path).read(), style, license_text))
            for style, path, weights in FACES
        ]

    sheet = STYLESHEET.read_text(encoding="utf-8")
    block = re.compile(re.escape(BEGIN) + r".*?" + re.escape(END), re.DOTALL)
    if not block.search(sheet):
        raise SystemExit(f"{STYLESHEET} has no {BEGIN} … {END} block to fill")
    STYLESHEET.write_text(block.sub(lambda _: css(faces), sheet), encoding="utf-8")
    for (weights, data), (style, _, _) in zip(faces, FACES):
        print(f"{FAMILY} {style} ({weights}): {len(data)} bytes")


if __name__ == "__main__":
    main()
