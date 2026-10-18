//! Realistic amateur callsigns, built from their structure rather than read from
//! a list.
//!
//! A callsign is `prefix + separator digit + suffix`: `W1AW`, `DL1ABC`, `9A1AA`.
//! The prefix says which country issued it, and so does what may follow it: a
//! country hands out only some of the ten digits, and suffixes of only some
//! lengths. A Briton is `G0` to `G8`, never `G9`; a Croatian is `9A` and then any
//! digit; a Frenchman has `F1`, `F4`, `F5`, `F6` or `F8`, and three letters. So
//! each prefix below carries the digits and the suffix lengths its country
//! issues, and a call is drawn from those alone — every call is one its country
//! could have issued. The prefixes are weighted by how much of the band they
//! are: US-heavy, then Europe and Japan, then everywhere else. Generating from
//! structure rather than a callsign database keeps this crate pure and the
//! output endless.
//!
//! Difficulty is the *shape* of the call, not the characters in it: even `W1AW`
//! needs four different characters, so there is no Koch-style ladder to climb
//! here. [`CALLSIGN_TIER_MAX`] tiers add suffixes of every length, then
//! two-letter prefixes, prefixes with a digit in them, portable operation
//! (`G3XYZ/P`), and last a station operating abroad (`DL/W1AW`). Tiers are
//! cumulative, so moving up never takes practice away — a high tier sounds
//! like a real band.

use crate::rng::{Rng, SplitMix64};

pub const CALLSIGN_TIER_MIN: u32 = 1;
pub const CALLSIGN_TIER_MAX: u32 = 6;

const LETTERS: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

/// How often a tier that allows portables actually sends one.
const PORTABLE_SHARE: f64 = 0.25;

/// How often a tier that allows it sends a station operating abroad.
const ABROAD_SHARE: f64 = 0.12;

/// The longest suffix any country here issues. Four letters would be a call
/// nobody holds.
pub(crate) const SUFFIX_MAX: usize = 3;

/// What a callsign's prefix looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrefixShape {
    /// One letter: `W1AW`, `G3XYZ`.
    Letter,
    /// Two letters: `DL1ABC`, `VE3AB`.
    LetterLetter,
    /// A digit in the prefix, either way round: `9A1AA`, `S51AB`.
    WithDigit,
}

/// Shapes are not equally common on the air, so a tier that allows the exotic
/// ones still sends mostly ordinary calls.
fn shape_weight(shape: PrefixShape) -> f64 {
    match shape {
        PrefixShape::Letter => 5.0,
        PrefixShape::LetterLetter => 4.0,
        PrefixShape::WithDigit => 1.0,
    }
}

struct Tier {
    shapes: &'static [PrefixShape],
    suffix_min: usize,
    suffix_max: usize,
    portable: bool,
    /// A station operating abroad, the host country's prefix first.
    abroad: bool,
}

use PrefixShape::{Letter, LetterLetter, WithDigit};

/// Each tier is the one before it plus something new.
const TIERS: &[Tier] = &[
    // 1 — the shape everyone knows: one letter, a digit, two letters.
    Tier {
        shapes: &[Letter],
        suffix_min: 2,
        suffix_max: 2,
        portable: false,
        abroad: false,
    },
    // 2 — the suffix stops being a fixed length.
    Tier {
        shapes: &[Letter],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
        abroad: false,
    },
    // 3 — two-letter prefixes, so the call can come from anywhere.
    Tier {
        shapes: &[Letter, LetterLetter],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
        abroad: false,
    },
    // 4 — prefixes with a digit in them, which break the rhythm.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 3,
        portable: false,
        abroad: false,
    },
    // 5 — portable and mobile operation.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 3,
        portable: true,
        abroad: false,
    },
    // 6 — stations operating abroad, under the host country's prefix.
    Tier {
        shapes: &[Letter, LetterLetter, WithDigit],
        suffix_min: 1,
        suffix_max: 3,
        portable: true,
        abroad: true,
    },
];

/// One prefix, and what its country issues after it.
#[derive(Clone, Copy, Debug)]
struct Prefix {
    prefix: &'static str,
    /// The separator digits the country hands out with this prefix.
    digits: &'static str,
    /// The shortest and longest suffix it hands out.
    suffix: (usize, usize),
    /// Which country it is, so a station abroad is never abroad at home.
    country: &'static str,
    /// The country's own operators mark where they are with a call-area
    /// digit — `W1AW/4`, `JA1ABC/3` — rather than only `/P` or `/M`.
    area_marker: bool,
    weight: f64,
}

/// A prefix of `country`, followed by one of `digits` and a suffix
/// `suffix.0` to `suffix.1` letters long.
const fn p(
    prefix: &'static str,
    digits: &'static str,
    suffix: (usize, usize),
    country: &'static str,
    weight: f64,
) -> Prefix {
    Prefix {
        prefix,
        digits,
        suffix,
        country,
        area_marker: false,
        weight,
    }
}

