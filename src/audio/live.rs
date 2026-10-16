//! Lock-free paddle sidetone gate.
//!
//! The audio callback reads these atomics on a cpal thread. The UI / keyer
//! writes them without borrowing the player, so a squeeze is never dropped
//! because Morse playback briefly held the `RefCell`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Gate and pitch for the paddle sidetone. Shared by the mixer callback and
/// whoever is keying — no locks on the hot path.
#[derive(Debug)]
pub struct LiveToneGate {
    on: AtomicBool,
    hz: AtomicU32,
    gain_bits: AtomicU32,
}

impl LiveToneGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            on: AtomicBool::new(false),
            hz: AtomicU32::new(500),
            gain_bits: AtomicU32::new(0.2f32.to_bits()),
        })
    }

    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    pub fn set(&self, on: bool, frequency_hz: f64, gain: f64) {
        let hz = frequency_hz.round().clamp(100.0, 2_000.0) as u32;
        let gain = gain.clamp(0.0, 1.0) as f32;
        self.hz.store(hz, Ordering::Relaxed);
        self.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
        self.set_on(on);
    }

    pub(crate) fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub(crate) fn hz(&self) -> f32 {
        self.hz.load(Ordering::Relaxed) as f32
    }

    pub(crate) fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Relaxed))
    }
}
