//! The guided setup: the settings that matter most when starting out, one at
//! a time, with the control to press pointed at on the page.
//!
//! Every step is a few moves — Random, Min 15, Max 25 — and each move knows
//! whether the settings show it made already. So the guide follows whatever
//! is done with the real controls: it ticks a move off once the settings show
//! it, however it got there, and points at the next. Show me makes the moves
//! still to make, one at a time and pointing at each first, through the same
//! calls the controls make, so the page changes the way it would by hand.
//!
//! It points by the ids the controls carry already ([`control_id`]), in a
//! style rule of its own, and needs nothing from the cards.

use cw_core::{CharSetMode, PresetId, RangeSetting, SequenceChoice, TrainingSettings};
use dioxus::core::Task;
use dioxus::prelude::*;

use crate::state::{AppCtx, Screen};
use crate::time::sleep_ms;
use crate::ui::widgets::{Icon, IconName, control_id, html_bool, pretty_number};

/// How long Show me points at a control before it makes the move there.
const POINT_MS: u32 = 700;
/// How long a move it made stays in sight before it points at the next.
const SHOWN_MS: u32 = 500;

/// A min–max setting, by the label the settings screen shows it under, which
/// is what the ids of its controls are made from.
#[derive(Clone, Copy, Debug)]
struct Range {
    which: RangeSetting,
    label: &'static str,
    unit: &'static str,
}

const CHARACTER_SPEED: Range = Range {
    which: RangeSetting::CharWpm,
    label: "Character speed",
    unit: "WPM",
};
const GROUP_SIZE: Range = Range {
    which: RangeSetting::GroupSize,
    label: "Group size",
    unit: "chars",
};
const SIDE_TONE: Range = Range {
    which: RangeSetting::SideTone,
    label: "Side tone",
    unit: "Hz",
};

/// The auto-level controls, by the labels they are shown under.
const AUTO_LEVEL_SWITCH: &str = "Adjust the level for me";
const LEVEL_UP_FIELD: &str = "Sessions above to level up";
const LEVEL_DOWN_FIELD: &str = "Sessions below to level down";

