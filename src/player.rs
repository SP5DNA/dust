//! The audio player, and whose turn it is to use it.
//!
//! One player sounds at a time, and several things want it: a training
//! session, the band preview, the letter player, an envelope sample. Whoever
//! claims it is handed a [`Lease`], and claiming it again — or giving it back
//! — ends every lease before. A lease is all a task needs to know whether the
//! audio is still its own: every send and every wait goes through one, and a
//! lease that has ended can neither send nor wake up as though it had not.
//!
//! This is also the only place the app talks to a [`MorseBackend`].

use std::cell::{Cell, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

use cw_core::{TrainingSettings, Transmission};

use crate::audio::{MorseBackend, PlaybackOutcome};
use crate::time::{POLL_MS, sleep_ms};

/// A factory for the audio backend. Injectable so the app can be driven
/// without a sound card.
pub type BackendFactory = Rc<dyn Fn() -> Result<Box<dyn MorseBackend>, String>>;

/// How many times one send is tried before the caller is told the audio is
/// gone. Each retry rebuilds the player, which is what picks up a device that
/// was swapped, unplugged or suspended mid-send.
const SEND_ATTEMPTS: u32 = 3;

/// The quietest the live sidetone is played, whatever the band's volume
/// floor: a paddle you cannot hear is no use.
const SIDETONE_FLOOR: f64 = 0.15;

/// Why audio did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// The lease ended before the work did: something else took the audio,
    /// or the send was stopped. Nothing went wrong, so nothing is shown.
    Cancelled,
    /// The device would not open, or would not take the send. Its own words.
    Device(String),
    /// A send started and never reached its end: a stream that stopped
    /// calling back, an audio clock that stopped moving.
    Stalled,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Audio was stopped."),
            Self::Device(message) => f.write_str(message),
            Self::Stalled => f.write_str("Audio playback stalled."),
        }
    }
}

/// A send that played to its end: how long it was, and the speeds it went at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sent {
    pub duration_sec: f64,
    pub char_wpm: f64,
    pub effective_wpm: f64,
}

/// The one audio player the app has. Cheap to clone; every clone is the same
/// player.
#[derive(Clone)]
pub struct Player(Rc<Shared>);

struct Shared {
    backend: RefCell<Option<Box<dyn MorseBackend>>>,
    make: BackendFactory,
    /// Moves on every claim and every release. A lease is current while the
    /// turn is still the one it was handed.
    turn: Cell<u64>,
    /// Moves whenever the send in flight is stopped, so a send that has not
    /// started yet — one waiting out a retry — can tell it is no longer wanted.
    sends: Cell<u64>,
}

impl Player {
    pub fn new(make: BackendFactory) -> Self {
        Self(Rc::new(Shared {
            backend: RefCell::new(None),
            make,
            turn: Cell::new(0),
            sends: Cell::new(0),
        }))
    }

    /// Take the audio. Whatever had it stops, the player is opened for the
    /// settings, and the lease that comes back is the new holder's.
    ///
    /// Whatever had it is ended even when the player cannot be opened, so a
    /// failed claim leaves nothing sounding and nothing holding the audio.
    pub fn claim(&self, settings: &TrainingSettings) -> Result<Lease, AudioError> {
        self.stop_sending();
        let turn = self.next_turn();
        match self.open(settings) {
            Ok(()) => Ok(Lease {
                player: self.clone(),
                turn,
            }),
            Err(err) => {
                self.with_backend(|backend| backend.shutdown());
                Err(err)
            }
        }
    }

    /// Give the audio back: every lease ends, and nothing sounds, the
    /// receiver included. What "stop" means when the user pressed it, or
    /// walked away from the screen that was making the sound.
    pub fn release(&self) {
        self.next_turn();
        self.with_backend(|backend| backend.shutdown());
    }

    /// Bring the band in line with settings that have just changed, while a
    /// preview of it is playing.
    pub fn follow_band(&self, settings: &TrainingSettings) {
        // A slider move has no one to tell. A band that cannot open is
        // reported by the next send, which rebuilds the player.
        self.with_backend(|backend| backend.apply_band(settings));
    }

