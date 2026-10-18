#!/usr/bin/env python3
"""Count how real callsigns are built and write the table the generator uses.

A callsign is `prefix + separator digit + suffix`, and a country issues only
some separator digits and some suffix lengths after each of its prefixes. This
reads lists of real calls and counts, for every prefix and the country it
belongs to, how many calls have each separator digit and each suffix length.
The country is the DXCC entity cty.dat places the call in, so `UA3ABC` and
`UA9ABC` are counted apart: European and Asiatic Russia are two entities.

The counts go to crates/cw-core/src/callsign/prefixes.rs, which the
generator draws from in proportion: a prefix as often as real calls carry it,
and a digit and a suffix length as often as that prefix's calls have them.
Only counts survive, never a call, so the generator still makes up every call
it sends.

What the counts leave out:
- a prefix and country with fewer than MIN_CALLS calls behind it, which is
  too few to say what the country issues;
- a digit or a suffix length that only a handful of a prefix's calls have:
  these are special-event and contest calls, and odd ones out in the lists;
- suffixes over three letters, which are special-event calls (`YL0YOTA`) and
  the rare Australian foundation call;
- prefixes of any other shape than one letter, two letters, or a letter and
  a digit, and any call with a slash.

Inputs:
- cty.dat, the CT-format country file by AD1C (https://www.country-files.com),
  which Tlf also ships;
- one or more call lists: Super Check Partial's MASTER.SCP
  (https://www.supercheckpartial.com), any list of one call per line such as
  Tlf's callmaster, or MorseRunner's Master.dta, which is the SCP list in
  MorseRunner's own binary layout.

Usage: scripts/callsign-prefixes.py CTY_DAT CALLS [CALLS ...]
"""

import re
import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "crates" / "cw-core" / "src" / "callsign" / "prefixes.rs"

MIN_CALLS = 10
# A digit or a suffix length is kept once at least this many of the prefix's
# calls have it, and this share of them.
MIN_USES = 2
MIN_DIGIT_SHARE = 0.01
MIN_LENGTH_SHARE = 0.02
SUFFIX_MAX = 3
# Countries whose operators sign a call area after a slash: `W1AW/4`.
AREA_MARKING = {"K", "JA", "VE", "VK"}

CALL = re.compile(r"^(.*?)([0-9])([A-Z]+)$")
PREFIX = re.compile(r"^(?:[A-Z]{1,2}|[A-Z][0-9]|[0-9][A-Z])$")


def read_cty(path):
    """Prefixes and whole calls to DXCC entities, by primary prefix.

    Entities marked `*` count only for the WAE award, not for DXCC; leaving
    them out puts their calls under the DXCC entity they belong to.
    """
    names, prefixes, exact = {}, {}, {}
    text = Path(path).read_text(encoding="latin-1")
    for record in text.split(";"):
        head, _, aliases = record.strip().partition("\n")
        fields = [field.strip() for field in head.split(":")]
        if len(fields) < 8 or fields[7].startswith("*"):
            continue
        name, primary = fields[0], fields[7]
        names[primary] = name
        for alias in aliases.replace("\n", "").split(","):
            # Drop the zone, position and continent overrides.
            alias = re.sub(r"[(\[<{~].*", "", alias.strip())
            if alias.startswith("="):
                exact[alias[1:]] = primary
            elif alias:
                prefixes[alias] = primary
    return names, prefixes, exact


def entity(call, prefixes, exact):
    if call in exact:
        return exact[call]
    # Only KG4 with a two-letter suffix is Guantanamo Bay. The rest of KG4 is
    # the US fourth call area, which cty.dat lists call by call and misses.
    match = CALL.match(call)
    if match and match.group(1) == "KG" and match.group(2) == "4" and len(match.group(3)) != 2:
        return "K"
    for end in range(len(call), 0, -1):
        if call[:end] in prefixes:
            return prefixes[call[:end]]
    return None


