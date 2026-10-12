//! The band: what a receiver in CW mode hears around the station you are
//! copying, shared by every backend.
//!
//! The names mean what an operator means by them. The noise floor is the
//! steady hiss of the band itself. QRN is atmospheric static — lightning. QSB
//! is fading. QRM, another station on top of yours, lives with the stations
//! rather than in here.
//!
//! The model follows the signal chain of a real receiver, because that is
//! where the sound comes from:
//!
//! 1. The band puts white Gaussian noise and impulsive static into the
//!    antenna ([`noise`]). Neither has a pitch.
//! 2. Everything passes through one CW filter ([`filter`]), the Morse
//!    included. That filter is what gives the hiss its pitch and width and
//!    makes every crash and every dit ring; narrowing it quiets the band,
//!    pushes off-frequency stations down and lengthens the ringing, all at
//!    once, because those are one thing.
//! 3. The AGC rides the gain ([`agc`]): a steady few decibels down under a
//!    keyed station, back up in the pauses, and only a moment's duck for a
//!    crash.
//! 4. The receiver's audio stage adds a faint, wide hiss of its own
//!    ([`AudioFloor`]), after the filter and the AGC, so the band is never
//!    quite silent outside the passband — as no real receiver is.
//!
//! What it deliberately does not do is invent a sound the receiver would not
//! make. Sparse clicks through resonators a few hertz wide, their tuning
//! swept about, is how a wind-whistle is synthesised. A real CW filter is
//! hundreds of hertz wide, sits still, and is fed with continuous noise; this
//! is that.

mod activity;
mod agc;
mod filter;
mod noise;
mod qsb;

// What the players, the settings screen and the pile-up planner use. The rest
// is the band's own business.
pub use agc::{
    AGC_ATTACK_SEC, AGC_COMPRESSOR_KNEE_DB, AGC_RELEASE_SEC, CompressorSetting, agc_compressor,
};
pub(crate) use filter::mirror_hz;
pub use filter::{
    FilterDesign, MAX_RECEIVER_SECTIONS, MAX_RECEIVER_WIDTH_HZ, ReceiverFilter, Section, Sections,
    receiver_width_hz,
};
pub use noise::{AudioFloor, BandSource, heard_snr_db};
pub use qsb::{QSB_MIN_GAIN, QSB_PATHS, QSB_SPREAD, qsb_gain_at, qsb_path};

use crate::settings::{STATIONS_MIN, TrainingSettings};
use agc::{AGC_TRIGGER, Agc};

/// What a receiver does with a loud crash: catches it, rather than letting it
/// square off. Linear up to the knee, asymptotic to full scale above it — so
/// the biggest static is loud without turning into a buzz.
fn soft_limit(x: f64) -> f64 {
    const KNEE: f64 = 0.7;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let over = (magnitude - KNEE) / (1.0 - KNEE);
    (KNEE + (1.0 - KNEE) * over.tanh()).copysign(x)
}

/// How far past full scale [`soft_limit_curve`] reaches before it simply holds
/// at its last value. Far beyond anything the band and the Morse add up to.
const SOFT_LIMIT_HEADROOM: f64 = 4.0;

/// [`soft_limit`] as a lookup table, for a Web Audio `WaveShaperNode`.
///
/// A shaper maps its input over -1..=1 onto the curve, so the input is to be
/// scaled down by [`SOFT_LIMIT_HEADROOM`] first; the curve maps that back up
/// before limiting. Between the points the shaper interpolates linearly, which
/// below the knee is exact — the limiter is a straight line there.
pub fn soft_limit_curve(points: usize) -> Vec<f32> {
    let points = points.max(2);
    let last = (points - 1) as f64;
    (0..points)
        .map(|i| {
            let x = (2.0 * i as f64 / last - 1.0) * SOFT_LIMIT_HEADROOM;
            soft_limit(x) as f32
        })
        .collect()
}

