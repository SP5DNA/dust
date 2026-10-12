//! Audio-less player for builds with no output backend.
//!
//! A send is waited on for the length of its plan, as on the real backends,
//! so a session keeps its Farnsworth pacing, timing and scoring — it just
//! never makes a sound.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use cw_core::{PlannedTransmission, TrainingSettings, Transmission};

use super::{MorseBackend, PlaybackSignal, WaitFlags};

pub struct MorsePlayer {
    epoch: Rc<Cell<u64>>,
}

struct SilentSignal {
    epoch: Rc<Cell<u64>>,
    mine: u64,
    started: Instant,
}

impl PlaybackSignal for SilentSignal {
    fn poll(&self) -> WaitFlags {
        let elapsed = self.started.elapsed().as_millis();
        WaitFlags {
            cancelled: self.epoch.get() != self.mine,
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
            epoch: Rc::new(Cell::new(0)),
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
        let mine = self.epoch.get().wrapping_add(1);
        self.epoch.set(mine);
        Ok(Rc::new(SilentSignal {
            epoch: Rc::clone(&self.epoch),
            mine,
            started: Instant::now(),
        }))
    }

    fn stop(&mut self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
    }

    fn shutdown(&mut self) {
        self.stop();
    }
}
