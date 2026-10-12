use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cw_core::{
    FastrandRng, GroupSession, Sampler, SessionMachine, SessionResult, StationVoice,
    TrainingSettings, Transmission, resolve_pileup, resolve_station,
};
use dioxus::prelude::*;

use crate::audio::MorseBackend;
use crate::time::seed_rng;

/// A factory for the audio backend. Injectable so the session runtime can be
/// driven without a sound card.
pub type BackendFactory = Rc<dyn Fn() -> Result<Box<dyn MorseBackend>, String>>;

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

#[derive(Clone)]
pub struct AppState {
    pub session_gen: Rc<Cell<u64>>,
    pub player: Rc<RefCell<Option<Box<dyn MorseBackend>>>>,
    pub rng: Rc<RefCell<FastrandRng>>,
    pub machine: Rc<RefCell<Option<SessionMachine<Sampler>>>>,
    make_player: BackendFactory,
}

impl AppState {
    pub fn new() -> Self {
        Self::with_backend(Rc::new(crate::audio::default_backend))
    }

    pub fn with_backend(make_player: BackendFactory) -> Self {
        Self {
            session_gen: Rc::new(Cell::new(0)),
            player: Rc::new(RefCell::new(None)),
            rng: Rc::new(RefCell::new(FastrandRng(seed_rng()))),
            machine: Rc::new(RefCell::new(None)),
            make_player,
        }
    }

    pub fn bump_session(&self) -> u64 {
        let next = self.session_gen.get() + 1;
        self.session_gen.set(next);
        *self.machine.borrow_mut() = None;
        next
    }

    fn ensure_player(&self, settings: &TrainingSettings) -> Result<(), String> {
        let mut slot = self
            .player
            .try_borrow_mut()
            .map_err(|_| "Audio is busy.".to_string())?;
        if slot.is_none() {
            *slot = Some((self.make_player)()?);
        }
        if let Some(player) = slot.as_mut() {
            player.resume_from_gesture();
            player.apply_band(settings);
            player.set_live_tone(
                false,
                settings.side_tone_center(),
                settings.band.volume_min.max(0.15),
            );
        }
        Ok(())
    }

    pub(crate) fn rebuild_player(&self, settings: &TrainingSettings) -> Result<(), String> {
        {
            // Scoped so the borrow is over before `ensure_player` takes its own.
            let Ok(mut slot) = self.player.try_borrow_mut() else {
                return Err("Audio is busy.".into());
            };
            if let Some(player) = slot.take().as_mut() {
                player.shutdown();
            }
        }
        self.ensure_player(settings)
    }

    /// Stop current audio, invalidate waiters, then arm the player for a new generation.
    pub fn takeover_audio(&self, settings: &TrainingSettings) -> Result<u64, String> {
        self.stop_sending();
        let generation = self.bump_session();
        self.ensure_player(settings)?;
        Ok(generation)
    }

    /// Tune in a station, drawn from the app's own generator.
    pub fn station(&self, settings: &TrainingSettings) -> StationVoice {
        match self.rng.try_borrow_mut() {
            Ok(mut rng) => resolve_station(settings, &mut *rng),
            // Only reachable if a send were started from inside another; the
            // point is a usable voice, not which one.
            Err(_) => resolve_station(settings, &mut FastrandRng(crate::time::seed_rng())),
        }
    }

    /// `text` from `voice`, with whoever else the settings have calling
    /// across it, each sending what `say` gives it. Drawn afresh each time,
    /// like a session's groups.
    pub fn pileup_for(
        &self,
        settings: &TrainingSettings,
        text: &str,
        voice: StationVoice,
        mut say: impl FnMut(&mut FastrandRng) -> String,
    ) -> Transmission {
        let others = settings.band.stations_max.saturating_sub(1) as usize;
        if others == 0 {
            return Transmission::alone(text, voice);
        }
        let mut draw = |rng: &mut FastrandRng| {
            let texts: Vec<String> = (0..others).map(|_| say(rng)).collect();
            resolve_pileup(settings, &voice, text, &texts, rng)
        };
        let others = match self.rng.try_borrow_mut() {
            Ok(mut rng) => draw(&mut rng),
            // As for `station`: only reachable from inside another send.
            Err(_) => draw(&mut FastrandRng(crate::time::seed_rng())),
        };
        Transmission {
            text: text.into(),
            voice,
            others,
        }
    }

    pub fn apply_band_live(&self, settings: &TrainingSettings) {
        if let Ok(mut slot) = self.player.try_borrow_mut()
            && let Some(player) = slot.as_mut()
        {
            // A slider move has no one to tell. A band that cannot open
            // is reported by the next send, which rebuilds the player.
            player.apply_band(settings);
        }
    }

    /// Stop whatever is being sent, and leave the receiver running. Between
    /// groups the background is meant to keep hissing — a real receiver does
    /// not go silent because the other station stopped keying.
    pub fn stop_sending(&self) {
        if let Ok(mut slot) = self.player.try_borrow_mut()
            && let Some(player) = slot.as_mut()
        {
            player.stop();
        }
    }

