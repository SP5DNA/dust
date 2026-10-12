use std::rc::Rc;

use cw_core::{GroupSession, Sampler, SessionEvent};
use dioxus::prelude::*;

use crate::persist::{load_sessions, load_settings, load_theme, save_settings, save_theme};
use crate::playback::{self, Preview, start_preview};
use crate::routes::{AppCallbacks, ViewState, app_routes};
use crate::session_runtime::{send_event, session_running, start_session};
use crate::state::{AppState, Screen, SessionSignals, TOAST_MS, Toasts};
use crate::theme::Theme;
use crate::time::{seed_rng, sleep_ms};
use crate::ui::paddle::use_paddle_keys;
use crate::ui::widgets::{Icon, control_id};

// Desktop inlines this in the window head. Web loads it through `asset!` so
// `dx bundle` copies the file; compiling the sheet into wasm would ship it
// twice. Mobile has neither, so it injects this string from AppHead.
#[cfg(not(feature = "web"))]
pub(crate) const STYLESHEET: &str = include_str!("../assets/styles.css");

// The web stylesheet, written into the page's `<head>` by `dx bundle`.
//
// `dx bundle` does not copy Dioxus.toml [web.resource] style files into the
// site — Pages then 404s `assets/styles.css` and the UI renders raw — so the
// sheet has to go through `asset!`. Left at that, though, the `<link>` only
// went in when the app mounted: the app drew its first frame unstyled and then
// snapped into place once the sheet arrived. A static-head asset is linked from
// the head of `index.html` itself, which the browser loads before it paints
// anything, so the boot screen holds until the app can appear already dressed.
// Nothing has to insert it at run time.
#[cfg(feature = "web")]
const _: Asset = asset!(
    "/assets/styles.css",
    AssetOptions::css().with_static_head(true)
);

fn injected_stylesheet() -> Element {
    // Desktop has the sheet inlined in its window head, and the web in the
    // page head; only mobile needs it put in from here.
    #[cfg(any(feature = "desktop", feature = "web"))]
    {
        rsx! {}
    }
    #[cfg(not(any(feature = "desktop", feature = "web")))]
    {
        rsx! {
            document::Style { { STYLESHEET } }
        }
    }
}

fn font_link() -> Element {
    #[cfg(feature = "desktop")]
    {
        rsx! {}
    }
    #[cfg(not(feature = "desktop"))]
    {
        rsx! {
            document::Link {
                rel: "stylesheet",
                href: "https://fonts.googleapis.com/css2?family=Fraunces:opsz,wght@9..144,600;700&family=Figtree:wght@400;500;600;700&family=IBM+Plex+Mono:wght@500;700&display=optional",
            }
        }
    }
}

/// Head tags are injected once, on mount. They live in their own component so
/// that App re-renders do not re-run them — `dioxus-document` warns on every
/// prop update of a head element ("Changing the props of `Style {}` is not
/// supported"), and a component with no props is memoized.
#[component]
fn AppHead() -> Element {
    rsx! {
        document::Title { "Dust" }
        {injected_stylesheet()}
        {font_link()}
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover",
        }
        document::Meta { name: "theme-color", content: "#1b2436" }
        document::Meta { name: "mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-capable", content: "yes" }
        document::Meta { name: "apple-mobile-web-app-status-bar-style", content: "black-translucent" }
    }
}