/// The same, for a country whose operators mark a call area after a slash.
const fn area(
    prefix: &'static str,
    digits: &'static str,
    suffix: (usize, usize),
    country: &'static str,
    weight: f64,
) -> Prefix {
    Prefix {
        prefix,
        digits,
        suffix,
        country,
        area_marker: true,
        weight,
    }
}

const ALL_DIGITS: &str = "0123456789";
const ONE_TO_NINE: &str = "123456789";

/// Single-letter prefixes actually in use, weighted by how much of the band
/// they are. Only countries that issue a lone letter: Russia's `R` and `U`
/// start blocks — `RA`, `UA`, Ukraine's `UR` — and stand alone only on special
/// calls, so they are two-letter prefixes below.
const SINGLE_LETTER: &[Prefix] = &[
    // USA: any call area, 1x1 special events to 1x3.
    area("W", ALL_DIGITS, (1, 3), "US", 9.0),
    area("K", ALL_DIGITS, (1, 3), "US", 9.0),
    area("N", ALL_DIGITS, (1, 3), "US", 7.0),
    // UK: G0 to G8; M0, M1, M3, M5, M6 and M7, with three letters.
    p("G", "012345678", (2, 3), "UK", 4.0),
    p("M", "013567", (3, 3), "UK", 2.0),
    p("F", "14568", (2, 3), "FR", 2.5),
    p("I", ALL_DIGITS, (2, 3), "IT", 2.5),
];

/// The US two-letter blocks, which are most of the two-letter calls you hear:
/// `AA` to `AK`, and `K`, `N` and `W` with any second letter but the `H`, `L`
/// and `P` of Hawaii, Alaska and Puerto Rico, which have prefixes of their
/// own below.
const US_TWO_LETTER: &[(char, &str)] = &[
    ('A', "ABCDEFGHIJK"),
    ('K', "ABCDEFGIJKMNOQRSTUVWXYZ"),
    ('N', "ABCDEFGIJKMNOQRSTUVWXYZ"),
    ('W', "ABCDEFGIJKMNOQRSTUVWXYZ"),
];

/// What a US two-letter block issues after it: any call area, and 2x1 to 2x3.
const US_TWO_LETTER_CALL: Prefix = area("", ALL_DIGITS, (1, 3), "US", 0.0);

/// How much of the two-letter traffic is American.
const US_TWO_LETTER_SHARE: f64 = 0.45;

