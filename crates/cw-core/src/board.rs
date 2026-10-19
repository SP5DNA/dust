//! The score board: the sessions kept on this device, best score first, and
//! how the score and the record have moved over time.
//!
//! There is one player, so a place on the board is a session's, not a
//! person's. It is read off the stored history, so a deleted session leaves
//! the board with it. Two equal scores are placed in the order they were set:
//! the first to reach a score holds the higher place, as on any record list.

use crate::session::SessionResult;
use crate::settings::CharSetMode;

/// One session's place on the board, and what it was.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardEntry {
    /// 1 for the best.
    pub place: usize,
    /// Where the session sits in the history it was read from.
    pub index: usize,
    pub score: f64,
    pub timestamp: u64,
    pub date: String,
    pub char_wpm: f64,
    pub effective_wpm: f64,
    pub accuracy: f64,
    pub groups: usize,
    pub mode: CharSetMode,
}

/// A session's score that can go on the board: one that could be read, and
/// earned something.
fn on_board(session: &SessionResult) -> bool {
    session.score.is_finite() && session.score > 0.0
}

fn of_mode(session: &SessionResult, mode: Option<CharSetMode>) -> bool {
    mode.is_none_or(|mode| session.char_set_mode == mode)
}

/// Every scored session of `mode` (or of every mode), best first.
pub fn score_board(sessions: &[SessionResult], mode: Option<CharSetMode>) -> Vec<BoardEntry> {
    let mut scored: Vec<(usize, &SessionResult)> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| on_board(session) && of_mode(session, mode))
        .collect();
    scored.sort_by(|(a_index, a), (b_index, b)| {
        b.score
            .total_cmp(&a.score)
            .then(a.timestamp.cmp(&b.timestamp))
            .then(a_index.cmp(b_index))
    });
    scored
        .into_iter()
        .enumerate()
        .map(|(rank, (index, session))| BoardEntry {
            place: rank + 1,
            index,
            score: session.score,
            timestamp: session.timestamp,
            date: session.date.clone(),
            char_wpm: session.char_wpm,
            effective_wpm: session.effective_wpm,
            accuracy: session.accuracy,
            groups: session.groups.len(),
            mode: session.char_set_mode,
        })
        .collect()
}

/// Where the session finished at `timestamp` stands among every scored
/// session of its own mode: its place, and how many there are.
pub fn board_place(sessions: &[SessionResult], timestamp: u64) -> Option<(usize, usize)> {
    let mode = sessions
        .iter()
        .find(|session| session.timestamp == timestamp)?
        .char_set_mode;
    let board = score_board(sessions, Some(mode));
    let place = board
        .iter()
        .find(|entry| entry.timestamp == timestamp)?
        .place;
    Some((place, board.len()))
}

/// The modes the board has scores for, in the order the trainer offers them.
pub fn board_modes(sessions: &[SessionResult]) -> Vec<CharSetMode> {
    [
        CharSetMode::Koch,
        CharSetMode::Digits,
        CharSetMode::Mixed,
        CharSetMode::Callsign,
        CharSetMode::Words,
    ]
    .into_iter()
    .filter(|mode| {
        sessions
            .iter()
            .any(|session| on_board(session) && session.char_set_mode == *mode)
    })
    .collect()
}

/// One session on the score's timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelinePoint {
    pub index: usize,
    pub timestamp: u64,
    pub score: f64,
    /// The best score up to and including this session.
    pub record: f64,
    /// Whether this session set that record: beat every score before it.
    pub set_record: bool,
}

