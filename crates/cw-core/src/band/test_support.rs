//! What the band's tests measure with: one FFT, one Welch spectrum, the
//! level statistics, and the receiver's shapes.

use std::f64::consts::{SQRT_2, TAU};

use super::filter::{FilterDesign, ReceiverFilter};
use super::knee::agc_knee;
use super::source::BandSource;
use crate::settings::{FilterShape, TrainingSettings};

/// The receiver `settings` describe, running.
pub fn receiver_for(sample_rate: u32, settings: &TrainingSettings) -> ReceiverFilter {
    ReceiverFilter::new(sample_rate, &FilterDesign::from_settings(settings))
}

/// The band `settings` describe, before the receiver.
pub fn source_for(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> BandSource {
    let receiver = FilterDesign::from_settings(settings);
    BandSource::new(sample_rate, &settings.band, &receiver, seed)
}

/// Where the AGC `settings` describe starts to act.
pub fn knee_for(settings: &TrainingSettings) -> Option<f64> {
    agc_knee(&settings.band, &FilterDesign::from_settings(settings))
}

/// Every shape the control offers.
pub const ALL_SHAPES: [FilterShape; 3] =
    [FilterShape::Sharp, FilterShape::Soft, FilterShape::Peaked];
/// The two IF filter shapes, for what is about the IF filter.
pub const IF_SHAPES: [FilterShape; 2] = [FilterShape::Sharp, FilterShape::Soft];

/// The IF filter a shape is built on: the peak filter sits behind the sharp
/// one.
pub fn if_shape(shape: FilterShape) -> FilterShape {
    match shape {
        FilterShape::Peaked => FilterShape::Sharp,
        other => other,
    }
}

/// A forward radix-2 FFT with its tables built once, which is what keeps a
/// ten-minute listen affordable in a debug build. The length is a power of
/// two.
pub struct Fft {
    twiddles: Vec<(f64, f64)>,
    reverse: Vec<usize>,
}

impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two(), "an FFT of {n} points");
        let bits = n.trailing_zeros();
        Self {
            twiddles: (0..n / 2)
                .map(|k| {
                    let (s, c) = (-TAU * k as f64 / n as f64).sin_cos();
                    (c, s)
                })
                .collect(),
            reverse: (0..n)
                .map(|i| i.reverse_bits() >> (usize::BITS - bits))
                .collect(),
        }
    }

    /// Transform `re + j·im` in place.
    pub fn run(&self, re: &mut [f64], im: &mut [f64]) {
        let n = re.len();
        assert!(n == self.reverse.len() && im.len() == n);
        for (i, &j) in self.reverse.iter().enumerate() {
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut half = 1;
        while half < n {
            let stride = n / (2 * half);
            for start in (0..n).step_by(2 * half) {
                for k in 0..half {
                    let (c, s) = self.twiddles[k * stride];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            half *= 2;
        }
    }
}

/// Welch's power spectrum — Hann window, half overlap — averaged over the
/// segments, with bin `k` at `k·rate/segment` hertz, up to and including
/// Nyquist. Not scaled to any unit: the tests read it as ratios.
pub fn welch<T: Copy + Into<f64>>(x: &[T], segment: usize) -> Vec<f64> {
    let fft = Fft::new(segment);
    let window = hann(segment);
    let mut power = vec![0.0; segment / 2 + 1];
    let (mut re, mut im) = (vec![0.0; segment], vec![0.0; segment]);
    let mut segments = 0;
    let mut start = 0;
    while start + segment <= x.len() {
        for (i, w) in window.iter().enumerate() {
            re[i] = x[start + i].into() * w;
        }
        im.fill(0.0);
        fft.run(&mut re, &mut im);
        for (k, p) in power.iter_mut().enumerate() {
            *p += re[k] * re[k] + im[k] * im[k];
        }
        segments += 1;
        start += segment / 2;
    }
    power
        .into_iter()
        .map(|p| p / f64::from(segments.max(1)))
        .collect()
}

/// A Hann window `n` long.
pub fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos())
        .collect()
}

pub fn peak(x: &[f32]) -> f64 {
    x.iter().fold(0.0, |a, s| a.max(f64::from(s.abs())))
}

pub fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// Fourth moment over the squared second: 3 for a Gaussian, far more for
/// anything impulsive.
pub fn kurtosis(x: &[f32]) -> f64 {
    let n = x.len() as f64;
    let mean = x.iter().map(|s| f64::from(*s)).sum::<f64>() / n;
    let moment = |power: i32| {
        x.iter()
            .map(|s| (f64::from(*s) - mean).powi(power))
            .sum::<f64>()
            / n
    };
    moment(4) / moment(2).powi(2)
}

/// Steady-state gain of the running filter for a tone at `hz`: half a second
/// to settle, then a whole number of cycles, so the RMS is exact.
pub fn tone_gain(sample_rate: u32, design: &FilterDesign, hz: f64) -> f64 {
    let mut filter = ReceiverFilter::new(sample_rate, design);
    let step = TAU * hz / f64::from(sample_rate);
    let settle = sample_rate as usize / 2;
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

#[test]
fn the_fft_is_the_dft() {
    let n = 64;
    let x: Vec<f64> = (0..n).map(|i| ((i * 7 % 13) as f64 - 6.0) / 6.0).collect();
    let (mut re, mut im) = (x.clone(), vec![0.0; n]);
    Fft::new(n).run(&mut re, &mut im);
    for k in 0..n {
        let (mut dr, mut di) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let (s, c) = (-TAU * (k * i) as f64 / n as f64).sin_cos();
            dr += v * c;
            di += v * s;
        }
        assert!(
            (re[k] - dr).abs() < 1e-9 && (im[k] - di).abs() < 1e-9,
            "bin {k}"
        );
    }
}
