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
//! 1. The band puts white Gaussian noise, impulsive static and other
//!    stations into the antenna ([`BandSource`]). Only the stations have a
//!    pitch.
//! 2. Everything passes through one CW filter ([`FilterDesign`]), the Morse
//!    included. That filter is what gives the hiss its pitch and width and
//!    makes every crash and every dit ring; narrowing it quiets the band,
//!    pushes off-frequency stations down and lengthens the ringing, all at
//!    once, because those are one thing.
//! 3. The AGC rides the gain: a steady few decibels down under a keyed
//!    station, back up in the pauses, and only a moment's duck for a crash.
//!    Its knee is set from the band's own floor ([`agc_knee`]).
//! 4. The receiver's audio stage adds a faint, wide hiss of its own
//!    ([`AudioFloor`]), after the filter and the AGC, so the band is never
//!    quite silent outside the passband — as no real receiver is. A soft
//!    limiter catches whatever is still too loud.
//!
//! QSB is applied to the Morse alone ([`qsb_gain_at`]): fading is a property
//! of the path a station arrives on, not of the noise around it.
//!
//! What it deliberately does not do is invent a sound the receiver would not
//! make. Sparse clicks through resonators a few hertz wide, their tuning
//! swept about, is how a wind-whistle is synthesised. A real CW filter is
//! hundreds of hertz wide, sits still, and is fed with continuous noise; this
//! is that.

mod activity;
mod agc;
mod calibration;
mod dsp;
mod filter;
mod key;
mod knee;
mod memo;
mod noise;
mod output;
mod qsb;
mod random;
mod source;

// What the players, the settings screen and the pile-up planner use. The rest
// is the band's own business.
pub use agc::{
    AGC_ATTACK_SEC, AGC_COMPRESSOR_KNEE_DB, AGC_RELEASE_SEC, CompressorSetting, agc_compressor,
};
pub use calibration::heard_snr_db;
pub(crate) use filter::mirror_hz;
pub use filter::{
    FilterDesign, MAX_RECEIVER_SECTIONS, MAX_RECEIVER_WIDTH_HZ, ReceiverFilter, Section, Sections,
};
pub use key::{BandKey, QsbKey};
pub use knee::agc_knee;
pub use noise::AudioFloor;
pub use output::{LimiterFeed, limiter_feed, soft_limit_curve};
pub use qsb::{QSB_MIN_GAIN, QSB_PATHS, QSB_SPREAD, qsb_gain_at, qsb_path};
pub use source::BandSource;

use crate::settings::{STATIONS_MIN, TrainingSettings};
use agc::Agc;
use calibration::{noise_level, qrn_level};
use output::soft_limit;

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
    /// The band these settings describe, through the receiver they describe.
    /// The source, the filter and the AGC's knee all hear the one receiver.
    pub fn new(sample_rate: u32, settings: &TrainingSettings, seed: u64) -> Self {
        let sample_rate = sample_rate.max(1);
        let settings = settings.clone().clamp();
        let band = &settings.band;
        let receiver = FilterDesign::from_settings(&settings);
        Self {
            source: BandSource::new(sample_rate, band, &receiver, seed),
            receiver: ReceiverFilter::new(sample_rate, &receiver),
            agc: match agc_knee(band, &receiver) {
                Some(knee) => Agc::new(sample_rate, knee),
                None => Agc::off(),
            },
            audio_floor: AudioFloor::for_band(sample_rate, band, seed),
            send_level: 0.0,
        }
    }

    /// Whether these settings put anything on the band at all. When they do
    /// not, there is no background stream to run.
    pub fn needs_background(settings: &TrainingSettings) -> bool {
        let band = &settings.band;
        qrn_level(band) > 0.0 || noise_level(band) > 0.0 || band.stations_max > STATIONS_MIN
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
mod test_support;
#[cfg(test)]
mod tests;