def read_dta(data):
    """MorseRunner's Master.dta: an index of 37 x 37 + 1 offsets, then every
    call NUL-terminated, once under each pair of characters it holds."""
    entries = 37 * 37 + 1
    index = struct.unpack(f"<{entries}i", data[: 4 * entries])
    if index[0] != 4 * entries or index[-1] != len(data):
        raise ValueError("not a MorseRunner call list")
    return data[index[0] :].decode("ascii").split("\0")


def read_calls(path):
    data = Path(path).read_bytes()
    if path.lower().endswith(".dta"):
        lines = read_dta(data)
    else:
        lines = data.decode("latin-1").splitlines()
    return {
        line.strip().upper()
        for line in lines
        if line.strip() and not line.lstrip().startswith("#")
    }


def count(calls, prefixes, exact):
    rows = defaultdict(lambda: {"digits": Counter(), "lengths": Counter()})
    for call in calls:
        match = CALL.match(call)
        if not match or not PREFIX.match(match.group(1)):
            continue
        prefix, digit, suffix = match.groups()
        country = entity(call, prefixes, exact)
        if country is None:
            continue
        row = rows[(prefix, country)]
        row["digits"][int(digit)] += 1
        row["lengths"][len(suffix)] += 1
    return rows


def kept(counter, total, share, size, offset):
    return [
        counter[i + offset]
        if counter[i + offset] >= max(MIN_USES, share * total)
        else 0
        for i in range(size)
    ]


def table(rows):
    """The rows worth keeping, grouped by country, largest first."""
    by_country = defaultdict(list)
    for (prefix, country), row in rows.items():
        total = sum(row["digits"].values())
        if total < MIN_CALLS:
            continue
        digits = kept(row["digits"], total, MIN_DIGIT_SHARE, 10, 0)
        lengths = kept(row["lengths"], total, MIN_LENGTH_SHARE, SUFFIX_MAX, 1)
        if any(digits) and any(lengths):
            by_country[country].append((prefix, digits, lengths))
    order = sorted(
        by_country,
        key=lambda country: (-sum(sum(r[2]) for r in by_country[country]), country),
    )
    return [
        (country, sorted(by_country[country], key=lambda r: (-sum(r[2]), r[0])))
        for country in order
    ]


def rust(grouped, names, sources, total):
    rows = sum(len(entries) for _, entries in grouped)
    lines = [
        "//! The prefixes real callsigns start with, and how many real calls carry",
        "//! each separator digit and each suffix length after them, by country:",
        f"//! {rows} prefixes of {len(grouped)} countries, counted from {total} calls.",
        "//!",
        f"//! Generated by `scripts/callsign-prefixes.py {' '.join(sources)}`.",
        "//! Run it again rather than editing this by hand.",
        "",
        "use super::{Prefix, area, p};",
        "",
        "#[rustfmt::skip]",
        "pub(super) const PREFIXES: &[Prefix] = &[",
    ]
    for country, entries in grouped:
        lines.append(f"    // {names[country]}")
        make = "area" if country in AREA_MARKING else "p"
        for prefix, digits, lengths in entries:
            lines.append(
                f'    {make}("{prefix}", "{country}", '
                f"[{', '.join(map(str, digits))}], [{', '.join(map(str, lengths))}]),"
            )
    lines.append("];")
    return "\n".join(lines) + "\n"


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__.strip().splitlines()[-1])
    names, prefixes, exact = read_cty(sys.argv[1])
    calls = set()
    for path in sys.argv[2:]:
        calls |= read_calls(path)
    calls = {call for call in calls if re.fullmatch(r"[A-Z0-9]+", call)}
    grouped = table(count(calls, prefixes, exact))
    sources = [Path(sys.argv[1]).name] + [Path(path).name for path in sys.argv[2:]]
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(rust(grouped, names, sources, len(calls)))
    print(f"{OUTPUT.relative_to(ROOT)}: {sum(len(e) for _, e in grouped)} prefixes")


if __name__ == "__main__":
    main()
