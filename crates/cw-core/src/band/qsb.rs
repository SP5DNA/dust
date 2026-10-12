//! Fading: how much of the signal the path is letting through.

/// The weakest a fade leaves the signal. Below this a group would simply be
/// lost rather than hard to copy.
pub const QSB_MIN_GAIN: f64 = 0.25;

/// Opens the multipath sum back out to the depth a single sine would have had.
/// Four sines rarely all line up, so their sum sits nearer the middle; this
/// scales it back by the ratio of the two RMS values, and the clamp above
/// catches the rare moment they do line up — which is a fade bottoming out,
/// and sounds like one.
pub const QSB_SPREAD: f64 = 1.85;

/// How many propagation paths the fading is made of.
pub const QSB_PATHS: usize = 4;

/// The rates of the paths a fade is made of, as multiples of its own rate.
///
/// The ratios are deliberately not simple fractions. Sines at 1, 2 and 3
/// times a rate come back round together and you are back to a pattern; at
/// these they do not, so the fading wanders and never repeats. The band's own
/// stations fade on the first three.
pub(super) const MULTIPATH_RATES: [f64; QSB_PATHS] = [1.000, 0.633, 1.471, 2.309];

/// Each path's rate, as a multiple of the rate setting
/// ([`MULTIPATH_RATES`]), and how much of the fading it carries.
///
/// Weights sum to one, so the swing still spans -1..=1 at its widest and the
/// depth setting keeps meaning what it meant.
pub fn qsb_path(index: usize) -> (f64, f64) {
    const WEIGHTS: [f64; QSB_PATHS] = [0.40, 0.28, 0.20, 0.12];
    let index = index.min(QSB_PATHS - 1);
    (MULTIPATH_RATES[index], WEIGHTS[index])
}

/// The QSB envelope: how much of the signal is getting through at `t_sec`.
///
/// Fading is not a tremolo. A single sine gives you a fade you can set your
/// watch by, which is the one thing real QSB never is — what actually happens
/// is several paths arriving with slightly different Doppler and beating
/// against each other, so the signal wanders down and back up on no schedule.
/// That is a sum of sinusoids at close but unrelated rates, which is the
/// classical multipath model and is also the only shape the browser's
/// oscillators can build, so both backends fade the same way.
///
/// `QSB_SPREAD` is there because a sum of four sines spends most of its time
/// nearer the middle than one sine does; without it, turning multipath on
/// would quietly halve the depth of every fade.
pub fn qsb_gain_at(t_sec: f64, enabled: bool, depth: f64, rate_hz: f64) -> f32 {
    if !enabled || depth <= 0.0 {
        return 1.0;
    }
    let depth = depth.clamp(0.0, 1.0);
    let rate = rate_hz.clamp(0.03, 1.5);
    let gain_range = depth.min(1.0 - QSB_MIN_GAIN);
    let base = 1.0 - gain_range / 2.0;
    let half = gain_range / 2.0;
    let swing: f64 = (0..QSB_PATHS)
        .map(|i| {
            let (multiple, weight) = qsb_path(i);
            weight * (std::f64::consts::TAU * rate * multiple * t_sec).sin()
        })
        .sum();
    let swing = (swing * QSB_SPREAD).clamp(-1.0, 1.0);
    (base + half * swing) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qsb_modulates_amplitude_without_reaching_silence() {
        let gains: Vec<f32> = (0..48_000)
            .map(|i| qsb_gain_at(f64::from(i) / 48_000.0, true, 1.0, 1.0))
            .collect();
        let min = gains.iter().copied().fold(f32::MAX, f32::min);
        let max = gains.iter().copied().fold(f32::MIN, f32::max);
        assert!(min < 0.8 && max > 0.9);
        assert!(
            min >= QSB_MIN_GAIN as f32,
            "a fade should not reach silence"
        );
    }

    #[test]
    fn qsb_is_unity_when_off() {
        assert_eq!(qsb_gain_at(0.25, false, 1.0, 1.0), 1.0);
        assert_eq!(qsb_gain_at(0.25, true, 0.0, 1.0), 1.0);
        let trough = qsb_gain_at(0.75, true, 1.0, 1.0);
        assert!(trough > 0.2 && trough < 0.4);
    }

    /// Real QSB is not a tremolo: several paths beating against each other
    /// wander and never come back round. Each cycle of the base rate has to
    /// differ from the first; a sine would differ by zero.
    #[test]
    fn fading_never_comes_back_round() {
        let (depth, rate, dt) = (0.8, 0.2, 0.01);
        let env: Vec<f64> = (0..20_000)
            .map(|i| f64::from(qsb_gain_at(f64::from(i) * dt, true, depth, rate)))
            .collect();
        let period = (1.0 / rate / dt) as usize;
        for cycle in 1..8 {
            let differs = (0..period)
                .map(|i| (env[i] - env[cycle * period + i]).abs())
                .fold(0.0_f64, f64::max);
            assert!(
                differs > 0.1,
                "cycle {cycle} repeated the first to within {differs:.3}"
            );
        }
    }

    /// And it still spans the depth it is asked for: down to the floor that keeps
    /// a group answerable, back up to full signal, and never outside that.
    #[test]
    fn fading_spans_its_depth_and_stays_inside_it() {
        let env: Vec<f64> = (0..20_000)
            .map(|i| f64::from(qsb_gain_at(f64::from(i) * 0.01, true, 0.8, 0.2)))
            .collect();
        let min = env.iter().copied().fold(f64::MAX, f64::min);
        let max = env.iter().copied().fold(f64::MIN, f64::max);
        assert!(
            min <= QSB_MIN_GAIN + 0.02,
            "deep fades never arrived: {min:.3}"
        );
        assert!(max >= 0.97, "the signal never came back up: {max:.3}");
        assert!(min >= QSB_MIN_GAIN - 1e-6, "faded past the floor: {min:.3}");

        let depth: f64 = 0.5;
        let range = depth.min(1.0 - QSB_MIN_GAIN);
        for step in 0..200 {
            let gain = f64::from(qsb_gain_at(f64::from(step) * 0.05, true, depth, 0.5));
            assert!(gain >= 1.0 - range - 1e-6 && gain <= 1.0 + 1e-6, "{gain}");
        }
        for t in [0.0, -1.0, 1e9] {
            assert!(qsb_gain_at(t, true, 0.6, 1.0).is_finite());
        }
    }
}