/// Two-letter prefixes from outside the US blocks.
const TWO_LETTER: &[Prefix] = &[
    // Hawaii, Alaska and Puerto Rico: their own prefix, their own call area.
    p("KH", "6", (1, 3), "KH6", 0.8),
    p("NH", "6", (1, 3), "KH6", 0.3),
    p("WH", "6", (1, 3), "KH6", 0.3),
    p("KL", "7", (1, 3), "KL7", 0.8),
    p("AL", "7", (1, 3), "KL7", 0.3),
    p("WL", "7", (1, 3), "KL7", 0.3),
    p("KP", "34", (1, 3), "KP4", 0.8),
    p("NP", "4", (1, 3), "KP4", 0.3),
    p("WP", "4", (1, 3), "KP4", 0.3),
    // Germany
    p("DL", ALL_DIGITS, (2, 3), "DE", 9.0),
    p("DK", ALL_DIGITS, (2, 3), "DE", 4.0),
    p("DJ", ALL_DIGITS, (2, 3), "DE", 3.0),
    p("DF", ALL_DIGITS, (2, 3), "DE", 2.0),
    p("DG", ALL_DIGITS, (3, 3), "DE", 1.5),
    // Japan
    area("JA", ALL_DIGITS, (2, 3), "JA", 8.0),
    area("JE", ALL_DIGITS, (3, 3), "JA", 2.5),
    area("JH", ALL_DIGITS, (3, 3), "JA", 3.0),
    area("JR", ALL_DIGITS, (3, 3), "JA", 2.5),
    area("JF", ALL_DIGITS, (3, 3), "JA", 1.5),
    // Rest of Europe
    p("EA", ONE_TO_NINE, (2, 3), "ES", 4.0),
    p("EB", ONE_TO_NINE, (3, 3), "ES", 1.0),
    p("IK", ALL_DIGITS, (3, 3), "IT", 3.0),
    p("IZ", ALL_DIGITS, (3, 3), "IT", 2.0),
    p("IW", ALL_DIGITS, (3, 3), "IT", 1.5),
    p("PA", ALL_DIGITS, (2, 3), "NL", 3.5),
    p("PD", ALL_DIGITS, (3, 3), "NL", 1.0),
    p("ON", "345678", (2, 3), "BE", 2.5),
    p("OK", "12", (2, 3), "CZ", 3.0),
    p("OM", "02345678", (2, 3), "SK", 2.0),
    p("SP", ONE_TO_NINE, (2, 3), "PL", 3.5),
    p("SQ", ONE_TO_NINE, (3, 3), "PL", 1.5),
    p("OH", ONE_TO_NINE, (2, 3), "FI", 3.0),
    p("SM", "01234567", (2, 3), "SE", 3.0),
    p("SA", "01234567", (3, 3), "SE", 1.0),
    p("LA", ONE_TO_NINE, (2, 3), "NO", 2.5),
    p("OZ", ONE_TO_NINE, (2, 3), "DK", 2.0),
    p("HB", "9", (2, 3), "CH", 2.0),
    p("OE", ONE_TO_NINE, (2, 3), "AT", 2.5),
    p("YO", "23456789", (2, 3), "RO", 2.0),
    p("LZ", "12345", (2, 3), "BG", 1.5),
    p("SV", ONE_TO_NINE, (2, 3), "GR", 2.0),
    p("TA", ONE_TO_NINE, (2, 3), "TR", 1.5),
    p("YU", ONE_TO_NINE, (2, 3), "RS", 1.5),
    p("YT", ONE_TO_NINE, (2, 3), "RS", 1.0),
    p("CT", "1247", (2, 3), "PT", 2.0),
    p("EI", "23456789", (2, 3), "IE", 1.5),
    p("HA", ONE_TO_NINE, (2, 3), "HU", 2.0),
    p("HG", ONE_TO_NINE, (2, 3), "HU", 1.0),
    p("TF", "12345678", (2, 3), "IS", 0.6),
    p("LY", "12345", (2, 3), "LT", 1.5),
    p("YL", "23", (2, 3), "LV", 1.5),
    p("ES", "12345678", (2, 3), "EE", 1.2),
    p("EW", "12345678", (2, 3), "BY", 1.0),
    // UK regions
    p("GM", "012345678", (2, 3), "GM", 1.5),
    p("GW", "012345678", (2, 3), "GW", 1.2),
    p("GI", "012345678", (2, 3), "GI", 0.8),
    p("MM", "013567", (3, 3), "GM", 0.6),
    // Former USSR
    p("UA", "013469", (2, 3), "RU", 4.0),
    p("RA", "013469", (2, 3), "RU", 3.5),
    p("RN", "13469", (2, 3), "RU", 2.0),
    p("RK", "13469", (2, 3), "RU", 1.5),
    p("RW", "13469", (2, 3), "RU", 1.2),
    p("RZ", "13469", (2, 3), "RU", 1.0),
    p("UR", ALL_DIGITS, (2, 3), "UA", 2.5),
    p("US", ALL_DIGITS, (2, 3), "UA", 1.0),
    p("UT", ALL_DIGITS, (2, 3), "UA", 2.0),
    p("UX", ALL_DIGITS, (2, 3), "UA", 1.2),
    p("UY", ALL_DIGITS, (2, 3), "UA", 0.8),
    // Canada
    area("VE", ONE_TO_NINE, (2, 3), "CA", 4.5),
    area("VA", "237", (2, 3), "CA", 2.0),
    area("VO", "12", (2, 3), "CA", 0.8),
    // Rest of the world
    area("VK", "12345678", (2, 3), "AU", 3.0),
    p("ZL", "1234", (2, 3), "NZ", 2.0),
    p("ZS", "123456", (2, 3), "ZA", 1.2),
    p("PY", ONE_TO_NINE, (2, 3), "BR", 3.0),
    p("PP", "12345678", (3, 3), "BR", 1.0),
    p("PU", ONE_TO_NINE, (3, 3), "BR", 1.5),
    p("LU", ONE_TO_NINE, (2, 3), "AR", 2.5),
    p("CE", "12345678", (2, 3), "CL", 1.5),
    p("CX", ONE_TO_NINE, (2, 3), "UY", 0.8),
    p("HK", ONE_TO_NINE, (2, 3), "CO", 1.0),
    p("XE", "123", (2, 3), "MX", 1.5),
    p("BA", ONE_TO_NINE, (2, 2), "CN", 1.5),
    p("BY", ONE_TO_NINE, (3, 3), "CN", 1.0),
    p("BG", ONE_TO_NINE, (3, 3), "CN", 1.5),
    p("HL", "12345", (2, 3), "KR", 1.5),
    p("DS", "12345", (3, 3), "KR", 1.0),
    p("VU", "23", (2, 3), "IN", 1.5),
];

