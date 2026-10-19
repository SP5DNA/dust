//! The score board: the best sessions kept on this device, the personal best
//! with its medal, and the score over time with the record it set.
//!
//! There is one player, so a row is a place, a score and when it was set —
//! no name, no callsign. The board is read off the stored history, so a
//! deleted session leaves it.

use chrono::{NaiveDate, TimeZone};
use cw_core::{
    BoardEntry, CharSetMode, SessionResult, TimelinePoint, board_modes, score_board, score_timeline,
};
use dioxus::prelude::*;

use crate::ui::settings_charset::CHAR_SET_MODES;
use crate::ui::widgets::{CardHead, IconName, Seg, control_id};

/// The rows the board shows before it is asked for all of them.
const BOARD_TOP: usize = 10;

/// "1st", "2nd", "3rd", "4th", "11th", "21st".
pub fn ordinal(place: usize) -> String {
    let suffix = match (place % 10, place % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{place}{suffix}")
}

fn mode_label(mode: CharSetMode) -> &'static str {
    CHAR_SET_MODES
        .iter()
        .find(|(each, _)| *each == mode)
        .map(|(_, label)| *label)
        .unwrap_or_default()
}

/// What a session of `mode` is called in a sentence: "callsign sessions".
pub fn mode_noun(mode: CharSetMode) -> &'static str {
    match mode {
        CharSetMode::Koch => "Koch",
        CharSetMode::Digits => "digit",
        CharSetMode::Mixed => "mixed",
        CharSetMode::Callsign => "callsign",
        CharSetMode::Words => "word",
    }
}

