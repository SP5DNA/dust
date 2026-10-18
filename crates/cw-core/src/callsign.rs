//! Realistic amateur callsigns, drawn the way real ones are built.
//!
//! A callsign is `prefix + separator digit + suffix`: `W1AW`, `DL1ABC`, `9A1AA`.
//! The prefix says which country issued it, and so does what may follow it: a
//! country hands out only some of the ten digits, and suffixes of only some
//! lengths. Spain's `EA` is followed by 1 to 5 and 7, because `EA6`, `EA8` and
//! `EA9` are the Balearics, the Canaries, and Ceuta and Melilla; Russia's lone
//! `R` by any digit, `R1` to `R7` in Europe and `R8`, `R9` and `R0` in Asia;
//! the US `AA` to `AK` blocks by one or two letters, never three.
//!
//! So the table in `callsign/prefixes.rs` is counted from real calls, by
//! `scripts/callsign-prefixes.py`: Super Check Partial's list of the calls
//! active in contests, and Tlf's. It holds every prefix with enough calls behind
//! it, the country cty.dat places those calls in, and how many of them have
//! each separator digit and each suffix length. A call is drawn in those
//! proportions — a prefix as often as real calls carry it, then a digit and a
//! suffix length as often as that prefix's calls have them, then the letters
//! — so every call is one its country could have issued, and a band of them
//! sounds like a real one. Only the counts are kept, never a call, so the
//! crate stays pure and the output endless.
//!
//! Difficulty is the *shape* of the call, not the characters in it: even `W1AW`
//! needs four different characters, so there is no Koch-style ladder to climb
//! here. [`CALLSIGN_TIER_MAX`] tiers add suffixes of every length, then
//! two-letter prefixes, prefixes with a digit in them, portable operation
//! (`G3XYZ/P`), and last a station operating abroad (`DL/W1AW`). Tiers are
//! cumulative, so moving up never takes practice away — a high tier sounds
//! like a real band.

mod prefixes;

use crate::rng::{Rng, SplitMix64};
use prefixes::PREFIXES;

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

/// The longest suffix in the table. Longer ones are special-event calls
/// (`YL0YOTA`), which it leaves out.
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

/// One prefix of one country, and how its real calls go on after it.
#[derive(Clone, Copy, Debug)]
struct Prefix {
    prefix: &'static str,
    /// The DXCC entity, by its primary prefix: `K`, `DL`, `UA9`. Countries
    /// can share a prefix — `UA3ABC` is European Russia and `UA9ABC` Asiatic
    /// — so each has an entry of its own.
    country: &'static str,
    /// How many real calls have each separator digit, `0` to `9`. A digit no
    /// call has is one the country does not hand out after this prefix.
    digits: [u32; 10],
    /// How many have a suffix of one, two and three letters.
    lengths: [u32; SUFFIX_MAX],
    /// The country's own operators mark where they are with a call-area
    /// digit — `W1AW/4`, `JA1ABC/3` — rather than only `/P` or `/M`.
    area_marker: bool,
}

/// A prefix of `country`, and how many of its calls have each separator digit
/// and each suffix length.
const fn p(
    prefix: &'static str,
    country: &'static str,
    digits: [u32; 10],
    lengths: [u32; SUFFIX_MAX],
) -> Prefix {
    Prefix {
        prefix,
        country,
        digits,
        lengths,
        area_marker: false,
    }
}

/// The same, for a country whose operators mark a call area after a slash.
const fn area(
    prefix: &'static str,
    country: &'static str,
    digits: [u32; 10],
    lengths: [u32; SUFFIX_MAX],
) -> Prefix {
    Prefix {
        prefix,
        country,
        digits,
        lengths,
        area_marker: true,
    }
}

impl Prefix {
    fn shape(&self) -> PrefixShape {
        if self.prefix.chars().any(|c| c.is_ascii_digit()) {
            WithDigit
        } else if self.prefix.chars().count() == 1 {
            Letter
        } else {
            LetterLetter
        }
    }

    /// How many real calls it has with a suffix `len` letters long.
    fn calls_of_length(&self, len: usize) -> u32 {
        len.checked_sub(1)
            .and_then(|index| self.lengths.get(index))
            .copied()
            .unwrap_or(0)
    }

    /// The suffix lengths `tier` allows, each with how many real calls of
    /// this prefix have it.
    fn lengths_in(&self, tier: &Tier) -> Vec<(usize, f64)> {
        (tier.suffix_min..=tier.suffix_max.min(SUFFIX_MAX))
            .map(|len| (len, f64::from(self.calls_of_length(len))))
            .filter(|(_, calls)| *calls > 0.0)
            .collect()
    }
}

