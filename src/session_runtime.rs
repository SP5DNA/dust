//! Runs a training session: the machine decides, and this carries out what
//! it decides — sends, timers, the results screen — under the session's
//! lease on the audio. The UI only hands it events.

use cw_core::{
    CharSamplingState, FastrandRng, MonoMs, Sampler, SessionEffect, SessionEvent, SessionId,
    SessionMachine, Transmission, apply_auto_level, evaluate_auto_level, generate_training_group,
    resolve_station,
};
use dioxus::prelude::*;

use crate::persist::{
    clear_auto_counters, load_auto_counters, save_auto_counters, save_sessions, save_settings,
};
use crate::playback::Preview;
use crate::player::{AudioError, Lease};
use crate::state::{AppState, Screen, SessionSignals};
use crate::time::{local_date_string, mono_ms, now_ms, seed_rng};

/// A session in progress: the machine that decides it, the stations it is
/// sent by, and its hold on the audio.
pub struct LiveSession {
    machine: SessionMachine<Sampler>,
    /// Where every group's stations are drawn from. Drawn afresh for each
    /// session, so no two sessions — in one launch or across launches — hear
    /// the same operators.
    stations: FastrandRng,
    lease: Lease,
}

impl LiveSession {
    /// What the receiver hears for group `index`: the station being copied,
    /// and whoever else is calling across it.
    ///
    /// Both are drawn from generators that depend only on the session and
    /// the group, so every repeat of a group — and every retry after a
    /// stalled send — comes from the same operators, with the same pile-up
    /// around them: a station repeating its call does not change frequency,
    /// speed or strength between sends. Nothing has to be carried across the
    /// sends for that, while the next group is somebody else.
    ///
    /// The others send groups of their own, drawn from the same curriculum
    /// so they sound like stations rather than noise — but off a blank
    /// slate, so a pile-up never teaches the sampler anything. Only the
    /// station you answer counts.
    fn transmission(&self, index: usize, text: String) -> Transmission {
        let settings = self.machine.session().settings();
        let group = self.stations.fork(index as u64);
        let voice = resolve_station(settings, &mut group.fork(GroupStream::Voice as u64));
        let mut elsewhere = CharSamplingState::default();
        Transmission::calling(
            settings,
            text,
            voice,
            &mut group.fork(GroupStream::Callers as u64),
            |rng| generate_training_group(settings, &mut elsewhere, rng),
        )
    }
}

/// The generators one group draws from, one per thing it decides.
#[derive(Clone, Copy)]
enum GroupStream {
    /// The station you are copying.
    Voice,
    /// Whoever else is calling.
    Callers,
}

/// Whether a session has the app: on its screen, or about to be.
pub fn session_running(signals: SessionSignals) -> bool {
    matches!(*signals.screen.peek(), Screen::Training) || signals.runtime.peek().is_some()
}

/// Start a session from the settings as they are.
///
/// Starting first settles the alphabet, and stores the result, so the
/// session and the screen agree on what is being practised before anything
/// sounds. The machine is built here, on the UI thread, so Training has its
/// groups before the first paint.
pub fn start_session(app: &AppState, signals: SessionSignals) {
    // Held before anything is written, not just before the audio is taken:
    // a session already running must leave the stored settings exactly as
    // they are.
    if session_running(signals) {
        return;
    }
    let SessionSignals {
        mut screen,
        mut runtime,
        mut settings,
        sessions,
        mut toast,
        mut preview,
        ..
    } = signals;
    let fitted = settings.peek().clone().clamp();
    if *settings.peek() != fitted {
        settings.set(fitted.clone());
    }
    // The session has the audio now, whether or not it can open it: any
    // preview that had it has stopped.
    preview.set(Preview::Idle);
    let lease = match app.player.claim(&fitted) {
        Ok(lease) => lease,
        Err(err) => {
            toast.set(Some(err.to_string()));
            return;
        }
    };
    // Each session draws from a sampler of its own, seeded afresh.
    let sampler = Sampler::new(&sessions.peek(), &fitted, seed_rng());
    let (machine, effects) = SessionMachine::start(
        SessionId::new(lease.turn()),
        MonoMs(mono_ms()),
        fitted,
        sampler,
    );
    runtime.set(Some(machine.session().clone()));
    screen.set(Screen::Training);
    *app.session.borrow_mut() = Some(LiveSession {
        machine,
        stations: FastrandRng(seed_rng()),
        lease: lease.clone(),
    });
    run_effects(app, signals, &lease, effects);
}

