//! The operator at the other end.

use crate::rng::Rng;
use crate::settings::TrainingSettings;

/// The operator at the other end: one pitch, one fist, one signal strength.
///
/// Drawn per station rather than per send, because a station that repeats its
/// call does not change frequency, speed or strength between repeats. Leaving
/// this to whenever the planner happened to reach for the RNG meant three
/// sends of one group arrived as three different operators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StationVoice {
    pub tone_hz: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    pub volume: f64,
    /// How heavy this operator's fist is: how much of each element is key-down
    /// against the gap after it. One is a keyer. Above one the elements run
    /// long and crowd the spaces, below it they are clipped and airy.
    pub weight: f64,
    /// How long their dahs run against their dits. Three is the book; real
    /// fists sit anywhere either side of it, and it is the first thing you
    /// notice about somebody's sending.
    pub dash_ratio: f64,
}

/// How far a fist can stray from a keyer's at the top of the setting.
const WEIGHT_SPREAD: f64 = 0.18;
const DASH_RATIO_SPREAD: f64 = 0.45;

/// Tune in a station, within whatever range the settings allow.
pub fn resolve_station(settings: &TrainingSettings, rng: &mut impl Rng) -> StationVoice {
    let char_wpm = resolve_char_wpm(settings, rng);
    // Every station is somebody, and on the air most of them are not keyers.
    // Drawn once per station and carried, so a repeat is the same operator
    // sending again rather than a different one with the same call.
    let spread = settings.playback.fist_variation.clamp(0.0, 1.0);
    // Tied means no Farnsworth spacing: the gaps go at the character speed.
    // Drawn again from the copied range they would not, about half the time.
    let effective_wpm = if settings.playback.link_char_to_effective {
        char_wpm
    } else {
        resolve_effective_wpm(settings, char_wpm, rng)
    };
    StationVoice {
        tone_hz: resolve_tone_hz(settings, rng),
        char_wpm,
        effective_wpm,
        volume: resolve_volume(settings, rng),
        weight: 1.0 + spread * rng.pick_in_range(-WEIGHT_SPREAD, WEIGHT_SPREAD),
        dash_ratio: 3.0 + spread * rng.pick_in_range(-DASH_RATIO_SPREAD, DASH_RATIO_SPREAD),
    }
}

fn resolve_char_wpm(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    rng.pick_in_range(
        settings.playback.char_wpm_min.max(1.0),
        settings.playback.char_wpm_max.max(1.0),
    )
    .max(1.0)
}

fn resolve_effective_wpm(settings: &TrainingSettings, char_wpm: f64, rng: &mut impl Rng) -> f64 {
    let sampled = rng
        .pick_in_range(
            settings.playback.effective_wpm_min.max(1.0),
            settings.playback.effective_wpm_max.max(1.0),
        )
        .max(1.0);
    sampled.min(char_wpm)
}

fn resolve_volume(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let range = settings.volume_range();
    rng.pick_in_range(*range.start(), *range.end())
}

fn resolve_tone_hz(settings: &TrainingSettings, rng: &mut impl Rng) -> f64 {
    let range = settings.side_tone_range();
    rng.pick_in_range_inclusive_int(*range.start(), *range.end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::FastrandRng;

    fn fist(variation: f64) -> TrainingSettings {
        let mut s = TrainingSettings::default();
        s.playback.fist_variation = variation;
        s.band.side_tone_min = 600.0;
        s.band.side_tone_max = 600.0;
        s.clamp()
    }

    /// Tied is the default, and tied means no Farnsworth spacing at all: the
    /// station sends its gaps at the speed it sends its characters. Drawing
    /// the effective speed separately from the same range stretched the gaps
    /// on about half the groups.
    #[test]
    fn a_tied_effective_speed_is_the_character_speed() {
        let settings = TrainingSettings::default().clamp();
        assert!(settings.playback.link_char_to_effective);
        assert!(settings.playback.char_wpm_min < settings.playback.char_wpm_max);
        let mut rng = FastrandRng(11);
        for _ in 0..2_000 {
            let voice = resolve_station(&settings, &mut rng);
            assert_eq!(voice.effective_wpm, voice.char_wpm);
        }
    }

    /// Turned off, every station is a keyer — which is what the trainer was
    /// before there was a setting, and what somebody learning their first
    /// characters should be able to get back.
    #[test]
    fn at_zero_every_station_is_a_keyer() {
        let s = fist(0.0);
        for seed in 0..100u64 {
            let voice = resolve_station(&s, &mut FastrandRng(seed * 31 + 7));
            assert!((voice.weight - 1.0).abs() < 1e-9);
            assert!((voice.dash_ratio - 3.0).abs() < 1e-9);
        }
    }

    /// Turned up, they are people: no two send quite alike.
    #[test]
    fn every_operator_has_their_own_fist() {
        let s = fist(1.0);
        let fists: Vec<(f64, f64)> = (0..100u64)
            .map(|seed| {
                let v = resolve_station(&s, &mut FastrandRng(seed * 2_654_435_761 + 11));
                (v.weight, v.dash_ratio)
            })
            .collect();
        let heaviest = fists.iter().map(|f| f.0).fold(f64::MIN, f64::max);
        let lightest = fists.iter().map(|f| f.0).fold(f64::MAX, f64::min);
        assert!(
            heaviest - lightest > 0.1,
            "every fist came out the same weight: {lightest:.3}..{heaviest:.3}"
        );
        let longest = fists.iter().map(|f| f.1).fold(f64::MIN, f64::max);
        let shortest = fists.iter().map(|f| f.1).fold(f64::MAX, f64::min);
        assert!(
            longest - shortest > 0.3,
            "every dah was the same length: {shortest:.2}..{longest:.2}"
        );
    }
}
