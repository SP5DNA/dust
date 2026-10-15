//! The settings screen: one card per part of the trainer.
//!
//! Each card reads the slice of the settings it shows, through a memo, so a
//! slider moving on one card redraws that card and leaves the rest of the
//! page alone.

use cw_core::{
    ADJUST_COUNT_MAX, ADJUST_THRESHOLD_MAX_PCT, ADJUST_THRESHOLD_MIN_PCT, AutoLevelSettings,
    COVERAGE_STRENGTH_MAX, CharSetMode, ERROR_WEIGHT_MAX, EXTRA_WORD_SPACE_MAX,
    EXTRA_WORD_SPACE_MIN, GROUP_PAUSE_MAX_SEC, GROUP_REPEAT_MAX, GROUP_REPEAT_MIN, GROUP_SIZE_MAX,
    GROUP_SIZE_MIN, GROUP_TIMEOUT_MAX_SEC, GroupSizeTarget, KeyerMode, NUM_GROUPS_MAX,
    NUM_GROUPS_MIN, RangeSetting, SIDE_TONE_MAX_HZ, SIDE_TONE_MIN_HZ, SettingsSection,
    TrainingSettings, VOLUME_MAX, VOLUME_MIN, WPM_MAX, WPM_MIN, adapts_group_size,
};
use dioxus::prelude::*;

use crate::ui::band::BandConditionsCard;
use crate::ui::envelope::EnvelopeCard;
use crate::ui::paddle::PaddlePad;
use crate::ui::settings_charset::CharsetCard;
use crate::ui::widgets::{
    CardHead, IconName, NumberField, SectionReset, Seg, SettingsRange, SliderField, Switch,
    control_id,
};

/// What a target for adaptive group size is called on screen.
pub(crate) fn target_label(target: GroupSizeTarget) -> &'static str {
    match target {
        GroupSizeTarget::Comfortable => "Comfortable",
        GroupSizeTarget::Balanced => "Balanced",
        GroupSizeTarget::Stretch => "Stretch",
    }
}

/// The keyer modes, in the order the control offers them.
const KEYER_MODES: [(KeyerMode, &str); 4] = [
    (KeyerMode::Straight, "Straight"),
    (KeyerMode::IambicA, "Iambic A"),
    (KeyerMode::IambicB, "Iambic B"),
    (KeyerMode::Ultimatic, "Ultimatic"),
];

/// What a keyer mode does with the paddles.
fn keyer_description(mode: KeyerMode) -> &'static str {
    match mode {
        KeyerMode::Straight => {
            "One contact. Hold [ or ] — a short press is a dit, a long one a dah. You time the elements."
        }
        KeyerMode::IambicA => {
            "Hold a paddle and it repeats at the key speed you set. Squeeze both to alternate. Let go and the element you were on finishes, then it stops."
        }
        KeyerMode::IambicB => {
            "Like Iambic A, but releasing a squeeze sends one extra opposite element — the extra dit after a dah is how C is often sent."
        }
        KeyerMode::Ultimatic => {
            "Last paddle you close wins and repeats. It does not alternate. Squeeze the dit paddle in a dah for the two dits in X, then let the dit go for the last dah."
        }
    }
}

/// A settings card's head: its icon, title and note, and the button that
/// puts the section back to its defaults.
#[component]
fn SectionHead(
    icon: IconName,
    title: &'static str,
    note: &'static str,
    settings: Signal<TrainingSettings>,
    section: SettingsSection,
) -> Element {
    rsx! {
        CardHead { icon, title, note,
            SectionReset {
                label: title.to_lowercase(),
                on_reset: move |()| settings.write().reset_section(section),
            }
        }
    }
}

#[component]
pub fn SettingsView(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    sample_playing: Option<String>,
    on_preview_band: EventHandler<()>,
    on_stop_band: EventHandler<()>,
    on_play_sample: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "stack",
            header { class: "page-head",
                h2 { class: "page-title", "Settings" }
                p { class: "page-sub", "Everything the trainer sends, and how hard it makes you work." }
            }
            CharsetCard { settings }
            SessionShapeCard { settings }
            SpeedCard { settings }
            ToneCard { settings }
            EnvelopeCard {
                settings,
                sample_playing,
                on_play: on_play_sample,
                on_stop: on_stop_band,
            }
            AutoLevelSettingsCard { settings }
            SamplingCard { settings }
            BandConditionsCard {
                settings,
                previewing,
                on_preview: on_preview_band,
                on_stop: on_stop_band,
            }
            crate::ui::privacy::PrivacyCard {}
        }
    }
}

