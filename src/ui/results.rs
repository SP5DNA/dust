use cw_core::SessionResult;
use dioxus::prelude::*;

use crate::ui::widgets::{CardHead, GroupResultRow, Icon, IconName, ScoreRing, control_id};

#[component]
pub fn ResultsView(
    result: SessionResult,
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
