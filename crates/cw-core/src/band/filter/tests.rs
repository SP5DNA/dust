//! The receiver filter against what a real IF filter does, measured the way
//! the off-air recordings were: white noise in, a Welch spectrum out, edges
//! read off it.

use std::f64::consts::{SQRT_2, TAU};

use super::*;
use crate::band::Gaussian;

/// Every shape the control offers.
const ALL_SHAPES: [FilterShape; 3] = [FilterShape::Sharp, FilterShape::Soft, FilterShape::Peaked];

/// The rate the long noise renders run at. Every edge measured here is below
/// 2 kHz, the design does not depend on the rate, and a quarter of 48 kHz keeps
/// a minute of noise quick to render and analyse.
const MEASURE_RATE: u32 = 12_000;
/// Welch segment: 4096 points at 12 kHz is 2.9 Hz per bin, inside the 3 Hz
/// the acceptance asks for.
const SEGMENT: usize = 4_096;
/// How many bins the spectrum is smoothed over before edges are read: about
/// 15 Hz, narrow beside the narrowest skirt and enough to steady a minute of
/// noise to a few hundredths of a decibel.
const SMOOTH_BINS: usize = 5;
/// A minute of noise, as the acceptance asks.
const RENDER_SEC: usize = 60;

/// In-place radix-2 FFT. Slow and simple, which is all a test needs.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let twiddles: Vec<(f64, f64)> = (0..n / 2)
        .map(|k| {
            let (s, c) = (-TAU * k as f64 / n as f64).sin_cos();
            (c, s)
        })
        .collect();
    let mut len = 2;
    while len <= n {
        let stride = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = twiddles[k * stride];
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// The receiver's power response to `seconds` of white noise: a Hann-window
/// Welch spectrum of the output divided by the input's variance, so a bin
/// reads |H(f)|² directly. Returns the bin spacing and the smoothed spectrum.
fn noise_response(design: &FilterDesign, seconds: usize) -> (f64, Vec<f64>) {
    let mut filter = ReceiverFilter::new(MEASURE_RATE, design);
    let mut noise = Gaussian::new(0x5EC7_0A11);
    // Let the narrowest, slowest filter fill before anything is kept.
    for _ in 0..MEASURE_RATE / 2 {
        filter.process(noise.sample());
    }
    let n = MEASURE_RATE as usize * seconds;
    let mut input_power = 0.0;
    let output: Vec<f64> = (0..n)
        .map(|_| {
            let x = noise.sample();
            input_power += x * x;
            filter.process(x)
        })
        .collect();
    let variance = input_power / n as f64;

    let window: Vec<f64> = (0..SEGMENT)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / SEGMENT as f64).cos())
        .collect();
    let window_power: f64 = window.iter().map(|w| w * w).sum();
    let mut power = vec![0.0; SEGMENT / 2];
    let mut segments = 0;
    let (mut re, mut im) = (vec![0.0; SEGMENT], vec![0.0; SEGMENT]);
    let mut start = 0;
    while start + SEGMENT <= output.len() {
        for (i, w) in window.iter().enumerate() {
            re[i] = output[start + i] * w;
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for (k, slot) in power.iter_mut().enumerate() {
            *slot += re[k] * re[k] + im[k] * im[k];
        }
        segments += 1;
        start += SEGMENT / 2;
    }
    let scale = 1.0 / (f64::from(segments) * window_power * variance);
    let raw: Vec<f64> = power.iter().map(|p| p * scale).collect();
    let half = SMOOTH_BINS / 2;
    let smoothed = (0..raw.len())
        .map(|k| {
            let lo = k.saturating_sub(half);
            let hi = (k + half + 1).min(raw.len());
            raw[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
        })
        .collect();
    (f64::from(MEASURE_RATE) / SEGMENT as f64, smoothed)
}

/// Where the spectrum first falls `db` below unity, walking out from the
/// pitch in `direction`, interpolated between bins in decibels. `None` if it
/// never does before zero hertz.
fn edge(bin_hz: f64, power: &[f64], pitch: f64, db: f64, direction: isize) -> Option<f64> {
    let level = |k: usize| 10.0 * power[k].max(1e-300).log10();
    let mut k = (pitch / bin_hz).round() as usize;
    loop {
        let next = k.checked_add_signed(direction)?;
        if next == 0 || next >= power.len() {
            return None;
        }
        if level(next) <= -db {
            let (a, b) = (level(k), level(next));
            let t = (a + db) / (a - b);
            let hz = (k as f64 + t * direction as f64) * bin_hz;
            return Some(hz);
        }
        k = next;
    }
}

/// The folded sideband's own contribution at `hz`, as an amplitude.
fn folded_at(design: &FilterDesign, hz: f64) -> f64 {
    design.gain() * design.lowpass(hz + design.pitch_hz()).abs()
}

/// Steady-state gain of the running filter for a tone at `hz`.
fn tone_gain(sample_rate: u32, design: &FilterDesign, hz: f64) -> f64 {
    let mut filter = ReceiverFilter::new(sample_rate, design);
    let step = TAU * hz / f64::from(sample_rate);
    let settle = sample_rate as usize / 2;
    // A whole number of cycles, so the RMS is exact.
    let cycles = (hz / 4.0).round().max(1.0);
    let measure = (cycles * f64::from(sample_rate) / hz).round() as usize;
    let mut sum_sq = 0.0;
    for n in 0..settle + measure {
        let out = filter.process((step * n as f64).sin());
        if n >= settle {
            sum_sq += out * out;
        }
    }
    (sum_sq / measure as f64).sqrt() * SQRT_2
}

/// The acceptance the off-air measurements set, for one shape at one pitch
/// across the three widths: a minute of white noise through the receiver, and
/// the hiss has to come out centred on the note with mirror-image skirts.
///
/// - −6 dB midpoint within ±5 Hz of the pitch.
/// - −6 dB edges within ±10 Hz of pitch ± width/2, the width being quoted
///   at 6 dB.
/// - −20 dB distances above and below the pitch within 5% of each other
///   wherever pitch − bandwidth/2 ≥ 150 Hz.
/// - Unity at the pitch to 0.1 dB.
///
/// An edge is only read where the folded sideband is far enough under the
/// level being measured there — and, since every level is read against the
/// pitch, at the pitch too — that it cannot move the edge out of tolerance.
/// For the 20 dB skirts, read as a 5% ratio, 20 dB under is enough. For the
/// 6 dB edges, read to ±10 Hz, it has to be 40 dB under: the two sidebands
/// are one coherent filter, so a fold 20 dB under can move a level by 0.8 dB,
/// and on a soft skirt, which falls only 0.026 dB a hertz at the 6 dB point
/// of a 1 kHz filter, that is 30 Hz; 40 dB under, 3 Hz. A soft 1 kHz filter
/// at an 800 Hz pitch is the case: its fold is 28 dB under its lower edge,
/// and puts the design's own 6 dB point 7 Hz inside pitch − width/2. Closer to zero beat than that the opposite
/// sideband folding back moves the edge on a real receiver as much as here.
/// The acceptance assumes no fold. For Sharp that excuses nothing it names
/// wherever pitch − bandwidth/2 ≥ 150 Hz: every one of those edges is read.
fn accept(shape: FilterShape, pitch: f64) {
    for bandwidth in [250.0, 500.0, 1_000.0] {
        let design = FilterDesign::new(pitch, bandwidth, shape);
        let (bin_hz, power) = noise_response(&design, RENDER_SEC);
        let name = format!("{shape:?} {pitch}/{bandwidth}");
        let reaches_down = pitch - bandwidth / 2.0 >= 150.0;
        let clear_by = |hz: f64, level_db: f64, margin_db: f64| {
            hz > 0.0 && folded_at(&design, hz) <= 10f64.powf(-(level_db + margin_db) / 20.0)
        };
        let clear = |hz: f64, level_db: f64| clear_by(hz, level_db, 20.0);
        let pitch_is_clear = clear(pitch, 0.0);
        // Where an excused edge is allowed to be excused.
        let excusable = |what: &str| {
            assert!(
                shape == FilterShape::Soft || !reaches_down,
                "{name}: the fold reached {what}, which the acceptance checks"
            );
        };

        let gain_db = 20.0 * tone_gain(MEASURE_RATE, &design, pitch).log10();
        assert!(gain_db.abs() < 0.1, "{name}: {gain_db:+.3} dB at the pitch");
        if !pitch_is_clear {
            excusable("the pitch");
            continue;
        }

        let upper3 = edge(bin_hz, &power, pitch, 3.0, 1).expect("an upper 3 dB edge");
        let upper6 = edge(bin_hz, &power, pitch, 6.0, 1).expect("an upper 6 dB edge");
        assert!(
            (upper6 - (pitch + bandwidth / 2.0)).abs() <= 10.0,
            "{name}: upper 6 dB edge at {upper6:.1} Hz"
        );
        if clear_by(2.0 * pitch - upper6, 6.0, 40.0) {
            let lower6 = edge(bin_hz, &power, pitch, 6.0, -1).expect("a lower 6 dB edge");
            assert!(
                (lower6 - (pitch - bandwidth / 2.0)).abs() <= 10.0,
                "{name}: lower 6 dB edge at {lower6:.1} Hz"
            );
            let midpoint = (lower6 + upper6) / 2.0;
            assert!(
                (midpoint - pitch).abs() <= 5.0,
                "{name}: the hiss is centred on {midpoint:.1} Hz"
            );
        } else {
            excusable("the lower 6 dB edge");
        }

        let upper20 = edge(bin_hz, &power, pitch, 20.0, 1).expect("an upper 20 dB edge");
        let lower = |db: f64| edge(bin_hz, &power, pitch, db, -1).unwrap_or(f64::NAN);
        println!(
            "{name}: pitch {gain_db:+.3} dB; -3 dB {:.1}/{upper3:.1}; -6 dB midpoint {:+.1}; \
             -20 dB {:.1}/{upper20:.1}",
            lower(3.0),
            (lower(6.0) + upper6) / 2.0 - pitch,
            lower(20.0),
        );
        if !reaches_down {
            continue;
        }
        if clear(2.0 * pitch - upper20, 20.0) {
            let lower20 = edge(bin_hz, &power, pitch, 20.0, -1).expect("a lower 20 dB edge");
            let ratio = (upper20 - pitch) / (pitch - lower20);
            assert!(
                ratio <= 1.05 && 1.0 / ratio <= 1.05,
                "{name}: −20 dB at {lower20:.1}/{upper20:.1} Hz, skirts {ratio:.3} apart"
            );
        } else {
            excusable("the lower 20 dB edge");
        }
    }
}

#[test]
fn sharp_hiss_is_centred_on_a_400_hz_note() {
    accept(FilterShape::Sharp, 400.0);
}

#[test]
fn sharp_hiss_is_centred_on_a_600_hz_note() {
    accept(FilterShape::Sharp, 600.0);
}

#[test]
fn sharp_hiss_is_centred_on_an_800_hz_note() {
    accept(FilterShape::Sharp, 800.0);
}

#[test]
fn soft_hiss_is_centred_on_a_400_hz_note() {
    accept(FilterShape::Soft, 400.0);
}

#[test]
fn soft_hiss_is_centred_on_a_600_hz_note() {
    accept(FilterShape::Soft, 600.0);
}

#[test]
fn soft_hiss_is_centred_on_an_800_hz_note() {
    accept(FilterShape::Soft, 800.0);
}

/// The browser builds the receiver from `IIRFilterNode`s, each running its
/// feedforward and feedback coefficients as the plain difference equation the
/// Web Audio spec defines. Run exactly that, section by section, and the sum
/// has to be the native filter sample for sample.
#[test]
fn the_sections_are_what_the_browsers_iir_nodes_run() {
    for shape in ALL_SHAPES {
        let live = if shape == FilterShape::Peaked { 9 } else { 8 };
        assert_eq!(
            FilterDesign::new(500.0, 300.0, shape)
                .sections(48_000.0)
                .len(),
            live,
            "{shape:?}"
        );
        for (pitch, bandwidth, sample_rate) in [
            (500.0, 300.0, 44_100),
            (500.0, 300.0, 48_000),
            (700.0, 1_000.0, 44_100),
            (300.0, 2_400.0, 48_000),
            (600.0, 500.0, 48_000),
            (400.0, 150.0, 44_100),
            (1_200.0, 2_000.0, 22_050),
            (250.0, 2_000.0, 96_000),
        ] {
            let design = FilterDesign::new(pitch, bandwidth, shape);
            let sections = design.sections(f64::from(sample_rate));
            let mut native = ReceiverFilter::new(sample_rate, &design);
            // Direct form I, one history per node, as the spec writes it.
            let mut history = [([0.0f64; 2], [0.0f64; 2]); MAX_RECEIVER_SECTIONS];
            let mut noise = Gaussian::new(3);
            for _ in 0..4_096 {
                let x = noise.sample();
                let mut browser = 0.0;
                for (section, (xs, ys)) in sections.iter().zip(history.iter_mut()) {
                    let [b0, b1] = section.feedforward;
                    let [a0, a1, a2] = section.feedback;
                    let y = (b0 * x + b1 * xs[0] - a1 * ys[0] - a2 * ys[1]) / a0;
                    *xs = [x, xs[0]];
                    *ys = [y, ys[0]];
                    browser += y;
                }
                let ours = native.process(x);
                assert!(
                    (ours - browser).abs() < 1e-9 * (1.0 + browser.abs()),
                    "{shape:?} {pitch}/{bandwidth} at {sample_rate}: {ours} against {browser}"
                );
            }
        }
    }
}

/// The digital filter is the analogue one sampled, so the response the
/// design reports holds at any rate a sound card or a browser runs at — the
/// sections' own transfer function, summed, against the analogue formula.
#[test]
fn the_design_response_holds_at_every_sample_rate() {
    for shape in ALL_SHAPES {
        for (pitch, bandwidth) in [(600.0, 500.0), (200.0, 2_000.0), (1_200.0, 2_000.0)] {
            let design = FilterDesign::new(pitch, bandwidth, shape);
            for sample_rate in [8_000.0, 22_050.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
                let sections = design.sections(sample_rate);
                for k in 1..60 {
                    let hz = f64::from(k) * 40.0;
                    let digital = sections
                        .iter()
                        .fold(Complex::new(0.0, 0.0), |acc, s| {
                            acc.add(s.response(sample_rate, hz))
                        })
                        .abs();
                    let analogue = design.response_at(hz);
                    // The first alias of the widest soft filter is what
                    // shows at the lowest rate; at the usual ones nothing
                    // does.
                    let tolerance = if sample_rate < 20_000.0 { 1e-3 } else { 1e-5 };
                    assert!(
                        (digital - analogue).abs() < tolerance,
                        "{shape:?} {pitch}/{bandwidth} at {sample_rate} Hz, {hz} Hz: \
                         {digital:.6} against {analogue:.6}"
                    );
                }
            }
        }
    }
}

/// Arithmetic, not geometric: a tone so many hertz above the pitch passes
/// exactly as well as one the same distance below — the prototype's own
/// response, moved — to a fiftieth of a decibel, everywhere the folded
/// sideband is 60 dB out of the way. At a 1200 Hz pitch that is all of a
/// narrow filter's skirts down past −80 dB.
#[test]
fn the_skirts_are_mirror_images_in_hertz() {
    for shape in ALL_SHAPES {
        for bandwidth in [FILTER_WIDTH_MIN, 500.0, 900.0] {
            let design = FilterDesign::new(1_200.0, bandwidth, shape);
            let mut checked = 0;
            for step in 0..=120 {
                let offset = f64::from(step) * bandwidth / 80.0;
                let (up, down) = (1_200.0 + offset, 1_200.0 - offset);
                let main = design.gain() * design.lowpass(offset).abs();
                if down < LOWEST_EDGE_HZ
                    || folded_at(&design, up).max(folded_at(&design, down)) > 1e-3 * main
                {
                    continue;
                }
                let (above, below) = (design.response_at(up), design.response_at(down));
                assert!(
                    (above / main).log10().abs() < 0.001 && (below / main).log10().abs() < 0.001,
                    "{shape:?} {bandwidth}: {offset} Hz off, {above:.3e} above, \
                     {below:.3e} below, {main:.3e} translated"
                );
                checked += 1;
            }
            // The passband and well down both skirts, even for the widest
            // soft one, whose gentle skirts meet the fold soonest.
            assert!(
                checked > 25,
                "{shape:?} {bandwidth}: only {checked} points clear"
            );
        }
    }
    // Narrow and sharp at a high pitch, the skirts are mirrored to beyond
    // −80 dB.
    let design = FilterDesign::new(1_200.0, FILTER_WIDTH_MIN, FilterShape::Sharp);
    let deep = design.response_at(1_200.0 - 2.0 * FILTER_WIDTH_MIN);
    assert!(deep < 1e-4);
    assert!((deep / design.response_at(1_200.0 + 2.0 * FILTER_WIDTH_MIN) - 1.0).abs() < 1e-3);
}

/// The reason the BFO phase is chosen at all. Left at zero, a filter wide
/// enough to fold the far sideband back can cancel its own pitch — a
/// Butterworth 1640 Hz wide at 3 dB at 240 Hz loses it by 64 dB — or lift
/// some other tone well above it, rewarding a station for sitting off to one
/// side. With the phase chosen, nothing anywhere in the audible passband comes
/// out more than a fraction of a decibel louder than the pitch you are tuned
/// to, at any pitch and width the receiver is built at, the widths the
/// side-tone range can widen it to included: half a decibel where the phase is
/// judged, three quarters between.
#[test]
fn no_tone_in_the_passband_is_louder_than_the_pitch() {
    let mut worst: (f64, f64, f64, FilterShape) = (0.0, 0.0, 0.0, FilterShape::Sharp);
    for shape in ALL_SHAPES {
        for pitch in (200..=1_200).step_by(50) {
            for bandwidth in (150..=MAX_RECEIVER_WIDTH_HZ as u32).step_by(50) {
                let (pitch, bandwidth) = (f64::from(pitch), f64::from(bandwidth));
                let design = FilterDesign::new(pitch, bandwidth, shape);
                let top = pitch + bandwidth;
                let loudest = (0..=400)
                    .map(|k| {
                        let hz = LOWEST_EDGE_HZ + (top - LOWEST_EDGE_HZ) * f64::from(k) / 400.0;
                        design.response_at(hz)
                    })
                    .fold(0.0, f64::max);
                let bump = 20.0 * loudest.log10();
                if bump > worst.0 {
                    worst = (bump, pitch, bandwidth, shape);
                }
            }
        }
    }
    let (bump, pitch, bandwidth, shape) = worst;
    println!("loudest tone above the pitch: {bump:.3} dB, {shape:?} {pitch}/{bandwidth}");
    assert!(
        bump < 0.75,
        "{shape:?} {pitch}/{bandwidth}: a tone came out {bump:.2} dB above the pitch"
    );

    // And the case that motivated it: without the phase, the pitch is gone.
    let at_6_db = 1_640.0 * half_voltage(FilterShape::Sharp);
    let design = FilterDesign::new(240.0, at_6_db, FilterShape::Sharp);
    let unturned = design.translated(Complex::new(1.0, 0.0), 240.0).abs();
    assert!(
        unturned < 0.01,
        "the fold no longer cancels the pitch unturned ({unturned:.4}), so this test \
         no longer shows why the phase is chosen"
    );
    assert!((design.response_at(240.0) - 1.0).abs() < 1e-9);
    assert!(design.gain() < 1.0 / MIN_PITCH_RESPONSE);
}

/// Every corner of the controls, at every rate a sound card or browser runs
/// at, makes a filter that can be built and that stays finite: stable pole
/// pairs, a numerator an `IIRFilterNode` will accept (not all zeros), and
/// numbers out for a burst of noise and an impulse.
#[test]
fn every_corner_of_the_controls_is_stable_and_finite() {
    for shape in ALL_SHAPES {
        for pitch in [200.0, 333.0, 600.0, 1_200.0] {
            for bandwidth in [
                FILTER_WIDTH_MIN,
                700.0,
                1_640.0,
                FILTER_WIDTH_MAX,
                MAX_RECEIVER_WIDTH_HZ,
            ] {
                let design = FilterDesign::new(pitch, bandwidth, shape);
                assert!(design.gain().is_finite() && design.gain() > 0.0);
                for sample_rate in [8_000u32, 11_025, 22_050, 44_100, 48_000, 96_000, 192_000] {
                    let name = format!("{shape:?} {pitch}/{bandwidth} at {sample_rate}");
                    for section in design.sections(f64::from(sample_rate)) {
                        let [_, a1, a2] = section.feedback;
                        assert_eq!(section.feedback[0], 1.0);
                        assert!(
                            section
                                .feedforward
                                .iter()
                                .chain(&section.feedback)
                                .all(|c| c.is_finite()),
                            "{name}: {section:?}"
                        );
                        assert!(
                            section.feedforward.iter().any(|b| *b != 0.0),
                            "{name}: {section:?}"
                        );
                        // Both poles inside the unit circle: a₂ is their
                        // product's magnitude, and |a₁| < 1 + a₂.
                        assert!((0.0..1.0).contains(&a2) && a1.abs() < 1.0 + a2, "{name}");
                    }
                    let mut filter = ReceiverFilter::new(sample_rate, &design);
                    let mut noise = Gaussian::new(11);
                    let mut loudest = 0.0f64;
                    for n in 0..sample_rate as usize / 4 {
                        let x = if n == 0 { 1.0 } else { noise.sample() };
                        let y = filter.process(x);
                        assert!(y.is_finite(), "{name}");
                        loudest = loudest.max(y.abs());
                    }
                    assert!(loudest < 20.0, "{name}: peaked at {loudest}");
                }
            }
        }
    }
}

/// A filter left ringing into silence has to come to rest rather than hang on
/// in the subnormal range, where every operation on a double takes a
/// microcode assist: inside the audio callback that is hundreds of
/// nanoseconds a sample for as long as the band is silent. The narrowest
/// filter rings longest and its poles decay at rates five times apart, so the
/// fast ones get there within a second or two of the last sound. Ten seconds
/// of silence after a click, and no state is ever subnormal on the way.
#[test]
fn a_filter_ringing_into_silence_comes_to_rest() {
    for shape in ALL_SHAPES {
        for sample_rate in [8_000, 48_000] {
            let design = FilterDesign::new(500.0, FILTER_WIDTH_MIN, shape);
            let mut filter = ReceiverFilter::new(sample_rate, &design);
            filter.process(1.0);
            for n in 0..sample_rate * 10 {
                filter.process(0.0);
                let states = filter.resonators[..filter.len]
                    .iter()
                    .flat_map(|r| [r.s1, r.s2]);
                for state in states {
                    assert!(
                        !state.is_subnormal(),
                        "{shape:?} at {sample_rate} Hz: a state went subnormal {n} samples \
                         into the silence"
                    );
                }
            }
            assert_eq!(filter.process(0.0), 0.0, "{shape:?} never came to rest");
        }
    }
}

/// The passband the pile-up is placed in is half the bandwidth either side of
/// the pitch, stopping short of zero beat; and the mirror is in hertz.
#[test]
fn the_passband_is_half_the_width_either_side() {
    assert_eq!(passband_edges(600.0, 500.0), (350.0, 850.0));
    assert_eq!(passband_edges(400.0, 2_000.0), (LOWEST_EDGE_HZ, 1_400.0));
    // Out-of-range inputs are clamped like the design clamps them.
    assert_eq!(passband_edges(600.0, 10.0), (525.0, 675.0));
    let (low, high) = passband_edges(0.0, 0.0);
    assert!(low.is_finite() && high > low);
    assert_eq!(mirror_hz(600.0, 450.0), 750.0);
    assert_eq!(mirror_hz(600.0, 750.0), 450.0);
}

// ------------------------------------------------------- the width, at 6 dB

fn db(gain: f64) -> f64 {
    20.0 * gain.max(1e-300).log10()
}

/// The full width at `level_db`, read off the design's real response.
fn full_width(design: &FilterDesign, level_db: f64) -> f64 {
    let (low, high) = design.edges_at(level_db);
    high - low
}

/// The audio peak filter on its own, written out from its definition rather
/// than taken from the design: a real pole at π·width, a zero the boost
/// further out, unity at the pitch.
fn apf_gain(offset_hz: f64) -> f64 {
    let a = PI * APF_WIDTH_HZ;
    let b = a * 10f64.powf(APF_BOOST_DB / 20.0);
    let w = TAU * offset_hz;
    w.hypot(b) / w.hypot(a) * (a / b)
}

/// A rig quotes its CW filter by the width between its 6 dB points, and so
/// does the control: wherever it is set and whatever the pitch, the IF filter
/// is 6 dB down half the width either side and untouched at the pitch. Where
/// the filter is wider than twice the pitch its lower edge is past zero beat,
/// so only the upper one is read. For the peaked shape it is its IF filter
/// that is held to this — the peak filter's resonance is divided back out —
/// because the resonance is meant to take a little more off either side.
///
/// The prototype is held to it everywhere. What you hear is held to it
/// wherever the folded sideband is 40 dB out of the way, at the pitch and at
/// the edge. Closer to zero beat than that the fold is part of the response:
/// it adds in power at the pitch, the pitch is brought back to unity, and the
/// edges, which only one sideband reaches, come out lower by as much — 3 dB
/// at the widest setting on a 300 Hz note. That is
/// [`FilterDesign::bfo_phase`] doing what it is for, not the width being
/// wrong, and the printout shows each one.
#[test]
fn the_six_db_points_are_where_the_width_says() {
    let mut worst: f64 = 0.0;
    for shape in ALL_SHAPES {
        for width in [150.0, 300.0, 500.0, 1_000.0, 2_000.0] {
            for pitch in [300.0, 500.0, 700.0] {
                let design = FilterDesign::new(pitch, width, shape);
                let if_part = |hz: f64| match shape {
                    FilterShape::Peaked => design.response_at(hz) / apf_gain(hz - pitch),
                    FilterShape::Sharp | FilterShape::Soft => design.response_at(hz),
                };
                let name = format!("{shape:?} {width}/{pitch}");
                let at_pitch = db(if_part(pitch));
                assert!(
                    at_pitch.abs() <= 0.01,
                    "{name}: {at_pitch:+.4} dB at the pitch"
                );
                let mut edges = vec![pitch + width / 2.0];
                if width <= 2.0 * pitch {
                    edges.push(pitch - width / 2.0);
                }
                for edge in edges {
                    // The IF filter's own skirt, moved to the pitch: what the
                    // width is a width of.
                    let prototype = db(design.lowpass(edge - pitch).abs()
                        / match shape {
                            FilterShape::Peaked => apf_gain(edge - pitch),
                            FilterShape::Sharp | FilterShape::Soft => 1.0,
                        });
                    assert!(
                        (prototype + 6.0).abs() <= 0.05,
                        "{name}: the prototype is {prototype:.3} dB at {edge} Hz"
                    );
                    // And what you hear, wherever the other sideband is too
                    // far down at the pitch and at the edge to touch it.
                    let level = db(if_part(edge));
                    let clear = design.lowpass(2.0 * pitch).abs() <= 0.01
                        && folded_at(&design, edge) <= 0.01 * 0.5;
                    println!(
                        "{name} at {edge} Hz: {level:.3} dB heard{}",
                        if clear { "" } else { " (folded)" }
                    );
                    if clear {
                        worst = worst.max((level + 6.0).abs());
                        assert!(
                            (level + 6.0).abs() <= 0.05,
                            "{name}: {level:.3} dB at {edge} Hz"
                        );
                    }
                }
            }
        }
    }
    println!("6 dB points: at worst {worst:.3} dB from -6");
}

/// A rig's 300 Hz CW filter, sharp, on a 500 Hz note: about 280 Hz wide at
/// 3 dB, 373 at 20 and 664 at 60 — a shape factor of 2.2 — and flat across
/// the default side-tone range either side of the note.
#[test]
fn a_sharp_300_hz_filter_is_a_rigs_cw_filter() {
    let mut settings = TrainingSettings::default();
    settings.band.filter_shape = FilterShape::Sharp;
    let design = FilterDesign::from_settings(&settings);
    assert_eq!(design.shape(), FilterShape::Sharp);
    assert_eq!(design.pitch_hz(), 500.0);
    assert_eq!(design.width_hz(), 300.0);
    let (w3, w20, w60) = (
        full_width(&design, -3.0),
        full_width(&design, -20.0),
        full_width(&design, -60.0),
    );
    let (at400, at600) = (db(design.response_at(400.0)), db(design.response_at(600.0)));
    println!(
        "sharp/300: {w3:.1} Hz at 3 dB, {w20:.1} at 20, {w60:.1} at 60; \
         {at400:+.3} dB at 400 Hz, {at600:+.3} at 600"
    );
    assert!((w3 - 280.0).abs() <= 2.0, "{w3:.1} Hz at 3 dB");
    assert!((w20 - 373.0).abs() <= 4.0, "{w20:.1} Hz at 20 dB");
    assert!((w60 - 664.0).abs() <= 8.0, "{w60:.1} Hz at 60 dB");
    assert!(at400 >= -0.1 && at600 >= -0.1);
}

/// The default receiver is that 300 Hz sharp filter with the audio peak
/// filter in behind it, on the 500 Hz note in the middle of the default
/// side-tone range. Nothing widens it: its IF filter passes 400-600 Hz flat,
/// and only the resonance takes the ends of the range down, by the 4 dB the
/// peak filter costs a station 100 Hz off.
#[test]
fn the_default_receiver_is_a_peaked_300_hz_filter() {
    let settings = TrainingSettings::default();
    assert_eq!(settings.band.filter_width_hz, DEFAULT_FILTER_WIDTH_HZ);
    assert_eq!(DEFAULT_FILTER_WIDTH_HZ, 300.0);
    assert_eq!(settings.band.filter_shape, FilterShape::Peaked);
    let design = FilterDesign::from_settings(&settings);
    assert_eq!(design.shape(), FilterShape::Peaked);
    assert_eq!(design.pitch_hz(), 500.0);
    assert_eq!(receiver_width_hz(&settings), 300.0);
    assert_eq!(design.width_hz(), 300.0);
    let (w3, w6, w20, w60) = (
        full_width(&design, -3.0),
        full_width(&design, -6.0),
        full_width(&design, -20.0),
        full_width(&design, -60.0),
    );
    let ends = [db(design.response_at(400.0)), db(design.response_at(600.0))];
    let if_ends = [400.0, 600.0].map(|hz| db(design.response_at(hz) / apf_gain(hz - 500.0)));
    println!(
        "default (peaked/300): {w3:.1} Hz at 3 dB, {w6:.1} at 6, {w20:.1} at 20, {w60:.1} at 60; \
         noise bandwidth {:.1} Hz; 400/600 Hz at {ends:.3?} dB, its IF filter {if_ends:.3?}",
        design.noise_bandwidth_hz()
    );
    assert!((w3 - 141.0).abs() <= 3.0, "{w3:.1} Hz at 3 dB");
    assert!((w6 - 264.0).abs() <= 3.0, "{w6:.1} Hz at 6 dB");
    for level in ends {
        assert!((level + 3.97).abs() <= 0.15, "{level:.3} dB at an end");
    }
    for level in if_ends {
        assert!(level >= -0.1, "the IF filter is {level:.3} dB at an end");
    }
    assert!((design.noise_bandwidth_hz() - 163.0).abs() <= 3.0);
    assert_eq!(design.sections(48_000.0).len(), 9);
}

/// The other two shapes at the same 300 Hz: soft rounds the shoulders off —
/// narrower at 3 dB, wider at 20 — and the peaked one adds the APF's
/// resonance, which narrows the top of it sharply and is a ninth resonator.
#[test]
fn soft_and_peaked_are_the_shapes_they_claim_at_300_hz() {
    let soft = FilterDesign::new(500.0, 300.0, FilterShape::Soft);
    let (w3, w20) = (full_width(&soft, -3.0), full_width(&soft, -20.0));
    let off = [db(soft.response_at(400.0)), db(soft.response_at(600.0))];
    println!("soft/300: {w3:.1} Hz at 3 dB, {w20:.1} at 20; ±100 Hz at {off:.3?} dB");
    assert!((w3 - 216.0).abs() <= 3.0, "soft: {w3:.1} Hz at 3 dB");
    assert!((w20 - 506.0).abs() <= 6.0, "soft: {w20:.1} Hz at 20 dB");
    for level in off {
        assert!(
            (level + 2.57).abs() <= 0.15,
            "soft: {level:.3} dB 100 Hz off"
        );
    }

    let peaked = FilterDesign::new(500.0, 300.0, FilterShape::Peaked);
    let (w3, w6) = (full_width(&peaked, -3.0), full_width(&peaked, -6.0));
    let off = [db(peaked.response_at(400.0)), db(peaked.response_at(600.0))];
    println!("peaked/300: {w3:.1} Hz at 3 dB, {w6:.1} at 6; ±100 Hz at {off:.3?} dB");
    assert!((w3 - 141.0).abs() <= 3.0, "peaked: {w3:.1} Hz at 3 dB");
    assert!((w6 - 264.0).abs() <= 3.0, "peaked: {w6:.1} Hz at 6 dB");
    for level in off {
        assert!(
            (level + 3.97).abs() <= 0.15,
            "peaked: {level:.3} dB 100 Hz off"
        );
    }
    assert_eq!(peaked.sections(48_000.0).len(), 9);
}

/// ∫|H(f)|² df, the slow way: the response integrated over every positive
/// frequency it reaches, by Simpson's rule.
fn integrated_noise_bandwidth(design: &FilterDesign) -> f64 {
    let top = design.pitch_hz() + 40.0 * design.width_hz() + 2_000.0;
    let steps = 200_000;
    let h = top / f64::from(steps);
    let power = |k: u32| design.response_at(f64::from(k) * h).powi(2);
    let inner: f64 = (1..steps)
        .map(|k| if k % 2 == 1 { 4.0 } else { 2.0 } * power(k))
        .sum();
    (power(0) + inner + power(steps)) * h / 3.0 / design.response_at(design.pitch_hz()).powi(2)
}

/// How much hiss the receiver lets through is its noise bandwidth, worked out
/// in closed form from the poles: about 282 Hz for the default sharp 300 Hz
/// filter, 225 for a soft one, 163 with the peak filter in. It has to be the
/// integral it stands for, fold and all — checked against the slow way,
/// across shapes, widths and pitches, wide ones reaching past zero beat
/// included.
#[test]
fn the_noise_bandwidth_is_the_integral_of_the_response() {
    for (shape, expected) in [
        (FilterShape::Sharp, 282.0),
        (FilterShape::Soft, 225.0),
        (FilterShape::Peaked, 163.0),
    ] {
        let design = FilterDesign::new(500.0, 300.0, shape);
        let closed = design.noise_bandwidth_hz();
        let slow = integrated_noise_bandwidth(&design);
        println!("{shape:?}/300: noise bandwidth {closed:.2} Hz, integrated {slow:.2}");
        assert!(
            (closed - expected).abs() <= 3.0,
            "{shape:?}: {closed:.1} Hz"
        );
    }
    let mut worst: f64 = 0.0;
    for shape in ALL_SHAPES {
        for (pitch, width) in [
            (500.0, 300.0),
            (500.0, 150.0),
            (700.0, 1_000.0),
            (1_200.0, 500.0),
            (300.0, 2_000.0),
            (700.0, 2_400.0),
            (240.0, 1_756.0),
        ] {
            let design = FilterDesign::new(pitch, width, shape);
            let closed = design.noise_bandwidth_hz();
            let slow = integrated_noise_bandwidth(&design);
            let error = (closed - slow).abs() / slow;
            worst = worst.max(error);
            assert!(
                error < 0.005,
                "{shape:?} {width}/{pitch}: closed form {closed:.2} Hz, integrated {slow:.2}"
            );
        }
    }
    println!(
        "closed form against the integral: at worst {:.4}%",
        100.0 * worst
    );
}

/// The reference filter the levels are measured through is the old receiver
/// exactly: a sharp filter 500 Hz wide at 3 dB, which at 6 dB is 500·3^(1/16),
/// 535.5 Hz — the same eight poles, so the same Butterworth response and the
/// noise bandwidth an eight-pole Butterworth that wide has, 503.2 Hz.
///
/// Its 3 dB width is read where the folded sideband is nowhere near. At the
/// 500 Hz note the floor is measured on, the fold — 76 dB down at the lower
/// edge, as it always was there — moves that edge by a hundredth of a hertz.
#[test]
fn the_reference_filter_is_the_old_500_hz_one() {
    let width = reference_width_hz();
    assert!((width - 535.54).abs() < 0.01, "{width}");
    let half_power_db = -10.0 * 2f64.log10();
    let clear = FilterDesign::new(1_500.0, width, FilterShape::Sharp);
    let half_power = full_width(&clear, half_power_db);
    // The prototype is the Butterworth whose 3 dB point is 250 Hz out.
    for offset in [0.0, 100.0, 200.0, 250.0, 300.0, 500.0] {
        let ideal = 1.0 / (1.0 + (offset / 250.0f64).powi(16)).sqrt();
        assert!(
            (clear.lowpass(offset).abs() - ideal).abs() < 1e-9,
            "{offset} Hz"
        );
    }
    let design = FilterDesign::new(500.0, width, FilterShape::Sharp);
    let at_the_note = full_width(&design, half_power_db);
    let ideal = REFERENCE_BANDWIDTH_HZ * (PI / 16.0) / (PI / 16.0).sin();
    println!(
        "reference: {width:.2} Hz at 6 dB, {half_power:.3} at 3 ({at_the_note:.3} at a 500 Hz \
         note), noise bandwidth {:.2} Hz",
        design.noise_bandwidth_hz()
    );
    assert!((half_power - REFERENCE_BANDWIDTH_HZ).abs() < 0.01);
    assert!((at_the_note - REFERENCE_BANDWIDTH_HZ).abs() < 0.02);
    assert!((design.noise_bandwidth_hz() - ideal).abs() < 1e-3 * ideal);
}

/// Every send goes through the one filter, fixed at the middle of the
/// side-tone range, so the filter has to pass the whole range: every pitch in
/// it within a decibel of the middle, through the design and through the
/// filter that runs, however narrow the control is set — and only as much
/// wider as that takes.
#[test]
fn the_receiver_always_passes_the_side_tone_range() {
    let mut widest: f64 = 0.0;
    for shape in ALL_SHAPES {
        for center in [400.0, 500.0, 700.0, 900.0] {
            for span in (0..=600).step_by(50) {
                let span = f64::from(span);
                let (low, high) = (center - span / 2.0, center + span / 2.0);
                if low < 200.0 || high > 1_200.0 {
                    continue;
                }
                let mut settings = TrainingSettings::default();
                settings.band.side_tone_min = low;
                settings.band.side_tone_max = high;
                settings.band.filter_shape = shape;
                settings.band.filter_width_hz = FILTER_WIDTH_MIN;
                let settings = settings.clamp();
                let width = receiver_width_hz(&settings);
                let design = FilterDesign::from_settings(&settings);
                assert_eq!(design.width_hz(), width);
                assert!(width >= FILTER_WIDTH_MIN);
                widest = widest.max(width);
                let name = format!("{shape:?} {low}-{high}: {width:.0} Hz");
                // The peak filter's own resonance is allowed to take more off
                // the ends; it is its IF filter, the sharp one, that passes
                // the range.
                let if_part = FilterDesign::new(center, width, if_shape(shape));
                for k in 0..=60 {
                    let hz = low + span * f64::from(k) / 60.0;
                    let level = db(if_part.response_at(hz));
                    assert!(
                        level >= COVERAGE_DB - 1e-6,
                        "{name}: {level:.3} dB at {hz} Hz"
                    );
                }
                if shape != FilterShape::Peaked {
                    for hz in [low, high] {
                        let level = db(tone_gain(48_000, &design, hz));
                        assert!(
                            level >= COVERAGE_DB - 0.01,
                            "{name}: the running filter passed {hz} Hz at {level:.3} dB"
                        );
                    }
                }
                // Widened only as far as it had to be: a little narrower and
                // an end of the range would not get through. Where the folded
                // sideband reaches the bottom of the range the response there
                // is no longer steady in the width — the fold comes and goes
                // with the BFO phase — so narrowest is only asked for where it
                // is clear.
                let narrower = FilterDesign::new(center, width * 0.99, if_shape(shape));
                if width > FILTER_WIDTH_MIN && folded_at(&narrower, low) < 1e-3 {
                    let ends = db(narrower.response_at(low)).min(db(narrower.response_at(high)));
                    assert!(ends < COVERAGE_DB, "{name}: wider than it needed to be");
                }
            }
        }
    }
    println!("widest receiver the coverage asked for: {widest:.0} Hz");
    // A width already enough is left exactly as it was set.
    let settings = TrainingSettings::default();
    assert_eq!(receiver_width_hz(&settings), settings.band.filter_width_hz);

    // The widest range the controls allow passes too, sharp: the folded
    // sideband is in it, and the width it is built at is what the real
    // response, fold and all, takes.
    let widest_range = |shape: FilterShape| {
        let mut settings = TrainingSettings::default();
        settings.band.side_tone_min = 200.0;
        settings.band.side_tone_max = 1_200.0;
        settings.band.filter_shape = shape;
        settings.clamp()
    };
    let quietest = |design: &FilterDesign| {
        (0..=100)
            .map(|k| db(design.response_at(200.0 + 10.0 * f64::from(k))))
            .fold(f64::INFINITY, f64::min)
    };
    let settings = widest_range(FilterShape::Sharp);
    let design = FilterDesign::from_settings(&settings);
    println!(
        "200-1200 Hz sharp: {:.0} Hz, at worst {:.3} dB",
        design.width_hz(),
        quietest(&design)
    );
    assert!(design.width_hz() < MAX_RECEIVER_WIDTH_HZ);
    assert!(quietest(&design) >= COVERAGE_DB - 1e-6);
    // Soft, no width gets there. Its prototype passes the range 2.4 kHz
    // wide, but that reaches 500 Hz past zero beat: the other sideband folds
    // onto the 700 Hz pitch, adds there in power, and bringing the pitch back
    // to unity leaves the top of the range a decibel and a half down — and
    // still more than a decibel down however much wider it goes. So it is
    // built at whichever width comes closest.
    let settings = widest_range(FilterShape::Soft);
    let design = FilterDesign::from_settings(&settings);
    let built = quietest(&design);
    println!(
        "200-1200 Hz soft: {:.0} Hz, at worst {built:.3} dB",
        design.width_hz()
    );
    assert!(built > 2.0 * COVERAGE_DB, "{built:.3} dB");
    let mut width = 2_000.0;
    while width <= MAX_RECEIVER_WIDTH_HZ {
        let other = quietest(&FilterDesign::new(700.0, width, FilterShape::Soft));
        assert!(
            other <= built + 0.02,
            "{width:.0} Hz would pass the range at {other:.3} dB, better than {built:.3}"
        );
        width += 50.0;
    }
}

/// The IF filter a shape is built on: the peak filter sits behind the sharp
/// one.
fn if_shape(shape: FilterShape) -> FilterShape {
    match shape {
        FilterShape::Peaked => FilterShape::Sharp,
        other => other,
    }
}

/// What the default band's hiss sounds like, measured the way the off-air
/// recordings were: thirty seconds of the noise alone through the whole
/// receiver, AGC and all.
///
/// A real receiver's hiss rings at the note. Its autocorrelation — how long
/// the hiss remembers its own waveform — has an envelope that is still above
/// 0.316 (−10 dB) 2.3 ms on and above 0.1 at 2.9 ms; the old 500 Hz default
/// had let go by about 1.5 ms, which is what made it sound like rushing air. And it
/// sits at the note, not under it: almost none of its power is below 0.7 of
/// the pitch, where the old one had nearly a fifth of it as rumble, and most of it
/// is inside the 6 dB width.
///
/// The default receiver is the peaked 300 Hz one, whose resonance holds the
/// hiss to the note longer still: measured, the envelope is 0.58 at 2.3 ms and
/// 0.41 at 2.9 ms, 0.5% of the power is below 350 Hz, and 99% of it is within
/// 350-650 Hz.
#[test]
fn the_default_hiss_rings_at_the_note() {
    const RATE: u32 = 48_000;
    const POINTS: usize = 8_192;
    let mut settings = TrainingSettings::default();
    settings.band.qrn_enabled = false;
    settings.band.noise_enabled = true;
    settings.band.agc_enabled = true;
    let settings = settings.clamp();
    let pitch = settings.side_tone_center();
    let width = receiver_width_hz(&settings);
    let mut mixer = crate::band::BandMixer::new(RATE, &settings, 0x4155);
    let mut settle = vec![0.0f32; RATE as usize / 2];
    mixer.fill_background(&mut settle);
    let mut x = vec![0.0f32; RATE as usize * 30];
    mixer.fill_background(&mut x);

    let window: Vec<f64> = (0..POINTS)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / POINTS as f64).cos())
        .collect();
    let mut power = vec![0.0; POINTS / 2 + 1];
    let (mut re, mut im) = (vec![0.0; POINTS], vec![0.0; POINTS]);
    let mut start = 0;
    while start + POINTS <= x.len() {
        for (i, w) in window.iter().enumerate() {
            re[i] = f64::from(x[start + i]) * w;
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for (k, slot) in power.iter_mut().enumerate() {
            *slot += re[k] * re[k] + im[k] * im[k];
        }
        start += POINTS / 2;
    }
    let bin_hz = f64::from(RATE) / POINTS as f64;
    let total: f64 = power.iter().sum();
    let share = |keep: &dyn Fn(f64) -> bool| {
        power
            .iter()
            .enumerate()
            .filter(|(k, _)| keep(*k as f64 * bin_hz))
            .map(|(_, p)| p)
            .sum::<f64>()
            / total
    };
    let below = share(&|f| f < 0.7 * pitch);
    let inside = share(&|f| (f - pitch).abs() <= width / 2.0);
    // The autocorrelation's envelope, from the spectrum: the analytic
    // autocorrelation is the one-sided spectrum transformed back, so its
    // magnitude at a lag is |Σ P(f)·e^{j2πfτ}| over Σ P(f).
    let envelope = |lag_sec: f64| {
        let (mut sum_re, mut sum_im) = (0.0, 0.0);
        for (k, p) in power.iter().enumerate() {
            let phase = TAU * k as f64 * bin_hz * lag_sec;
            sum_re += p * phase.cos();
            sum_im += p * phase.sin();
        }
        sum_re.hypot(sum_im) / total
    };
    let (at_2_3, at_2_9) = (envelope(0.0023), envelope(0.0029));
    println!(
        "default hiss: envelope {at_2_3:.3} at 2.3 ms, {at_2_9:.3} at 2.9 ms; \
         {:.2}% below {:.0} Hz, {:.1}% within {pitch}±{:.0} Hz",
        100.0 * below,
        0.7 * pitch,
        100.0 * inside,
        width / 2.0
    );
    assert!(at_2_3 >= 0.316, "the hiss let go by 2.3 ms: {at_2_3:.3}");
    assert!(at_2_9 >= 0.1, "the hiss let go by 2.9 ms: {at_2_9:.3}");
    assert!(below <= 0.03, "{:.1}% of the hiss is rumble", 100.0 * below);
    assert!(
        inside >= 0.85,
        "only {:.1}% of the hiss is in the passband",
        100.0 * inside
    );
}

