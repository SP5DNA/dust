use cw_core::{AutoLevelChange, AutoLevelProgress, LevelAxis, MixedAutoLevelAxis};
use dioxus::prelude::*;

use crate::ui::widgets::{CardHead, IconName};

/// What to tell the user when a session moved their level.
pub fn change_message(change: &AutoLevelChange) -> String {
    let verb = if change.raised() {
        "increased"
    } else {
        "decreased"
    };
    let evidence = format!(
        "accuracy {}%, threshold {}%, {} sessions {}",
        change.accuracy_pct.round(),
        change.threshold_pct,
        change.sessions,
        if change.raised() { "above" } else { "below" },
    );
    let Some(next) = change.next_mixed_axis else {
        let label = if change.axis == LevelAxis::Callsign {
            "Callsign tier"
        } else {
            "Level"
        };
        return format!("{label} {verb} to {} ({evidence})", change.to);
    };
    let (axis, level) = if change.axis == LevelAxis::Digits {
        ("digit", format!("digits {}", change.to))
    } else {
        ("letter", format!("alphabet {}", change.to))
    };
    let next = match next {
        MixedAutoLevelAxis::Letters => "letter",
        MixedAutoLevelAxis::Digits => "digit",
    };
    format!("Mixed {axis} level {verb} — {level} (next: {next}, {evidence})")
}

#[component]
pub fn AutoLevelCard(progress: AutoLevelProgress) -> Element {
    let up_pct = if progress.above_disabled || progress.above_target == 0 {
        0.0
    } else {
        (f64::from(progress.above_count) / f64::from(progress.above_target) * 100.0).min(100.0)
    };
    let down_pct = if progress.below_disabled || progress.below_target == 0 {
        0.0
    } else {
        (f64::from(progress.below_count) / f64::from(progress.below_target) * 100.0).min(100.0)
    };
    let mixed = if progress.alternating_mixed {
        match progress.next_mixed_axis {
            Some(MixedAutoLevelAxis::Letters) => "Alternating letters/digits — next: Letters",
            Some(MixedAutoLevelAxis::Digits) => "Alternating letters/digits — next: Digits",
            None => "Alternates letter/digit level",
        }
    } else {
        "Resets on level change"
    };
    let up_label = if progress.above_disabled {
        "—".to_string()
    } else {
        format!("{}/{}", progress.above_count, progress.above_target)
    };
    let down_label = if progress.below_disabled {
        "—".to_string()
    } else {
        format!("{}/{}", progress.below_count, progress.below_target)
    };
    rsx! {
        section { class: "card",
            CardHead { icon: IconName::Target, title: "Auto level", note: mixed,
                span { class: "chip", "{progress.threshold as u32}% to pass" }
            }
            div { class: "auto-bars",
                div { class: if progress.above_disabled { "auto-meter disabled" } else { "auto-meter" },
                    div { class: "row", style: "justify-content: space-between;",
                        span { class: "muted", "Up" }
                        span { class: "mono", "{up_label}" }
                    }
                    div { class: "meter-track up",
                        span { style: "width: {up_pct}%;" }
                    }
                }
                div { class: if progress.below_disabled { "auto-meter disabled" } else { "auto-meter" },
                    div { class: "row", style: "justify-content: space-between;",
                        span { class: "muted", "Down" }
                        span { class: "mono", "{down_label}" }
                    }
                    div { class: "meter-track down",
                        span { style: "width: {down_pct}%;" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::{CharSetMode, CountersKey, SessionResult, TrainingSettings, evaluate_auto_level};

    fn change(settings: &TrainingSettings, accuracy: f64) -> AutoLevelChange {
        let result = SessionResult {
            accuracy,
            groups: vec![Default::default(); 20],
            ..crate::testing::stored_session("2026-09-01", &[("KM", "KM")])
        };
        evaluate_auto_level(&result, settings, &mut Default::default()).expect("a change")
    }

    #[test]
    fn the_message_names_the_level_and_the_evidence() {
        let mut koch = TrainingSettings::default();
        koch.curriculum.char_set_mode = CharSetMode::Koch;
        koch.curriculum.level = 4;
        koch.auto_level.auto_adjust_above_threshold_count = 1;
        assert_eq!(
            change_message(&change(&koch, 0.95)),
            "Level increased to 5 (accuracy 95%, threshold 90%, 1 sessions above)"
        );

        let mut callsigns = koch.clone();
        callsigns.curriculum.char_set_mode = CharSetMode::Callsign;
        callsigns.curriculum.callsign_level = 3;
        assert!(
            change_message(&change(&callsigns, 0.2)).starts_with("Callsign tier decreased to 2")
        );

        let mut mixed = koch;
        mixed.curriculum.char_set_mode = CharSetMode::Mixed;
        let message = change_message(&change(&mixed, 1.0));
        assert!(
            message.starts_with("Mixed letter level increased — alphabet 5 (next: digit"),
            "{message}"
        );
        assert_ne!(
            CountersKey::of(&mixed),
            change(&mixed, 1.0).counters_cleared[1]
        );
    }
}