/// One thing to set, on one control.
#[derive(Clone, Copy, Debug)]
enum Move {
    /// Open a range up, so every group draws its own value from it.
    Random(Range),
    /// Where an open range starts.
    Min(Range, f64),
    /// Where it ends.
    Max(Range, f64),
    /// One of these unlock orders. Show me takes the first.
    Order(&'static [PresetId]),
    /// Let the trainer move the level.
    AutoLevel,
    /// How many good sessions move the level up.
    UpAfter(u32),
    /// How many poor ones move it back down.
    DownAfter(u32),
}

/// Whether the character set is learnt in an unlock order. Digits and
/// callsigns have none, and the settings screen offers none for them.
fn has_unlock_order(settings: &TrainingSettings) -> bool {
    !matches!(
        settings.curriculum.char_set_mode,
        CharSetMode::Digits | CharSetMode::Callsign
    )
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

impl Move {
    /// What it reads as on the guide.
    fn label(self) -> String {
        match self {
            Self::Random(_) => "Random".to_string(),
            Self::Min(range, value) => format!("Min {} {}", pretty_number(value), range.unit),
            Self::Max(range, value) => format!("Max {} {}", pretty_number(value), range.unit),
            Self::Order(presets) => presets
                .iter()
                .map(|id| id.preset().name)
                .collect::<Vec<_>>()
                .join(" or "),
            Self::AutoLevel => "Auto level on".to_string(),
            Self::UpAfter(sessions) => format!("Up after {sessions}"),
            Self::DownAfter(sessions) => format!("Down after {sessions}"),
        }
    }

    /// Whether the settings show it made.
    fn done(self, settings: &TrainingSettings) -> bool {
        let auto = &settings.auto_level;
        match self {
            Self::Random(range) => !settings.range(range.which).linked,
            Self::Min(range, value) => {
                let values = settings.range(range.which);
                !values.linked && same(values.min, value)
            }
            Self::Max(range, value) => {
                let values = settings.range(range.which);
                !values.linked && same(values.max, value)
            }
            Self::Order(presets) => {
                has_unlock_order(settings)
                    && presets
                        .iter()
                        .any(|id| settings.sequence_choice() == SequenceChoice::Preset(*id))
            }
            Self::AutoLevel => auto.auto_adjust_level,
            Self::UpAfter(sessions) => auto.auto_adjust_above_threshold_count == sessions,
            Self::DownAfter(sessions) => auto.auto_adjust_below_threshold_count == sessions,
        }
    }

    /// Make it, the way its control does.
    fn make(self, settings: &mut TrainingSettings) {
        match self {
            Self::Random(range) => settings.set_range_linked(range.which, false),
            Self::Min(range, value) => settings.set_range_min(range.which, value),
            Self::Max(range, value) => settings.set_range_max(range.which, value),
            Self::Order(presets) => {
                if !has_unlock_order(settings) {
                    settings.set_char_set_mode(CharSetMode::Koch);
                }
                if let Some(id) = presets.first() {
                    settings.choose_sequence(SequenceChoice::Preset(*id));
                }
            }
            Self::AutoLevel => settings.auto_level.auto_adjust_level = true,
            Self::UpAfter(sessions) => {
                settings.auto_level.auto_adjust_above_threshold_count = sessions;
            }
            Self::DownAfter(sessions) => {
                settings.auto_level.auto_adjust_below_threshold_count = sessions;
            }
        }
    }

    /// The ids of the controls it is made on, as the settings screen draws
    /// them for these settings.
    fn targets(self, settings: &TrainingSettings) -> Vec<String> {
        match self {
            Self::Random(range) => vec![control_id("link", range.label)],
            Self::Min(range, _) => vec![control_id("from", range.label)],
            Self::Max(range, _) => vec![control_id("to", range.label)],
            Self::Order(presets) if has_unlock_order(settings) => presets
                .iter()
                .map(|id| control_id("pill", id.preset().name))
                .collect(),
            // The orders are offered once there are letters to learn: Koch
            // first.
            Self::Order(_) => vec![control_id("seg", "Koch")],
            Self::AutoLevel => vec![control_id("switch", AUTO_LEVEL_SWITCH)],
            Self::UpAfter(_) => vec![control_id("number", LEVEL_UP_FIELD)],
            Self::DownAfter(_) => vec![control_id("number", LEVEL_DOWN_FIELD)],
        }
    }
}

/// One stop on the guide.
struct Step {
    title: &'static str,
    text: &'static str,
    moves: &'static [Move],
}

const STEPS: &[Step] = &[
    Step {
        title: "Unlock order",
        text: "The order new characters arrive in, one per level. MorseMania is the classic Koch order, from K M R S U; CW Academy starts with the commonest letters, E T A O N.",
        moves: &[Move::Order(&[PresetId::MorseMania, PresetId::CwAcademy])],
    },
    Step {
        title: "Character speed",
        text: "Press Random, then set where the range starts and ends. Every group is sent at a speed of its own from inside it.",
        moves: &[
            Move::Random(CHARACTER_SPEED),
            Move::Min(CHARACTER_SPEED, 15.0),
            Move::Max(CHARACTER_SPEED, 25.0),
        ],
    },
    Step {
        title: "Group size",
        text: "Random again, for groups of 2 to 3 characters: short enough to copy whole while the letters are new.",
        moves: &[
            Move::Random(GROUP_SIZE),
            Move::Min(GROUP_SIZE, 2.0),
            Move::Max(GROUP_SIZE, 3.0),
        ],
    },
    Step {
        title: "Side tone",
        text: "A range gives every group a pitch of its own, the way each station on the band sounds different. 500–700 Hz is a start; set what suits your ear.",
        moves: &[
            Move::Random(SIDE_TONE),
            Move::Min(SIDE_TONE, 500.0),
            Move::Max(SIDE_TONE, 700.0),
        ],
    },
    Step {
        title: "Auto level",
        text: "Let the trainer move the level: three good sessions unlock the next character, and one poor session takes it back. A session is good at or above the accuracy threshold.",
        moves: &[Move::AutoLevel, Move::UpAfter(3), Move::DownAfter(1)],
    },
    Step {
        title: "All set",
        text: "Every change is saved as you make it, and the reset button on each card puts that card back as it shipped. Start a session from Practice.",
        moves: &[],
    },
];

/// The move the guide points at: the one Show me is making, or else the
/// first still to make.
fn pointed(step: &Step, settings: &TrainingSettings, showing: Option<usize>) -> Option<usize> {
    showing.or_else(|| {
        step.moves
            .iter()
            .position(|step_move| !step_move.done(settings))
    })
}

/// The move Show me is making on step `at`, if it is making one there.
fn showing_on(showing: Option<(usize, usize)>, at: usize) -> Option<usize> {
    showing.filter(|(on, _)| *on == at).map(|(_, index)| index)
}

/// The rule that points at the controls `ids` name, those of them the page
/// draws. Its pulse is the stylesheet's.
fn pointer_css(ids: &[String]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    let selectors: Vec<String> = ids
        .iter()
        .map(|id| {
            // A switch's box lies see-through over its track: the knob is
            // what shows.
            if id.starts_with("switch-") {
                format!("#{id} + i")
            } else {
                format!("#{id}")
            }
        })
        .collect();
    format!(
        "{} {{ outline: 2px solid var(--copper); outline-offset: 2px; animation: tour-ring 1.4s ease-in-out infinite; animation-timing-function: steps(6, jump-none); }}",
        selectors.join(", ")
    )
}

/// The script that brings the first control of `ids` the page has into
/// sight, above the guide's dock, and leaves it where it is if it is in
/// sight already: a move made by hand points at the control next to it, and
/// the page should not jump for that.
fn scroll_script(ids: &[String]) -> String {
    let ids: Vec<String> = ids.iter().map(|id| format!("{id:?}")).collect();
    format!(
        r#"const target = [{}].map((id) => document.getElementById(id)).find((el) => el);
if (!target) {{
    return;
}}
// The dock covers the foot of the window: what is under it is out of sight.
const dock = document.getElementById("tour-dock");
const floor = dock ? dock.getBoundingClientRect().top : window.innerHeight;
const box = target.getBoundingClientRect();
if (box.top >= 0 && box.bottom <= floor) {{
    return;
}}
target.scrollIntoView({{ block: "center", inline: "nearest", behavior: "smooth" }});"#,
        ids.join(", ")
    )
}

fn bring_into_sight(ids: &[String]) {
    // Tests have no page to run it on.
    #[cfg(not(test))]
    {
        let script = scroll_script(ids);
        spawn(async move {
            let _ = document::eval(&script).await;
        });
    }
    #[cfg(test)]
    let _ = ids;
}

/// Open the guide on the settings screen, or close it if it is open.
fn toggle(ctx: AppCtx) {
    let mut tour = ctx.tour;
    if tour.peek().is_some() {
        tour.set(None);
        return;
    }
    if *ctx.screen.peek() != Screen::Settings {
        ctx.go_to(Screen::Settings);
    }
    // A session running holds the app where it is.
    if *ctx.screen.peek() == Screen::Settings {
        tour.set(Some(0));
    }
}

/// The way in: a small button in the header.
#[component]
pub fn TourButton() -> Element {
    let ctx = use_context::<AppCtx>();
    let open = ctx.tour.read().is_some();
    rsx! {
        button {
            id: control_id("btn", "guided setup"),
            class: if open { "icon-btn on" } else { "icon-btn" },
            r#type: "button",
            title: "Guided setup",
            aria_label: "Guided setup",
            aria_pressed: open,
            onclick: move |_| toggle(ctx),
            Icon { name: IconName::Help }
        }
    }
}

/// The guide itself, docked above the nav while it is open on the settings
/// screen, and the rule that points at the control it means.
#[component]
pub fn TourDock() -> Element {
    let ctx = use_context::<AppCtx>();
    let AppCtx {
        mut tour,
        settings,
        screen,
        ..
    } = ctx;
    // The move Show me is making, and the step it is making it on.
    let mut showing = use_signal(|| None::<(usize, usize)>);
    let mut player = use_signal(|| None::<Task>);

    // Show me stops wherever the guide goes: another step, or away.
    use_effect(move || {
        let _ = tour.read();
        if let Some(task) = player.write().take() {
            task.cancel();
        }
        showing.set(None);
    });

    // What to bring into sight: the controls pointed at, or once every move
    // of the step is made, those of its first.
    let aim = use_memo(move || {
        let Some((at, step)) = tour().and_then(|at| Some((at, STEPS.get(at)?))) else {
            return Vec::new();
        };
        let settings = settings.read();
        let shown = showing_on(showing(), at);
        pointed(step, &settings, shown)
            .or((!step.moves.is_empty()).then_some(0))
            .and_then(|index| step.moves.get(index))
            .map(|step_move| step_move.targets(&settings))
            .unwrap_or_default()
    });
    use_effect(move || {
        let ids = aim();
        if !ids.is_empty() {
            bring_into_sight(&ids);
        }
    });

    let Some(at) = tour() else {
        return rsx! {};
    };
    let Some(step) = STEPS.get(at) else {
        return rsx! {};
    };
    if screen() != Screen::Settings {
        return rsx! {};
    }
    let shown = showing_on(showing(), at);
    let (chips, pointer) = {
        let settings = settings.read();
        let pointing = pointed(step, &settings, shown);
        let chips: Vec<(String, &'static str)> = step
            .moves
            .iter()
            .enumerate()
            .map(|(index, step_move)| {
                let class = if step_move.done(&settings) {
                    "tour-move done"
                } else if pointing == Some(index) {
                    "tour-move next"
                } else {
                    "tour-move"
                };
                (step_move.label(), class)
            })
            .collect();
        let pointer = pointing
            .and_then(|index| step.moves.get(index))
            .map(|step_move| pointer_css(&step_move.targets(&settings)))
            .unwrap_or_default();
        (chips, pointer)
    };
    let all_made = chips.iter().all(|(_, class)| *class == "tour-move done");
    let last = at + 1 == STEPS.len();
    let total = STEPS.len();

    let show_me = move |_| {
        let task = spawn(async move {
            let mut settings = ctx.settings;
            let moves = STEPS.get(at).map_or(&[][..], |step| step.moves);
            for (index, step_move) in moves.iter().copied().enumerate() {
                if step_move.done(&settings.peek()) {
                    continue;
                }
                showing.set(Some((at, index)));
                sleep_ms(POINT_MS).await;
                // The guide went elsewhere while it pointed.
                if *tour.peek() != Some(at) {
                    return;
                }
                step_move.make(&mut settings.write());
                sleep_ms(SHOWN_MS).await;
            }
            showing.set(None);
        });
        player.set(Some(task));
    };

    rsx! {
        div { class: "tour-dock",
            section { id: "tour-dock", class: "tour", aria_label: "Guided setup",
                div { class: "tour-head",
                    span { class: "eyebrow", "Guided setup · {at + 1} of {total}" }
                    button {
                        id: control_id("tour", "close"),
                        class: "icon-btn",
                        r#type: "button",
                        title: "Close the guided setup",
                        aria_label: "Close the guided setup",
                        onclick: move |_| tour.set(None),
                        Icon { name: IconName::X }
                    }
                }
                div { aria_live: "polite",
                    h3 { class: "tour-title", "{step.title}" }
                    p { class: "tour-text", "{step.text}" }
                }
                if !chips.is_empty() {
                    div { class: "tour-moves",
                        for (label, class) in chips {
                            span { class,
                                if class == "tour-move done" {
                                    Icon { name: IconName::Check }
                                }
                                "{label}"
                            }
                        }
                    }
                }
                div { class: "tour-actions",
                    button {
                        id: control_id("tour", "back"),
                        class: "btn btn-secondary btn-sm",
                        r#type: "button",
                        disabled: html_bool(at == 0),
                        onclick: move |_| tour.set(Some(at.saturating_sub(1))),
                        Icon { name: IconName::Back }
                        "Back"
                    }
                    if !step.moves.is_empty() {
                        button {
                            id: control_id("tour", "show me"),
                            class: "btn btn-secondary btn-sm",
                            r#type: "button",
                            disabled: html_bool(all_made || shown.is_some()),
                            onclick: show_me,
                            Icon { name: IconName::Play }
                            "Show me"
                        }
                    }
                    if last {
                        button {
                            id: control_id("tour", "start practising"),
                            class: "btn btn-primary btn-sm tour-next",
                            r#type: "button",
                            onclick: move |_| ctx.go_to(Screen::Home),
                            "Start practising"
                        }
                    } else {
                        button {
                            id: control_id("tour", "next"),
                            class: if all_made { "btn btn-primary btn-sm tour-next" } else { "btn btn-secondary btn-sm tour-next" },
                            r#type: "button",
                            onclick: move |_| tour.set(Some(at + 1)),
                            "Next"
                        }
                    }
                }
            }
            if !pointer.is_empty() {
                style { "{pointer}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use cw_core::{
        CharSetMode, PresetId, RangeSetting, RangeValues, SequenceChoice, TrainingSettings,
    };

    use super::{Move, POINT_MS, SHOWN_MS, STEPS, pointer_css, scroll_script};
    use crate::persist::Store;
    use crate::testing::{Ui, run, test_settings};
    use crate::ui::widgets::pretty_number;

    /// Settings on which every move the guide asks for is still to make: a
    /// fixed speed, size and pitch, callsigns, which have no unlock order,
    /// and no auto level.
    fn needs_everything() -> TrainingSettings {
        let mut settings = test_settings();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.min_group_size = 4;
        settings.curriculum.max_group_size = 4;
        settings.curriculum.link_group_size = true;
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 20.0;
        settings.playback.link_char_wpm = true;
        settings.sync_effective_to_char();
        settings.band.side_tone_min = 600.0;
        settings.band.side_tone_max = 600.0;
        settings.auto_level.auto_adjust_level = false;
        settings.auto_level.auto_adjust_above_threshold_count = 5;
        settings.auto_level.auto_adjust_below_threshold_count = 2;
        settings
    }

    /// What the guide sets the trainer up for, from [`needs_everything`].
    fn assert_set_up(settings: &TrainingSettings) {
        let open = |min, max| RangeValues {
            min,
            max,
            linked: false,
        };
        assert_eq!(settings.range(RangeSetting::CharWpm), open(15.0, 25.0));
        assert_eq!(settings.range(RangeSetting::GroupSize), open(2.0, 3.0));
        assert_eq!(settings.range(RangeSetting::SideTone), open(500.0, 700.0));
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(
            settings.sequence_choice(),
            SequenceChoice::Preset(PresetId::MorseMania)
        );
        let auto = &settings.auto_level;
        assert!(auto.auto_adjust_level);
        assert_eq!(auto.auto_adjust_above_threshold_count, 3);
        assert_eq!(auto.auto_adjust_below_threshold_count, 1);
    }

    /// Make a move by hand, on the control with this id.
    fn by_hand(ui: &mut Ui, step_move: Move, id: &str) {
        match step_move {
            Move::Random(_) | Move::Order(_) => ui.click(id),
            Move::AutoLevel => ui.commit(id, "true"),
            Move::Min(_, value) | Move::Max(_, value) => ui.commit(id, &pretty_number(value)),
            Move::UpAfter(sessions) | Move::DownAfter(sessions) => {
                ui.commit(id, &sessions.to_string());
            }
        }
    }

    fn moves_made(ui: &Ui) -> usize {
        ui.html().matches("tour-move done").count()
    }

    #[test]
    fn the_guide_opens_on_the_settings_screen_and_closes_again() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app();
            assert!(!ui.shows("tour-next"));
            ui.click("btn-guided-setup");
            assert_eq!(ui.screen(), "settings");
            assert!(ui.has(&format!("Guided setup · 1 of {}", STEPS.len())));
            assert!(ui.has(STEPS[0].title));
            assert!(ui.input_tag("tour-back").contains("disabled"));
            assert!(
                ui.has("#pill-morsemania, #pill-cw-academy {"),
                "{}",
                ui.html()
            );
            ui.click("tour-close");
            assert!(!ui.shows("tour-next"));
            assert!(!ui.has("tour-ring"), "the pointer stayed up");
            // The header button opens it, and closes it again.
            ui.click("btn-guided-setup");
            assert!(ui.shows("tour-next"));
            ui.click("btn-guided-setup");
            assert!(!ui.shows("tour-next"));
            assert_eq!(ui.screen(), "settings");
        });
    }

    #[test]
    fn leaving_the_settings_screen_ends_the_guide() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
            ui.click("btn-guided-setup");
            ui.click("tour-next");
            ui.click("nav-practice");
            assert!(!ui.shows("tour-next"));
            ui.click("nav-settings");
            assert!(!ui.shows("tour-next"), "it came back with the screen");
            // And there is no way in while a session runs.
            ui.click("nav-practice");
            ui.click("btn-start-training");
            assert_eq!(ui.screen(), "training");
            assert!(!ui.shows("btn-guided-setup"));
        });
    }