#[component]
pub fn App() -> Element {
    let mut settings = use_signal(load_settings);
    let sessions = use_signal(load_sessions);
    let mut screen = use_signal(|| Screen::Home);
    let runtime = use_signal(|| None::<GroupSession>);
    let result = use_signal(|| None::<cw_core::SessionResult>);
    let auto_message = use_signal(|| None::<String>);
    let toasts = use_hook(Toasts::new);
    let mut preview = use_signal(Preview::default);
    let mut stream_heard = use_signal(Vec::<String>::new);
    let mut theme = use_signal(|| Theme::from_key(&load_theme()));
    // The audio backend is injectable through context so a test can drive the
    // whole app with a player that records instead of one that needs a device.
    let app = use_hook(|| try_consume_context::<AppState>().unwrap_or_else(AppState::new));
    let app = Rc::new(app);
    let paddle = use_paddle_keys(settings, screen, app.clone());
    let signals = SessionSignals {
        screen,
        runtime,
        result,
        auto_message,
        sessions,
        settings,
        toasts,
        preview,
    };

    use_effect(move || {
        let key = theme().key();
        save_theme(key);
        // Theme is a data-theme attribute on the app root, not a JS write to
        // <html>. Both the desktop webview and the web build key CSS off that.
    });

    use_effect(move || {
        if matches!(screen(), Screen::Training) {
            return;
        }
        // On the settings screen the live values are left as typed, so a
        // field part-way through a number is not snapped under the cursor;
        // what is stored is always clamped.
        let persist = settings().clamp();
        if !matches!(screen(), Screen::Settings) && persist != settings() {
            settings.set(persist.clone());
        }
        save_settings(&persist);
    });

    use_effect(move || {
        if let Some(shown) = toasts.current() {
            spawn(async move {
                sleep_ms(TOAST_MS).await;
                toasts.dismiss(&shown);
            });
        }
    });

    // Leaving for another screen is the same move wherever you are going:
    // a session in progress holds you, and otherwise whatever was sounding
    // stops before the screen changes.
    let go_to = use_callback({
        let app = app.clone();
        move |to: Screen| {
            if session_running(signals) {
                return;
            }
            app.player.release();
            preview.set(Preview::Idle);
            stream_heard.set(Vec::new());
            screen.set(to);
        }
    });
    let go_home = use_callback(move |()| go_to.call(Screen::Home));

    let start_training = use_callback({
        let app = app.clone();
        move |()| start_session(&app, signals)
    });

    let start_listen = use_callback({
        let app = app.clone();
        move |chars: String| {
            start_preview(&app.player, signals, Preview::Letters, |lease, settings| {
                playback::play_chars(lease, settings, chars, 420)
            });
        }
    });

    let start_stream = use_callback({
        let app = app.clone();
        move |()| {
            start_preview(&app.player, signals, Preview::Stream, |lease, first| {
                stream_heard.set(Vec::new());
                // Leaning on the same weak characters a session would.
                let sampler = Sampler::new(&sessions.peek(), &first, seed_rng());
                playback::loop_stream(lease, settings, sampler, stream_heard)
            });
        }
    });

    // Envelope test chips: send one short sample with the current keying settings.
    let play_sample = use_callback({
        let app = app.clone();
        move |text: String| {
            start_preview(
                &app.player,
                signals,
                Preview::Sample(text.clone()),
                |lease, settings| playback::play_sample(lease, settings, text),
            );
        }
    });

    let start_band_preview = use_callback({
        let app = app.clone();
        move |()| {
            start_preview(&app.player, signals, Preview::Band, |lease, _| {
                playback::loop_band_preview(lease, settings, "CQ", 280)
            });
        }
    });

    let stop_preview = use_callback({
        let app = app.clone();
        move |()| {
            if session_running(signals) {
                return;
            }
            app.player.release();
            preview.set(Preview::Idle);
        }
    });

    use_effect({
        let app = (*app).clone();
        move || {
            if !preview().is_band() {
                return;
            }
            let settings_now = settings().clamp();
            app.player.follow_band(&settings_now);
        }
    });

    // Exit is End session from the header: the session decides what is
    // worth keeping, the same way whichever button asked.
    let exit_training = use_callback({
        let app = app.clone();
        move |()| send_event(&app, signals, SessionEvent::FinishNow)
    });
    let show_nav = !matches!(screen(), Screen::Training);
    let shell_class = if show_nav { "shell has-nav" } else { "shell" };
    let screen_key = screen_key(screen());
    let theme_attr = theme().key();

    rsx! {
        AppHead {}
        div {
            class: "app-root",
            "data-theme": theme_attr,
            onkeydown: move |e| {
                if e.key() == Key::F11 {
                    e.prevent_default();
                    toggle_fullscreen();
                    return;
                }
                paddle.keydown.call(e);
            },
            onkeyup: move |e| paddle.keyup.call(e),
            div { class: shell_class,
                header { class: "header-bar",
                    div { class: "brand",
                        span { class: "brand-mark", aria_hidden: "true", "·−" }
                        div {
                            p { class: "brand-name", "Dust" }
                            p { class: "brand-sub", "CW group trainer" }
                        }
                    }
                    div { class: "header-actions",
                        button {
                            id: control_id("btn", "theme"),
                            class: "icon-btn",
                            title: "{theme().label()}",
                            aria_label: "{theme().label()}",
                            onclick: move |_| {
                                let next = theme().next();
                                theme.set(next);
                            },
                            Icon { name: theme().icon() }
                        }
                        if screen() == Screen::Training {
                            button {
                                id: control_id("btn", "exit"),
                                class: "btn btn-secondary btn-sm",
                                onclick: move |_| exit_training.call(()),
                                "Exit"
                            }
                        }
                    }
                }
                div { class: "screen", id: "screen-{screen_key}", key: "{screen_key}",
                    { app_routes(
                        signals,
                        ViewState {
                            previewing: preview().is_band(),
                            listen_playing: preview().is_letters(),
                            listen_streaming: preview().is_stream(),
                            stream_heard: stream_heard(),
                            sample_playing: preview().sample(),
                            paddle_heard: (paddle.heard)(),
                        },
                        app,
                        AppCallbacks {
                            start_training,
                            go_home,
                            go_listen: EventHandler::new(move |()| go_to.call(Screen::Listen)),
                            start_band_preview,
                            stop_preview,
                            start_listen,
                            start_stream,
                            play_sample,
                            on_paddle_down: paddle.down,
                            on_paddle_up: paddle.up,
                        },
                    ) }
                }
            }
            if show_nav {
                div { class: "bottom-dock",
                    nav { class: "bottom-nav",
                        button {
                            id: control_id("nav", "practice"),
                            class: if matches!(screen(), Screen::Home | Screen::Listen | Screen::Results) { "nav-item active" } else { "nav-item" },
                            onclick: move |_| go_home.call(()),
                            Icon { name: "signal" }
                            span { "Practice" }
                        }
                        button {
                            id: control_id("nav", "stats"),
                            class: if screen() == Screen::Stats { "nav-item active" } else { "nav-item" },
                            onclick: move |_| go_to.call(Screen::Stats),
                            Icon { name: "chart" }
                            span { "Stats" }
                        }
                        button {
                            id: control_id("nav", "settings"),
                            class: if screen() == Screen::Settings { "nav-item active" } else { "nav-item" },
                            onclick: move |_| go_to.call(Screen::Settings),
                            Icon { name: "sliders" }
                            span { "Settings" }
                        }
                    }
                }
            }
            if let Some(toast) = toasts.current() {
                div { class: if show_nav { "toast-dock has-nav" } else { "toast-dock" },
                    div { class: "toast", "{toast.text}" }
                }
            }
        }
    }
}