/// Where no width passes a wide side-tone range — the other sideband folding
/// back takes the bottom of it down however wide the filter is built — the
/// receiver is not built wider for nothing: a wider build is used only when
/// it nearly passes, and otherwise the width asked for stands.
#[test]
fn a_receiver_that_cannot_pass_the_range_is_not_widened_for_nothing() {
    use crate::settings::{FilterShape, TrainingSettings};
    for (low, high, asked) in [
        (200.0, 1_000.0, 1_900.0),
        (200.0, 1_000.0, 2_000.0),
        (200.0, 1_050.0, 2_000.0),
    ] {
        let mut settings = TrainingSettings::default();
        settings.band.filter_shape = FilterShape::Sharp;
        settings.band.side_tone_min = low;
        settings.band.side_tone_max = high;
        settings.band.filter_width_hz = asked;
        let settings = settings.clamp();
        let built = crate::band::receiver_width_hz(&settings);
        let design = FilterDesign::from_settings(&settings);
        let quietest = (0..=32)
            .map(|i| design.response_at(low + (high - low) * f64::from(i) / 32.0))
            .fold(f64::INFINITY, f64::min);
        let passes = 20.0 * quietest.log10() >= -1.0 - 1e-6;
        assert!(
            passes || (built - asked).abs() < 1e-9 || 20.0 * quietest.log10() >= -1.5,
            "{low}-{high} asked {asked}: built {built:.0} Hz and still {:.2} dB down",
            20.0 * quietest.log10()
        );
    }
}
