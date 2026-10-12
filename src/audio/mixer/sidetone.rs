//! The paddle's sidetone: an oscillator the audio callback runs, gated by
//! the UI without a lock.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::audio::shape::ToneRequest;

/// The sidetone's gate, pitch and level. The UI sets them on every squeeze
/// and the audio thread reads them every frame: atomics, so a squeeze never
/// waits on the mixer and the lag is one audio buffer.
pub struct SidetoneControl {
    on: AtomicBool,
    hz_bits: AtomicU32,
    gain_bits: AtomicU32,
}

impl SidetoneControl {
    pub fn new() -> Self {
        Self {
            on: AtomicBool::new(false),
            hz_bits: AtomicU32::new(600.0f32.to_bits()),
            gain_bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn set(&self, tone: ToneRequest) {
        self.hz_bits.store(tone.hz.to_bits(), Ordering::Relaxed);
        self.gain_bits.store(tone.gain.to_bits(), Ordering::Relaxed);
        self.on.store(tone.on, Ordering::Relaxed);
    }

    pub fn off(&self) {
        self.on.store(false, Ordering::Relaxed);
    }

    fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }
}

/// How fast the sidetone's gate opens and closes: half a millisecond, quick
/// enough to follow a fast fist and slow enough not to click.
const GATE_SEC: f32 = 0.0005;

/// The oscillator itself, owned by the audio callback.
pub struct Sidetone {
    phase: f32,
    /// The gate, eased towards open or shut a sample at a time.
    env: f32,
    coeff: f32,
    sample_rate: f32,
}

impl Sidetone {
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        Self {
            phase: 0.0,
            env: 0.0,
            coeff: 1.0 - (-1.0 / (sample_rate * GATE_SEC)).exp(),
            sample_rate,
        }
    }

    /// Add the sidetone to a buffer of mono frames.
    pub fn add_to(&mut self, out: &mut [f32], control: &SidetoneControl) {
        let tau = std::f32::consts::TAU;
        let hz = f32::from_bits(control.hz_bits.load(Ordering::Relaxed));
        let gain = f32::from_bits(control.gain_bits.load(Ordering::Relaxed));
        let step = tau * hz / self.sample_rate;
        for slot in out {
            // Read the gate every frame. A Default Pulse callback can be
            // hundreds of milliseconds: one sample of "on" would otherwise
            // fill the whole thing.
            let want = if control.is_on() { 1.0 } else { 0.0 };
            self.env += (want - self.env) * self.coeff;
            if want == 0.0 && self.env < 1e-5 {
                // Shut: nothing to work out until the next squeeze, which
                // starts the tone from the top of its cycle.
                self.env = 0.0;
                self.phase = 0.0;
                continue;
            }
            *slot += self.phase.sin() * self.env * gain;
            self.phase += step;
            if self.phase >= tau {
                self.phase -= tau;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 8_000;

    fn run(sidetone: &mut Sidetone, control: &SidetoneControl, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames];
        sidetone.add_to(&mut out, control);
        out
    }

    /// A squeeze opens the gate within a millisecond or two, without a step,
    /// at the pitch and level asked for; letting go closes it the same way
    /// and leaves nothing behind.
    #[test]
    fn the_sidetone_follows_the_key_without_a_click() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        assert!(run(&mut sidetone, &control, 64).iter().all(|s| *s == 0.0));

        control.set(ToneRequest::new(true, 500.0, 0.5));
        let keyed = run(&mut sidetone, &control, 800);
        let peak = keyed.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
        let biggest = keyed
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        // A 500 Hz sine at 0.5 moves at most 0.2 a sample at 8 kHz.
        assert!(biggest < 0.2, "a step of {biggest}");

        control.off();
        let released = run(&mut sidetone, &control, 400);
        assert!(released[..8].iter().any(|s| *s != 0.0), "it cut off");
        assert!(released[200..].iter().all(|s| *s == 0.0), "it never shut");
    }

    /// The sidetone adds to what is already there.
    #[test]
    fn the_sidetone_mixes_rather_than_overwrites() {
        let control = SidetoneControl::new();
        let mut sidetone = Sidetone::new(SR);
        let mut out = vec![0.25f32; 16];
        sidetone.add_to(&mut out, &control);
        assert!(out.iter().all(|s| *s == 0.25));
    }
}
