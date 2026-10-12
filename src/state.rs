use std::cell::RefCell;
use std::rc::Rc;

use cw_core::{GroupSession, SessionResult, TrainingSettings};
use dioxus::prelude::*;

use crate::playback::Preview;
use crate::player::{AudioError, BackendFactory, Player};
use crate::session_runtime::LiveSession;

/// Everything a session writes back into the app: the screen it is on, the
/// session in flight, the history it joins, and the messages it raises.
/// Signals are cheap to copy, so this travels by value.
#[derive(Clone, Copy)]
pub struct SessionSignals {
    pub screen: Signal<Screen>,
    pub runtime: Signal<Option<GroupSession>>,
    pub result: Signal<Option<SessionResult>>,
    pub auto_message: Signal<Option<String>>,
    pub sessions: Signal<Vec<SessionResult>>,
    pub settings: Signal<TrainingSettings>,
    pub toast: Signal<Option<String>>,
    /// Whatever is playing outside a session.
    pub preview: Signal<Preview>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Settings,
    Training,
    Results,
    Stats,
    Listen,
}

/// What the app holds outside the reactive tree: the audio player, and the
/// session being run on it. Cheap to clone; every clone is the same app.
#[derive(Clone)]
pub struct AppState {
    pub player: Player,
    pub session: Rc<RefCell<Option<LiveSession>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self::with_backend(Rc::new(crate::audio::default_backend))
    }

    pub fn with_backend(make_player: BackendFactory) -> Self {
        Self {
            player: Player::new(make_player),
            session: Rc::new(RefCell::new(None)),
        }
    }

    /// Gate a live sidetone for paddle practice.
    pub fn set_live_tone(&self, on: bool, settings: &TrainingSettings) {
        self.player.set_live_tone(on, settings);
    }

    /// Open the sidetone stream ahead of the first squeeze.
    pub fn ensure_live_sidetone(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        self.player.ensure_live_sidetone(settings)
    }
}