    /// Gate the live sidetone for paddle practice. Opens the player if there
    /// is none, so a squeeze outside training still has somewhere to go.
    pub fn set_live_tone(&self, on: bool, settings: &TrainingSettings) {
        // A key that cannot sound has nowhere to report it mid-squeeze;
        // `ensure_live_sidetone` is where the reason is surfaced.
        let _ = self.gate_live_tone(on, settings);
    }

    /// Open the silent sidetone stream, so the first paddle squeeze is one
    /// audio buffer of lag rather than a stream start. Says why when it
    /// cannot.
    pub fn ensure_live_sidetone(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        self.gate_live_tone(false, settings)
    }

    fn gate_live_tone(&self, on: bool, settings: &TrainingSettings) -> Result<(), AudioError> {
        let (hz, gain) = live_tone(settings);
        let mut backend = self.opened()?;
        backend.resume_from_gesture();
        backend.set_live_tone(on, hz, gain);
        Ok(())
    }

    fn next_turn(&self) -> u64 {
        let turn = self.0.turn.get() + 1;
        self.0.turn.set(turn);
        turn
    }

    /// Stop the send in flight and anything waiting to start one, leaving
    /// the receiver running. Between groups the background is meant to keep
    /// hissing: a real receiver does not go silent because the other station
    /// stopped keying.
    fn stop_sending(&self) {
        self.0.sends.set(self.0.sends.get() + 1);
        self.with_backend(|backend| backend.stop());
    }

    /// The backend, opened first if there is none.
    fn opened(&self) -> Result<RefMut<'_, Box<dyn MorseBackend>>, AudioError> {
        let mut slot = self.0.backend.borrow_mut();
        if slot.is_none() {
            *slot = Some((self.0.make)().map_err(AudioError::Device)?);
        }
        RefMut::filter_map(slot, Option::as_mut).map_err(|_| unavailable())
    }

    /// Open the player if there is none, and set it up for `settings`.
    fn open(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        let (hz, gain) = live_tone(settings);
        let mut backend = self.opened()?;
        backend.resume_from_gesture();
        backend.apply_band(settings);
        backend.set_live_tone(false, hz, gain);
        Ok(())
    }

    /// Throw the player away and open a fresh one.
    fn rebuild(&self, settings: &TrainingSettings) -> Result<(), AudioError> {
        // Taken out first, so the borrow is over before `open` takes its own.
        let old = self.0.backend.borrow_mut().take();
        if let Some(mut old) = old {
            old.shutdown();
        }
        self.open(settings)
    }

    /// The way in to the backend, when there is one. Every borrow is
    /// synchronous, so a borrow that is already out is a bug, and panics
    /// rather than being skipped in silence.
    fn with_backend(&self, act: impl FnOnce(&mut dyn MorseBackend)) {
        if let Some(backend) = self.0.backend.borrow_mut().as_mut() {
            act(backend.as_mut());
        }
    }

    /// Start one send on the player as it is.
    fn start(
        &self,
        transmission: &Transmission,
        settings: &TrainingSettings,
    ) -> Result<crate::audio::PlaybackWait, AudioError> {
        let mut slot = self.0.backend.borrow_mut();
        let backend = slot.as_mut().ok_or_else(unavailable)?;
        crate::audio::start_send(backend.as_mut(), transmission, settings)
            .map_err(AudioError::Device)
    }
}

fn unavailable() -> AudioError {
    AudioError::Device("Audio is unavailable.".into())
}

/// The live sidetone's pitch and level for `settings`.
fn live_tone(settings: &TrainingSettings) -> (f64, f64) {
    (
        settings.side_tone_center(),
        settings.band.volume_min.max(SIDETONE_FLOOR),
    )
}

/// The audio, held: a claim nothing has superseded yet.
#[derive(Clone)]
pub struct Lease {
    player: Player,
    turn: u64,
}

impl Lease {
    /// Whether this lease still holds the audio.
    pub fn is_current(&self) -> bool {
        self.player.0.turn.get() == self.turn
    }

