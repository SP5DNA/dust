use cw_core::{BOARD_MIN_GROUPS, SessionResult};
use dioxus::prelude::*;

use crate::ui::format::ordinal;
use crate::ui::scores::{Medal, mode_noun, per_group_label};
use crate::ui::widgets::{CardHead, GroupResultRow, Icon, IconName, ScoreRing, control_id};

/// What the session's place on the score board says, and the medal it
/// earns, if any: among the sessions of its own mode, kept on this device,
/// by what each earned a group.
fn place_line(place: usize, of: usize, noun: &str, per_group: f64) -> (String, Option<usize>) {
    let medal = (place <= 3).then_some(place);
    let rating = per_group_label(per_group);
    let text = if of == 1 {
        format!("Your first {noun} session on the board — {rating} points a group")
    } else if place == 1 {
        format!("New personal best — {rating} points a group, the top of your {of} {noun} sessions")
    } else {
        format!(
            "{} best of your {of} {noun} sessions — {rating} points a group",
            ordinal(place)
        )
    };
    (text, medal)
}

#[cfg(test)]
mod tests {
    use super::place_line;

    #[test]
    fn a_place_is_said_among_the_sessions_of_its_mode() {
        assert_eq!(
            place_line(1, 1, "callsign", 64.0),
            (
                "Your first callsign session on the board — 64.0 points a group".to_string(),
                Some(1)
            )
        );
        assert_eq!(
            place_line(1, 8, "Koch", 72.44).0,
            "New personal best — 72.4 points a group, the top of your 8 Koch sessions"
        );
        assert_eq!(
            place_line(3, 8, "Koch", 60.0),
            (
                "3rd best of your 8 Koch sessions — 60.0 points a group".to_string(),
                Some(3)
            )
        );
        assert_eq!(
            place_line(12, 40, "word", 10.0).1,
            None,
            "medals go to three"
        );
    }
}

#[component]
pub fn ResultsView(
    result: SessionResult,
    /// Where the session stands on the score board, and of how many.
    #[props(default)]
    place: Option<(usize, usize)>,
    auto_message: Option<String>,
    on_again: EventHandler<()>,
    on_home: EventHandler<()>,
) -> Element {
    let acc_pct = (result.accuracy * 100.0).round();
    let correct = result.groups.iter().filter(|g| g.correct).count();
    let avg = if result.avg_response_ms > 0.0 {
        format!("{}ms", result.avg_response_ms.round())
    } else {
        "—".into()
    };
    // About the copy, not the level: whether the level moves is the
    // learner's own rule, and the banner below says so when it does.
    let verdict = if result.accuracy >= 0.95 {
        "Clean copy — that pace is yours."
    } else if result.accuracy >= 0.9 {
        "Solid session — a few slips, nothing that stuck."
    } else if result.accuracy >= 0.7 {
        "Getting there — the weak letters come round more often now."
    } else {
        "Rough band. Slow the speed or shrink the pool for a run."
    };
    rsx! {
        div { class: "stack",
            div { class: "card",
                div { class: "score-hero",
                    ScoreRing { pct: acc_pct, value: format!("{acc_pct}%"), caption: "ACCURACY".to_string() }
                    div { class: "score-copy",
                        h2 { "Session complete" }
                        p { class: "muted", style: "margin: 0;", "{verdict}" }
                    }
                }
            }
            if let (Some((place, of)), Some(per_group)) = (place, cw_core::per_group(&result)) {
                {
                    let (text, medal) = place_line(place, of, mode_noun(result.char_set_mode), per_group);
                    rsx! {
                        div { class: if medal == Some(1) { "place-banner best" } else { "place-banner" },
                            if let Some(medal) = medal {
                                Medal { place: medal }
                            } else {
                                span { class: "board-number", "{place}" }
                            }
                            span { "{text}" }
                        }
                    }
                }
            } else if result.groups.len() < BOARD_MIN_GROUPS && result.score > 0.0 {
                p { class: "muted place-note",
                    "Sessions of {BOARD_MIN_GROUPS} groups or more go on the score board."
                }
            }
            if let Some(msg) = auto_message {
                div { class: "auto-banner",
                    Icon { name: IconName::Sparkle }
                    span { "{msg}" }
                }
            }
            div { class: "grid-3",
                div { class: "kpi blue",
                    div { class: "tiny", "Groups" }
                    div { class: "value", "{correct}/{result.groups.len()}" }
                }
                div { class: "kpi purple",
                    div { class: "tiny", "Avg time" }
                    div { class: "value", "{avg}" }
                }
                div { class: "kpi indigo",
                    div { class: "tiny", "Score" }
                    div { class: "value", "{result.score.round()}" }
                }
            }
            div { class: "card",
                CardHead {
                    icon: IconName::Letters,
                    title: "Group by group",
                    note: "Sent on top, your copy below.",
                }
                div { class: "result-rows",
                    for (idx, group) in result.groups.iter().cloned().enumerate() {
                        GroupResultRow { index: idx, group }
                    }
                }
            }
            div { class: "hero-actions", style: "justify-content: center;",
                button {
                    id: control_id("btn", "train again"),
                    class: "btn btn-primary",
                    onclick: move |_| on_again.call(()),
                    Icon { name: IconName::Repeat }
                    "Train again"
                }
                button {
                    id: control_id("btn", "back"),
                    class: "btn btn-secondary",
                    onclick: move |_| on_home.call(()),
                    Icon { name: IconName::Back }
                    "Back"
                }
            }
        }
    }
}
