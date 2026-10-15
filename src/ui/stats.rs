use cw_core::{
    AccuracyPoint, MasteryStatus, SessionResult, SpanPoint, TrainingSettings, accuracy_chart,
    adapts_group_size, character_diagnostics, span_history,
};
use dioxus::prelude::*;

use crate::ui::settings::target_label;
use crate::ui::stats_detail::{HistoryTab, LettersTab, MistakesTab, SamplingTab};
use crate::ui::widgets::{CardHead, IconName, Seg};

// 420×96 keeps the drawing box at the same aspect the card gives it, so the
// SVG can scale uniformly: stretched coordinates would turn the dots into
// ellipses and thin the stroke unevenly.
const CHART_W: f64 = 420.0;
const CHART_H: f64 = 96.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatsTab {
    Overview,
    Letters,
    Mistakes,
    Sampling,
    History,
}

/// The tabs, in the order the control offers them.
const TABS: [(StatsTab, &str); 5] = [
    (StatsTab::Overview, "Overview"),
    (StatsTab::Letters, "Letters"),
    (StatsTab::Mistakes, "Mistakes"),
    (StatsTab::Sampling, "Sampling"),
    (StatsTab::History, "History"),
];

/// Chart shape for the accuracy series. `None` when there is nothing to draw.
struct ChartGeometry {
    line: String,
    area: String,
    dots: Vec<(f64, f64)>,
    floor_pct: f64,
    threshold_y: Option<f64>,
}

/// Accuracy clusters near the top, so the axis starts below the worst session
/// instead of at zero — otherwise every series is a flat line in the top tenth.
fn chart_floor(points: &[AccuracyPoint]) -> f64 {
    let min = points
        .iter()
        .map(|p| p.accuracy_pct.clamp(0.0, 100.0))
        .fold(100.0_f64, f64::min);
    (((min - 6.0) / 10.0).floor() * 10.0).clamp(0.0, 80.0)
}

/// Where each of `count` sessions sits across the chart: spread over its
/// whole width, or, for a single session, a flat line across it — which beats
/// an empty chart, the way a one-point polyline renders.
fn series(values: &[f64], y_at: impl Fn(f64) -> f64) -> Vec<(f64, f64)> {
    if let [only] = values {
        let y = y_at(*only);
        return vec![(0.0, y), (CHART_W, y)];
    }
    let last = values.len().saturating_sub(1).max(1) as f64;
    values
        .iter()
        .enumerate()
        .map(|(i, value)| (i as f64 / last * CHART_W, y_at(*value)))
        .collect()
}