/// Where a station operating abroad most often is, as the prefix it signs
/// first and that country's DXCC entity: `DL/W1AW`, `EA8/G3XYZ`. Mostly CEPT
/// countries, whose licences travel.
const HOSTS: &[(&str, &str, f64)] = &[
    ("DL", "DL", 3.0),
    ("EA", "EA", 2.0),
    ("EA8", "EA8", 1.5),
    ("F", "F", 2.0),
    ("G", "G", 1.2),
    ("GM", "GM", 0.6),
    ("I", "I", 1.5),
    ("OE", "OE", 1.0),
    ("HB9", "HB", 1.0),
    ("PA", "PA", 1.0),
    ("ON", "ON", 0.8),
    ("OH", "OH", 0.8),
    ("SM", "SM", 0.8),
    ("LA", "LA", 0.6),
    ("OZ", "OZ", 0.8),
    ("SP", "SP", 1.0),
    ("OK", "OK", 0.8),
    ("9A", "9A", 1.0),
    ("S5", "S5", 0.5),
    ("SV", "SV", 0.8),
    ("SV9", "SV9", 0.5),
    ("CT", "CT", 0.8),
    ("CT3", "CT3", 0.5),
    ("EI", "EI", 0.6),
    ("TF", "TF", 0.3),
];

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

/// A prefix of a shape the tier allows, as often as real calls carry it with
/// a suffix of a length the tier allows.
fn pick_prefix(tier: &Tier, rng: &mut impl Rng) -> &'static Prefix {
    let usable: Vec<(&'static Prefix, f64)> = PREFIXES
        .iter()
        .filter(|prefix| tier.shapes.contains(&prefix.shape()))
        .map(|prefix| {
            let calls: f64 = prefix.lengths_in(tier).iter().map(|(_, calls)| calls).sum();
            (prefix, calls)
        })
        .filter(|(_, calls)| *calls > 0.0)
        .collect();
    // Every tier has US calls to draw, so this is only ever a fallback.
    pick_weighted(&usable, rng).copied().unwrap_or(&PREFIXES[0])
}

/// One of the digits real calls have after `prefix`, as often as they have it.
fn separator(prefix: &Prefix, rng: &mut impl Rng) -> char {
    let digits: Vec<(char, f64)> = ('0'..='9')
        .zip(prefix.digits)
        .map(|(digit, calls)| (digit, f64::from(calls)))
        .collect();
    pick_weighted(&digits, rng).copied().unwrap_or('1')
}

/// A suffix length both the tier and the country allow, as often as real
/// calls of the prefix have it.
fn suffix_len(tier: &Tier, prefix: &Prefix, rng: &mut impl Rng) -> usize {
    pick_weighted(&prefix.lengths_in(tier), rng)
        .copied()
        .unwrap_or(tier.suffix_min.max(1))
}