fn parse_date(date: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// "Fri 16 Oct 2026", or the date as stored if it cannot be read.
fn short_date(date: &str) -> String {
    parse_date(date).map_or_else(
        || date.to_string(),
        |day| day.format("%a %-d %b %Y").to_string(),
    )
}

/// "Friday 16 October 2026".
fn long_date(date: &str) -> String {
    parse_date(date).map_or_else(
        || date.to_string(),
        |day| day.format("%A %-d %B %Y").to_string(),
    )
}

/// The local time of day a session finished, "14:32".
fn clock(timestamp: u64) -> Option<String> {
    let millis = i64::try_from(timestamp).ok()?;
    chrono::Local
        .timestamp_millis_opt(millis)
        .single()
        .map(|at| at.format("%H:%M").to_string())
}

/// How long ago `date` was, seen from `today`: "today", "yesterday", "12 days
/// ago". Nothing for a date that cannot be read or is still to come.
fn ago(date: &str, today: &str) -> Option<String> {
    let days = (parse_date(today)? - parse_date(date)?).num_days();
    match days {
        0 => Some("today".to_string()),
        1 => Some("yesterday".to_string()),
        2.. => Some(format!("{days} days ago")),
        _ => None,
    }
}

/// When a session was set, in full: "Friday 16 October 2026 at 14:32".
fn when_set(entry: &BoardEntry) -> String {
    match clock(entry.timestamp) {
        Some(time) => format!("{} at {time}", long_date(&entry.date)),
        None => long_date(&entry.date),
    }
}

/// The speed a session was sent at: "25 WPM", or "25/15 WPM" under
/// Farnsworth.
fn speed(entry: &BoardEntry) -> String {
    if entry.char_wpm - entry.effective_wpm >= 0.5 {
        format!("{:.0}/{:.0} WPM", entry.char_wpm, entry.effective_wpm)
    } else {
        format!("{:.0} WPM", entry.char_wpm)
    }
}

/// A medal for the first three places: gold, silver and bronze, with the
/// place on it.
#[component]
pub fn Medal(place: usize, #[props(default)] large: bool) -> Element {
    let class = match (place, large) {
        (1, false) => "medal medal-gold",
        (1, true) => "medal medal-gold large",
        (2, false) => "medal medal-silver",
        (2, true) => "medal medal-silver large",
        (_, false) => "medal medal-bronze",
        (_, true) => "medal medal-bronze large",
    };
    let label = format!("{} place", ordinal(place));
    rsx! {
        svg {
            class: class,
            view_box: "0 0 32 40",
            role: "img",
            "aria-label": "{label}",
            path { class: "medal-ribbon-left", d: "M7 1h7l4.5 15h-7z" }
            path { class: "medal-ribbon-right", d: "M25 1h-7l-4.5 15h7z" }
            circle { class: "medal-disc", cx: "16", cy: "26.5", r: "12" }
            circle { class: "medal-ring", cx: "16", cy: "26.5", r: "8.8" }
            text {
                class: "medal-number",
                x: "16",
                y: "30.3",
                text_anchor: "middle",
                "{place}"
            }
        }
    }
}

const TIMELINE_W: f64 = 420.0;
const TIMELINE_H: f64 = 120.0;
/// Room at the chart's edges, so the outermost dots are not cut in half.
const TIMELINE_PAD: f64 = 8.0;

/// Where each session sits on the timeline: across by when it was set, up
/// by its score, against the best.
fn timeline_xy(points: &[TimelinePoint]) -> Vec<(f64, f64)> {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    let span = last.timestamp.saturating_sub(first.timestamp) as f64;
    let best = points.iter().map(|point| point.score).fold(0.0, f64::max);
    let width = TIMELINE_W - 2.0 * TIMELINE_PAD;
    let height = TIMELINE_H - 2.0 * TIMELINE_PAD;
    points
        .iter()
        .map(|point| {
            let x = if span > 0.0 {
                TIMELINE_PAD + (point.timestamp - first.timestamp) as f64 / span * width
            } else {
                TIMELINE_W / 2.0
            };
            let y = if best > 0.0 {
                TIMELINE_PAD + (1.0 - point.score / best) * height
            } else {
                TIMELINE_H - TIMELINE_PAD
            };
            (x, y)
        })
        .collect()
}

/// The record as it stood over time: a step up at every session that set
/// one, flat in between, on to the last session.
fn record_path(points: &[TimelinePoint], xy: &[(f64, f64)]) -> String {
    let best = points.iter().map(|point| point.score).fold(0.0, f64::max);
    if best <= 0.0 {
        return String::new();
    }
    let height = TIMELINE_H - 2.0 * TIMELINE_PAD;
    let y_of = |score: f64| TIMELINE_PAD + (1.0 - score / best) * height;
    let mut path = String::new();
    let mut level: Option<f64> = None;
    for (point, (x, _)) in points.iter().zip(xy) {
        let y = y_of(point.record);
        match level {
            None => path.push_str(&format!("M{x:.1},{y:.1}")),
            Some(before) if (before - y).abs() > f64::EPSILON => {
                path.push_str(&format!(" L{x:.1},{before:.1} L{x:.1},{y:.1}"));
            }
            Some(_) => {}
        }
        level = Some(y);
    }
    if let (Some(y), Some((x, _))) = (level, xy.last()) {
        path.push_str(&format!(" L{x:.1},{y:.1}"));
    }
    path
}

/// Every session of `mode`, best first, with the personal best, the score
/// over time and the board itself. Pointing at a row (or tapping it) marks
/// on the timeline when it was set.
#[component]
pub fn ScoresTab(sessions: Vec<SessionResult>) -> Element {
    let mut mode = use_signal(|| None::<CharSetMode>);
    let mut hovered = use_signal(|| None::<u64>);
    let mut pinned = use_signal(|| None::<u64>);
    let mut show_all = use_signal(|| false);
    let modes = board_modes(&sessions);
    // A mode whose sessions have all been deleted shows every mode again.
    let current = mode().filter(|chosen| modes.contains(chosen));
    let board = score_board(&sessions, current);
    let timeline = score_timeline(&sessions, current);
    let today = crate::time::local_date_string();

    let Some(best) = board.first().cloned() else {
        return rsx! {
            div { class: "card",
                CardHead { icon: IconName::Medal, title: "No scores yet" }
                p { class: "muted", style: "margin: 0;",
                    "Finish a session and its score goes on the board. The best one gets the gold."
                }
            }
        };
    };
    let focus = hovered().or(pinned()).unwrap_or(best.timestamp);
    let focused = board.iter().find(|entry| entry.timestamp == focus).cloned();
    let xy = timeline_xy(&timeline);
    let record = record_path(&timeline, &xy);
    let marker = timeline
        .iter()
        .zip(&xy)
        .find(|(point, _)| point.timestamp == focus)
        .map(|(_, at)| *at);
    let set_ago = ago(&best.date, &today);
    let first_date = timeline
        .first()
        .and_then(|point| sessions.get(point.index))
        .map(|session| short_date(&session.date))
        .unwrap_or_default();
    let last_date = timeline
        .last()
        .and_then(|point| sessions.get(point.index))
        .map(|session| short_date(&session.date))
        .unwrap_or_default();
    let records = timeline.iter().filter(|point| point.set_record).count();
    let total = board.len();
    let mut rows = board;
    if !show_all() {
        rows.truncate(BOARD_TOP);
    }
    let noun = if total == 1 { "session" } else { "sessions" };

    rsx! {
        div { class: "stack",
            if modes.len() > 1 {
                div { class: "board-modes",
                    Seg { label: "All modes", pill: true, active: current.is_none(), onclick: move |_| mode.set(None) }
                    for each in modes.iter().copied() {
                        Seg {
                            label: mode_label(each),
                            pill: true,
                            active: current == Some(each),
                            onclick: move |_| mode.set(Some(each)),
                        }
                    }
                }
            }
            div { class: "card best-card",
                Medal { place: 1, large: true }
                div { class: "best-copy",
                    div { class: "tiny", "Personal best" }
                    div { class: "best-score", "{best.score.round()}" }
                    p { class: "best-when",
                        "{when_set(&best)}"
                        if let Some(set_ago) = set_ago {
                            span { class: "muted", " · {set_ago}" }
                        }
                    }
                    div { class: "best-chips",
                        span { class: "chip", "{speed(&best)}" }
                        span { class: "chip good", "{(best.accuracy * 100.0).round()}% accuracy" }
                        span { class: "chip", "{best.groups} groups" }
                        if current.is_none() {
                            span { class: "chip", "{mode_label(best.mode)}" }
                        }
                    }
                }
            }
            div { class: "card chart-card",
                CardHead {
                    icon: IconName::Chart,
                    title: "Score over time",
                    note: format!(
                        "{total} {noun} · the record set {}",
                        if records == 1 { "once".to_string() } else { format!("{records} times") }
                    ),
                }
                svg {
                    class: "sparkline score-timeline",
                    view_box: "0 0 420 120",
                    if !record.is_empty() {
                        path { class: "record-line", d: "{record}" }
                    }
                    if let Some((x, _)) = marker {
                        line {
                            class: "timeline-guide",
                            x1: "{x:.1}",
                            y1: "0",
                            x2: "{x:.1}",
                            y2: "120",
                        }
                    }
                    for (point, (x, y)) in timeline.iter().zip(xy.iter()) {
                        circle {
                            class: if point.set_record { "timeline-dot record" } else { "timeline-dot" },
                            cx: "{x:.1}",
                            cy: "{y:.1}",
                            r: if point.set_record { "3.4" } else { "2.6" },
                        }
                    }
                    if let Some((x, y)) = marker {
                        circle { class: "timeline-mark", cx: "{x:.1}", cy: "{y:.1}", r: "6.5" }
                    }
                }
                if let Some(entry) = focused.as_ref() {
                    p { class: "timeline-focus",
                        span { class: "timeline-focus-place", "{ordinal(entry.place)}" }
                        " · {entry.score.round()} points · {short_date(&entry.date)}"
                        if let Some(time) = clock(entry.timestamp) {
                            " {time}"
                        }
                    }
                }
                div { class: "row-between",
                    span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                        "{first_date}"
                    }
                    span { class: "tiny", style: "text-transform: none; letter-spacing: 0.02em;",
                        "{last_date}"
                    }
                }
            }
            div { class: "card stack-sm",
                CardHead {
                    icon: IconName::Medal,
                    title: "Best sessions",
                    note: "Kept on this device. Deleting a session takes it off the board.",
                }
                div { class: "board-list",
                    for entry in rows {
                        {
                            let at = entry.timestamp;
                            let is_focus = at == focus;
                            let is_pinned = pinned() == Some(at);
                            let class = match (is_focus, is_pinned) {
                                (_, true) => "board-row focused pinned",
                                (true, false) => "board-row focused",
                                (false, false) => "board-row",
                            };
                            let detail = if current.is_none() {
                                format!("{} · {}% · {}", speed(&entry), (entry.accuracy * 100.0).round(), mode_label(entry.mode))
                            } else {
                                format!("{} · {}%", speed(&entry), (entry.accuracy * 100.0).round())
                            };
                            let when = match clock(at) {
                                Some(time) => format!("{} · {time}", short_date(&entry.date)),
                                None => short_date(&entry.date),
                            };
                            rsx! {
                                button {
                                    id: control_id("board", &entry.place.to_string()),
                                    class: class,
                                    aria_pressed: if is_pinned { "true" } else { "false" },
                                    onpointerenter: move |evt: PointerEvent| {
                                        if evt.pointer_type() == "mouse" {
                                            hovered.set(Some(at));
                                        }
                                    },
                                    onpointerleave: move |evt: PointerEvent| {
                                        if evt.pointer_type() == "mouse" && *hovered.peek() == Some(at) {
                                            hovered.set(None);
                                        }
                                    },
                                    onclick: move |_| {
                                        let again = *pinned.peek() == Some(at);
                                        pinned.set(if again { None } else { Some(at) });
                                    },
                                    span { class: "board-place",
                                        if entry.place <= 3 {
                                            Medal { place: entry.place }
                                        } else {
                                            span { class: "board-number", "{entry.place}" }
                                        }
                                    }
                                    span { class: "board-main",
                                        span { class: "board-score", "{entry.score.round()}" }
                                        span { class: "board-when", "{when}" }
                                    }
                                    span { class: "board-detail", "{detail}" }
                                }
                            }
                        }
                    }
                }
                if total > BOARD_TOP {
                    button {
                        id: control_id("board", "all"),
                        class: if show_all() { "link-toggle board-more open" } else { "link-toggle board-more" },
                        onclick: move |_| show_all.toggle(),
                        if show_all() {
                            "Show the top {BOARD_TOP}"
                        } else {
                            "Show all {total}"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_said_as_ordinals() {
        let said: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111]
            .into_iter()
            .map(ordinal)
            .collect();
        assert_eq!(
            said,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd",
                "101st", "111th"
            ]
        );
    }

    #[test]
    fn a_date_reads_in_words() {
        assert_eq!(short_date("2026-10-16"), "Fri 16 Oct 2026");
        assert_eq!(long_date("2026-10-16"), "Friday 16 October 2026");
        assert_eq!(short_date("not a date"), "not a date");
    }

    #[test]
    fn how_long_ago_is_said_in_days() {
        assert_eq!(ago("2026-10-16", "2026-10-16").as_deref(), Some("today"));
        assert_eq!(
            ago("2026-10-15", "2026-10-16").as_deref(),
            Some("yesterday")
        );
        assert_eq!(
            ago("2026-10-04", "2026-10-16").as_deref(),
            Some("12 days ago")
        );
        assert_eq!(ago("2026-10-17", "2026-10-16"), None, "still to come");
        assert_eq!(ago("garbled", "2026-10-16"), None);
    }

    #[test]
    fn every_mode_has_a_name() {
        for (mode, label) in CHAR_SET_MODES {
            assert_eq!(mode_label(mode), label);
            assert!(!mode_noun(mode).is_empty());
        }
    }

    fn point(timestamp: u64, score: f64, record: f64, set_record: bool) -> TimelinePoint {
        TimelinePoint {
            index: 0,
            timestamp,
            score,
            record,
            set_record,
        }
    }

    /// Across by time, up by score: the best at the top, the first and last
    /// at the edges.
    #[test]
    fn the_timeline_spreads_sessions_by_when_they_were_set() {
        let points = [
            point(0, 100.0, 100.0, true),
            point(50, 50.0, 100.0, false),
            point(100, 200.0, 200.0, true),
        ];
        let xy = timeline_xy(&points);
        assert_eq!(xy[0].0, TIMELINE_PAD);
        assert_eq!(xy[1].0, TIMELINE_W / 2.0);
        assert_eq!(xy[2].0, TIMELINE_W - TIMELINE_PAD);
        assert_eq!(xy[2].1, TIMELINE_PAD, "the best is at the top");
        assert!(xy[1].1 > xy[0].1, "a lower score sits lower");
        // One session sits in the middle.
        let one = timeline_xy(&[point(5, 10.0, 10.0, true)]);
        assert_eq!(one[0].0, TIMELINE_W / 2.0);
        assert!(timeline_xy(&[]).is_empty());
    }

    /// The record steps up where it was set and stays flat in between.
    #[test]
    fn the_record_line_steps_up_at_each_new_record() {
        let points = [
            point(0, 100.0, 100.0, true),
            point(50, 50.0, 100.0, false),
            point(100, 200.0, 200.0, true),
        ];
        let xy = timeline_xy(&points);
        let path = record_path(&points, &xy);
        assert!(path.starts_with("M8.0,"), "{path}");
        // Flat to the third session, then up to the new record.
        assert_eq!(path.matches(" L").count(), 3, "{path}");
        assert!(
            path.ends_with(&format!("L{:.1},{:.1}", xy[2].0, xy[2].1)),
            "{path}"
        );
        assert!(record_path(&[], &[]).is_empty());
    }

    #[test]
    fn a_farnsworth_speed_says_both() {
        let mut entry = BoardEntry {
            place: 1,
            index: 0,
            score: 500.0,
            timestamp: 0,
            date: "2026-10-16".to_string(),
            char_wpm: 25.0,
            effective_wpm: 25.0,
            accuracy: 0.95,
            groups: 20,
            mode: CharSetMode::Callsign,
        };
        assert_eq!(speed(&entry), "25 WPM");
        entry.effective_wpm = 15.0;
        assert_eq!(speed(&entry), "25/15 WPM");
    }
}

#[cfg(test)]
mod board_tests {
    use super::*;
    use crate::testing::{Ui, stored_session};

    /// A session on the `day`th of October 2026, scored `score`.
    fn run_of(day: u32, score: f64, mode: CharSetMode) -> SessionResult {
        let mut session = stored_session(&format!("2026-10-{day:02}"), &[("KM", "KM")]);
        session.timestamp = 1_790_000_000_000 + u64::from(day) * 86_400_000;
        session.score = score;
        session.char_set_mode = mode;
        session
    }

    fn koch(day: u32, score: f64) -> SessionResult {
        run_of(day, score, CharSetMode::Koch)
    }

    fn board(sessions: Vec<SessionResult>) -> Ui {
        Ui::new(ScoresTab, ScoresTabProps { sessions })
    }

    #[test]
    fn the_best_session_gets_the_gold_and_the_rest_follow() {
        let ui = board(vec![
            koch(1, 300.0),
            koch(2, 900.0),
            koch(3, 500.0),
            koch(4, 100.0),
        ]);
        assert!(ui.has("Personal best"));
        assert!(ui.has(">900<"));
        assert!(ui.has("Friday 2 October 2026"));
        assert_eq!(ui.controls("board-").len(), 4);
        for place in ["1st", "2nd", "3rd"] {
            assert!(ui.has(&format!("aria-label=\"{place} place\"")), "{place}");
        }
        assert!(!ui.has("aria-label=\"4th place\""), "medals stop at third");
        assert!(ui.has("class=\"board-number\">4<"));
        // The record was set on the 1st and the 2nd.
        assert!(ui.has("the record set 2 times"));
        assert!(ui.has("record-line"));
        // The timeline marks the personal best until a row is chosen.
        assert!(ui.has("1st</span> · 900 points"));
    }

    /// Pointing at a row marks when it was set; a tap keeps it marked.
    #[test]
    fn a_row_marks_its_session_on_the_timeline() {
        let mut ui = board(vec![
            koch(1, 300.0),
            koch(2, 900.0),
            koch(3, 500.0),
            koch(4, 100.0),
        ]);
        ui.point_at("board-3", "mouse", (0.0, 0.0), (0.0, 0.0));
        assert!(ui.has("3rd</span> · 300 points"));
        ui.point_away("board-3");
        assert!(ui.has("1st</span> · 900 points"));

        ui.click("board-2");
        assert!(ui.input_tag("board-2").contains("pinned"));
        assert!(ui.has("2nd</span> · 500 points"));
        ui.point_at("board-4", "mouse", (0.0, 0.0), (0.0, 0.0));
        assert!(ui.has("4th</span> · 100 points"));
        ui.point_away("board-4");
        assert!(
            ui.has("2nd</span> · 500 points"),
            "the tapped row stays marked"
        );
        ui.click("board-2");
        assert!(ui.has("1st</span> · 900 points"));
        // A finger does not point.
        ui.point_at("board-4", "touch", (0.0, 0.0), (0.0, 0.0));
        assert!(ui.has("1st</span> · 900 points"));
    }

    #[test]
    fn a_board_can_be_narrowed_to_one_mode() {
        let mut ui = board(vec![
            koch(1, 300.0),
            run_of(2, 900.0, CharSetMode::Callsign),
            koch(3, 500.0),
        ]);
        assert!(ui.shows("pill-all-modes"));
        assert!(ui.shows("pill-koch"));
        assert!(ui.shows("pill-callsigns"));
        assert!(!ui.shows("pill-words"), "only modes with scores");
        assert_eq!(ui.controls("board-").len(), 3);
        assert!(
            ui.has("class=\"chip\">Callsigns<"),
            "the best names its mode"
        );

        ui.click("pill-koch");
        assert_eq!(ui.controls("board-").len(), 2);
        assert!(ui.has("1st</span> · 500 points"));
        ui.click("pill-all-modes");
        assert_eq!(ui.controls("board-").len(), 3);
    }

    #[test]
    fn one_mode_needs_no_choice_of_mode() {
        let ui = board(vec![koch(1, 300.0), koch(2, 500.0)]);
        assert!(!ui.shows("pill-all-modes"));
    }

    #[test]
    fn a_long_board_shows_the_top_ten_until_asked() {
        let mut ui = board(
            (1..=12)
                .map(|day| koch(day, f64::from(day) * 10.0))
                .collect(),
        );
        assert_eq!(
            ui.controls("board-").len(),
            10 + 1,
            "ten rows and the toggle"
        );
        assert!(ui.has("Show all 12"));
        ui.click("board-all");
        assert_eq!(ui.controls("board-").len(), 12 + 1);
        assert!(ui.has("Show the top 10"));
    }

    #[test]
    fn no_score_no_board() {
        let ui = board(vec![koch(1, 0.0)]);
        assert!(ui.has("No scores yet"));
        assert!(ui.controls("board-").is_empty());
        assert!(board(Vec::new()).has("No scores yet"));
    }
}