fn screen_key(screen: Screen) -> &'static str {
    match screen {
        Screen::Home => "home",
        Screen::Settings => "settings",
        Screen::Training => "training",
        Screen::Results => "results",
        Screen::Stats => "stats",
        Screen::Listen => "listen",
    }
}

fn toggle_fullscreen() {
    #[cfg(feature = "desktop")]
    {
        let desktop = dioxus::desktop::window();
        let fullscreen = desktop.window.fullscreen().is_some();
        desktop.set_fullscreen(!fullscreen);
    }
    #[cfg(feature = "web")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        if document.fullscreen_element().is_some() {
            document.exit_fullscreen();
            return;
        }
        if let Some(element) = document.document_element() {
            let _ = element.request_fullscreen();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::screen_key;
    use crate::state::Screen;

    #[test]
    fn every_screen_has_its_own_key() {
        let keys: Vec<&str> = [
            Screen::Home,
            Screen::Settings,
            Screen::Training,
            Screen::Results,
            Screen::Stats,
            Screen::Listen,
        ]
        .into_iter()
        .map(screen_key)
        .collect();
        // The key remounts the screen wrapper, so it has to differ per screen.
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len());
    }
}

#[cfg(test)]
mod ui_tests {
    use crate::testing::{Ui, run, stored_session, test_settings};
    use cw_core::{CharSetMode, PracticeWindow, TrainingSettings};

    fn short_session() -> TrainingSettings {
        let mut settings = test_settings();
        settings.curriculum.num_groups = 1;
        settings
    }

    /// Start a session and wait for the first group's answer box to open.
    async fn awaiting_answer(
        settings: TrainingSettings,
    ) -> (Ui, std::rc::Rc<crate::audio::fake::Recorder>) {
        let (mut ui, recorder) = Ui::app_with_settings(settings);
        ui.click("btn-start-training");
        assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
        (ui, recorder)
    }

    #[test]
    fn the_app_opens_on_the_practice_screen() {
        run(|| async {
            let (ui, _recorder) = Ui::app();
            assert_eq!(ui.screen(), "home");
            assert!(ui.has("Start training"));
            assert!(ui.has("bottom-nav"));
        });
    }