/// Prefixes carrying a digit, both ways round: `9A` and `S5` alike.
const DIGIT_PREFIX: &[Prefix] = &[
    p("9A", ONE_TO_NINE, (2, 3), "HR", 3.0),
    p("S5", ALL_DIGITS, (2, 3), "SI", 3.0),
    p("4X", "123456", (2, 3), "IL", 2.0),
    p("4Z", "45", (2, 3), "IL", 1.5),
    p("Z3", "1235", (2, 3), "MK", 1.5),
    p("E7", ALL_DIGITS, (2, 3), "BA", 1.5),
    p("T7", "07", (1, 2), "SM", 0.8),
    p("9H", "135", (2, 3), "MT", 1.2),
    p("3A", "2", (2, 3), "MC", 0.6),
    p("4O", "34", (2, 3), "ME", 0.8),
    p("4L", "145", (2, 3), "GE", 1.0),
    p("5B", "4", (2, 3), "CY", 1.0),
    p("9K", "2", (2, 3), "KW", 0.8),
    p("9V", "1", (2, 3), "SG", 0.8),
    p("9M", "268", (2, 3), "MY", 1.0),
    p("E2", "012", (2, 3), "TH", 0.8),
    p("3B", "8", (2, 3), "MU", 0.5),
    p("8P", "6", (2, 3), "BB", 0.5),
    p("7X", "2345", (2, 3), "DZ", 0.5),
    p("6Y", "5", (2, 3), "JM", 0.6),
    p("9Y", "4", (2, 3), "TT", 0.6),
];

/// Where a station operating abroad most often is, as the prefix it signs
/// first: `DL/W1AW`, `EA8/G3XYZ`. Mostly CEPT countries, whose licences
/// travel.
const HOSTS: &[(&str, &str, f64)] = &[
    ("DL", "DE", 3.0),
    ("EA", "ES", 2.0),
    ("EA8", "ES", 1.5),
    ("F", "FR", 2.0),
    ("G", "UK", 1.2),
    ("GM", "GM", 0.6),
    ("I", "IT", 1.5),
    ("OE", "AT", 1.0),
    ("HB9", "CH", 1.0),
    ("PA", "NL", 1.0),
    ("ON", "BE", 0.8),
    ("OH", "FI", 0.8),
    ("SM", "SE", 0.8),
    ("LA", "NO", 0.6),
    ("OZ", "DK", 0.8),
    ("SP", "PL", 1.0),
    ("OK", "CZ", 0.8),
    ("9A", "HR", 1.0),
    ("S5", "SI", 0.5),
    ("SV", "GR", 0.8),
    ("SV9", "GR", 0.5),
    ("CT", "PT", 0.8),
    ("CT3", "PT", 0.5),
    ("EI", "IE", 0.6),
    ("TF", "IS", 0.3),
];

/// Suffix lengths are not uniform either: two and three letters dominate.
const SUFFIX_WEIGHTS: &[(usize, f64)] = &[(1, 1.0), (2, 5.0), (3, 5.0)];

/// What follows the slash. `/P` and `/M` are much the most common.
const PORTABLE_MARKERS: &[(&str, f64)] = &[("P", 6.0), ("M", 4.0), ("QRP", 1.5), ("A", 1.0)];

/// The share of portables an area-marking country signs with a call-area
/// digit instead: `W1AW/4`.
const AREA_MARKER_SHARE: f64 = 0.35;

fn tier_at(tier: u32) -> &'static Tier {
    let index = tier.clamp(CALLSIGN_TIER_MIN, CALLSIGN_TIER_MAX) - CALLSIGN_TIER_MIN;
    &TIERS[index as usize]
}

/// Draw from a weighted table. Weights are the point of these tables, so a
/// table that somehow sums to nothing still yields its first entry rather than
/// nothing at all.
fn pick_weighted<'t, T>(table: &'t [(T, f64)], rng: &mut impl Rng) -> Option<&'t T> {
    let total: f64 = table.iter().map(|(_, weight)| weight.max(0.0)).sum();
    if total <= 0.0 {
        return table.first().map(|(item, _)| item);
    }
    let mut target = rng.f64() * total;
    for (item, weight) in table {
        target -= weight.max(0.0);
        if target <= 0.0 {
            return Some(item);
        }
    }
    table.last().map(|(item, _)| item)
}

fn pick_shape(tier: &Tier, rng: &mut impl Rng) -> PrefixShape {
    let table: Vec<(PrefixShape, f64)> = tier
        .shapes
        .iter()
        .map(|shape| (*shape, shape_weight(*shape)))
        .collect();
    pick_weighted(&table, rng).copied().unwrap_or(Letter)
}

/// Whether `prefix` issues a suffix of a length the tier allows.
fn fits(prefix: &Prefix, tier: &Tier) -> bool {
    prefix.suffix.0 <= tier.suffix_max && prefix.suffix.1 >= tier.suffix_min
}

