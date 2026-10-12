//! What both players share about how a change sounds: how long a stop and a
//! band change take, the shapes they take, and the range the paddle's
//! sidetone plays in. Kept in one place so the desktop and the browser cannot
//! drift apart — values that had to match used to be copied by hand.

use cw_core::{SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ};

/// How long a stopped send takes to reach silence.
///
/// Matched to the keying envelope's own rise time: a trainer this careful
/// about click-free keying should not end a session with a click. Short
/// enough that "stop" still means stop.
pub const SEND_RELEASE_SEC: f64 = 0.008;

/// How long one band takes to hand over to the next when the settings
/// change. Long enough to hide the seam, short enough that the change is
/// immediate.
pub const BAND_FADE_SEC: f64 = 0.02;

/// The release shape — a raised cosine, the same family the keying envelope
/// uses, so a cut send tails off like the end of a dit rather than a
/// fade-out. Full at `progress` 0, silent at 1.
#[cfg(any(feature = "native-audio", test))]
pub fn release_gain(progress: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    0.5 * (1.0 + (std::f64::consts::PI * p).cos())
}

/// The gain of a band fading in, `progress` of the way through: the rising
/// half of an equal-power crossfade, whose falling half is the same curve
/// read backwards.
///
/// Two bands are two unrelated noises, so their powers add, not their
/// amplitudes. Gains whose squares sum to one keep the hiss level through
/// the change; a straight-line crossfade sags 3 dB in the middle of it.
pub fn fade_in_gain(progress: f64) -> f64 {
    (std::f64::consts::FRAC_PI_2 * progress.clamp(0.0, 1.0)).sin()
}

/// [`fade_in_gain`] as `points` values from start to end, for a browser gain
/// to play as a curve: rising, or falling from `from` to silence.
#[cfg(any(feature = "web", test))]
pub fn fade_curve(points: usize, rising: bool, from: f64) -> Vec<f32> {
    let points = points.max(2);
    let last = (points - 1) as f64;
    (0..points)
        .map(|i| {
            let progress = i as f64 / last;
            let gain = if rising {
                fade_in_gain(progress)
            } else {
                from * fade_in_gain(1.0 - progress)
            };
            gain as f32
        })
        .collect()
}

/// What the paddle's sidetone is asked to play, held to what either player
/// will play: a pitch inside the side-tone range the settings allow, and a
/// level no louder than full scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneRequest {
    pub on: bool,
    pub hz: f32,
    pub gain: f32,
}

impl ToneRequest {
    pub fn new(on: bool, frequency_hz: f64, gain: f64) -> Self {
        let hz = if frequency_hz.is_finite() {
            frequency_hz.clamp(SIDE_TONE_MIN_HZ, SIDE_TONE_MAX_HZ)
        } else {
            SIDE_TONE_MIN_HZ
        };
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            on,
            hz: hz as f32,
            gain: gain as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_shape_runs_from_full_to_silent() {
        assert_eq!(release_gain(0.0), 1.0);
        assert_eq!(release_gain(1.0), 0.0);
        assert!(release_gain(0.5) > 0.4 && release_gain(0.5) < 0.6);
        assert_eq!(release_gain(-1.0), 1.0, "out of range is still in range");
        assert_eq!(release_gain(2.0), 0.0);
        let mut previous = 2.0;
        for step in 0..=20 {
            let value = release_gain(f64::from(step) / 20.0);
            assert!(value < previous, "the release rose at {step}");
            previous = value;
        }
    }

    /// Two unrelated noises crossfaded keep their power all the way through:
    /// the outgoing gain is the incoming one read backwards, and their
    /// squares sum to one at every point.
    #[test]
    fn a_band_crossfade_keeps_the_power() {
        for step in 0..=100 {
            let p = f64::from(step) / 100.0;
            let (out, into) = (fade_in_gain(1.0 - p), fade_in_gain(p));
            assert!((out * out + into * into - 1.0).abs() < 1e-12, "at {p}");
        }
        assert_eq!(fade_in_gain(0.0), 0.0);
        assert_eq!(fade_in_gain(1.0), 1.0);
        assert_eq!(fade_in_gain(-1.0), 0.0);
        assert_eq!(fade_in_gain(2.0), 1.0);

        let rising = fade_curve(5, true, 1.0);
        let falling = fade_curve(5, false, 0.5);
        assert_eq!(rising.first(), Some(&0.0));
        assert_eq!(rising.last(), Some(&1.0));
        assert_eq!(falling.first(), Some(&0.5));
        assert_eq!(falling.last(), Some(&0.0));
        assert!(rising.windows(2).all(|w| w[1] > w[0]));
        assert!(falling.windows(2).all(|w| w[1] < w[0]));
        assert_eq!(fade_curve(0, true, 1.0).len(), 2);
    }

    /// One rule for the sidetone's range on both players.
    #[test]
    fn the_sidetone_is_held_to_the_side_tone_range() {
        let tone = ToneRequest::new(true, 600.0, 0.4);
        assert_eq!((tone.hz, tone.gain), (600.0, 0.4));
        assert_eq!(
            ToneRequest::new(true, 20.0, 2.0).hz,
            SIDE_TONE_MIN_HZ as f32
        );
        assert_eq!(ToneRequest::new(true, 9e9, 2.0).gain, 1.0);
        assert_eq!(
            ToneRequest::new(true, 5_000.0, 0.1).hz,
            SIDE_TONE_MAX_HZ as f32
        );
        let nonsense = ToneRequest::new(true, f64::NAN, f64::NAN);
        assert!(nonsense.hz.is_finite() && nonsense.gain == 0.0);
    }
}