    /// Whether two leases are the same claim.
    pub fn is(&self, other: &Lease) -> bool {
        self.turn == other.turn && Rc::ptr_eq(&self.player.0, &other.player.0)
    }

    /// Which claim this is, in the order they were made.
    pub fn turn(&self) -> u64 {
        self.turn
    }

    /// Give the audio back if this lease still holds it. Says whether it did.
    pub fn release(&self) -> bool {
        let current = self.is_current();
        if current {
            self.player.release();
        }
        current
    }

    /// Stop the send in flight, if this lease still holds the audio. The
    /// receiver keeps running.
    pub fn stop_sending(&self) {
        if self.is_current() {
            self.player.stop_sending();
        }
    }

    /// Wait `ms`, in one go, and say whether the lease outlived it.
    pub async fn sleep(&self, ms: u32) -> Result<(), AudioError> {
        sleep_ms(ms).await;
        self.check()
    }

    fn check(&self) -> Result<(), AudioError> {
        if self.is_current() {
            Ok(())
        } else {
            Err(AudioError::Cancelled)
        }
    }

    /// Send `transmission` and wait for it to be heard.
    ///
    /// A send that fails is tried again on a freshly built player, up to
    /// [`SEND_ATTEMPTS`] in all, and the last failure is the one reported.
    /// A send that is stopped — the lease ending, or [`Lease::stop_sending`]
    /// — comes back [`AudioError::Cancelled`] from wherever it had got to,
    /// a retry included.
    pub async fn send(
        &self,
        transmission: &Transmission,
        settings: &TrainingSettings,
    ) -> Result<Sent, AudioError> {
        let ticket = Ticket {
            lease: self,
            sends: self.player.0.sends.get(),
        };
        let mut outcome = ticket.attempt(transmission, settings).await;
        for _ in 1..SEND_ATTEMPTS {
            if matches!(outcome, Ok(_) | Err(AudioError::Cancelled)) {
                break;
            }
            let rebuilt = self.player.rebuild(settings);
            sleep_ms(POLL_MS).await;
            ticket.check()?;
            outcome = match rebuilt {
                Ok(()) => ticket.attempt(transmission, settings).await,
                // There is no player to send through, and the reason it could
                // not be opened is the one worth showing.
                Err(err) => Err(err),
            };
        }
        outcome
    }
}

/// One send's right to the player: the lease it was made under, and which
/// send it is.
struct Ticket<'a> {
    lease: &'a Lease,
    sends: u64,
}