/// What the session-shape card shows.
#[derive(Clone, PartialEq)]
struct SessionShape {
    callsigns: bool,
    /// The group size is one length, so there is nothing to adapt.
    one_size: bool,
    adaptive: bool,
    /// Adaptive group size is on and has lengths to choose between: what the
    /// sampler itself goes by.
    adapts: bool,
    target: GroupSizeTarget,
    num_groups: u32,
    group_timeout: f64,
    group_pause_sec: f64,
    lock_input: bool,
    keyer_mode: KeyerMode,
    keyer_wpm: f64,
    paddle_swap: bool,
}

impl SessionShape {
    fn of(settings: &TrainingSettings) -> Self {
        let p = &settings.playback;
        Self {
            callsigns: settings.curriculum.char_set_mode == CharSetMode::Callsign,
            one_size: settings.curriculum.min_group_size >= settings.curriculum.max_group_size,
            adaptive: settings.curriculum.adaptive_group_size,
            adapts: adapts_group_size(settings),
            target: settings.curriculum.group_size_target,
            num_groups: settings.curriculum.num_groups,
            group_timeout: p.group_timeout,
            group_pause_sec: p.group_pause_sec,
            lock_input: p.lock_input_during_group_playback,
            keyer_mode: p.keyer_mode,
            keyer_wpm: p.keyer_wpm,
            paddle_swap: p.paddle_swap,
        }
    }
}

/// What a target asks of you, in a sentence.
fn target_note(target: GroupSizeTarget) -> String {
    format!(
        "{} aims for {:.0}% of groups copied whole.",
        target_label(target),
        target.success_rate() * 100.0
    )
}

#[component]
fn SessionShapeCard(settings: Signal<TrainingSettings>) -> Element {
    let shape = use_memo(move || SessionShape::of(&settings.read()));
    let s = shape.read();
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Timer,
                title: "Session shape",
                note: "How much gets sent in one run",
                settings,
                section: SettingsSection::SessionShape,
            }
            div { class: "field-grid",
                NumberField {
                    label: if s.callsigns { "Callsigns per session" } else { "Groups per session" },
                    value: f64::from(s.num_groups),
                    min: f64::from(NUM_GROUPS_MIN),
                    max: f64::from(NUM_GROUPS_MAX),
                    step: 1.0,
                    onchange: move |v| settings.write().curriculum.num_groups = v as u32,
                }
                NumberField {
                    label: "Answer timeout",
                    value: s.group_timeout,
                    min: 0.0,
                    max: GROUP_TIMEOUT_MAX_SEC,
                    step: 1.0,
                    unit: "sec",
                    onchange: move |v| settings.write().playback.group_timeout = v,
                }
                NumberField {
                    label: "Pause after each group",
                    value: s.group_pause_sec,
                    min: 0.0,
                    max: GROUP_PAUSE_MAX_SEC,
                    step: 0.5,
                    unit: "sec",
                    onchange: move |v| settings.write().playback.group_pause_sec = v,
                }
            }
            p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                "Time to read the last group before the next send. Zero keeps contest pace. Repeats of the same group stay a word space apart."
            }
            // A callsign is as long as it is, so there is nothing to set.
            if !s.callsigns {
                SettingsRange {
                    settings,
                    which: RangeSetting::GroupSize,
                    label: "Group size",
                    unit: "chars",
                    min_bound: f64::from(GROUP_SIZE_MIN),
                    max_bound: f64::from(GROUP_SIZE_MAX),
                    step: 1.0,
                }
                Switch {
                    title: "Adapt group size to me",
                    description: if s.one_size {
                        "Set Group size to Random to use this: one size leaves nothing to adapt."
                    } else {
                        "Within this range: longer groups while you copy them whole, shorter ones when you don't. Your copy span is under Stats."
                    },
                    checked: s.adaptive,
                    onchange: move |on| settings.write().curriculum.adaptive_group_size = on,
                }
                if s.adapts {
                    div { class: "field",
                        span { class: "field-label", "Aim for" }
                        div { class: "segmented",
                            for target in GroupSizeTarget::ALL {
                                Seg {
                                    label: target_label(target),
                                    active: s.target == target,
                                    onclick: move |_| settings.write().curriculum.group_size_target = target,
                                }
                            }
                        }
                        p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                            {target_note(s.target)}
                        }
                    }
                }
            }
            SettingsRange {
                settings,
                which: RangeSetting::GroupRepeat,
                label: "Sends per group",
                unit: "×",
                min_bound: f64::from(GROUP_REPEAT_MIN),
                max_bound: f64::from(GROUP_REPEAT_MAX),
                step: 1.0,
                hint: "Each group is repeated this many times before the answer box opens — one word space apart, like a station repeating its call. Switch to Random to vary it per group.",
            }
            Switch {
                title: "Lock typing while sending",
                description: "Keeps you copying by ear instead of typing along.",
                checked: s.lock_input,
                onchange: move |on| settings.write().playback.lock_input_during_group_playback = on,
            }
            div { class: "field",
                span { class: "field-label", "Key" }
                div { class: "segmented",
                    for (mode, label) in KEYER_MODES {
                        Seg {
                            label,
                            active: s.keyer_mode == mode,
                            onclick: move |_| settings.write().playback.keyer_mode = mode,
                        }
                    }
                }
                p { class: "muted", style: "margin: 0; font-size: 0.8rem;",
                    "{keyer_description(s.keyer_mode)}"
                }
            }
            NumberField {
                label: "Key speed",
                value: s.keyer_wpm,
                min: WPM_MIN,
                max: WPM_MAX,
                step: 1.0,
                unit: "WPM",
                onchange: move |v| settings.write().playback.keyer_wpm = v,
            }
            PaddlePad { mode: s.keyer_mode }
            if !s.keyer_mode.is_straight() {
                Switch {
                    title: "Swap [ and ] paddles",
                    description: "[ is dit and ] is dah. Turn this on if your paddles are the other way around.",
                    checked: s.paddle_swap,
                    onchange: move |on| settings.write().playback.paddle_swap = on,
                }
            }
        }
    }
}