/// One realistic callsign at this tier.
pub fn generate_callsign(tier: u32, rng: &mut impl Rng) -> String {
    let tier = tier_at(tier);
    let prefix = pick_prefix(tier, rng);
    let mut call = prefix.prefix.to_string();
    call.push(separator(prefix, rng));
    for _ in 0..suffix_len(tier, prefix, rng) {
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

    /// The countries whose real calls include `prefix`, then `digit`, then a
    /// suffix `len` letters long.
    fn issuing(prefix: &str, digit: char, len: usize) -> Vec<&'static Prefix> {
        let Some(digit) = digit.to_digit(10) else {
            return Vec::new();
        };
        PREFIXES
            .iter()
            .filter(|entry| {
                entry.prefix == prefix
                    && entry.digits[digit as usize] > 0
                    && entry.calls_of_length(len) > 0
            })
            .collect()
    }

    /// Every call is one its country could have issued: real calls have its
    /// prefix, then its digit, then a suffix of its length; a station abroad
    /// is not abroad at home; and only a country that marks call areas signs
    /// one after the slash.
    #[test]
    fn every_call_is_one_its_country_could_issue() {
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            for call in calls(tier, 2_000) {
                let parts = parse_callsign(&call).expect("a callsign");
                let len = parts.suffix.chars().count();
                let countries = issuing(parts.prefix, parts.separator, len);
                assert!(
                    !countries.is_empty(),
                    "{call:?}: no country issues {}{} with a {len}-letter suffix",
                    parts.prefix,
                    parts.separator
                );
                if let Some(host) = parts.host {
                    let (_, host_country, _) = HOSTS
                        .iter()
                        .find(|(prefix, _, _)| *prefix == host)
                        .unwrap_or_else(|| panic!("{call:?}: {host} is not a host"));
                    assert!(
                        countries
                            .iter()
                            .any(|country| country.country != *host_country),
                        "{call:?} is abroad at home"
                    );
                }
                if let Some(marker) = parts.portable
                    && marker.chars().all(|c| c.is_ascii_digit())
                {
                    assert!(
                        countries.iter().any(|country| country.area_marker),
                        "{call:?}: {} marks no call area",
                        parts.prefix
                    );
                }
            }
        }
    }

    /// The table says what the regulators say, because real calls do.
    #[test]
    fn the_table_knows_what_countries_issue() {
        // Russia's lone R takes every digit, R1 to R7 in Europe and R8, R9
        // and R0 in Asia, and two or three letters after it.
        for digit in '0'..='9' {
            assert!(!issuing("R", digit, 2).is_empty(), "R{digit}");
            assert!(!issuing("R", digit, 3).is_empty(), "R{digit}");
        }
        for (digit, country) in [('3', "UA"), ('9', "UA9"), ('0', "UA9")] {
            assert!(
                issuing("R", digit, 2)
                    .iter()
                    .any(|entry| entry.country == country),
                "R{digit} is {country}"
            );
        }
        // There is no lone U: UA, UR and US are prefixes, U is not.
        assert!(PREFIXES.iter().all(|entry| entry.prefix != "U"));
        // EA6, EA8 and EA9 are the Balearics, the Canaries, and Ceuta and
        // Melilla, not Spain.
        for digit in ['6', '8', '9'] {
            let countries = issuing("EA", digit, 2);
            assert!(!countries.is_empty(), "EA{digit}");
            assert!(
                countries.iter().all(|entry| entry.country != "EA"),
                "EA{digit} is not Spain"
            );
        }
        // The US AA to AK blocks are 2x1 and 2x2 calls: AA1AA, never AA1AAA.
        for block in ["AA", "AB", "AC", "AD", "AE", "AF", "AG", "AI", "AJ", "AK"] {
            for digit in '0'..='9' {
                assert!(issuing(block, digit, 3).is_empty(), "{block}{digit}");
            }
        }
        // Hawaii is KH6, and Alaska KL7.
        for (prefix, digit, country) in [("KH", '6', "KH6"), ("KL", '7', "KL")] {
            let countries = issuing(prefix, digit, 2);
            assert!(
                !countries.is_empty() && countries.iter().all(|entry| entry.country == country),
                "{prefix}{digit} is {country}"
            );
        }
    }

    #[test]
    fn the_prefix_table_is_well_formed() {
        assert!(!PREFIXES.is_empty());
        for entry in PREFIXES {
            let prefix = entry.prefix;
            let letters = prefix.chars().filter(char::is_ascii_uppercase).count();
            let digits = prefix.chars().filter(char::is_ascii_digit).count();
            let expected = match entry.shape() {
                Letter => (1, 0),
                LetterLetter => (2, 0),
                WithDigit => (1, 1),
            };
            assert_eq!(
                (letters, digits),
                expected,
                "{prefix} is not a {:?}",
                entry.shape()
            );
            for ch in prefix.chars() {
                assert!(morse_for(ch).is_some(), "{prefix} cannot be sent");
            }
            assert!(entry.digits.iter().any(|&calls| calls > 0), "{prefix}");
            assert!(entry.lengths.iter().any(|&calls| calls > 0), "{prefix}");
            assert!(!entry.country.is_empty(), "{prefix}");
        }
        // A prefix listed twice for one country would quietly double its
        // share of the band.
        let mut seen: Vec<(&str, &str)> = PREFIXES
            .iter()
            .map(|entry| (entry.prefix, entry.country))
            .collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "a prefix is in the table twice");
        // Hosts name their country as the table does, or a station could be
        // abroad at home.
        for (host, country, weight) in HOSTS {
            assert!(*weight > 0.0, "{host}");
            assert!(host.chars().all(|c| morse_for(c).is_some()), "{host}");
            assert!(
                PREFIXES.iter().any(|entry| entry.country == *country),
                "{host}: no prefix of {country}"
            );
        }
        // Every tier has something to draw.
        for tier in CALLSIGN_TIER_MIN..=CALLSIGN_TIER_MAX {
            let spec = tier_at(tier);
            assert!(
                PREFIXES
                    .iter()
                    .any(|entry| spec.shapes.contains(&entry.shape())
                        && !entry.lengths_in(spec).is_empty()),
                "tier {tier} has no prefix"
            );
        }
    }

    /// Real calls are mostly American, and so is the band: a prefix comes up
    /// as often as real calls carry it.
    #[test]
    fn prefixes_come_up_as_often_as_real_calls_carry_them() {
        let top = calls(CALLSIGN_TIER_MAX, 4_000);
        let share = |country: &str| {
            let hits = top
                .iter()
                .filter_map(|call| parse_callsign(call))
                .filter(|parts| {
                    let len = parts.suffix.chars().count();
                    issuing(parts.prefix, parts.separator, len)
                        .iter()
                        .any(|entry| entry.country == country)
                })
                .count();
            hits as f64 / top.len() as f64
        };
        let us = share("K");
        assert!((0.2..0.5).contains(&us), "{us:.2} of the band is American");
        assert!(share("DL") > share("9A"), "more Germans than Croats");
        assert!(share("9A") > 0.0, "some Croats");
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