    #[test]
    fn the_bottom_nav_reaches_every_screen() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            for (button, expected) in [
                ("nav-stats", "stats"),
                ("nav-settings", "settings"),
                ("nav-practice", "home"),
                ("btn-listen-to-letters", "listen"),
                ("btn-listen-back", "home"),
            ] {
                ui.click(button);
                assert_eq!(ui.screen(), expected, "after pressing {button}");
            }
        });
    }

    #[test]
    fn the_theme_button_cycles_and_is_remembered() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            for (shown, key) in [
                ("Theme: light", "light"),
                ("Theme: dark", "dark"),
                ("Theme: follows your system", "auto"),
            ] {
                ui.click("btn-theme");
                assert!(ui.has(shown));
                assert_eq!(crate::persist::load_theme(), key);
            }
        });
    }

    #[test]
    fn a_session_can_be_played_from_the_practice_screen() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert!(ui.has("Listen — the answer box unlocks"));
            // The nav is out of the way while a session runs.
            assert!(!ui.has("bottom-nav"));
            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);

            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(5_000, |ui| ui.screen() == "results").await);
            assert!(ui.has("Session complete"));
            assert!(ui.has("Clean copy"));
            assert_eq!(crate::persist::load_sessions().len(), 1);
        });
    }

    #[test]
    fn a_group_locks_while_it_plays_then_unlocks_with_autofocus() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 2;
            settings.playback.lock_input_during_group_playback = true;
            let (mut ui, recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert!(ui.has("Listen — the answer box unlocks"));
            let sending = ui.input_tag("group-input-0");
            assert!(sending.contains("readonly"), "{sending}");
            assert!(!sending.contains("autofocus"), "{sending}");
            ui.type_into("group-input-0", "XX");
            assert!(!ui.has("committed"), "{}", ui.html());

            assert!(ui.run_until(5_000, |ui| ui.has("Your turn")).await);
            let open = ui.input_tag("group-input-0");
            assert!(!open.contains("readonly"), "{open}");
            assert!(!open.contains("disabled"), "{open}");
            assert!(open.contains("autofocus"), "{open}");

            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(
                ui.run_until(5_000, |ui| ui
                    .input_tag("group-input-1")
                    .contains("readonly"))
                    .await,
                "the next group should lock while it plays: {}",
                ui.html()
            );
            assert!(
                ui.run_until(8_000, |ui| {
                    let tag = ui.input_tag("group-input-1");
                    tag.contains("autofocus")
                        && !tag.contains("readonly")
                        && !tag.contains("disabled")
                })
                .await,
                "the next group's box should unlock and autofocus: {}",
                ui.html()
            );
            let first = ui.input_tag("group-input-0");
            assert!(
                first.contains("disabled") || first.contains("readonly"),
                "the finished group must not stay live: {first}"
            );
        });
    }

    #[test]
    fn the_answer_box_stays_open_when_lock_while_sending_is_off() {
        run(|| async {
            let mut settings = short_session();
            settings.playback.lock_input_during_group_playback = false;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            assert!(ui.has("Listen, or type along"));
            let tag = ui.input_tag("group-input-0");
            assert!(!tag.contains("readonly"), "{tag}");
            assert!(!tag.contains("disabled"), "{tag}");
            assert!(tag.contains("autofocus"), "{tag}");
        });
    }

    #[test]
    fn a_typed_answer_can_be_confirmed_with_the_enter_key() {
        run(|| async {
            let (mut ui, recorder) = awaiting_answer(short_session()).await;
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            // One character short, so nothing confirms on its own.
            ui.type_into("group-input-0", &sent[..1]);
            ui.advance(500).await;
            assert_eq!(ui.screen(), "training");
            ui.press_enter("group-input-0");
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            assert!(ui.has("badge bad"));
        });
    }

    #[test]
    fn every_way_out_of_a_session_is_honoured() {
        run(|| async {
            for button in ["btn-exit", "btn-end-session", "btn-discard"] {
                let (mut ui, _recorder) = Ui::app_with_settings(short_session());
                ui.click("btn-start-training");
                ui.click(button);
                assert!(
                    ui.run_until(2_000, |ui| ui.screen() == "home").await,
                    "{button} should have left the session"
                );
                assert!(crate::persist::load_sessions().is_empty());
            }
        });
    }

    #[test]
    fn ending_a_session_early_keeps_the_answered_groups() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 3;
            let (mut ui, recorder) = awaiting_answer(settings).await;
            let sent = recorder.texts().first().cloned().expect("a group was sent");
            ui.type_into("group-input-0", &sent);
            assert!(ui.run_until(5_000, |ui| ui.has("group done")).await);
            ui.click("btn-end-session");
            assert!(ui.run_until(2_000, |ui| ui.screen() == "results").await);
            assert_eq!(crate::persist::load_sessions().len(), 1);
            assert!(ui.has("Group by group"));
        });
    }

    /// Exit and End session leave a session the same way. Exit used to
    /// decide for itself, from whether anything was confirmed yet, and threw
    /// away an answer End session would have kept.
    #[test]
    fn exit_keeps_a_typed_answer_just_as_end_session_does() {
        run(|| async {
            for button in ["btn-exit", "btn-end-session"] {
                let mut settings = test_settings();
                settings.curriculum.num_groups = 3;
                let (mut ui, recorder) = awaiting_answer(settings).await;
                let sent = recorder.texts().first().cloned().expect("a group was sent");
                ui.type_into("group-input-0", &sent[..1]);
                ui.advance(200).await;
                ui.click(button);
                assert!(
                    ui.run_until(2_000, |ui| ui.screen() == "results").await,
                    "{button} threw the answer away"
                );
                assert_eq!(crate::persist::load_sessions().len(), 1, "{button}");
            }
        });
    }

    #[test]
    fn the_results_screen_can_start_another_session_or_go_back() {
        run(|| async {
            for (button, expected) in [("btn-train-again", "training"), ("btn-back", "home")] {
                let (mut ui, recorder) = awaiting_answer(short_session()).await;
                let sent = recorder.texts().first().cloned().expect("a group");
                ui.type_into("group-input-0", &sent);
                assert!(ui.run_until(5_000, |ui| ui.screen() == "results").await);
                ui.click(button);
                assert!(
                    ui.run_until(2_000, |ui| ui.screen() == expected).await,
                    "{button} should have reached {expected}"
                );
            }
        });
    }

    #[test]
    fn an_audio_failure_is_shown_as_a_toast_that_clears_itself() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(short_session());
            recorder.build_error.set(true);
            ui.click("btn-start-training");
            assert!(ui.has("class=\"toast\""), "the failure should be on screen");
            assert!(ui.has("No audio output device found"));
            // Still on the practice screen: no session was started.
            assert_eq!(ui.screen(), "home");
            assert!(ui.run_until(6_000, |ui| !ui.has("class=\"toast\"")).await);
        });
    }

    #[test]
    fn the_listen_screen_plays_one_character_and_stops() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Play one character"));
            ui.click("btn-play");
            assert!(ui.has("Stop"));
            assert!(ui.run_until(5_000, |_| !recorder.texts().is_empty()).await);
            assert_eq!(recorder.texts(), vec!["M".to_string()]);
            ui.click("btn-stop");
            assert!(ui.has("Play all"));
        });
    }

    #[test]
    fn the_listen_screen_lets_a_character_finish_on_its_own() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            ui.click("btn-play");
            assert!(ui.run_until(20_000, |ui| !ui.has("Stop")).await);
            assert_eq!(recorder.texts().len(), 1);
            assert!(ui.has("Play all"));
        });
    }

    #[test]
    fn the_listen_screen_can_play_the_whole_pool() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            ui.click("btn-play-all");
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            assert_eq!(recorder.texts()[..2], ["K".to_string(), "M".to_string()]);
        });
    }

    #[test]
    fn the_listen_screen_can_stream_groups_without_storing_anything() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Stream groups"));
            ui.click("btn-stream-groups");
            assert!(ui.has("Stop"));
            assert!(ui.has("Nothing is scored"));
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            assert!(recorder.texts().iter().all(|text| text.len() == 2));
            assert!(ui.run_until(20_000, |ui| ui.has("listen-heard")).await);
            ui.click("btn-stop");
            assert!(ui.has("Play all"));
            assert!(ui.has("Stream groups"));
            assert_eq!(ui.screen(), "listen");
            assert!(crate::persist::load_sessions().is_empty());
        });
    }

    #[test]
    fn a_character_chip_changes_what_is_shown() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-listen-to-letters");
            assert!(ui.has("Newest unlocked character"));
            // K is the older of the two, so it is not the newest.
            ui.click("chip-k");
            assert!(ui.has("From your current pool"));
            assert!(ui.has("listen-glyph"));
        });
    }

    #[test]
    fn the_character_set_can_be_switched_and_is_saved() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            ui.click("nav-settings");
            for (button, note, mode) in [
                (
                    "seg-koch",
                    "Letters unlocked in Koch order",
                    CharSetMode::Koch,
                ),
                ("seg-digits", "Digits only", CharSetMode::Digits),
                ("seg-custom", "Your own character list", CharSetMode::Custom),
                (
                    "seg-mixed",
                    "Letters and digits together",
                    CharSetMode::Mixed,
                ),
            ] {
                ui.click(button);
                assert!(ui.has(note), "{button} should say {note}");
                assert_eq!(
                    crate::persist::load_settings().curriculum.char_set_mode,
                    mode
                );
            }
            assert!(ui.has("Share of letters"));
        });
    }

    #[test]
    fn the_unlock_order_can_be_swapped_for_another_preset() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            ui.click("nav-settings");
            ui.click("pill-cw-academy");
            let stored = crate::persist::load_settings();
            assert_eq!(
                stored.sequence_choice(),
                cw_core::SequenceChoice::Preset(cw_core::PresetId::CwAcademy)
            );
            assert!(!stored.curriculum.sequence_is_custom);
            assert!(stored.progress_alphabet().starts_with(&['E', 'T', 'A']));
            ui.click("pill-lcwo");
            assert!(
                crate::persist::load_settings()
                    .curriculum
                    .custom_sequence
                    .is_empty()
            );
        });
    }

    #[test]
    fn the_practice_window_can_be_narrowed_to_the_newest_characters() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            for (button, window, pool) in [
                ("pill-newest-3", PracticeWindow::Last3, 3),
                ("pill-newest-5", PracticeWindow::Last5, 5),
                ("pill-everything", PracticeWindow::All, 10),
            ] {
                ui.click(button);
                let stored = crate::persist::load_settings();
                assert_eq!(stored.curriculum.practice_window, window);
                assert_eq!(cw_core::compute_char_pool(&stored).len(), pool);
            }
        });
    }

    /// A sequence is shortened on its way to being retyped, a character at
    /// a time. The level it carries is read inside it, and kept for when it
    /// grows back.
    #[test]
    fn shortening_the_alphabet_keeps_the_level_for_when_it_grows_back() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            ui.type_into("field-sequence-order", "KM");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!(stored.curriculum.custom_sequence, vec!['K', 'M']);
            // Two characters can only carry level 1.
            assert_eq!(stored.active_level(), 1);
            assert_eq!(stored.clone().clamp(), stored);

            ui.type_into("field-sequence-order", "KMURESNAPTL");
            ui.advance(50).await;
            assert_eq!(crate::persist::load_settings().active_level(), 9);
        });
    }

    #[test]
    fn an_envelope_chip_sends_a_sample_and_clears_when_it_is_over() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("chip-dit");
            assert!(ui.has("test-chip playing"));
            assert_eq!(recorder.texts(), vec!["E".to_string()]);
            assert!(
                ui.run_until(20_000, |ui| !ui.has("test-chip playing"))
                    .await
            );
        });
    }

    #[test]
    fn an_envelope_chip_with_no_audio_device_says_so() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            recorder.build_error.set(true);
            ui.click("nav-settings");
            ui.click("chip-dah");
            ui.advance(200).await;
            assert!(ui.has("No audio output device found"));
        });
    }

    #[test]
    fn the_band_preview_loops_until_the_screen_is_left() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| recorder.texts().len() >= 2).await);
            assert!(recorder.texts().iter().all(|text| text == "CQ"));

            ui.click("nav-practice");
            assert_eq!(ui.screen(), "home");
            let sent = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), sent, "the loop should have stopped");
        });
    }

    /// The band preview, the letter player and an envelope sample all want the
    /// same audio, so starting one has to stop whatever else was sounding.
    ///
    /// That used to be three separate flags reset by hand at every call site,
    /// eight times over and already not the same twice. It is one value now,
    /// which cannot hold two of them at once — this is that promise, checked
    /// from the outside.
    #[test]
    fn starting_one_preview_stops_whatever_else_was_playing() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.open_disclosures();

            // The band preview is running and says so.
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            assert!(ui.has("Stop"), "the preview should be showing as live");

            // A sample chip takes the audio, and the preview gives it up.
            ui.click("chip-dit");
            ui.advance(50).await;
            assert!(
                ui.has("Live preview"),
                "the band preview should have stopped when the sample started"
            );

            // And the sample finishes on its own, leaving nothing playing.
            assert!(ui.run_until(20_000, |ui| ui.has("Live preview")).await);
            let settled = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), settled, "nothing should still loop");
        });
    }

    #[test]
    fn the_band_preview_can_be_stopped_where_it_was_started() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            ui.click("btn-band-stop");
            let sent = recorder.texts().len();
            ui.advance(3_000).await;
            assert_eq!(recorder.texts().len(), sent);
            assert!(ui.has("Live preview"));
        });
    }

    /// The pile-up controls only exist once more than one station can call, so
    /// a sweep that never turns the count up never touches them — which is how
    /// a card of sliders goes untested.
    /// Each card can be put back on its own, without taking the rest of the
    /// screen with it.
    #[test]
    fn a_section_resets_itself_and_leaves_the_others_alone() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.open_disclosures();

            // Move something on two different cards.
            ui.type_into("slider-qrn-intensity", "0.9");
            ui.advance(50).await;
            ui.commit("fixed-stations-calling", "4");
            ui.advance(50).await;
            ui.click("seg-filter-soft");
            ui.advance(50).await;
            ui.type_into("slider-rise-time", "23");
            ui.advance(50).await;

            let changed = crate::persist::load_settings();
            assert_eq!(changed.band.stations_max, 4);
            assert_eq!(changed.band.filter_shape, cw_core::FilterShape::Soft);
            assert!((changed.band.steepness - 23.0).abs() < 0.5);

            // Reset the keying envelope only.
            ui.click("reset-keying-envelope");
            ui.advance(50).await;
            let after = crate::persist::load_settings();
            let fresh = cw_core::TrainingSettings::default();
            assert!(
                (after.band.steepness - fresh.band.steepness).abs() < 0.001,
                "the rise time should be back to its default"
            );
            assert_eq!(
                after.band.stations_max, 4,
                "resetting the keying envelope must not touch the band conditions"
            );
            assert_eq!(after.band.filter_shape, cw_core::FilterShape::Soft);

            // And now the band conditions, which owns the stations.
            ui.click("reset-band-conditions");
            ui.advance(50).await;
            let after = crate::persist::load_settings();
            assert_eq!(after.band.stations_max, fresh.band.stations_max);
            assert!((after.band.qrn_level - fresh.band.qrn_level).abs() < 0.001);
        });
    }

    #[test]
    fn the_pile_up_controls_appear_with_the_stations_and_work() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            assert!(!ui.has("Pile-up spread"), "one station needs no spread");

            // One station is the pair held together, so the count is fixed.
            assert!(ui.has("Fixed"));
            ui.commit("fixed-stations-calling", "4");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (4, 4));
            assert!(
                ui.has("Pile-up spread"),
                "the controls should have appeared"
            );

            // Opened up, it becomes a range with both ends editable.
            ui.click("link-stations-calling");
            ui.advance(50).await;
            ui.commit("from-stations-calling", "2");
            ui.advance(50).await;
            ui.commit("to-stations-calling", "5");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (2, 5));
            assert!(ui.has("2–5 at once"), "the range should read back");

            for (slider, value, reads) in [
                ("slider-pile-up-spread", "260", "±260 Hz"),
                ("slider-pile-up-is-weaker-by", "20", "20 dB"),
            ] {
                ui.type_into(slider, value);
                ui.advance(50).await;
                assert!(
                    ui.has(&format!("class=\"slider-value\">{reads}<")),
                    "{slider} should read {reads}"
                );
            }
            let stored = crate::persist::load_settings();
            assert_eq!(stored.band.pileup_spread_hz, 260.0);
            assert_eq!(stored.band.pileup_level_db, 20.0);
            assert_eq!(stored.clone().clamp(), stored);

            // Back to one station, alone, and they go away again.
            ui.click("link-stations-calling");
            ui.advance(50).await;
            ui.commit("fixed-stations-calling", "1");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!((stored.band.stations_min, stored.band.stations_max), (1, 1));
            assert_eq!(stored.band.filter_shape, test_settings().band.filter_shape);
            assert!(!ui.has("Pile-up spread"));
        });
    }

    /// Test settings run a dead-quiet band; these tests need one that is on.
    fn with_receiver() -> TrainingSettings {
        let mut settings = test_settings();
        settings.band.noise_enabled = true;
        settings.band.noise_level = 0.4;
        settings
    }

    /// Pressing stop has to stop everything. The receiver background lives on
    /// its own stream, outliving any one send, so stopping only the send left
    /// it hissing away with nothing to hear it under — and the quieter the
    /// band used to be, the longer that went unnoticed.
    #[test]
    fn stopping_the_preview_silences_the_receiver_too() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            assert!(
                recorder.band_running.get(),
                "the preview should have the receiver running"
            );

            ui.click("btn-band-stop");
            ui.advance(200).await;
            assert!(
                !recorder.band_running.get(),
                "stop left the receiver background playing"
            );
        });
    }

    /// Same for walking away from the screen that was making the sound.
    #[test]
    fn leaving_the_screen_silences_the_receiver_too() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            assert!(recorder.band_running.get());

            ui.click("nav-practice");
            ui.advance(200).await;
            assert_eq!(ui.screen(), "home");
            assert!(
                !recorder.band_running.get(),
                "the receiver followed us off the screen"
            );
        });
    }

    #[test]
    fn a_preview_that_loses_the_audio_stops_and_says_so() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            // The device disappears mid-loop.
            recorder.set(crate::audio::fake::Behaviour::RefuseToStart);
            assert!(ui.run_until(30_000, |ui| ui.has("class=\"toast\"")).await);
            assert!(ui.run_until(5_000, |ui| ui.has("Live preview")).await);
        });
    }

    /// A band preview that gives up on a failing device has to take the
    /// receiver down with it. Its retries rebuild the player, which brings
    /// the band back up on the new one; stopping only the loop left that
    /// hissing with the Stop button already gone.
    #[test]
    fn a_preview_that_gives_up_silences_the_receiver() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(with_receiver());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);
            recorder.set(crate::audio::fake::Behaviour::RefuseToStart);
            assert!(ui.run_until(30_000, |ui| ui.has("class=\"toast\"")).await);
            assert!(ui.run_until(5_000, |ui| ui.has("Live preview")).await);
            assert!(
                !recorder.band_running.get(),
                "the receiver was left running with nothing to stop it"
            );
        });
    }

    /// Taking the audio stops whatever had it, whether or not the new owner
    /// then gets a player. A claim that failed used to leave the screen
    /// showing a band preview whose loop it had already stopped.
    #[test]
    fn a_claim_that_fails_leaves_no_preview_showing() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            assert!(ui.run_until(20_000, |_| !recorder.texts().is_empty()).await);

            // The device goes away mid-send, and with it any new player.
            recorder.build_error.set(true);
            recorder.fail_next(1);
            let mut waited = 0;
            while !recorder
                .calls()
                .contains(&crate::audio::fake::Call::Shutdown)
            {
                assert!(waited < 5_000, "the send was never retried");
                ui.advance(4).await;
                waited += 4;
            }
            // The retry has no player left; a sample asks for the audio now.
            ui.click("chip-dit");
            ui.advance(50).await;
            assert!(ui.has("No audio output device found"));
            assert!(
                ui.has("Live preview"),
                "the band preview still shows as running: {}",
                ui.html()
            );
        });
    }

    /// The same message twice is shown for its full time the second time
    /// too. The first one's timer used to clear the second a second in.
    #[test]
    fn a_toast_raised_again_stays_for_its_full_time() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            recorder.set(crate::audio::fake::Behaviour::RefuseToStart);
            ui.click("nav-settings");
            ui.click("chip-dit");
            assert!(ui.run_until(5_000, |ui| ui.has("class=\"toast\"")).await);
            ui.advance(3_000).await;
            ui.click("chip-dit");
            assert!(ui.run_until(5_000, |ui| !ui.has("test-chip playing")).await);
            assert!(ui.has("class=\"toast\""));
            ui.advance(1_500).await;
            assert!(
                ui.has("class=\"toast\""),
                "the second toast was cleared by the first one's timer"
            );
            assert!(ui.run_until(4_000, |ui| !ui.has("class=\"toast\"")).await);
        });
    }

    #[test]
    fn a_preview_with_no_audio_device_is_reported() {
        run(|| async {
            let (mut ui, recorder) = Ui::app_with_settings(test_settings());
            recorder.build_error.set(true);
            ui.click("nav-settings");
            ui.click("btn-band-preview");
            ui.advance(100).await;
            assert!(ui.has("No audio output device found"));
            assert!(ui.has("Live preview"), "nothing should be playing");
        });
    }

    #[test]
    fn the_stats_screen_shows_every_tab_of_a_real_history() {
        run(|| async {
            let history = [
                stored_session("2026-09-01", &[("KM", "KX"), ("MU", "MU")]),
                stored_session("2026-09-02", &[("KM", "KM")]),
            ];
            let (mut ui, _recorder) = Ui::app_with_history(test_settings(), &history);
            ui.click("nav-stats");
            assert!(ui.has("Average"));
            for (tab, marker) in [
                ("seg-letters", "Letter accuracy"),
                ("seg-mistakes", "Common confusions"),
                ("seg-sampling", "Sampling snapshot"),
                ("seg-history", "Session history"),
                ("seg-overview", "Accuracy over time"),
            ] {
                ui.click(tab);
                assert!(ui.has(marker), "{tab} should show {marker}");
            }
        });
    }

    #[test]
    fn the_practice_screen_shows_a_history_once_there_is_one() {
        run(|| async {
            let (ui, _recorder) = Ui::app_with_settings(short_session());
            assert!(!ui.has("Practice calendar"));
            assert!(ui.has("First session"));
            drop(ui);

            let history = [stored_session(
                &crate::time::local_date_string(),
                &[("KM", "KM")],
            )];
            let (ui, _recorder) = Ui::app_with_history(short_session(), &history);
            assert!(ui.has("Practice calendar"));
            assert!(ui.has("Last accuracy"));
            assert!(ui.has("streak"));
        });
    }

    /// Every control on a screen, pressed and typed into in turn: none may
    /// panic, and none may leave settings the trainer would refuse to start
    /// from. Each one starts from a fresh app, so the sweep cannot walk itself
    /// off the screen it is testing, and every panel behind a toggle is opened
    /// first so the sweep reaches what the screen hides as well as what it
    /// shows.
    async fn sweep(nav: &str, settings: TrainingSettings) {
        let (mut probe, _recorder) = Ui::app_with_settings(settings.clone());
        probe.click(nav);
        probe.open_disclosures();
        let controls = probe.controls("");
        drop(probe);
        assert!(controls.len() > 5, "the screen should have controls");

        for control in controls {
            for value in ["4", "-3", "999", "KMU", "true"] {
                let (mut ui, _recorder) = Ui::app_with_settings(settings.clone());
                ui.click(nav);
                ui.open_disclosures();
                if !ui.shows(&control) {
                    continue;
                }
                ui.click(&control);
                if ui.shows(&control) {
                    ui.type_into(&control, value);
                }
                if ui.shows(&control) {
                    ui.commit(&control, value);
                }
                ui.advance(50).await;
                let stored = crate::persist::load_settings();
                assert_eq!(stored.clone().clamp(), stored, "{value:?} into {control}");
                assert!(!ui.html().is_empty());
                ui.character_boxes_hold_single_characters();
            }
        }
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_it() {
        run(|| async {
            sweep("nav-settings", TrainingSettings::default()).await;
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_a_custom_alphabet() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Custom;
            settings.curriculum.custom_set = vec!['K', 'M', 'U', 'R', 'E'];
            settings.curriculum.level = 4;
            sweep("nav-settings", settings).await;
        });
    }

    #[test]
    fn nothing_on_the_settings_screen_can_break_callsign_mode() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            settings.curriculum.callsign_level = 4;
            sweep("nav-settings", settings).await;
        });
    }

    /// The controls that mean nothing for a callsign have to be gone, not just
    /// ignored: a group-size slider that does nothing is worse than no slider.
    #[test]
    fn the_settings_screen_only_offers_what_a_callsign_has() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.char_set_mode = CharSetMode::Callsign;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            assert!(ui.has("Callsign tier"));
            assert!(ui.has("Callsigns per session"));
            assert!(ui.has("What this tier sends"));
            for gone in [
                "field-sequence-order",
                "field-custom-alphabet",
                "field-digits-level",
            ] {
                assert!(!ui.shows(gone), "{gone} has no meaning for a callsign");
            }
            assert!(!ui.has("Practice window"), "there is no unlock order");
            assert!(!ui.has("Group size"), "a callsign is as long as it is");

            // The tier changes what the preview shows.
            let before = ui.html();
            ui.commit("field-level", "6");
            ui.advance(50).await;
            assert_ne!(before, ui.html(), "the preview should follow the tier");
            assert_eq!(crate::persist::load_settings().curriculum.callsign_level, 6);
        });
    }

    /// Switching away and back must not leave one mode's level in the other's
    /// field — the whole reason callsigns carry their own.
    #[test]
    fn each_character_set_keeps_its_own_level() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.level = 9;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("nav-settings");
            ui.click("seg-callsigns");
            ui.commit("field-level", "5");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!(stored.curriculum.callsign_level, 5);
            assert_eq!(stored.curriculum.level, 9, "the Koch level is untouched");

            ui.click("seg-koch");
            ui.advance(50).await;
            let stored = crate::persist::load_settings();
            assert_eq!(stored.curriculum.level, 9);
            assert_eq!(stored.curriculum.callsign_level, 5);
        });
    }

    #[test]
    fn nothing_on_the_stats_screen_can_break_it() {
        run(|| async {
            sweep("nav-stats", TrainingSettings::default()).await;
        });
    }

    /// Nothing that happens over and over may cost the renderer more than a
    /// handful of mutations.
    ///
    /// On the desktop build every mutation crosses to the webview on the same
    /// loop that carries your keystrokes, so this is the number that decides
    /// whether typing feels immediate. Building a screen is allowed to be
    /// expensive — opening Settings is hundreds, and it happens once. What is
    /// not allowed is a repeating cost that scales with anything.
    #[test]
    fn nothing_that_repeats_costs_the_renderer_much() {
        run(|| async {
            let (mut ui, _rec) = Ui::app_with_settings(test_settings());
            ui.click("btn-start-training");
            assert!(ui.run_until(20_000, |ui| ui.screen() == "training").await);

            // A frame of the receiver trace while a group is sending.
            let _ = ui.take_work();
            ui.advance(40).await;
            let frame = ui.take_work();
            assert!(frame <= 4, "a scope frame cost {frame} mutations");

            assert!(ui.run_until(30_000, |ui| ui.has("Your turn")).await);

            // Nothing may run on its own while you are reading and typing.
            let _ = ui.take_work();
            ui.advance(400).await;
            let idle = ui.take_work();
            assert_eq!(idle, 0, "the screen repainted {idle} times doing nothing");

            // And a key is one box, not the list.
            ui.type_into("group-input-0", "K");
            let key = ui.take_work();
            assert!(key <= 4, "a keystroke cost {key} mutations");
        });
    }
}