    /// Everything off, receiver included. What "stop" means when the user
    /// pressed it, or walked away from the screen that was making the sound.
    pub fn silence_audio(&self) {
        if let Ok(mut slot) = self.player.try_borrow_mut()
            && let Some(player) = slot.as_mut()
        {
            player.shutdown();
        }
    }

    /// Gate a live sidetone for paddle practice. Opens the player if needed so
    /// a squeeze outside training still has somewhere to go.
    pub fn set_live_tone(&self, on: bool, settings: &TrainingSettings) {
        // A key that cannot sound has nowhere to report it mid-squeeze;
        // `ensure_live_sidetone` is where the reason is surfaced.
        let _ = self.gate_live_tone(on, settings);
    }

    fn gate_live_tone(&self, on: bool, settings: &TrainingSettings) -> Result<(), String> {
        let hz = settings.side_tone_center();
        let gain = settings.band.volume_min.max(0.15);
        // Busy means a send is being scheduled this instant; the next
        // squeeze will find it free.
        let Ok(mut slot) = self.player.try_borrow_mut() else {
            return Ok(());
        };
        if slot.is_none() {
            *slot = Some((self.make_player)()?);
        }
        if let Some(player) = slot.as_mut() {
            player.resume_from_gesture();
            player.set_live_tone(on, hz, gain);
        }
        Ok(())
    }

    /// Open the silent sidetone stream so the first paddle squeeze is one
    /// audio buffer of lag, not a stream start. Reports why when it cannot.
    pub fn ensure_live_sidetone(&self, settings: &TrainingSettings) -> Result<(), String> {
        self.gate_live_tone(false, settings)
    }
}

#[cfg(test)]
mod tests {
    use crate::audio::fake::Call;
    use crate::testing::{Harness, run, test_settings};

    #[test]
    fn taking_the_audio_over_stops_what_was_playing_and_moves_the_generation_on() {
        run(|| async {
            let h = Harness::new();
            let settings = test_settings();
            let first = h.app.takeover_audio(&settings).expect("player");
            assert_eq!(first, 1);
            assert!(h.calls().contains(&Call::New));

            h.recorder.clear();
            let second = h.app.takeover_audio(&settings).expect("player");
            assert_eq!(second, 2);
            assert!(h.calls().contains(&Call::Stop));
            // The same player is reused: taking over is not a rebuild.
            assert_eq!(h.recorder.players_built.get(), 1);
        });
    }

    #[test]
    fn a_player_that_will_not_open_is_reported_rather_than_ignored() {
        run(|| async {
            let h = Harness::new();
            h.recorder.build_error.set(true);
            let err = h.app.takeover_audio(&test_settings()).unwrap_err();
            assert_eq!(err, "No audio output device found");
            // The generation still moved, so nothing stale survives the attempt.
            assert_eq!(h.app.session_gen.get(), 1);
        });
    }

    #[test]
    fn stopping_and_shutting_down_without_a_player_are_harmless() {
        run(|| async {
            let h = Harness::new();
            h.app.stop_sending();
            h.app.silence_audio();
            h.app.apply_band_live(&test_settings());
            assert!(h.calls().is_empty());
        });
    }

    #[test]
    fn the_band_follows_the_settings_while_a_preview_runs() {
        run(|| async {
            let h = Harness::new();
            let mut settings = test_settings();
            h.app.takeover_audio(&settings).expect("player");
            h.recorder.clear();
            settings.band.qrn_enabled = true;
            settings.band.qrn_level = 0.4;
            h.app.apply_band_live(&settings);
            assert_eq!(
                h.calls(),
                vec![Call::Band(cw_core::band::BandKey::of(&settings.band))]
            );
        });
    }

    #[test]
    fn one_station_calling_is_sent_alone_and_more_bring_a_pile_up() {
        let h = Harness::new();
        let mut settings = test_settings();
        let voice = h.app.station(&settings);
        let say = |_: &mut cw_core::FastrandRng| "DL1ABC".to_string();
        assert!(
            h.app
                .pileup_for(&settings, "CQ", voice, say)
                .others
                .is_empty()
        );
        settings.band.stations_min = 4;
        settings.band.stations_max = 4;
        let call = h.app.pileup_for(&settings, "CQ", voice, say);
        assert_eq!(call.text, "CQ");
        assert_eq!(call.voice, voice);
        assert!(
            (1..=3).contains(&call.others.len()),
            "{}",
            call.others.len()
        );
        assert!(call.others.iter().all(|other| other.text == "DL1ABC"));
    }

    #[test]
    fn taking_over_a_borrowed_player_is_refused() {
        run(|| async {
            let h = Harness::new();
            let player = h.app.player.clone();
            let held = player.borrow_mut();
            assert_eq!(
                h.app.takeover_audio(&test_settings()).unwrap_err(),
                "Audio is busy."
            );
            drop(held);
        });
    }
}
