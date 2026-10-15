//! Practice calendar from session history: one tile per month, one square per
//! day.

use std::collections::BTreeMap;

use crate::session::SessionResult;
use crate::streak::{
    date_to_day_index, day_index_to_date, days_from_civil, days_in_month, parse_ymd,
    weekday_monday0,
};

/// Whole months on the calendar, the one today is in among them.
pub const HEATMAP_MONTHS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeatmapColorMode {
    Volume,
    Accuracy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapDay {
    pub date: String,
    pub chars: u32,
    pub sessions: u32,
    pub group_count: u32,
    pub accuracy_sum: f64,
    /// Sessions whose accuracy could be read, which is what the average is
    /// over. A session with a broken accuracy still practised that day, but
    /// it says nothing about how well.
    pub accuracy_sessions: u32,
}

impl HeatmapDay {
    pub fn avg_accuracy(&self) -> Option<f64> {
        if self.accuracy_sessions == 0 {
            None
        } else {
            Some(self.accuracy_sum / f64::from(self.accuracy_sessions))
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapCell {
    pub date: String,
    pub in_future: bool,
    pub chars: u32,
    pub sessions: u32,
    pub group_count: u32,
    pub avg_accuracy: Option<f64>,
    /// How strongly to shade the day, from 0 to 1, by what the grid is
    /// showing: the share of the busiest day's characters, or the day's
    /// accuracy. None where there is nothing to shade — a day with no
    /// practice, one still to come, or one whose accuracy could not be read.
    pub intensity: Option<f64>,
}

/// One month's tile: its days, and where the first of them falls in a week.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapMonth {
    pub year: i32,
    /// 1 for January.
    pub month: u32,
    /// The weekday of the 1st, Monday as 0: how many days of the month's
    /// first week belong to the month before.
    pub lead: u32,
    /// Every day of the month, the 1st first.
    pub cells: Vec<HeatmapCell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapGrid {
    pub max_chars: u32,
    /// Oldest first, ending with the month today is in.
    pub months: Vec<HeatmapMonth>,
}

impl HeatmapGrid {
    /// Every day on the calendar, in order.
    pub fn cells(&self) -> impl Iterator<Item = &HeatmapCell> {
        self.months.iter().flat_map(|month| &month.cells)
    }

    pub fn cell(&self, date: &str) -> Option<&HeatmapCell> {
        self.cells().find(|cell| cell.date == date)
    }
}

pub fn aggregate_heatmap(sessions: &[SessionResult]) -> BTreeMap<String, HeatmapDay> {
    let mut days = BTreeMap::new();
    for session in sessions {
        if date_to_day_index(&session.date).is_none() {
            continue;
        }
        let entry = days
            .entry(session.date.clone())
            .or_insert_with(|| HeatmapDay {
                date: session.date.clone(),
                chars: 0,
                sessions: 0,
                group_count: 0,
                accuracy_sum: 0.0,
                accuracy_sessions: 0,
            });
        entry.chars = entry.chars.saturating_add(session.total_chars);
        entry.sessions = entry.sessions.saturating_add(1);
        entry.group_count = entry
            .group_count
            .saturating_add(u32::try_from(session.groups.len()).unwrap_or(u32::MAX));
        if session.accuracy.is_finite() && (0.0..=1.0).contains(&session.accuracy) {
            entry.accuracy_sum += session.accuracy;
            entry.accuracy_sessions += 1;
        }
    }
    days
}

/// The calendar for the `months` whole months up to and including today's.
pub fn build_heatmap(
    sessions: &[SessionResult],
    today: &str,
    months: u32,
    mode: HeatmapColorMode,
) -> Option<HeatmapGrid> {
    let today_idx = date_to_day_index(today)?;
    let (mut year, mut month, _) = parse_ymd(today)?;
    let mut spans = Vec::new();
    for _ in 0..months.max(1) {
        spans.push((year, month));
        (year, month) = if month == 1 {
            (year - 1, 12)
        } else {
            (year, month - 1)
        };
    }
    spans.reverse();

    let by_date = aggregate_heatmap(sessions);
    let days = |(year, month): (i32, u32)| {
        let first = days_from_civil(year, month, 1);
        first..first + i64::from(days_in_month(year, month))
    };
    let max_chars = spans
        .iter()
        .flat_map(|&span| days(span))
        .filter_map(|idx| by_date.get(&day_index_to_date(idx)))
        .map(|day| day.chars)
        .fold(1, u32::max);

    let cell = |idx: i64| {
        let date = day_index_to_date(idx);
        let in_future = idx > today_idx;
        let day = by_date.get(&date);
        let chars = day.map(|d| d.chars).unwrap_or(0);
        let sessions_n = day.map(|d| d.sessions).unwrap_or(0);
        let group_count = day.map(|d| d.group_count).unwrap_or(0);
        let avg_accuracy = day.and_then(HeatmapDay::avg_accuracy);
        let intensity = if in_future || sessions_n == 0 {
            None
        } else {
            match mode {
                HeatmapColorMode::Volume => {
                    (chars > 0).then(|| (f64::from(chars) / f64::from(max_chars.max(1))).min(1.0))
                }
                HeatmapColorMode::Accuracy => avg_accuracy.map(|a| a.clamp(0.0, 1.0)),
            }
        };
        HeatmapCell {
            date,
            in_future,
            chars,
            sessions: sessions_n,
            group_count,
            avg_accuracy,
            intensity,
        }
    };

    let months = spans
        .into_iter()
        .map(|(year, month)| {
            let days = days((year, month));
            HeatmapMonth {
                year,
                month,
                lead: weekday_monday0(days.start),
                cells: days.map(cell).collect(),
            }
        })
        .collect();

    Some(HeatmapGrid { max_chars, months })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{GroupResult, SessionResult};

    fn session(date: &str, chars: u32, accuracy: f64) -> SessionResult {
        SessionResult {
            date: date.into(),
            groups: vec![GroupResult {
                sent: "A".repeat(chars as usize),
                received: "A".repeat(chars as usize),
                correct: true,
            }],
            accuracy,
            total_chars: chars,
            ..SessionResult::empty_for_test()
        }
    }

    #[test]
    fn aggregates_same_day() {
        let sessions = [
            session("2026-07-17", 10, 0.8),
            session("2026-07-17", 6, 1.0),
        ];
        let days = aggregate_heatmap(&sessions);
        let day = days.get("2026-07-17").expect("day");
        assert_eq!(day.chars, 16);
        assert_eq!(day.sessions, 2);
        assert!((day.avg_accuracy().unwrap_or(0.0) - 0.9).abs() < 1e-9);
    }

    #[test]
    fn a_session_with_an_unreadable_date_is_skipped() {
        let days = aggregate_heatmap(&[session("not-a-date", 10, 1.0)]);
        assert!(days.is_empty());
    }

    #[test]
    fn an_out_of_range_accuracy_is_not_summed() {
        let days = aggregate_heatmap(&[session("2026-07-17", 4, f64::NAN)]);
        let day = days.get("2026-07-17").expect("day");
        assert_eq!(day.sessions, 1);
        assert_eq!(day.accuracy_sum, 0.0);
        assert_eq!(day.avg_accuracy(), None);
    }

    /// A broken session counts as practice that day, but not as a zero in
    /// the day's accuracy — which would drag a good day's colour towards red.
    #[test]
    fn an_unreadable_accuracy_does_not_drag_the_average_down() {
        let days = aggregate_heatmap(&[
            session("2026-07-17", 4, 0.9),
            session("2026-07-17", 4, f64::NAN),
        ]);
        let day = days.get("2026-07-17").expect("day");
        assert_eq!(day.sessions, 2);
        assert_eq!(day.avg_accuracy(), Some(0.9));
    }

    #[test]
    fn a_day_without_sessions_has_no_average() {
        let day = HeatmapDay {
            date: "2026-07-17".into(),
            chars: 0,
            sessions: 0,
            group_count: 0,
            accuracy_sum: 0.0,
            accuracy_sessions: 0,
        };
        assert_eq!(day.avg_accuracy(), None);
    }

    #[test]
    fn volume_shades_each_day_against_the_busiest() {
        let grid = build_heatmap(
            &[
                session("2026-07-16", 5, 1.0),
                session("2026-07-17", 10, 1.0),
            ],
            "2026-07-17",
            1,
            HeatmapColorMode::Volume,
        )
        .expect("grid");
        let day = |date: &str| grid.cell(date).unwrap().intensity;
        assert_eq!(day("2026-07-17"), Some(1.0));
        assert_eq!(day("2026-07-16"), Some(0.5));
        assert_eq!(day("2026-07-15"), None, "no practice, nothing to shade");
    }

    /// The busiest day is the busiest on the calendar, whichever month it is
    /// in, so one shade means one amount of copy on every tile.
    #[test]
    fn every_month_is_shaded_against_the_same_busiest_day() {
        let grid = build_heatmap(
            &[
                session("2026-06-30", 20, 1.0),
                session("2026-07-17", 10, 1.0),
            ],
            "2026-07-17",
            2,
            HeatmapColorMode::Volume,
        )
        .expect("grid");
        assert_eq!(grid.max_chars, 20);
        assert_eq!(grid.cell("2026-07-17").unwrap().intensity, Some(0.5));
    }

    #[test]
    fn a_day_whose_accuracy_cannot_be_read_is_not_shaded() {
        let grid = build_heatmap(
            &[session("2026-07-17", 4, f64::NAN)],
            "2026-07-17",
            1,
            HeatmapColorMode::Accuracy,
        )
        .expect("grid");
        let friday = grid.cell("2026-07-17").unwrap();
        assert_eq!(friday.sessions, 1);
        assert_eq!(friday.intensity, None);
    }

    #[test]
    fn an_unreadable_today_draws_no_grid() {
        assert!(build_heatmap(&[], "nonsense", 4, HeatmapColorMode::Volume).is_none());
    }

    #[test]
    fn the_grid_paints_practice_and_leaves_the_future_empty() {
        // The rest of July 2026 after Friday the 17th is still ahead.
        let grid = build_heatmap(
            &[session("2026-07-17", 10, 0.5)],
            "2026-07-17",
            1,
            HeatmapColorMode::Accuracy,
        )
        .expect("grid");
        assert_eq!(grid.months.len(), 1);
        assert_eq!(grid.max_chars, 10);
        let friday = grid.cell("2026-07-17").unwrap();
        assert_eq!(friday.sessions, 1);
        assert_eq!(friday.group_count, 1);
        assert_eq!(friday.avg_accuracy, Some(0.5));
        assert_eq!(friday.intensity, Some(0.5));
        assert!(!friday.in_future);
        let last = grid.cells().last().expect("the 31st");
        assert_eq!(last.date, "2026-07-31");
        assert!(last.in_future);
        assert_eq!(last.intensity, None);
        // Zero months still draws one.
        assert_eq!(
            build_heatmap(&[], "2026-07-17", 0, HeatmapColorMode::Volume)
                .expect("grid")
                .cells()
                .count(),
            31
        );
    }

    #[test]
    fn a_month_runs_from_its_first_to_its_last_day() {
        // 2026-07-01 is a Wednesday, two days into its week.
        let grid = build_heatmap(&[], "2026-07-17", 1, HeatmapColorMode::Volume).expect("grid");
        let july = grid.months.first().expect("july");
        assert_eq!((july.year, july.month, july.lead), (2026, 7, 2));
        assert_eq!(july.cells.len(), 31);
        assert_eq!(
            july.cells.first().map(|c| c.date.as_str()),
            Some("2026-07-01")
        );
        assert_eq!(
            july.cells.last().map(|c| c.date.as_str()),
            Some("2026-07-31")
        );
    }

    #[test]
    fn months_run_oldest_first_back_across_new_year() {
        let grid = build_heatmap(&[], "2026-01-15", 3, HeatmapColorMode::Volume).expect("grid");
        let spans: Vec<(i32, u32, usize)> = grid
            .months
            .iter()
            .map(|m| (m.year, m.month, m.cells.len()))
            .collect();
        assert_eq!(spans, [(2025, 11, 30), (2025, 12, 31), (2026, 1, 31)]);
        // The months follow on from each other with no day left out.
        let dates: Vec<&str> = grid.cells().map(|c| c.date.as_str()).collect();
        assert_eq!(dates.first(), Some(&"2025-11-01"));
        assert_eq!(dates.last(), Some(&"2026-01-31"));
        assert_eq!(dates.len(), 30 + 31 + 31);
    }

    #[test]
    fn a_leap_february_has_its_29th() {
        let grid = build_heatmap(&[], "2028-02-10", 1, HeatmapColorMode::Volume).expect("grid");
        assert_eq!(grid.cells().count(), 29);
        assert!(grid.cell("2028-02-29").is_some());
    }
}
