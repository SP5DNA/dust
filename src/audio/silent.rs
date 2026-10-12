//! Audio-less player for builds with no output backend.
//!
//! A send is waited on for the length of its plan, as on the real backends,
//! so a session keeps its Farnsworth pacing, timing and scoring — it just
//! never makes a sound.

use std::rc::Rc;
use std::time::Instant;

use cw_core::{PlannedTransmission, TrainingSettings, Transmission};

use super::epoch::{Epoch, Ticket};
use super::{MorseBackend, PlaybackSignal, WaitFlags};

pub struct MorsePlayer {
    epoch: Epoch,
}

struct SilentSignal {
    ticket: Ticket,
    started: Instant,
}

impl PlaybackSignal for SilentSignal {
    fn poll(&self) -> WaitFlags {
        let elapsed = self.started.elapsed().as_millis();
        WaitFlags {
            cancelled: self.ticket.is_cancelled(),
            finished: false,
            failed: false,
            // There is no audio to park, so a send always runs to its length.
            suspended: false,
            // Nothing is playing, so the wall clock is the only clock there is.
            played_ms: Some(elapsed.min(u128::from(u32::MAX)) as u32),
        }
    }
}

impl MorsePlayer {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            epoch: Epoch::default(),
        })
    }
}

impl MorseBackend for MorsePlayer {
    fn apply_band(&mut self, _settings: &TrainingSettings) {}

    fn start(
        &mut self,
        _transmission: &Transmission,
        _plan: &PlannedTransmission,
        _settings: &TrainingSettings,
    ) -> Result<Rc<dyn PlaybackSignal>, String> {
        // Nothing is heard: the plan's length, which the wait already has,
        // is all there is to a send.
        Ok(Rc::new(SilentSignal {
            ticket: self.epoch.arm(),
            started: Instant::now(),
        }))
    }

    fn stop(&mut self) {
        self.epoch.bump();
    }

    fn shutdown(&mut self) {
        self.stop();
    }
}