fn polyline(coords: &[(f64, f64)]) -> String {
    coords
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The points marked with a dot: every session on a short history, the last
/// on a long one, the middle of the line for a single session.
fn marks(coords: &[(f64, f64)], sessions: usize) -> Vec<(f64, f64)> {
    if sessions == 1 {
        coords
            .first()
            .map(|(_, y)| (CHART_W / 2.0, *y))
            .into_iter()
            .collect()
    } else if sessions <= 30 {
        coords.to_vec()
    } else {
        coords.last().copied().into_iter().collect()
    }
}

fn chart_geometry(points: &[AccuracyPoint], threshold_pct: f64) -> Option<ChartGeometry> {
    if points.is_empty() {
        return None;
    }
    let floor_pct = chart_floor(points);
    let span = (100.0 - floor_pct).max(1.0);
    let y_at = |pct: f64| {
        let pct = pct.clamp(0.0, 100.0);
        (CHART_H - ((pct - floor_pct) / span) * CHART_H).clamp(0.0, CHART_H)
    };

    let values: Vec<f64> = points.iter().map(|p| p.accuracy_pct).collect();
    let coords = series(&values, y_at);
    let line = polyline(&coords);
    let mut area = format!("M{:.1},{CHART_H:.1}", coords[0].0);
    for (x, y) in &coords {
        area.push_str(&format!(" L{x:.1},{y:.1}"));
    }
    area.push_str(&format!(
        " L{:.1},{CHART_H:.1} Z",
        coords.last().map(|(x, _)| *x).unwrap_or(0.0)
    ));

    Some(ChartGeometry {
        line,
        area,
        dots: marks(&coords, points.len()),
        floor_pct,
        threshold_y: (threshold_pct > floor_pct && threshold_pct < 100.0)
            .then(|| y_at(threshold_pct)),
    })
}

/// Chart shape for the copy span: its estimate after each session, and a
/// band one standard deviation either side of it, which narrows as the
/// estimate firms up.
struct SpanChart {
    line: String,
    band: String,
    dots: Vec<(f64, f64)>,
    /// The scale, in characters, bottom to top.
    low: f64,
    high: f64,
}

fn span_chart(points: &[SpanPoint]) -> Option<SpanChart> {
    if points.is_empty() {
        return None;
    }
    let below = |p: &SpanPoint| p.estimate.mean - p.estimate.sd;
    let above = |p: &SpanPoint| p.estimate.mean + p.estimate.sd;
    let low = points.iter().map(below).fold(f64::INFINITY, f64::min);
    let high = points.iter().map(above).fold(f64::NEG_INFINITY, f64::max);
    let low = low.floor().max(0.0);
    let high = high.ceil().max(low + 2.0);
    let y_at = |chars: f64| (CHART_H - (chars - low) / (high - low) * CHART_H).clamp(0.0, CHART_H);

    let means: Vec<f64> = points.iter().map(|p| p.estimate.mean).collect();
    let tops: Vec<f64> = points.iter().map(above).collect();
    let bottoms: Vec<f64> = points.iter().map(below).collect();
    let coords = series(&means, y_at);
    let mut band = String::new();
    for (i, (x, y)) in series(&tops, y_at).into_iter().enumerate() {
        band.push_str(&format!("{}{x:.1},{y:.1}", if i == 0 { "M" } else { " L" }));
    }
    for (x, y) in series(&bottoms, y_at).into_iter().rev() {
        band.push_str(&format!(" L{x:.1},{y:.1}"));
    }
    band.push_str(" Z");

    Some(SpanChart {
        line: polyline(&coords),
        band,
        dots: marks(&coords, points.len()),
        low,
        high,
    })
}

/// How many sessions of span the overview draws.
const SPAN_SESSIONS_SHOWN: usize = 30;

/// The copy span: how long a group you copy whole, and how that has moved.
/// Nothing for callsigns, which are as long as they are, or before a session
/// of the character set being trained.
#[component]
fn SpanCard(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    let points = span_history(&settings, &sessions);
    let shown = &points[points.len().saturating_sub(SPAN_SESSIONS_SHOWN)..];
    let (Some(first), Some(last), Some(chart)) = (shown.first(), shown.last(), span_chart(shown))
    else {
        return rsx! {};
    };
    let change = last.estimate.mean - first.estimate.mean;
    let count = shown.len();
    let trend = if count == 1 {
        "after 1 session".to_string()
    } else if change.abs() < 0.05 {
        format!("steady over {count} sessions")
    } else if change > 0.0 {
        format!("up {change:.1} over {count} sessions")
    } else {
        format!("down {:.1} over {count} sessions", -change)
    };
    let adapting = adapts_group_size(&settings);
    let mode = if adapting {
        format!(
            "Adapting · {}",
            target_label(settings.curriculum.group_size_target)
        )
    } else {
        "Adaptive length off".to_string()
    };
    rsx! {
        div { class: "card chart-card",
            CardHead {
                icon: IconName::Target,
                title: "Copy span",
                note: format!("{:.1} ± {:.1} chars · {trend}", last.estimate.mean, last.estimate.sd),
            }
            svg {
                class: "sparkline",
                view_box: "0 0 420 96",
                path { d: "{chart.band}", fill: "var(--info)", fill_opacity: "0.16", stroke: "none" }
                polyline {
                    points: "{chart.line}",
                    fill: "none",
                    stroke: "var(--info)",
                    stroke_width: "2.2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                }
                for (x, y) in chart.dots.iter() {
                    circle {
                        cx: "{x:.1}",
                        cy: "{y:.1}",
                        r: "3",
                        fill: "var(--info)",
                        stroke: "var(--surface)",
                        stroke_width: "1.5",
                    }
                }
            }
            div { class: "row-between",
                span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                    "Copied whole about half the time · scale {chart.low:.0}–{chart.high:.0} chars"
                }
                span { class: if adapting { "chip good" } else { "chip" }, "{mode}" }
            }
        }
    }
}

#[component]
pub fn StatsView(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    let mut tab = use_signal(|| StatsTab::Overview);
    // Accuracy, letters and mistakes are counted over the whole history: a
    // letter is the same letter whichever character set it was sent under.
    // Only the sampling snapshot narrows to the current set, and it does that
    // itself so it keeps matching what the trainer will actually draw. Each
    // tab works out its own figures when it is shown, and only then: a tab
    // that is not open costs nothing.
    rsx! {
        div { class: "stack",
            header { class: "page-head",
                h2 { class: "page-title", "Stats" }
                p { class: "page-sub", "Every session scored on this device, whatever it practised." }
            }
            div { class: "segmented",
                for (which, label) in TABS {
                    Seg { label, active: tab() == which, onclick: move |_| tab.set(which) }
                }
            }
            match tab() {
                StatsTab::Overview => rsx! {
                    OverviewTab { settings, sessions }
                },
                StatsTab::Letters => rsx! { LettersTab { sessions } },
                StatsTab::Mistakes => rsx! { MistakesTab { sessions } },
                StatsTab::Sampling => rsx! { SamplingTab { settings, sessions } },
                StatsTab::History => rsx! { HistoryTab { sessions } },
            }
        }
    }
}

#[component]
fn OverviewTab(settings: TrainingSettings, sessions: Vec<SessionResult>) -> Element {
    let threshold = settings.auto_level.auto_adjust_threshold;
    let chart = accuracy_chart(&sessions);
    let letters = character_diagnostics(&sessions);
    let avg = if sessions.is_empty() {
        0.0
    } else {
        sessions.iter().map(|s| s.accuracy).sum::<f64>() / sessions.len() as f64 * 100.0
    };
    let best = sessions
        .iter()
        .map(|s| s.accuracy * 100.0)
        .fold(0.0_f64, f64::max);
    let mastered = letters
        .iter()
        .filter(|d| d.status == MasteryStatus::Mastered)
        .count();
    let geometry = chart_geometry(&chart, threshold);
    let session_count = sessions.len();
    let session_noun = if session_count == 1 {
        "session"
    } else {
        "sessions"
    };
    let latest = chart.last().map(|p| p.accuracy_pct).unwrap_or(0.0);
    rsx! {
        div { class: "stack",
            if sessions.is_empty() {
                div { class: "card",
                    CardHead { icon: IconName::Chart, title: "Nothing scored yet" }
                    p { class: "muted", style: "margin: 0;",
                        "Finish a session to unlock accuracy over time, letter mastery and sampling weights."
                    }
                }
            } else {
                div { class: "grid-3",
                    div { class: "kpi emerald",
                        div { class: "tiny", "Average" }
                        div { class: "value", "{avg.round()}%" }
                    }
                    div { class: "kpi indigo",
                        div { class: "tiny", "Best" }
                        div { class: "value", "{best.round()}%" }
                    }
                    div { class: "kpi blue",
                        div { class: "tiny", "Mastered" }
                        div { class: "value", "{mastered}" }
                    }
                }
                if let Some(geometry) = geometry {
                    div { class: "card chart-card",
                        CardHead {
                            icon: IconName::Chart,
                            title: "Accuracy over time",
                            note: format!("{session_count} {session_noun} · latest {}%", latest.round()),
                        }
                        svg {
                            class: "sparkline",
                            view_box: "0 0 420 96",
                            defs {
                                linearGradient { id: "dust-acc-fill", x1: "0", y1: "0", x2: "0", y2: "1",
                                    stop { offset: "0%", style: "stop-color: var(--copper); stop-opacity: 0.42;" }
                                    stop { offset: "100%", style: "stop-color: var(--copper); stop-opacity: 0.02;" }
                                }
                            }
                            if let Some(y) = geometry.threshold_y {
                                line {
                                    x1: "0",
                                    y1: "{y:.1}",
                                    x2: "420",
                                    y2: "{y:.1}",
                                    stroke: "var(--good)",
                                    stroke_width: "1",
                                    stroke_dasharray: "4 4",
                                    opacity: "0.65",
                                }
                            }
                            path { d: "{geometry.area}", fill: "url(#dust-acc-fill)", stroke: "none" }
                            polyline {
                                points: "{geometry.line}",
                                fill: "none",
                                stroke: "var(--copper)",
                                stroke_width: "2.2",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                            }
                            for (x, y) in geometry.dots.iter() {
                                circle {
                                    cx: "{x:.1}",
                                    cy: "{y:.1}",
                                    r: "3",
                                    fill: "var(--copper)",
                                    stroke: "var(--surface)",
                                    stroke_width: "1.5",
                                }
                            }
                        }
                        div { class: "row-between",
                            span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                                "Scale {geometry.floor_pct}–100%"
                            }
                            span { class: "chip good", "Level up at {threshold.round()}%" }
                        }
                    }
                }
                SpanCard { settings, sessions }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CHART_H, CHART_W, chart_floor, chart_geometry};
    use cw_core::AccuracyPoint;

    fn points(pcts: &[f64]) -> Vec<AccuracyPoint> {
        pcts.iter()
            .enumerate()
            .map(|(i, pct)| AccuracyPoint {
                date: format!("2026-09-{:02}", i + 1),
                accuracy_pct: *pct,
                timestamp: 1_757_000_000_000 + i as u64,
            })
            .collect()
    }

    #[test]
    fn no_sessions_means_no_chart() {
        assert!(chart_geometry(&[], 90.0).is_none());
    }

    #[test]
    fn one_session_draws_a_flat_line_with_a_point() {
        let geometry = chart_geometry(&points(&[89.0]), 90.0).unwrap();
        // Two coordinates, so the polyline actually has a segment to draw.
        assert_eq!(geometry.line.split(' ').count(), 2);
        assert!(geometry.line.starts_with("0.0,"));
        assert!(geometry.area.ends_with(" Z"));
        assert_eq!(geometry.dots.len(), 1);
        assert!((geometry.dots[0].0 - CHART_W / 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_axis_starts_below_the_worst_session() {
        assert_eq!(chart_floor(&points(&[88.0, 94.0])), 80.0);
        assert_eq!(chart_floor(&points(&[71.0, 94.0])), 60.0);
        // Never below zero, and never so high that the span disappears.
        assert_eq!(chart_floor(&points(&[3.0])), 0.0);
        assert_eq!(chart_floor(&points(&[100.0])), 80.0);
    }

    #[test]
    fn the_series_spans_the_full_width_and_height() {
        let geometry = chart_geometry(&points(&[60.0, 100.0, 80.0]), 90.0).unwrap();
        let coords: Vec<(f64, f64)> = geometry
            .line
            .split(' ')
            .map(|pair| {
                let (x, y) = pair.split_once(',').unwrap();
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect();
        assert_eq!(coords.len(), 3);
        assert_eq!(coords[0].0, 0.0);
        assert_eq!(coords[2].0, CHART_W);
        // 100% sits on the top edge; the worst session sits above the bottom,
        // because the axis floor is the round ten below it.
        assert_eq!(coords[1].1, 0.0);
        assert_eq!(geometry.floor_pct, 50.0);
        assert!(coords[0].1 > coords[2].1, "60% must sit below 80%");
        assert!(coords.iter().all(|(_, y)| (0.0..=CHART_H).contains(y)));
        assert_eq!(geometry.dots.len(), 3);
    }

    #[test]
    fn only_the_last_point_is_marked_on_a_long_history() {
        let long: Vec<f64> = (0..40).map(|i| 60.0 + f64::from(i % 20)).collect();
        let geometry = chart_geometry(&points(&long), 90.0).unwrap();
        assert_eq!(geometry.dots.len(), 1);
    }

    #[test]
    fn the_threshold_line_hides_when_it_is_off_the_scale() {
        // Sessions in the 90s: the 90% guide sits inside the visible range.
        let inside = chart_geometry(&points(&[92.0, 97.0]), 90.0).unwrap();
        assert!(inside.threshold_y.is_some());
        // A threshold under the floor has nowhere to sit.
        let below = chart_geometry(&points(&[96.0, 99.0]), 50.0).unwrap();
        assert_eq!(below.floor_pct, 80.0);
        assert!(below.threshold_y.is_none());
        // Neither does one at the very top.
        let top = chart_geometry(&points(&[80.0]), 100.0).unwrap();
        assert!(top.threshold_y.is_none());
    }
}