/// The gains the browser feeds its soft limiter through, for a compressor
/// set as `setting`.
///
/// The browser's limiter is a shaper whose input is scaled down by
/// [`SOFT_LIMIT_HEADROOM`], so whatever reaches it has to be too. What has
/// come through the compressor also carries the make-up gain the compressor
/// adds of its own accord, which a receiver's AGC never would, so the
/// receiver's feed takes that back off. The audio-stage floor joins after the
/// compressor, as it does after the native AGC, and has nothing to take back:
/// sent through the receiver's feed it would come out quieter by the make-up
/// gain whenever the AGC is on. Either way, both reach the limiter at the
/// gain the native player gives them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterFeed {
    /// For the receiver's output, after the compressor.
    pub receiver: f64,
    /// For the [`AudioFloor`], which bypasses the compressor.
    pub audio_floor: f64,
}

/// The [`LimiterFeed`] for the browser's compressor set as `setting`.
pub fn limiter_feed(setting: &CompressorSetting) -> LimiterFeed {
    LimiterFeed {
        receiver: 10f64.powf(-setting.makeup_db / 20.0) / SOFT_LIMIT_HEADROOM,
        audio_floor: 1.0 / SOFT_LIMIT_HEADROOM,
    }
}

/// How high this band's floor ordinarily peaks, through the receiver.
///
/// The level a 60 ms peak follower on the filtered band sits at, as a median
/// over three seconds so a crash that happens to land in the measurement does
/// not move it: about 2.2 times the floor's RMS for plain hiss, more on a band
/// busy with crackle. Other stations are left out — they are what the AGC
/// rides over, not the floor it is set against.
///
/// Measured rather than worked out, because the filter's shape and width and
/// the static all move it. The band is calibrated to sound the same at any
/// sample rate, so a low rate measures it as well as a high one.
fn band_standing_level(sample_rate: u32, settings: &TrainingSettings) -> f64 {
    const SEED: u64 = 0x5EED_1234;
    const SETTLE_SEC: f64 = 0.25;
    const MEASURE_SEC: f64 = 2.75;
    const FOLLOWER_SEC: f64 = 0.06;
    const BLOCK_SEC: f64 = 0.01;
    let sample_rate = sample_rate.max(1);
    let sr = f64::from(sample_rate);
    let mut floor = settings.clone().clamp();
    // The floor is the band without its stations: they are what the knee is
    // there to answer, so they cannot be part of setting it.
    floor.band.stations_min = STATIONS_MIN;
    floor.band.stations_max = STATIONS_MIN;
    let mut source = BandSource::new(sample_rate, &floor, SEED);
    let mut receiver = ReceiverFilter::from_settings(sample_rate, &floor);
    let fall = (-1.0 / (FOLLOWER_SEC * sr)).exp();
    let mut follower = 0.0f64;
    let mut step = || {
        let heard = receiver.process(source.next_sample()).abs();
        follower = heard.max(follower * fall);
        follower
    };
    for _ in 0..(SETTLE_SEC * sr) as usize {
        step();
    }
    let block = ((BLOCK_SEC * sr) as usize).max(1);
    let mut levels: Vec<f64> = (0..((MEASURE_SEC / BLOCK_SEC) as usize))
        .map(|_| {
            let mut last = 0.0;
            for _ in 0..block {
                last = step();
            }
            last
        })
        .collect();
    levels.sort_by(f64::total_cmp);
    levels.get(levels.len() / 2).copied().unwrap_or(0.0)
}

/// The rate the floor is measured at for the AGC's knee. Both backends use
/// it, so both arrive at the same knee to the bit; it is low because the
/// browser measures on its main thread every time a band setting moves.
const FLOOR_MEASURE_RATE: u32 = 16_000;

/// Where the receiver's AGC starts to turn the gain down, as the amplitude of
/// a steady tone; `None` with the AGC switched off.
///
/// [`AGC_TRIGGER`] times the band's own floor ([`band_standing_level`]), and
/// never below that of the default floor heard through a
/// [`REFERENCE_BANDWIDTH_HZ`] filter. Above that it follows the floor, the way
/// an operator backs the RF gain off on a noisy band or a wide filter, so the
/// hiss on its own never pumps the gain. Below it, it stays put, as a real
/// receiver's AGC threshold does: closing the filter or finding a quiet band
/// takes noise away from under the knee, and leaves the station you are
/// copying exactly as loud as it was.
pub fn agc_knee(settings: &TrainingSettings) -> Option<f64> {
    if !settings.band.agc_enabled {
        return None;
    }
    let floor = band_standing_level(FLOOR_MEASURE_RATE, settings).max(reference_floor_level());
    Some(AGC_TRIGGER * floor)
}