/// Hand the running session something the user did. Nothing happens when
/// no session is running.
pub fn send_event(app: &AppState, signals: SessionSignals, event: SessionEvent) {
    let lease = app.session.borrow().as_ref().map(|live| live.lease.clone());
    if let Some(lease) = lease {
        dispatch(app, signals, &lease, event);
    }
}

/// Let the machine decide what `event` means, then do what it says.
///
/// `lease` is the session the event belongs to: an event from a session
/// that has ended, or from one before the session now running, is dropped
/// rather than handed to a machine it means nothing to.
fn dispatch(app: &AppState, signals: SessionSignals, lease: &Lease, event: SessionEvent) {
    let mut runtime = signals.runtime;
    let effects = {
        let mut slot = app.session.borrow_mut();
        let Some(live) = slot
            .as_mut()
            .filter(|live| live.lease.is(lease) && lease.is_current())
        else {
            return;
        };
        let effects = live.machine.apply(event, MonoMs(mono_ms()));
        runtime.set(Some(live.machine.session().clone()));
        effects
    };
    run_effects(app, signals, lease, effects);
}

fn run_effects(
    app: &AppState,
    signals: SessionSignals,
    lease: &Lease,
    effects: Vec<SessionEffect>,
) {
    for effect in effects {
        match effect {
            SessionEffect::StopAudio => lease.stop_sending(),
            SessionEffect::Play { index, text } => play(app, signals, lease, index, text),
            SessionEffect::StartTimer { id, ms } => {
                let (app, lease) = (app.clone(), lease.clone());
                // One wait for the whole of it. A timer the machine has since
                // forgotten wakes once, is told so, and is gone.
                spawn(async move {
                    if lease.sleep(ms).await.is_ok() {
                        dispatch(&app, signals, &lease, SessionEvent::TimerFired { id });
                    }
                });
            }
            SessionEffect::PersistAndShowResults => finish_session(app, signals),
            SessionEffect::AbortToHome => abandon_session(app, signals),
        }
    }
}

/// Send group `index`, and tell the machine how it went.
fn play(app: &AppState, signals: SessionSignals, lease: &Lease, index: usize, text: String) {
    // Every group is built from the settings the session started with, not
    // from whatever the settings screen holds now: a change made mid-session
    // belongs to the next one.
    let Some((settings, transmission)) = app.session.borrow().as_ref().map(|live| {
        let settings = live.machine.session().settings().clone();
        (settings, live.transmission(index, text))
    }) else {
        return;
    };
    let (app, lease) = (app.clone(), lease.clone());
    let mut toast = signals.toast;
    spawn(async move {
        let event = match lease.send(&transmission, &settings).await {
            Ok(sent) => SessionEvent::PlaybackEnded {
                index,
                duration_sec: sent.duration_sec,
                char_wpm: sent.char_wpm,
                effective_wpm: sent.effective_wpm,
            },
            Err(AudioError::Cancelled) => SessionEvent::PlaybackStopped { index },
            Err(failure) => {
                toast.set(Some(failure.to_string()));
                SessionEvent::PlaybackStopped { index }
            }
        };
        dispatch(&app, signals, &lease, event);
    });
}

/// End the session without keeping it.
fn abandon_session(app: &AppState, signals: SessionSignals) {
    let SessionSignals {
        mut screen,
        mut runtime,
        ..
    } = signals;
    let live = app.session.borrow_mut().take();
    if let Some(live) = live {
        live.lease.release();
    }
    runtime.set(None);
    screen.set(Screen::Home);
}