/// What the speed card shows beyond its two ranges.
#[derive(Clone, Copy, PartialEq)]
struct Speed {
    tied: bool,
    fist_variation: f64,
    extra_word_space: f64,
}

#[component]
fn SpeedCard(settings: Signal<TrainingSettings>) -> Element {
    let speed = use_memo(move || {
        let p = &settings.read().playback;
        Speed {
            tied: p.link_char_to_effective,
            fist_variation: p.fist_variation,
            extra_word_space: p.extra_word_space_multiplier,
        }
    })();
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Gauge,
                title: "Speed",
                note: "Character speed and Farnsworth spacing",
                settings,
                section: SettingsSection::Speed,
            }
            SettingsRange {
                settings,
                which: RangeSetting::CharWpm,
                label: "Character speed",
                unit: "WPM",
                min_bound: WPM_MIN,
                max_bound: WPM_MAX,
                step: 1.0,
            }
            if !speed.tied {
                SettingsRange {
                    settings,
                    which: RangeSetting::EffectiveWpm,
                    label: "Effective speed",
                    unit: "WPM",
                    min_bound: WPM_MIN,
                    max_bound: WPM_MAX,
                    step: 1.0,
                    hint: "Farnsworth: characters stay fast, the gaps between them stretch.",
                }
            }
            SliderField {
                label: "Operator fists",
                id: control_id("slider", "operator fists"),
                value_label: if speed.fist_variation <= 0.0 {
                    "Keyers".to_string()
                } else {
                    format!("{:.0}%", speed.fist_variation * 100.0)
                },
                value: speed.fist_variation,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: false,
                onchange: move |v| settings.write().playback.fist_variation = v,
            }
            p { class: "muted", style: "margin: 0;",
                "How far the stations calling stray from a keyer — heavier or lighter elements, longer or shorter dahs, each operator sending their own way. At zero every station is a machine, which is what you want while a character is still new."
            }
            NumberField {
                label: "Extra word spacing",
                value: speed.extra_word_space,
                min: EXTRA_WORD_SPACE_MIN,
                max: EXTRA_WORD_SPACE_MAX,
                step: 0.1,
                unit: "×",
                onchange: move |v| settings.write().playback.extra_word_space_multiplier = v,
            }
            Switch {
                title: "Tie effective speed to character speed",
                description: "Off gives you Farnsworth spacing controls.",
                checked: speed.tied,
                onchange: move |on| {
                    let w = &mut *settings.write();
                    w.playback.link_char_to_effective = on;
                    w.sync_effective_to_char();
                },
            }
        }
    }
}