/// [`band_standing_level`] of the default floor alone, through the reference
/// filter ([`filter::reference_width_hz`]): a sharp one [`REFERENCE_BANDWIDTH_HZ`]
/// wide at 3 dB, which is where the knee has always been measured. Measured
/// once.
fn reference_floor_level() -> f64 {
    static LEVEL: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *LEVEL.get_or_init(|| {
        let mut settings = TrainingSettings::default();
        settings.band.noise_enabled = true;
        settings.band.noise_level = noise::QRN_REFERENCE_NOISE_LEVEL;
        settings.band.qrn_enabled = false;
        settings.band.stations_min = STATIONS_MIN;
        settings.band.stations_max = STATIONS_MIN;
        settings.band.filter_width_hz = filter::reference_width_hz();
        settings.band.filter_shape = crate::settings::FilterShape::Sharp;
        band_standing_level(FLOOR_MEASURE_RATE, &settings)
    })
}

/// The native receiver: the band, through the filter, under the AGC, one
/// sample at a time. QSB is applied separately, to the Morse samples
/// themselves — fading is a property of the path a station arrives on, not of
/// the noise around it.
pub struct BandMixer {
    source: BandSource,
    receiver: ReceiverFilter,
    agc: Agc,
    /// The receiver's audio stage, joining after the AGC.
    audio_floor: AudioFloor,
    send_level: f64,
}

impl BandMixer {
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sample_rate = sample_rate.max(1);
        let settings = settings.clone().clamp();
        Self {
            source: BandSource::new(sample_rate, &settings, seed),
            receiver: ReceiverFilter::from_settings(sample_rate, &settings),
            agc: match agc_knee(&settings) {
                Some(knee) => Agc::new(sample_rate, knee),
                None => Agc::off(),
            },
            audio_floor: AudioFloor::from_settings(sample_rate, &settings, seed),
            send_level: 0.0,
        }
    }

    /// Whether these settings put anything on the band at all. When they do
    /// not, there is no background stream to run.
    pub fn needs_background(settings: &TrainingSettings) -> bool {
        let band = &settings.band;
        (band.qrn_enabled && band.qrn_level > 0.0)
            || (band.noise_enabled && band.noise_level > 0.0)
            || band.stations_max > STATIONS_MIN
    }

    pub fn next_background(&mut self) -> f32 {
        // Everything meets at the receiver's filter, which is why narrowing
        // it quiets the whole band at once rather than one layer of it.
        let heard = self.receiver.process(self.source.next_sample());
        // At the gain the receiver is riding at, which a crash has just
        // pulled down and which everything else has to come down with —
        // including the send, which reads this through `agc_gain`.
        let ridden = self.agc.process(heard, self.send_level);
        // The audio stage's own hiss comes in after all of that: the filter
        // never had it, so it runs on outside the passband, and the AGC never
        // saw it, so it neither moves the gain nor ducks under a station.
        soft_limit(ridden + self.audio_floor.next_sample()) as f32
    }

    /// A pile-up is calling with this many stations besides yours: they come
    /// out of how many of the band's own stations may key at once, for as
    /// long as they call. Zero when the send is over.
    pub fn reserve_stations(&mut self, callers: usize) {
        self.source.reserve_stations(callers);
    }

    /// How loud the send is — its peak, as the native player hands it over a
    /// buffer at a time — so the receiver's gain answers to everything
    /// reaching it rather than to the noise alone. That is what a real one
    /// does, and what the browser's compressor does for free by sitting where
    /// the two have already met.
    pub fn note_send_level(&mut self, level: f64) {
        self.send_level = if level.is_finite() {
            level.max(0.0)
        } else {
            0.0
        };
    }

    /// What the AGC is holding the band down to right now.
    ///
    /// The send rides this too. The two are separate playback objects by the
    /// time they reach the sound card, so the only honest way for a crash to
    /// duck the Morse as well is for the send to read the gain the receiver
    /// arrived at.
    pub fn agc_gain(&self) -> f64 {
        self.agc.gain()
    }

    pub fn fill_background(&mut self, out: &mut [f32]) {
        for slot in out {
            *slot = self.next_background();
        }
    }
}

#[cfg(test)]
mod tests;