    /// The guide followed by hand, on the real controls: each move is on a
    /// control the guide points at, and making it there ticks it off and
    /// points at the next. A label changed on the settings screen and not
    /// here would leave the guide pointing at nothing, and fails this.
    #[test]
    fn following_the_guide_by_hand_sets_the_trainer_up() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            for (at, step) in STEPS.iter().enumerate() {
                assert!(ui.has(step.title), "step {at} is not {}", step.title);
                for step_move in step.moves.iter().copied() {
                    // Callsigns take two presses to an unlock order: Koch,
                    // then the order.
                    for _ in 0..2 {
                        let settings = ui.stored_settings().await;
                        if step_move.done(&settings) {
                            break;
                        }
                        let id = step_move
                            .targets(&settings)
                            .into_iter()
                            .find(|id| ui.shows(id))
                            .unwrap_or_else(|| panic!("{step_move:?} has no control on the page"));
                        assert!(
                            ui.has(&format!("#{id}")),
                            "the guide does not point at {id} for {step_move:?}"
                        );
                        by_hand(&mut ui, step_move, &id);
                    }
                    let settings = ui.stored_settings().await;
                    assert!(step_move.done(&settings), "{step_move:?} was not made");
                }
                assert_eq!(moves_made(&ui), step.moves.len(), "{}", step.title);
                assert!(
                    !ui.has("tour-ring"),
                    "every move is made, and it still points"
                );
                if at + 1 < STEPS.len() {
                    ui.click("tour-next");
                }
            }
            ui.click("tour-start-practising");
            assert_eq!(ui.screen(), "home");
            assert!(!ui.shows("tour-next"));
            assert_set_up(&ui.store.load_settings());
        });
    }

    /// Show me points at a control before it makes the move there, then
    /// points at the next.
    #[test]
    fn show_me_points_at_each_control_before_it_makes_the_move() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            ui.click("tour-next");
            assert!(ui.has("Character speed"));
            ui.click("tour-show-me");
            assert!(ui.input_tag("tour-show-me").contains("disabled"));
            assert!(ui.has("#link-character-speed {"));
            ui.advance(u64::from(POINT_MS) / 2).await;
            assert!(ui.shows("fixed-character-speed"), "Random was made unseen");
            ui.advance(u64::from(POINT_MS)).await;
            assert!(ui.shows("from-character-speed"), "Random was not made");
            assert_eq!(moves_made(&ui), 1);
            ui.advance(u64::from(SHOWN_MS)).await;
            assert!(ui.has("#from-character-speed {"));

            assert!(
                ui.run_until(5_000, |ui| moves_made(ui) == 3).await,
                "{}",
                ui.html()
            );
            let speed = ui.stored_settings().await.range(RangeSetting::CharWpm);
            assert_eq!((speed.min, speed.max, speed.linked), (15.0, 25.0, false));
            assert!(!ui.input_tag("tour-next").contains("secondary"));
        });
    }

    #[test]
    fn show_me_on_every_step_sets_the_trainer_up() {
        run(|| async {
            let (mut ui, _recorder) = Ui::app_with_settings(needs_everything());
            ui.click("btn-guided-setup");
            for step in &STEPS[..STEPS.len() - 1] {
                ui.click("tour-show-me");
                let budget = 3 * u64::from(POINT_MS + SHOWN_MS) * step.moves.len() as u64;
                assert!(
                    ui.run_until(budget, |ui| moves_made(ui) == step.moves.len())
                        .await,
                    "{} was not shown: {}",
                    step.title,
                    ui.html()
                );
                ui.click("tour-next");
            }
            ui.click("tour-start-practising");
            assert_set_up(&ui.store.load_settings());
        });
    }

    /// A move already made is left as it is, and so is anything Show me did
    /// not reach before the guide went elsewhere.
    #[test]
    fn show_me_makes_only_what_is_left_and_stops_when_the_guide_moves() {
        run(|| async {
            for away in ["tour-next", "tour-back", "tour-close"] {
                let mut settings = needs_everything();
                settings.playback.link_char_wpm = false;
                settings.playback.char_wpm_max = 25.0;
                let (mut ui, _recorder) = Ui::app_with_settings(settings);
                ui.click("btn-guided-setup");
                ui.click("tour-next");
                assert_eq!(moves_made(&ui), 2, "Random and Max 25 are made already");
                ui.click("tour-show-me");
                assert!(ui.has("#from-character-speed {"), "{away}");
                ui.click(away);
                ui.advance(5_000).await;
                let speed = ui.stored_settings().await.range(RangeSetting::CharWpm);
                assert_eq!((speed.min, speed.max), (20.0, 25.0), "after {away}");
                assert_eq!(
                    ui.stored_settings()
                        .await
                        .range(RangeSetting::GroupSize)
                        .min,
                    4.0,
                    "after {away}"
                );
            }
        });
    }

    #[test]
    fn a_switch_is_pointed_at_by_its_knob() {
        let css = pointer_css(&[
            "switch-adjust-the-level-for-me".to_string(),
            "pill-koch".to_string(),
        ]);
        assert!(
            css.starts_with("#switch-adjust-the-level-for-me + i, #pill-koch {"),
            "{css}"
        );
        assert!(css.contains("animation: tour-ring"), "{css}");
        assert!(pointer_css(&[]).is_empty());
        assert!(
            include_str!("../../assets/styles.css").contains("@keyframes tour-ring"),
            "the pointer's pulse is the stylesheet's"
        );
    }

    /// The harness never mounts anything on a page, so the script is the one
    /// part of bringing a control into sight a test can see.
    #[test]
    fn the_first_control_there_is_is_brought_into_sight_above_the_dock() {
        let script = scroll_script(&["from-side-tone".to_string(), "to-side-tone".to_string()]);
        assert!(
            script.contains(r#"["from-side-tone", "to-side-tone"].map("#),
            "{script}"
        );
        assert!(
            script.contains(r#"document.getElementById("tour-dock")"#),
            "{script}"
        );
        let in_sight = script
            .find("box.bottom <= floor")
            .expect("a control in sight is left where it is");
        let scroll = script
            .find(r#"target.scrollIntoView({ block: "center""#)
            .expect("it is centred");
        assert!(in_sight < scroll, "{script}");
    }

    /// Every move reads as what it sets.
    #[test]
    fn each_move_says_what_it_sets() {
        let labels: Vec<Vec<String>> = STEPS
            .iter()
            .map(|step| {
                step.moves
                    .iter()
                    .map(|step_move| step_move.label())
                    .collect()
            })
            .collect();
        assert_eq!(
            labels,
            [
                vec!["MorseMania or CW Academy"],
                vec!["Random", "Min 15 WPM", "Max 25 WPM"],
                vec!["Random", "Min 2 chars", "Max 3 chars"],
                vec!["Random", "Min 500 Hz", "Max 700 Hz"],
                vec!["Auto level on", "Up after 3", "Down after 1"],
                vec![],
            ]
        );
    }
}