/// Score the session, move the level if it earned it, store it, and show
/// the results.
fn finish_session(app: &AppState, signals: SessionSignals) {
    let SessionSignals {
        mut screen,
        mut runtime,
        mut result,
        mut auto_message,
        mut sessions,
        settings: mut settings_sig,
        mut toast,
        ..
    } = signals;
    let Some(live) = app.session.borrow_mut().take() else {
        return;
    };
    live.lease.release();
    let session = live.machine.session();
    let settings = session.settings().clone();
    // The machine only finishes a session it has scored something in.
    let built = session.result(now_ms(), local_date_string());

    let mut counters = load_auto_counters(&settings);
    let mut next_settings = settings.clone();
    if let Some(change) = evaluate_auto_level(&built, &settings, &mut counters) {
        clear_auto_counters(&change.counters_cleared);
        apply_auto_level(&mut next_settings, &change);
        settings_sig.set(next_settings.clone());
        save_settings(&next_settings);
        let message = crate::ui::auto_level::change_message(&change);
        auto_message.set(Some(message.clone()));
        toast.set(Some(message));
    } else {
        save_auto_counters(&settings, counters);
        auto_message.set(None);
    }

    let mut history = sessions();
    history.push(built.clone());
    save_sessions(&history);
    sessions.set(history);
    result.set(Some(built));
    runtime.set(None);
    screen.set(Screen::Results);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::{Behaviour, Call};
    use crate::testing::{Harness, run, test_settings};
    use cw_core::AutoLevelCounters;
    use cw_core::CharSetMode;
    use cw_core::SessionEffect;
    use cw_core::SessionPhase;

    impl LiveSession {
        /// The session's hold on the audio, for a test to end or to stop.
        pub(crate) fn lease(&self) -> &Lease {
            &self.lease
        }
    }

    /// A whole session, answered correctly, ends on the results screen with the
    /// session stored.
    #[test]
    fn answering_every_group_finishes_the_session_and_stores_it() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 2);
            assert!(result.groups.iter().all(|g| g.correct));
            assert_eq!(result.accuracy, 1.0);
            assert_eq!(h.sessions.peek().len(), 1);
            assert_eq!(crate::persist::load_sessions().len(), 1);
            // The machine is torn down, and the audio with it.
            assert!(h.app.session.borrow().is_none());
            assert!(h.calls().contains(&Call::Shutdown));
            // Two groups, one send each.
            assert_eq!(h.texts().len(), 2);
        });
    }

    #[test]
    fn a_group_nobody_answers_times_out_and_the_session_moves_on() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.group_timeout = 1.0;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            // Nothing typed: the answer window closes on its own.
            assert!(
                h.run_until(4_000, |h| h.texts().len() == 2).await,
                "the second group should have been sent"
            );
            let confirmed = h.runtime.peek().as_ref().map(|s| s.view().confirmed);
            assert_eq!(confirmed, Some(vec![true, false]));
            assert_eq!(
                h.runtime
                    .peek()
                    .as_ref()
                    .and_then(|s| s.group(0))
                    .map(|g| g.input().to_string()),
                Some(String::new())
            );
        });
    }

    #[test]
    fn a_group_sent_more_than_once_opens_the_answer_box_only_at_the_end() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            settings.playback.link_group_repeat = true;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            let sent = h.sent(0);

            assert!(h.run_until(20_000, |h| h.texts().len() == 3).await);
            // Every send is the same group.
            assert_eq!(h.texts(), vec![sent.clone(); 3]);
            assert!(h.run_until(5_000, |h| h.awaiting_answer() == Some(0)).await);
            // And no fourth send after the answer window opens.
            h.advance(2_000).await;
            assert_eq!(h.texts().len(), 3);
        });
    }

    #[test]
    fn a_matching_answer_confirms_itself_and_the_next_group_follows() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let first = h.sent(0);
            h.type_answer(0, &first.to_lowercase());
            // Nothing is confirmed until the auto-confirm delay has passed.
            assert!(
                h.runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| !s.view().confirmed[0])
            );
            assert!(
                h.run_until(1_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
            // Typed in lower case, stored the way it was sent.
            assert_eq!(
                h.runtime
                    .peek()
                    .as_ref()
                    .and_then(|s| s.group(0))
                    .map(|g| g.input().to_string()),
                Some(first)
            );
            // The next group is generated before it is played, never sent empty.
            assert!(h.run_until(8_000, |h| h.texts().len() == 2).await);
            assert!(h.texts().iter().all(|text| !text.is_empty()));
            assert_ne!(h.sent(1), "");
        });
    }

    #[test]
    fn changing_an_answer_withdraws_the_confirm_that_was_pending() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            // Backspace inside the 300 ms window.
            h.advance(100).await;
            h.type_answer(0, &sent[..1]);
            h.advance(1_000).await;
            assert!(
                h.runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| !s.view().confirmed[0])
            );
            assert_eq!(h.awaiting_answer(), Some(0));
        });
    }

    #[test]
    fn typing_is_refused_while_the_group_is_being_sent() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            h.advance(1_000).await;
            assert!(
                h.runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| !s.view().confirmed[0])
            );
        });
    }

    /// A session that ends mid-send leaves nothing behind to disturb the
    /// next one: the send it cancelled must not end the session after it.
    #[test]
    fn a_new_session_after_one_left_mid_send_keeps_nothing_of_the_old_one() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let first = h.lease();
            h.advance(100).await;
            h.send(SessionEvent::Abort);

            h.recorder.clear();
            h.start_training();
            let second = h.lease();
            assert!(second.turn() > first.turn());
            assert_eq!(h.texts().len(), 1);

            // Let the old send's cancellation land: it must not end the new session.
            h.advance(2_000).await;
            assert_eq!(h.screen(), Screen::Training);
            assert!(!matches!(h.phase(), Some(SessionPhase::Aborted)));
            assert_eq!(
                h.app.session.borrow().as_ref().map(|live| live
                    .machine
                    .session()
                    .session_id()
                    .raw()),
                Some(second.turn())
            );
            assert!(h.run_until(6_000, |h| h.awaiting_answer() == Some(0)).await);
        });
    }

    /// Leaving the session while a group is in flight: the cancelled send must
    /// not drag the app back onto a results screen.
    #[test]
    fn walking_away_mid_send_goes_home_and_stays_there() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(100).await;
            h.send(SessionEvent::Abort);
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.runtime.peek().is_none());
            assert!(h.app.session.borrow().is_none());

            h.advance(5_000).await;
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.result.peek().is_none());
            assert!(h.sessions.peek().is_empty());
        });
    }

    #[test]
    fn ending_a_session_early_keeps_what_was_already_scored() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(1_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
            h.send(SessionEvent::FinishNow);
            h.advance(200).await;
            assert_eq!(h.screen(), Screen::Results);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 1);
            assert_eq!(h.sessions.peek().len(), 1);
        });
    }

    /// The regression test for a send that is counted down on the wall clock:
    /// with the audio clock frozen, the session must not walk on as though the
    /// group had been heard.
    #[test]
    fn a_stalled_send_never_passes_for_a_finished_one() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::Stall);
            h.start_training();

            // Well past the length of the group, and still nothing is confirmed
            // and no answer window has opened.
            h.advance(1_500).await;
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
            assert!(h.awaiting_answer().is_none());

            // The stall is eventually reported, the player rebuilt, and after
            // three attempts the session gives up rather than pretending.
            assert!(
                h.run_until(60_000, |h| h.screen() == Screen::Home).await,
                "a stalled player should end the session, not hang"
            );
            assert!(h.recorder.players_built.get() > 1, "the player is rebuilt");
            assert_eq!(h.toast().as_deref(), Some("Audio playback stalled."));
            assert!(h.sessions.peek().is_empty());
        });
    }

    /// Leaving the tab must not cost the listener their session. The audio
    /// clock parks while the page is away, and a send that is counted down
    /// against it would run out the stall budget in silence and end the
    /// session — on the browser build, on the first group, for a tab switch.
    #[test]
    fn a_session_survives_the_page_going_to_the_background() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(200).await;
            h.set_page_hidden(true);

            // Far longer than the stall grace, in a phase that would have
            // failed on the wall clock.
            h.advance(60_000).await;
            assert_eq!(
                h.phase(),
                Some(SessionPhase::Playing { index: 0 }),
                "the group is still waiting to be heard"
            );
            assert_eq!(h.toast(), None, "nothing has gone wrong");
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.texts().len(), 1, "and it was not sent again");

            // Back to the tab: the same send finishes and the session goes on.
            h.set_page_hidden(false);
            assert!(
                h.run_until(30_000, |h| h.awaiting_answer().is_some()).await,
                "the answer window should open once the page is back"
            );
            assert_eq!(h.screen(), Screen::Training);
            h.play_through(120_000).await;
            assert_eq!(h.screen(), Screen::Results);
        });
    }

    /// Callsign mode changes how a group is built and nothing else. The proof
    /// is that a whole session runs through the same machine, the same audio
    /// and the same scoring, and lands on the results screen with real
    /// callsigns in it.
    #[test]
    fn a_callsign_session_runs_through_the_same_machine_as_any_other() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            settings.curriculum.callsign_level = cw_core::CALLSIGN_TIER_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(120_000).await;

            assert_eq!(h.screen(), Screen::Results);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 2);
            assert_eq!(result.accuracy, 1.0, "every call was answered correctly");
            // The calls themselves are cw-core's to get right; here, every
            // group has the digit that separates a call's prefix from its
            // suffix.
            for group in &result.groups {
                assert!(
                    group.sent.chars().any(|c| c.is_ascii_digit()),
                    "{:?} is not a callsign",
                    group.sent
                );
            }
            // The tier is what this session was run at, not the Koch level.
            assert_eq!(result.level, cw_core::CALLSIGN_TIER_MAX);
            assert_eq!(result.char_set_mode, CharSetMode::Callsign);
            assert_eq!(h.sessions.peek().len(), 1);
            // And what was sent is what was played.
            assert_eq!(h.texts().len(), 2);
            for text in h.texts() {
                assert!(text.chars().any(|c| c.is_ascii_digit()), "{text:?}");
            }
        });
    }

    /// A callsign session must not teach the group sampler, or a tier-6 call
    /// full of `Q` and `Z` would drag the Koch curriculum around behind it.
    #[test]
    fn callsign_history_stays_out_of_the_group_sampler() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(120_000).await;
            let stored = h.sessions.peek().clone();
            assert_eq!(stored.len(), 1);

            let mut koch = test_settings();
            koch.curriculum.char_set_mode = CharSetMode::Koch;
            assert!(
                !stored[0].usable_for_sampling(&koch),
                "a callsign session must not seed the Koch sampler"
            );
            let mut callsigns = test_settings();
            callsigns.curriculum.char_set_mode = CharSetMode::Callsign;
            callsigns.curriculum.callsign_level = cw_core::CALLSIGN_TIER_MAX;
            assert!(
                stored[0].usable_for_sampling(&callsigns),
                "but every tier shares one callsign history"
            );
        });
    }

    /// A station repeating its call is the same station. Redrawing the tone,
    /// speed and strength on every send made three repeats of one group arrive
    /// as three different operators — which is what the screen promises they
    /// are not, and which makes a repeat useless for confirming what you heard.
    #[test]
    fn every_repeat_of_a_group_comes_from_the_same_station() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            // Ranges wide enough that a redraw could not pass for a repeat.
            settings.playback.char_wpm_min = 15.0;
            settings.playback.char_wpm_max = 40.0;
            settings.playback.link_char_to_effective = true;
            settings.band.side_tone_min = 400.0;
            settings.band.side_tone_max = 900.0;
            settings.band.volume_min = 0.2;
            settings.band.volume_max = 1.0;
            settings.band.link_volume = false;

            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;
            assert_eq!(h.screen(), Screen::Results);

            let voices = h.recorder.voices();
            let texts = h.texts();
            assert_eq!(voices.len(), texts.len());
            assert_eq!(texts.len(), 6, "two groups, three sends each");

            // Every send of one group is the same operator...
            for group in texts.chunks(3).zip(voices.chunks(3)) {
                let (group_texts, group_voices) = group;
                assert!(
                    group_texts.windows(2).all(|w| w[0] == w[1]),
                    "a group's repeats should be the same text: {group_texts:?}"
                );
                assert!(
                    group_voices.windows(2).all(|w| w[0] == w[1]),
                    "a group's repeats changed station: {group_voices:?}"
                );
            }
            // ...and the next group is somebody else.
            assert_ne!(
                voices[0], voices[3],
                "every group came from the same station, which is its own problem"
            );
        });
    }

    /// Every launch tunes in its own stations. The first session of every
    /// launch used to bring the same pitch, speed, strength and fist for each
    /// of its groups, and the same callers across them, because the stations
    /// were drawn from a counter that starts at zero each time the app does.
    #[test]
    fn two_launches_do_not_hear_the_same_stations() {
        run(|| async {
            let first_session = || {
                let mut settings = test_settings();
                settings.playback.char_wpm_min = 15.0;
                settings.playback.char_wpm_max = 40.0;
                settings.band.side_tone_min = 400.0;
                settings.band.side_tone_max = 900.0;
                settings.band.link_volume = false;
                settings.band.volume_min = 0.2;
                settings.band.volume_max = 1.0;
                let mut h = Harness::with_settings(settings);
                h.start_training();
                h.recorder.voices()
            };
            let (one, other) = (first_session(), first_session());
            assert_eq!(one.len(), 1);
            assert_ne!(one, other, "two launches opened on the same station");
        });
    }

    /// A pile-up changes what you hear, not what you answer. The session still
    /// scores the station you were copying, and the others are only ever in the
    /// way — which is the whole point of them.
    #[test]
    fn a_pile_up_is_heard_but_never_answered() {
        run(|| async {
            let mut settings = test_settings();
            settings.band.stations_max = cw_core::STATIONS_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;
            assert_eq!(h.screen(), Screen::Results);

            let sent = h.recorder.sent();
            assert!(!sent.is_empty());
            assert!(
                sent.iter().any(|t| !t.others.is_empty()),
                "with five allowed, somebody else should have called"
            );
            for transmission in &sent {
                for other in &transmission.others {
                    assert_ne!(
                        other.text, transmission.text,
                        "an interfering station sent the answer"
                    );
                    assert!(
                        other.voice.volume < transmission.voice.volume,
                        "an interfering station was the strongest"
                    );
                }
            }

            // Every group was answered correctly, because the pile-up is not
            // part of the answer.
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.accuracy, 1.0);
            assert_eq!(h.sessions.peek().len(), 1);
        });
    }

    /// A repeat is the same moment on the band, pile-up and all. Redrawing the
    /// others would make a repeat a different puzzle rather than another look
    /// at the same one.
    #[test]
    fn a_repeat_brings_back_the_same_pile_up() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.group_repeat_min = 3;
            settings.playback.group_repeat_max = 3;
            settings.band.stations_max = cw_core::STATIONS_MAX;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(200_000).await;

            let sent = h.recorder.sent();
            assert_eq!(sent.len(), 6, "two groups, three sends each");
            for group in sent.chunks(3) {
                assert!(
                    group.windows(2).all(|w| w[0] == w[1]),
                    "a repeat brought a different pile-up"
                );
            }
            assert_ne!(sent[0], sent[3], "both groups drew the same band");
        });
    }

    #[test]
    fn a_broken_stream_is_retried_before_the_session_is_given_up() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::FailMidSend);
            h.start_training();
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Home).await);
            // Three attempts, each on a freshly built player.
            assert_eq!(h.texts().len(), 3);
            assert!(h.recorder.players_built.get() >= 3);
            assert!(h.toast().is_some());
        });
    }

    #[test]
    fn a_player_that_refuses_to_start_is_reported_once_the_retries_run_out() {
        run(|| async {
            let mut h = Harness::new();
            h.set_behaviour(Behaviour::RefuseToStart);
            h.start_training();
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Home).await);
            assert_eq!(h.toast().as_deref(), Some("Audio stream: device is gone"));
        });
    }

    /// A group answered while its send is being retried is not sent again.
    ///
    /// With the box open during the send, the answer can land while a
    /// failed send waits for its rebuilt player. Confirming it stopped the
    /// audio, but the retry only checked that the session was the same one,
    /// so it went on to send the confirmed group across the gap before the
    /// next.
    #[test]
    fn an_answer_given_during_a_retry_stops_the_retry() {
        run(|| async {
            let mut settings = test_settings();
            settings.playback.lock_input_during_group_playback = false;
            let mut h = Harness::with_settings(settings);
            h.recorder.fail_next(1);
            h.start_training();
            // The first send breaks, and the retry rebuilds the player.
            let mut waited = 0;
            while h.recorder.players_built.get() < 2 {
                assert!(waited < 5_000, "the send was never retried");
                h.advance(4).await;
                waited += 4;
            }
            let sends = h.texts().len();
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            h.send(SessionEvent::Confirm);
            assert_eq!(h.phase(), Some(SessionPhase::InterGroupGap { next: 1 }));
            h.advance(500).await;
            assert_eq!(
                h.texts().len(),
                sends,
                "the answered group was sent again: {:?}",
                h.texts()
            );
        });
    }

    /// A device that comes back mid-session must not cost the user the session.
    #[test]
    fn a_send_that_fails_once_and_then_works_carries_on() {
        run(|| async {
            let mut h = Harness::new();
            // The first send breaks; the device is back for the second.
            h.recorder.fail_next(1);
            h.start_training();
            assert!(
                h.run_until(20_000, |h| h.awaiting_answer() == Some(0))
                    .await
            );
            assert_eq!(h.texts().len(), 2, "the group is sent again, not skipped");
            assert!(h.recorder.players_built.get() >= 2, "the player is rebuilt");
            assert_eq!(h.screen(), Screen::Training);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(2_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
        });
    }

    /// A hard audio failure after something has been scored keeps the score.
    #[test]
    fn audio_that_dies_after_a_scored_group_still_shows_the_results() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            let sent = h.sent(0);
            h.type_answer(0, &sent);
            assert!(
                h.run_until(1_000, |h| h
                    .runtime
                    .peek()
                    .as_ref()
                    .is_some_and(|s| s.view().confirmed[0]))
                    .await
            );
            h.set_behaviour(Behaviour::RefuseToStart);
            assert!(h.run_until(30_000, |h| h.screen() == Screen::Results).await);
            let result = h.result.peek().clone().expect("a result");
            assert_eq!(result.groups.len(), 1);
        });
    }

    #[test]
    fn an_event_for_a_session_that_is_gone_is_ignored() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let stale = h.lease();
            let (app, signals) = (h.app.clone(), h.signals());
            // The user leaves; an old send then reports that it ended.
            h.send(SessionEvent::Abort);
            h.in_app(|| {
                dispatch(
                    &app,
                    signals,
                    &stale,
                    SessionEvent::PlaybackEnded {
                        index: 0,
                        duration_sec: 1.0,
                        char_wpm: 20.0,
                        effective_wpm: 20.0,
                    },
                )
            });
            h.pump();
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.runtime.peek().is_none());
        });
    }

    #[test]
    fn a_session_at_the_end_of_its_groups_does_not_ask_for_another() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.texts().len(), 1);
        });
    }

    #[test]
    fn an_event_with_no_session_behind_it_does_nothing() {
        run(|| async {
            let mut h = Harness::new();
            h.send(SessionEvent::Confirm);
            assert_eq!(h.screen(), Screen::Home);
            assert!(h.runtime.peek().is_none());
        });
    }

    #[test]
    fn an_event_from_an_older_session_never_reaches_the_new_machine() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let older = h.lease();
            h.send(SessionEvent::Abort);
            h.start_training();
            let (app, signals) = (h.app.clone(), h.signals());
            h.in_app(|| dispatch(&app, signals, &older, SessionEvent::Abort));
            h.pump();
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
        });
    }

    /// Stopping the audio without ending the session is what a stray stop looks
    /// like from inside: the send reports cancelled and the session is closed
    /// with whatever it had, rather than sitting in silence.
    #[test]
    fn a_send_stopped_from_outside_ends_the_session_cleanly() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            h.advance(100).await;
            h.lease().stop_sending();
            assert!(h.run_until(5_000, |h| h.screen() == Screen::Home).await);
            assert!(h.sessions.peek().is_empty());
        });
    }

    #[test]
    fn a_timer_the_machine_never_started_does_not_move_the_session_on() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let (app, signals, lease) = (h.app.clone(), h.signals(), h.lease());
            h.in_app(|| {
                run_effects(
                    &app,
                    signals,
                    &lease,
                    vec![SessionEffect::StartTimer { id: 999, ms: 20 }],
                )
            });
            h.pump();
            h.advance(200).await;
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
        });
    }

    #[test]
    fn effects_from_a_session_that_is_over_are_dropped() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let (app, signals, stale) = (h.app.clone(), h.signals(), h.lease());
            h.send(SessionEvent::Abort);
            h.recorder.clear();
            h.in_app(|| {
                run_effects(
                    &app,
                    signals,
                    &stale,
                    vec![
                        SessionEffect::Play {
                            index: 1,
                            text: "KM".into(),
                        },
                        SessionEffect::StartTimer { id: 1, ms: 10 },
                    ],
                )
            });
            h.pump();
            h.advance(500).await;
            assert!(h.texts().is_empty());
            assert_eq!(h.screen(), Screen::Home);
        });
    }

    #[test]
    fn a_timer_a_finished_session_no_longer_needs_does_nothing() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            let (app, signals, stale) = (h.app.clone(), h.signals(), h.lease());
            h.play_through(30_000).await;
            assert_eq!(h.screen(), Screen::Results);
            h.in_app(|| {
                run_effects(
                    &app,
                    signals,
                    &stale,
                    vec![SessionEffect::StartTimer { id: 1, ms: 10 }],
                )
            });
            h.pump();
            h.advance(100).await;
            assert_eq!(h.screen(), Screen::Results);
        });
    }

    #[test]
    fn a_session_plays_its_first_group_and_opens_the_answer_box() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            assert_eq!(h.screen(), Screen::Training);
            assert_eq!(h.phase(), Some(SessionPhase::Playing { index: 0 }));
            let sent = h.sent(0);
            assert_eq!(sent.chars().count(), 2);
            assert_eq!(h.texts(), vec![sent.clone()]);
            // The answer box stays shut until the send is over.
            assert!(h.runtime.peek().as_ref().is_some_and(|s| s.input_locked(0)));

            assert!(h.run_until(4_000, |h| h.awaiting_answer() == Some(0)).await);
            assert!(!h.runtime.peek().as_ref().is_some_and(|s| s.input_locked(0)));
        });
    }

    /// A session already running is not started over: not its audio, not
    /// its machine, and not the settings it was started with.
    #[test]
    fn starting_again_while_a_session_runs_changes_nothing() {
        run(|| async {
            let mut h = Harness::new();
            h.start_training();
            let lease = h.lease();
            h.settings.set({
                let mut changed = test_settings();
                changed.curriculum.num_groups = 7;
                changed
            });
            h.start_training();
            assert!(h.lease().is(&lease) && lease.is_current());
            assert_eq!(h.settings.peek().curriculum.num_groups, 7);
            assert_eq!(h.texts().len(), 1);
        });
    }

    #[test]
    fn a_good_session_can_move_the_level_and_says_so() {
        run(|| async {
            let mut settings = test_settings();
            settings.auto_level.auto_adjust_level = true;
            settings.auto_level.auto_adjust_above_threshold_count = 1;
            settings.auto_level.auto_adjust_threshold = 50.0;
            settings.curriculum.level = 1;
            let mut h = Harness::with_settings(settings);
            h.start_training();
            h.play_through(60_000).await;
            assert_eq!(h.screen(), Screen::Results);
            assert_eq!(h.settings.peek().curriculum.level, 2);
            let message = h.auto_message.peek().clone().expect("a message");
            assert!(message.contains("Level increased to 2"), "{message}");
            assert_eq!(h.toast().as_deref(), Some(message.as_str()));
            // The level that was just left behind keeps no counters.
            assert_eq!(
                crate::persist::load_auto_counters(&h.settings.peek().clone()),
                AutoLevelCounters::default()
            );
        });
    }

    #[test]
    fn a_session_that_does_not_move_the_level_keeps_counting() {
        run(|| async {
            let mut settings = test_settings();
            settings.auto_level.auto_adjust_level = true;
            settings.auto_level.auto_adjust_above_threshold_count = 5;
            settings.auto_level.auto_adjust_threshold = 50.0;
            let mut h = Harness::with_settings(settings.clone());
            h.start_training();
            h.play_through(60_000).await;
            assert!(h.auto_message.peek().is_none());
            assert_eq!(
                crate::persist::load_auto_counters(&settings),
                AutoLevelCounters { above: 1, below: 0 }
            );
            let progress = crate::persist::current_auto_progress(&settings).expect("progress");
            assert_eq!(progress.above_count, 1);
            assert_eq!(progress.above_target, 5);
        });
    }
}