/// Every scored session of `mode` (or of every mode), oldest first, with the
/// record as it stood after each.
pub fn score_timeline(sessions: &[SessionResult], mode: Option<CharSetMode>) -> Vec<TimelinePoint> {
    let mut scored: Vec<(usize, &SessionResult)> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| on_board(session) && of_mode(session, mode))
        .collect();
    scored.sort_by(|(a_index, a), (b_index, b)| {
        a.timestamp.cmp(&b.timestamp).then(a_index.cmp(b_index))
    });
    let mut record = 0.0_f64;
    scored
        .into_iter()
        .map(|(index, session)| {
            let set_record = session.score > record;
            record = record.max(session.score);
            TimelinePoint {
                index,
                timestamp: session.timestamp,
                score: session.score,
                record,
                set_record,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(timestamp: u64, score: f64, mode: CharSetMode) -> SessionResult {
        SessionResult {
            timestamp,
            score,
            char_set_mode: mode,
            date: format!("2026-10-{:02}", 1 + timestamp % 28),
            ..SessionResult::empty_for_test()
        }
    }

    fn koch(timestamp: u64, score: f64) -> SessionResult {
        session(timestamp, score, CharSetMode::Koch)
    }

    #[test]
    fn the_best_score_comes_first() {
        let sessions = [koch(1, 300.0), koch(2, 900.0), koch(3, 500.0)];
        let board = score_board(&sessions, None);
        let scores: Vec<f64> = board.iter().map(|entry| entry.score).collect();
        assert_eq!(scores, [900.0, 500.0, 300.0]);
        let places: Vec<usize> = board.iter().map(|entry| entry.place).collect();
        assert_eq!(places, [1, 2, 3]);
        assert_eq!(board[0].index, 1, "the entry points back at its session");
    }

    /// The first to reach a score holds the higher place.
    #[test]
    fn an_equal_score_ranks_below_the_one_set_first() {
        let sessions = [koch(20, 500.0), koch(10, 500.0)];
        let board = score_board(&sessions, None);
        assert_eq!(board[0].timestamp, 10);
        assert_eq!(board[1].timestamp, 20);
    }

    /// A broken or empty score is not a place on the board.
    #[test]
    fn only_a_score_that_earned_something_is_placed() {
        let sessions = [
            koch(1, 0.0),
            koch(2, f64::NAN),
            koch(3, f64::INFINITY),
            koch(4, -5.0),
            koch(5, 40.0),
        ];
        let board = score_board(&sessions, None);
        assert_eq!(board.len(), 1);
        assert_eq!(board[0].timestamp, 5);
        assert!(score_board(&[], None).is_empty());
    }

    #[test]
    fn a_board_can_hold_one_mode() {
        let sessions = [
            koch(1, 300.0),
            session(2, 900.0, CharSetMode::Callsign),
            koch(3, 500.0),
        ];
        let calls = score_board(&sessions, Some(CharSetMode::Callsign));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].place, 1);
        let koch_board = score_board(&sessions, Some(CharSetMode::Koch));
        assert_eq!(koch_board.len(), 2);
        assert_eq!(koch_board[0].score, 500.0);
        assert_eq!(
            board_modes(&sessions),
            [CharSetMode::Koch, CharSetMode::Callsign]
        );
    }

    /// A session is placed among its own mode's: a callsign run against
    /// callsign runs.
    #[test]
    fn a_session_knows_its_place() {
        let sessions = [
            koch(1, 300.0),
            session(2, 900.0, CharSetMode::Callsign),
            koch(3, 500.0),
            koch(4, 400.0),
        ];
        assert_eq!(board_place(&sessions, 3), Some((1, 3)));
        assert_eq!(board_place(&sessions, 4), Some((2, 3)));
        assert_eq!(board_place(&sessions, 2), Some((1, 1)));
        assert_eq!(board_place(&sessions, 99), None, "no such session");
        let unscored = [koch(1, 0.0)];
        assert_eq!(board_place(&unscored, 1), None);
    }

    #[test]
    fn the_timeline_runs_oldest_first_and_marks_each_new_record() {
        let sessions = [
            koch(30, 800.0),
            koch(10, 300.0),
            koch(20, 200.0),
            koch(40, 900.0),
        ];
        let timeline = score_timeline(&sessions, None);
        let at: Vec<u64> = timeline.iter().map(|point| point.timestamp).collect();
        assert_eq!(at, [10, 20, 30, 40]);
        let records: Vec<f64> = timeline.iter().map(|point| point.record).collect();
        assert_eq!(records, [300.0, 300.0, 800.0, 900.0]);
        let set: Vec<bool> = timeline.iter().map(|point| point.set_record).collect();
        assert_eq!(set, [true, false, true, true]);
        // Equalling the record does not set it.
        let tie = score_timeline(&[koch(1, 300.0), koch(2, 300.0)], None);
        assert!(!tie[1].set_record);
    }
}