/// Nothing but ranges, which read their own values: this card itself never
/// redraws for a setting.
#[component]
fn ToneCard(settings: Signal<TrainingSettings>) -> Element {
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Volume,
                title: "Tone & volume",
                note: "Side tone pitch and sending level",
                settings,
                section: SettingsSection::ToneAndVolume,
            }
            SettingsRange {
                settings,
                which: RangeSetting::SideTone,
                label: "Side tone",
                unit: "Hz",
                min_bound: SIDE_TONE_MIN_HZ,
                max_bound: SIDE_TONE_MAX_HZ,
                step: 10.0,
                hint: "A range picks a fresh pitch per group, the way different stations sound.",
            }
            SettingsRange {
                settings,
                which: RangeSetting::Volume,
                label: "Volume",
                unit: "gain",
                min_bound: VOLUME_MIN,
                max_bound: VOLUME_MAX,
                step: 0.05,
            }
        }
    }
}

/// The auto-level settings, which both of the cards below read.
fn use_auto_level(settings: Signal<TrainingSettings>) -> Memo<AutoLevelSettings> {
    use_memo(move || settings.read().auto_level.clone())
}

#[component]
fn AutoLevelSettingsCard(settings: Signal<TrainingSettings>) -> Element {
    let auto = use_auto_level(settings);
    let a = auto.read();
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Target,
                title: "Auto level",
                note: "Move the level from session accuracy",
                settings,
                section: SettingsSection::AutoLevel,
            }
            Switch {
                title: "Adjust the level for me",
                description: "Unlocks a new character after a run of good sessions.",
                checked: a.auto_adjust_level,
                onchange: move |on| settings.write().auto_level.auto_adjust_level = on,
            }
            NumberField {
                label: "Accuracy threshold",
                value: a.auto_adjust_threshold,
                min: ADJUST_THRESHOLD_MIN_PCT,
                max: ADJUST_THRESHOLD_MAX_PCT,
                step: 1.0,
                unit: "%",
                onchange: move |v| settings.write().auto_level.auto_adjust_threshold = v,
            }
            div { class: "field-grid",
                NumberField {
                    label: "Sessions above to level up",
                    value: f64::from(a.auto_adjust_above_threshold_count),
                    min: 0.0,
                    max: f64::from(ADJUST_COUNT_MAX),
                    step: 1.0,
                    onchange: move |v| settings.write().auto_level.auto_adjust_above_threshold_count = v as u32,
                }
                NumberField {
                    label: "Sessions below to level down",
                    value: f64::from(a.auto_adjust_below_threshold_count),
                    min: 0.0,
                    max: f64::from(ADJUST_COUNT_MAX),
                    step: 1.0,
                    onchange: move |v| settings.write().auto_level.auto_adjust_below_threshold_count = v as u32,
                }
            }
        }
    }
}

#[component]
fn SamplingCard(settings: Signal<TrainingSettings>) -> Element {
    let auto = use_auto_level(settings);
    let a = auto.read();
    rsx! {
        div { class: "card stack-sm",
            SectionHead {
                icon: IconName::Shuffle,
                title: "Character sampling",
                note: "Which characters come round more often",
                settings,
                section: SettingsSection::CharacterSampling,
            }
            div { class: "field-grid",
                NumberField {
                    label: "Error-weight strength",
                    value: a.error_weight_strength,
                    min: 0.0,
                    max: ERROR_WEIGHT_MAX,
                    step: 0.5,
                    onchange: move |v| settings.write().auto_level.error_weight_strength = v,
                }
                NumberField {
                    label: "Coverage strength",
                    value: a.char_sampling_coverage_strength,
                    min: 0.0,
                    max: COVERAGE_STRENGTH_MAX,
                    step: 0.5,
                    onchange: move |v| settings.write().auto_level.char_sampling_coverage_strength = v,
                }
            }
            Switch {
                title: "Thompson sampling",
                description: "Draws from each letter's uncertainty instead of its average error rate.",
                checked: a.char_sampling_thompson,
                onchange: move |on| settings.write().auto_level.char_sampling_thompson = on,
            }
        }
    }
}