/// A prefix of `table` the tier can use, by weight.
fn pick_prefix(table: &'static [Prefix], tier: &Tier, rng: &mut impl Rng) -> Option<Prefix> {
    let usable: Vec<(Prefix, f64)> = table
        .iter()
        .filter(|prefix| fits(prefix, tier))
        .map(|prefix| (*prefix, prefix.weight))
        .collect();
    pick_weighted(&usable, rng).copied()
}

/// A US two-letter block, and what it issues.
fn us_two_letter(rng: &mut impl Rng) -> Option<(String, Prefix)> {
    let (first, seconds) = US_TWO_LETTER.get(rng.usize_in(0, US_TWO_LETTER.len() - 1))?;
    let choices: Vec<char> = seconds.chars().collect();
    let second = choices.get(rng.usize_in(0, choices.len().saturating_sub(1)))?;
    Some((format!("{first}{second}"), US_TWO_LETTER_CALL))
}

/// The prefix a call of `shape` starts with, and what its country issues.
fn prefix_for(shape: PrefixShape, tier: &Tier, rng: &mut impl Rng) -> (String, Prefix) {
    let picked = match shape {
        Letter => pick_prefix(SINGLE_LETTER, tier, rng),
        LetterLetter => {
            if rng.f64() < US_TWO_LETTER_SHARE
                && let Some(us) = us_two_letter(rng)
            {
                return us;
            }
            pick_prefix(TWO_LETTER, tier, rng)
        }
        WithDigit => pick_prefix(DIGIT_PREFIX, tier, rng),
    };
    // Every tier's lengths are some US call's, so this is only ever a
    // fallback for a table that cannot fit.
    let prefix = picked.unwrap_or(SINGLE_LETTER[0]);
    (prefix.prefix.to_string(), prefix)
}

/// A suffix length both the tier and the country allow, by how common it is.
fn suffix_len(tier: &Tier, prefix: &Prefix, rng: &mut impl Rng) -> usize {
    let low = tier.suffix_min.max(prefix.suffix.0);
    let high = tier.suffix_max.min(prefix.suffix.1).min(SUFFIX_MAX);
    let allowed: Vec<(usize, f64)> = SUFFIX_WEIGHTS
        .iter()
        .copied()
        .filter(|(len, _)| (low..=high).contains(len))
        .collect();
    pick_weighted(&allowed, rng).copied().unwrap_or(low.max(1))
}

/// One of the digits `prefix`'s country issues.
fn separator(prefix: &Prefix, rng: &mut impl Rng) -> char {
    let digits: Vec<char> = prefix.digits.chars().collect();
    digits
        .get(rng.usize_in(0, digits.len().saturating_sub(1)))
        .copied()
        .unwrap_or('1')
}

/// One realistic callsign at this tier.
pub fn generate_callsign(tier: u32, rng: &mut impl Rng) -> String {
    let tier = tier_at(tier);
    let shape = pick_shape(tier, rng);
    let (mut call, prefix) = prefix_for(shape, tier, rng);
    call.push(separator(&prefix, rng));
    for _ in 0..suffix_len(tier, &prefix, rng) {
        call.push(LETTERS[rng.usize_in(0, LETTERS.len() - 1)]);
    }
    // One of the two at most: abroad, or portable at home.
    if tier.abroad
        && rng.f64() < ABROAD_SHARE
        && let Some(host) = pick_host(prefix.country, rng)
    {
        return format!("{host}/{call}");
    }
    if tier.portable && rng.f64() < PORTABLE_SHARE {
        call.push('/');
        if prefix.area_marker && rng.f64() < AREA_MARKER_SHARE {
            call.push(char::from_digit(rng.usize_in(0, 9) as u32, 10).unwrap_or('1'));
        } else if let Some(marker) = pick_weighted(PORTABLE_MARKERS, rng) {
            call.push_str(marker);
        }
    }
    call
}

/// A country to be operating in that is not `home`.
fn pick_host(home: &str, rng: &mut impl Rng) -> Option<&'static str> {
    let away: Vec<(&'static str, f64)> = HOSTS
        .iter()
        .filter(|(_, country, _)| *country != home)
        .map(|(prefix, _, weight)| (*prefix, *weight))
        .collect();
    pick_weighted(&away, rng).copied()
}

/// Every character a callsign at this tier can contain — what the stats and the
/// listen screen practise against.
pub fn callsign_pool(tier: u32) -> Vec<char> {
    let mut pool: Vec<char> = LETTERS.to_vec();
    pool.extend(crate::morse::DIGITS.iter().copied());
    let tier = tier_at(tier);
    if tier.portable || tier.abroad {
        pool.push('/');
    }
    pool
}

/// A few calls to show beside the tier setting. Deterministic, so the preview
/// does not churn every time the screen redraws.
pub fn tier_examples(tier: u32) -> Vec<String> {
    let mut rng = SplitMix64::new(0x51A7_C0DE_u64 ^ u64::from(tier).wrapping_mul(0x9E37_79B9));
    (0..4).map(|_| generate_callsign(tier, &mut rng)).collect()
}