impl Ticket<'_> {
    fn check(&self) -> Result<(), AudioError> {
        self.lease.check()?;
        if self.lease.player.0.sends.get() == self.sends {
            Ok(())
        } else {
            Err(AudioError::Cancelled)
        }
    }

    async fn attempt(
        &self,
        transmission: &Transmission,
        settings: &TrainingSettings,
    ) -> Result<Sent, AudioError> {
        let player = &self.lease.player;
        self.check()?;
        let wait = player.start(transmission, settings)?;
        let sent = Sent {
            duration_sec: wait.duration_sec,
            char_wpm: wait.char_wpm,
            effective_wpm: wait.effective_wpm,
        };
        match wait.wait().await {
            PlaybackOutcome::Completed => self.check().map(|()| sent),
            PlaybackOutcome::Cancelled => Err(AudioError::Cancelled),
            // The send never reached its end: rebuilding the player on the
            // next attempt is what recovers it.
            PlaybackOutcome::Failed => Err(AudioError::Stalled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call, Recorder};
    use crate::testing::{run, test_settings};

    fn player() -> (Player, Rc<Recorder>) {
        let recorder = Recorder::new();
        (Player::new(recorder.factory()), recorder)
    }

    fn cq() -> Transmission {
        let voice = cw_core::resolve_station(&test_settings(), &mut cw_core::SplitMix64::new(1));
        Transmission::alone("CQ", voice)
    }

    #[test]
    fn a_claim_ends_the_lease_before_it_and_keeps_the_player() {
        run(|| async {
            let (player, recorder) = player();
            let first = player.claim(&test_settings()).expect("a player");
            assert!(first.is_current());
            assert_eq!(recorder.calls().first(), Some(&Call::New));

            let second = player.claim(&test_settings()).expect("a player");
            assert!(!first.is_current() && second.is_current());
            assert!(!first.is(&second) && second.is(&second.clone()));
            assert!(recorder.calls().contains(&Call::Stop));
            // Taking over is not a rebuild.
            assert_eq!(recorder.players_built.get(), 1);

            // An ended lease cannot give back what it no longer holds.
            assert!(!first.release());
            assert!(second.is_current());
            assert!(second.release());
            assert!(!second.is_current());
            assert_eq!(recorder.calls().last(), Some(&Call::Shutdown));
        });
    }

    /// A claim that cannot open a player still ends whatever had the audio,
    /// and leaves nothing sounding behind it.
    #[test]
    fn a_failed_claim_still_ends_what_came_before() {
        run(|| async {
            let mut settings = test_settings();
            settings.band.noise_enabled = true;
            let (player, recorder) = player();
            let first = player.claim(&settings).expect("a player");
            assert!(recorder.band_running.get());

            // The player goes, and no new one can be had.
            player.0.backend.borrow_mut().take();
            recorder.build_error.set(true);
            assert_eq!(
                player.claim(&settings).err(),
                Some(AudioError::Device("No audio output device found".into()))
            );
            assert!(!first.is_current());
        });
    }

    #[test]
    fn nothing_to_stop_is_no_trouble() {
        let (player, recorder) = player();
        player.release();
        player.follow_band(&test_settings());
        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn the_band_follows_the_settings_while_a_preview_runs() {
        run(|| async {
            let (player, recorder) = player();
            let mut settings = test_settings();
            let _lease = player.claim(&settings).expect("a player");
            recorder.clear();
            settings.band.qrn_enabled = true;
            settings.band.qrn_level = 0.4;
            player.follow_band(&settings);
            assert_eq!(
                recorder.calls(),
                vec![Call::Band(cw_core::band::BandKey::of(&settings.band))]
            );
        });
    }

    #[test]
    fn a_send_reports_how_it_was_sent() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            let sent = lease.send(&cq(), &test_settings()).await.expect("sent");
            assert!(sent.duration_sec > 0.0);
            assert_eq!(sent.char_wpm, 40.0);
            assert_eq!(recorder.texts(), vec!["CQ".to_string()]);
        });
    }

    #[test]
    fn a_send_under_an_ended_lease_never_starts() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            player.release();
            assert_eq!(
                lease.send(&cq(), &test_settings()).await,
                Err(AudioError::Cancelled)
            );
            assert!(recorder.texts().is_empty());
            assert_eq!(lease.sleep(10).await, Err(AudioError::Cancelled));
        });
    }

    /// The last failure is the one reported, after a fresh player for each
    /// retry.
    #[test]
    fn a_failing_device_is_retried_on_a_fresh_player_then_reported() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            recorder.set(Behaviour::FailMidSend);
            assert_eq!(
                lease.send(&cq(), &test_settings()).await,
                Err(AudioError::Stalled)
            );
            assert_eq!(recorder.texts().len(), SEND_ATTEMPTS as usize);
            assert_eq!(recorder.players_built.get(), SEND_ATTEMPTS);

            recorder.set(Behaviour::RefuseToStart);
            assert_eq!(
                lease.send(&cq(), &test_settings()).await,
                Err(AudioError::Device("Audio stream: device is gone".into()))
            );
        });
    }

    #[test]
    fn a_send_with_no_player_left_opens_one() {
        run(|| async {
            let (player, recorder) = player();
            let lease = player.claim(&test_settings()).expect("a player");
            // The player is gone, the way a failed rebuild leaves it.
            player.0.backend.borrow_mut().take();
            assert!(lease.send(&cq(), &test_settings()).await.is_ok());
            assert_eq!(recorder.players_built.get(), 2);
        });
    }

    #[test]
    fn every_failure_says_what_happened() {
        assert_eq!(
            AudioError::Device("No audio output device found".into()).to_string(),
            "No audio output device found"
        );
        assert_eq!(AudioError::Stalled.to_string(), "Audio playback stalled.");
    }
}
