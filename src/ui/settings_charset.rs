use cw_core::{
    CALLSIGN_TIER_MAX, CharSetMode, LevelAxis, MAX_DIGITS_LEVEL, PracticeWindow, SEQUENCE_PRESETS,
    SequenceChoice, SettingsSection, TrainingSettings, compute_char_pool, current_practice_window,
    tier_examples, unlocked_practice_count,
};
use dioxus::prelude::*;

use crate::ui::widgets::{CardHead, IconName, NumberField, SectionReset, Seg, control_id};

/// A list of characters, typed out.
///
/// What is typed stays as typed until it is committed — on Enter, or on
/// leaving the field — and only then is it cleaned down to the characters
/// that can be sent. Cleaned on every key, the box would move under the
/// cursor whenever a key was dropped, and could never be emptied to start
/// again: an empty unlock order is LCWO's, so clearing it would fill it
/// straight back up.
#[component]
fn AlphabetField(label: &'static str, value: String, on_commit: EventHandler<String>) -> Element {
    let mut typed = use_signal(|| None::<String>);
    let shown = typed().unwrap_or(value);
    let id = control_id("field", label);
    rsx! {
        div { class: "field",
            label { r#for: "{id}", "{label}" }
            input {
                id,
                class: "mono",
                value: "{shown}",
                autocapitalize: "characters",
                spellcheck: false,
                oninput: move |e| typed.set(Some(e.value())),
                onchange: move |e| {
                    typed.set(None);
                    on_commit.call(e.value());
                },
            }
        }
    }
}

/// The character sets, in the order the control offers them.
const CHAR_SET_MODES: [(CharSetMode, &str); 5] = [
    (CharSetMode::Koch, "Koch"),
    (CharSetMode::Digits, "Digits"),
    (CharSetMode::Mixed, "Mixed"),
    (CharSetMode::Custom, "Custom"),
    (CharSetMode::Callsign, "Callsigns"),
];

/// The practice windows, and how many characters each needs unlocked before
/// it is offered.
const PRACTICE_WINDOWS: [(PracticeWindow, &str, usize); 3] = [
    (PracticeWindow::All, "Everything", 0),
    (PracticeWindow::Last3, "Newest 3", 3),
    (PracticeWindow::Last5, "Newest 5", 5),
];

/// What the character-set card shows, worked out from the settings. A memo
/// of this is what the card reads, so it redraws when one of these moves and
/// not for a slider on another card.
#[derive(Clone, PartialEq)]
struct Charset {
    mode: CharSetMode,
    sequence: String,
    custom_set: String,
    choice: SequenceChoice,
    level: u32,
    level_max: u32,
    digits_level: u32,
    letters_percent: u32,
    unlocked: usize,
    window: PracticeWindow,
    pool: Vec<char>,
    tier_examples: Vec<String>,
}

impl Charset {
    fn of(settings: &TrainingSettings) -> Self {
        let c = &settings.curriculum;
        let callsigns = c.char_set_mode == CharSetMode::Callsign;
        Self {
            mode: c.char_set_mode,
            sequence: settings.sequence().iter().collect(),
            custom_set: c.custom_set.iter().collect(),
            choice: settings.sequence_choice(),
            level: settings.active_level(),
            level_max: settings.max_active_level(),
            digits_level: settings.level(LevelAxis::Digits),
            letters_percent: c.mixed_letters_percent,
            unlocked: unlocked_practice_count(settings),
            window: current_practice_window(settings),
            pool: compute_char_pool(settings),
            tier_examples: if callsigns {
                tier_examples(settings.active_level())
            } else {
                Vec::new()
            },
        }
    }

    fn note(&self) -> String {
        let what = match self.mode {
            CharSetMode::Koch => "Letters unlocked in Koch order",
            CharSetMode::Digits => "Digits only",
            CharSetMode::Mixed => "Letters and digits together",
            CharSetMode::Custom => "Your own character list",
            CharSetMode::Callsign => "Real callsigns, off the air",
        };
        if self.mode == CharSetMode::Callsign {
            format!("{what} · tier {} of {CALLSIGN_TIER_MAX}", self.level)
        } else {
            format!("{what} · {} in play", self.unlocked)
        }
    }
}

#[component]
pub fn CharsetCard(settings: Signal<TrainingSettings>) -> Element {
    let charset = use_memo(move || Charset::of(&settings.read()));
    let c = charset.read();
    let callsigns = c.mode == CharSetMode::Callsign;
    rsx! {
        div { class: "card stack-sm",
            CardHead { icon: IconName::Letters, title: "Character set", note: c.note(),
                SectionReset {
                    label: "character set",
                    on_reset: move |()| {
                        settings.write().reset_section(SettingsSection::CharacterSet);
                    },
                }
            }
            div { class: "segmented",
                for (mode, label) in CHAR_SET_MODES {
                    Seg {
                        label,
                        active: c.mode == mode,
                        onclick: move |_| settings.write().set_char_set_mode(mode),
                    }
                }
            }
            if c.mode != CharSetMode::Digits && !callsigns {
                div { class: "eyebrow", "Unlock order" }
                div { class: "mode-pills",
                    for choice in SEQUENCE_PRESETS
                        .iter()
                        .map(|preset| (SequenceChoice::Preset(preset.id), preset.name))
                        .chain([(SequenceChoice::Custom, "Custom")])
                    {
                        Seg {
                            label: choice.1,
                            pill: true,
                            active: c.choice == choice.0,
                            onclick: move |_| settings.write().choose_sequence(choice.0),
                        }
                    }
                }
                AlphabetField {
                    label: "Sequence order",
                    value: c.sequence.clone(),
                    on_commit: move |typed: String| settings.write().type_sequence(&typed),
                }
            }
            div { class: "field-grid",
                NumberField {
                    label: if callsigns {
                        format!("Callsign tier (1–{})", c.level_max)
                    } else {
                        format!("Level (1–{})", c.level_max)
                    },
                    id: "field-level",
                    value: f64::from(c.level),
                    min: 1.0,
                    max: f64::from(c.level_max),
                    step: 1.0,
                    onchange: move |v| settings.write().set_active_level(v as u32),
                }
                if c.mode == CharSetMode::Mixed {
                    NumberField {
                        label: format!("Digits level (1–{MAX_DIGITS_LEVEL})"),
                        id: "field-digits-level",
                        value: f64::from(c.digits_level),
                        min: 1.0,
                        max: f64::from(MAX_DIGITS_LEVEL),
                        step: 1.0,
                        onchange: move |v| settings.write().curriculum.digits_level = v as u32,
                    }
                }
            }
            if c.mode == CharSetMode::Mixed {
                NumberField {
                    label: "Share of letters",
                    value: f64::from(c.letters_percent),
                    min: 0.0,
                    max: 100.0,
                    step: 5.0,
                    unit: "%",
                    onchange: move |v| settings.write().curriculum.mixed_letters_percent = v as u32,
                }
            }
            if c.mode == CharSetMode::Custom {
                AlphabetField {
                    label: "Custom alphabet",
                    value: c.custom_set.clone(),
                    on_commit: move |typed: String| {
                        settings.write().curriculum.custom_set =
                            TrainingSettings::unique_alphabet(&typed.chars().collect::<Vec<_>>());
                    },
                }
                p { class: "muted", style: "margin: 0;", "Level unlocks this list from the start. Leave it empty to use the sequence above." }
            }
            if callsigns {
                div { class: "eyebrow", "What this tier sends" }
                div { class: "calls",
                    for call in c.tier_examples.iter() {
                        span { class: "call", "{call}" }
                    }
                }
                p { class: "muted", style: "margin: 0;",
                    "Every tier keeps what the ones below it send, so moving up only adds. Prefixes are real allocations, weighted the way you hear them."
                }
            } else {
                div { class: "eyebrow", "Practice window" }
                div { class: "mode-pills",
                    for (window, label, needs) in PRACTICE_WINDOWS {
                        if c.unlocked >= needs {
                            Seg {
                                label,
                                pill: true,
                                active: c.window == window,
                                onclick: move |_| settings.write().curriculum.practice_window = window,
                            }
                        }
                    }
                }
            }
            div { class: "chars", style: "gap: 4px;",
                for ch in c.pool.iter() {
                    span { class: "ch sent-row", "{ch}" }
                }
            }
        }
    }
}