/// A callsign taken apart: what the tests hold every generated call to.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CallsignParts<'a> {
    /// The country a station operating abroad signs first: the `DL` of
    /// `DL/W1AW`.
    pub host: Option<&'a str>,
    pub prefix: &'a str,
    pub separator: char,
    pub suffix: &'a str,
    pub portable: Option<&'a str>,
}

/// Split a callsign into its parts, or `None` if it is not one.
///
/// The separator is the *last* digit before the suffix, which is what makes
/// `9A1AA` split as `9A` + `1` + `AA` rather than at its leading digit. A
/// slash has a marker after the call (`G3XYZ/P`) or a host country before it
/// (`DL/W1AW`); whichever side is a whole call is the call.
#[cfg(test)]
pub(crate) fn parse_callsign(call: &str) -> Option<CallsignParts<'_>> {
    let Some((left, right)) = call.split_once('/') else {
        return parse_base(call, None, None);
    };
    let marker = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    };
    if !marker(left) || !marker(right) {
        return None;
    }
    parse_base(left, None, Some(right)).or_else(|| parse_base(right, Some(left), None))
}

#[cfg(test)]
fn parse_base<'a>(
    base: &'a str,
    host: Option<&'a str>,
    portable: Option<&'a str>,
) -> Option<CallsignParts<'a>> {
    let separator = base.rfind(|c: char| c.is_ascii_digit())?;
    let prefix = &base[..separator];
    let suffix = &base[separator + 1..];
    if prefix.is_empty() || suffix.is_empty() {
        return None;
    }
    if !prefix
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }
    if !suffix.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    Some(CallsignParts {
        host,
        prefix,
        separator: base[separator..].chars().next()?,
        suffix,
        portable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::morse::morse_for;

    fn calls(tier: u32, count: usize) -> Vec<String> {
        let mut rng = SplitMix64::new(u64::from(tier).wrapping_mul(0x2545_F491) | 1);
        (0..count)
            .map(|_| generate_callsign(tier, &mut rng))
            .collect()
    }

    #[test]
    fn every_tier_sends_something_a_radio_could_send() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            for call in calls(tier, 500) {
                let parts = parse_callsign(&call)
                    .unwrap_or_else(|| panic!("tier {tier}: {call:?} is not a callsign"));
                assert!(!parts.prefix.is_empty());
                assert!(parts.separator.is_ascii_digit());
                assert!(!parts.suffix.is_empty());
                // Every character has to be sendable, or the group is silent.
                for ch in call.chars() {
                    assert!(
                        morse_for(ch).is_some(),
                        "tier {tier}: {call:?} contains {ch:?}, which has no Morse"
                    );
                }
            }
        }
    }

    #[test]
    fn a_tier_only_sends_what_it_is_allowed_to() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let spec = tier_at(tier);
            for call in calls(tier, 500) {
                let parts = parse_callsign(&call).expect("a callsign");
                let len = parts.suffix.chars().count();
                assert!(
                    len >= spec.suffix_min && len <= spec.suffix_max,
                    "tier {tier}: {call:?} has a {len}-character suffix"
                );
                if !spec.portable {
                    assert!(
                        parts.portable.is_none(),
                        "tier {tier}: {call:?} is portable"
                    );
                }
                if !spec.abroad {
                    assert!(parts.host.is_none(), "tier {tier}: {call:?} is abroad");
                }
                if !spec.portable && !spec.abroad {
                    assert!(!call.contains('/'), "tier {tier}: {call:?} has a slash");
                }
                let has_digit_prefix = parts.prefix.chars().any(|c| c.is_ascii_digit());
                if !spec.shapes.contains(&WithDigit) {
                    assert!(
                        !has_digit_prefix,
                        "tier {tier}: {call:?} has a digit in its prefix"
                    );
                }
                if !spec.shapes.contains(&LetterLetter) && !has_digit_prefix {
                    assert_eq!(
                        parts.prefix.chars().count(),
                        1,
                        "tier {tier}: {call:?} has a long prefix"
                    );
                }
            }
        }
    }

    #[test]
    fn the_tiers_keep_what_the_ones_below_them_had() {
        // Each tier must still be able to produce the shapes below it, or moving
        // up would take practice away rather than add to it.
        for tier in (CALLSIGN_TIER_MIN + 1)..=CALLSIGN_TIER_MAX {
            let lower = tier_at(tier - 1);
            let upper = tier_at(tier);
            assert!(
                lower
                    .shapes
                    .iter()
                    .all(|shape| upper.shapes.contains(shape)),
                "tier {tier} dropped a prefix shape"
            );
            assert!(upper.suffix_min <= lower.suffix_min);
            assert!(upper.suffix_max >= lower.suffix_max);
            assert!(upper.portable || !lower.portable);
            assert!(upper.abroad || !lower.abroad);
        }
    }

    #[test]
    fn the_first_tier_is_the_shape_everybody_knows() {
        for call in calls(1, 200) {
            assert_eq!(call.chars().count(), 4, "{call:?} is not a 1x2 call");
            let parts = parse_callsign(&call).expect("a callsign");
            assert_eq!(parts.prefix.chars().count(), 1);
            assert_eq!(parts.suffix.chars().count(), 2);
        }
    }

    #[test]
    fn the_top_tiers_go_portable_and_abroad_sometimes_but_not_always() {
        let portable = calls(5, 500);
        let marked = portable.iter().filter(|call| call.contains('/')).count();
        assert!(marked > 0, "no portable calls at tier 5");
        assert!(marked < portable.len(), "every call was portable");
        let top = calls(CALLSIGN_TIER_MAX, 1_000);
        let abroad = top
            .iter()
            .filter(|call| parse_callsign(call).is_some_and(|parts| parts.host.is_some()))
            .count();
        assert!(abroad > 0, "nobody operating abroad at the top tier");
        assert!(abroad < top.len() / 4, "{abroad} of {} abroad", top.len());
    }

    #[test]
    fn the_pool_covers_everything_a_tier_can_send() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let pool = callsign_pool(tier);
            for call in calls(tier, 500) {
                for ch in call.chars() {
                    assert!(
                        pool.contains(&ch),
                        "tier {tier}: {ch:?} is outside the pool"
                    );
                }
            }
            let spec = tier_at(tier);
            assert!(pool.contains(&'/') == (spec.portable || spec.abroad));
        }
    }

    #[test]
    fn a_tier_outside_the_ladder_is_pulled_back_onto_it() {
        let mut rng = SplitMix64::new(7);
        let low = generate_callsign(0, &mut rng);
        assert_eq!(low.chars().count(), 4, "tier 0 should behave as tier 1");
        let high = generate_callsign(9_999, &mut rng);
        assert!(parse_callsign(&high).is_some());
        assert_eq!(callsign_pool(0), callsign_pool(CALLSIGN_TIER_MIN));
        assert_eq!(callsign_pool(9_999), callsign_pool(CALLSIGN_TIER_MAX));
    }

    #[test]
    fn examples_are_stable_and_belong_to_their_tier() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let first = tier_examples(tier);
            assert_eq!(first, tier_examples(tier), "the preview must not churn");
            assert_eq!(first.len(), 4);
            for call in first {
                assert!(parse_callsign(&call).is_some(), "{call:?}");
            }
        }
    }

    #[test]
    fn what_is_not_a_callsign_is_refused() {
        assert!(parse_callsign("").is_none());
        assert!(parse_callsign("W1").is_none(), "no suffix");
        assert!(parse_callsign("1AW").is_none(), "no prefix");
        assert!(parse_callsign("WAW").is_none(), "no separator");
        assert!(parse_callsign("W1A/").is_none(), "an empty portable marker");
        assert!(parse_callsign("W1A/P/M").is_none(), "two slashes");
        assert!(parse_callsign("w1aw").is_none(), "lower case");
        assert!(parse_callsign("W1A?").is_none(), "punctuation");
        let parts = parse_callsign("9A1AA").expect("a callsign");
        assert_eq!(parts.prefix, "9A");
        assert_eq!(parts.separator, '1');
        assert_eq!(parts.suffix, "AA");
        assert_eq!(parts.portable, None);
        let portable = parse_callsign("W1AW/4").expect("a callsign");
        assert_eq!(portable.prefix, "W");
        assert_eq!(portable.suffix, "AW");
        assert_eq!(portable.portable, Some("4"));
        assert_eq!(portable.host, None);
        let abroad = parse_callsign("EA8/G3XYZ").expect("a callsign");
        assert_eq!(abroad.host, Some("EA8"));
        assert_eq!(abroad.prefix, "G");
        assert_eq!(abroad.separator, '3');
        assert_eq!(abroad.suffix, "XYZ");
        assert_eq!(abroad.portable, None);
        assert!(parse_callsign("DL/W1AW").is_some_and(|parts| parts.host == Some("DL")));
    }

    /// What a country issues after `prefix`: a table's entry for it, or a
    /// US two-letter block's.
    fn issued(prefix: &str) -> Option<Prefix> {
        let listed = SINGLE_LETTER
            .iter()
            .chain(TWO_LETTER)
            .chain(DIGIT_PREFIX)
            .find(|entry| entry.prefix == prefix)
            .copied();
        listed.or_else(|| {
            let mut chars = prefix.chars();
            let (first, second) = (chars.next()?, chars.next()?);
            let block = US_TWO_LETTER.iter().find(|(lead, _)| *lead == first)?;
            (chars.next().is_none() && block.1.contains(second)).then_some(US_TWO_LETTER_CALL)
        })
    }

    /// Every call is one its country could have issued: the prefix is a real
    /// one, the digit after it is one that country hands out with it, the
    /// suffix is a length it issues, and a station abroad is not abroad at
    /// home. `G9`, `F9`, `9M1`, `NL0` and the lone `U` of `U7A` are none of
    /// those.
    #[test]
    fn every_call_is_one_its_country_could_issue() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            for call in calls(tier, 2_000) {
                let parts = parse_callsign(&call).expect("a callsign");
                let country = issued(parts.prefix)
                    .unwrap_or_else(|| panic!("{call:?}: no country issues {}", parts.prefix));
                assert!(
                    country.digits.contains(parts.separator),
                    "{call:?}: {} is not followed by {}",
                    parts.prefix,
                    parts.separator
                );
                let len = parts.suffix.chars().count();
                assert!(
                    (country.suffix.0..=country.suffix.1).contains(&len) && len <= SUFFIX_MAX,
                    "{call:?}: {} issues no {len}-letter suffix",
                    parts.prefix
                );
                if let Some(host) = parts.host {
                    let (_, host_country, _) = HOSTS
                        .iter()
                        .find(|(prefix, _, _)| *prefix == host)
                        .unwrap_or_else(|| panic!("{call:?}: {host} is not a host"));
                    assert_ne!(*host_country, country.country, "{call:?} is abroad at home");
                }
                if let Some(marker) = parts.portable
                    && marker.chars().all(|c| c.is_ascii_digit())
                {
                    assert!(
                        country.area_marker,
                        "{call:?}: {} marks no call area",
                        parts.prefix
                    );
                }
            }
        }
        for lone in ["U", "R"] {
            assert!(
                issued(lone).is_none(),
                "{lone} alone is no country's prefix"
            );
        }
    }

    #[test]
    fn the_prefix_tables_are_real_prefixes() {
        let tables: [(&[Prefix], PrefixShape); 3] = [
            (SINGLE_LETTER, Letter),
            (TWO_LETTER, LetterLetter),
            (DIGIT_PREFIX, WithDigit),
        ];
        for (table, shape) in tables {
            assert!(!table.is_empty());
            for entry in table {
                let prefix = entry.prefix;
                assert!(entry.weight > 0.0, "{prefix} has no weight");
                let letters = prefix.chars().filter(char::is_ascii_uppercase).count();
                let digits = prefix.chars().filter(char::is_ascii_digit).count();
                let expected = match shape {
                    Letter => (1, 0),
                    LetterLetter => (2, 0),
                    WithDigit => (1, 1),
                };
                assert_eq!((letters, digits), expected, "{prefix} is not a {shape:?}");
                for ch in prefix.chars() {
                    assert!(morse_for(ch).is_some(), "{prefix} cannot be sent");
                }
                assert!(!entry.digits.is_empty(), "{prefix} issues no digit");
                assert!(
                    entry.digits.chars().all(|c| c.is_ascii_digit()),
                    "{prefix}: {}",
                    entry.digits
                );
                let (low, high) = entry.suffix;
                assert!(
                    1 <= low && low <= high && high <= SUFFIX_MAX,
                    "{prefix}: {low}..{high}"
                );
                assert!(!entry.country.is_empty());
            }
        }
        // The US blocks leave Hawaii, Alaska and Puerto Rico to their own
        // prefixes and call areas.
        for (lead, seconds) in US_TWO_LETTER {
            if *lead != 'A' {
                assert!(!seconds.contains(['H', 'L', 'P']), "{lead}: {seconds}");
            }
        }
        for (host, country, weight) in HOSTS {
            assert!(*weight > 0.0 && !country.is_empty(), "{host}");
            assert!(host.chars().all(|c| morse_for(c).is_some()), "{host}");
        }
        // A prefix listed twice would quietly double its share of the band.
        let mut seen: Vec<&str> = SINGLE_LETTER
            .iter()
            .chain(TWO_LETTER)
            .chain(DIGIT_PREFIX)
            .map(|entry| entry.prefix)
            .collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "a prefix is in the tables twice");
    }

    #[test]
    fn a_weighted_table_always_yields_something() {
        let mut rng = SplitMix64::new(3);
        assert_eq!(pick_weighted(&[("only", 0.0)], &mut rng), Some(&"only"));
        assert_eq!(pick_weighted::<&str>(&[], &mut rng), None);
        // Weight decides: a thousand draws can only land on the heavy entry.
        let table = [("rare", 0.0), ("common", 1.0)];
        for _ in 0..1_000 {
            assert_eq!(pick_weighted(&table, &mut rng), Some(&"common"));
        }
    }
}
