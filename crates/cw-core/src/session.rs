//! One training session: its groups, what was answered, and the result it
//! scores to. [`crate::machine::SessionMachine`] is the only thing that moves
//! it on; everyone else reads it.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::alignment::{CopyTally, LetterAccuracy};
use crate::level::LEVEL_MIN;
use crate::score::{
    ScoreConstants, calculate_effective_alphabet_size, calculate_total_chars,
    compute_average_response_ms, compute_session_score,
};
use crate::settings::{CharSetMode, LevelAxis, PlaybackSettings, TrainingSettings};

/// The quickest a listener can be said to answer, per character.
///
/// An answer finished while the group was still being sent — with the input
/// left open during the send, real-time copying always is — has no response
/// time worth the name. Scored as the 1 ms it measured, the speed term ran
/// away with the score: ten to twenty times an answer given a second after
/// the send. A tenth of a second a character is about as fast as anyone
/// types one.
pub const MIN_RESPONSE_MS_PER_CHAR: f64 = 100.0;

/// The longest an answer is kept, against the group it answers: twice its
/// length, or this many characters for a short one. Room for every honest
/// mistake, and none for a paste of a novel to be stored, scored and aligned
/// on every visit to the stats screen.
const LONGEST_ANSWER: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl SessionId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A moment on a clock that only runs forward, in milliseconds from wherever
/// that clock starts.
///
/// Response times are measured between two of these, so they must come from
/// a monotonic clock: the wall clock steps when it is corrected, and a step
/// back between a send ending and its answer used to score the answer as
/// instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonoMs(pub u64);

impl MonoMs {
    /// Milliseconds since `earlier`; none if it is not earlier.
    pub fn since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    #[must_use]
    pub fn plus(self, ms: u64) -> Self {
        Self(self.0.saturating_add(ms))
    }
}

/// Where a session is. Every phase that is about a group names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    /// Counting down to the first group, `remaining` seconds to go. Nothing
    /// is sent and nothing can be typed yet.
    Countdown {
        remaining: u32,
    },
    Playing {
        index: usize,
    },
    /// Word-space pause between two sends of the same group.
    RepeatGap {
        index: usize,
    },
    InterGroupGap {
        next: usize,
    },
    AwaitingAnswer {
        index: usize,
    },
    Finished,
    Aborted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    sent: String,
    input: String,
    /// What was copied, once the group is answered. Answered is exactly
    /// when there is one.
    tally: Option<CopyTally>,
    start_at: MonoMs,
    /// When the last send of the group ended; none while it is still going.
    end_at: Option<MonoMs>,
    /// When the answer was complete: the moment its length first matched,
    /// or the confirm.
    answer_at: Option<MonoMs>,
    /// The character and effective speed it was sent at, once measured.
    played_wpm: Option<(f64, f64)>,
    /// How many times this group is sent before the answer window opens.
    repeats: u32,
    /// Sends already finished for this group.
    plays_done: u32,
}

impl Group {
    fn blank() -> Self {
        Self {
            sent: String::new(),
            input: String::new(),
            tally: None,
            start_at: MonoMs::default(),
            end_at: None,
            answer_at: None,
            played_wpm: None,
            repeats: 1,
            plays_done: 0,
        }
    }

    pub fn sent(&self) -> &str {
        &self.sent
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn confirmed(&self) -> bool {
        self.tally.is_some()
    }

    /// How long the answer took, from the end of the send — never quicker
    /// than a person types.
    fn timing(&self) -> SessionTiming {
        let chars = self.sent.chars().count().max(1) as f64;
        let measured = match (self.end_at, self.answer_at) {
            (Some(end), Some(answer)) => answer.since(end) as f64,
            // Answered before the send ended: as fast as it gets.
            _ => 0.0,
        };
        SessionTiming {
            time_to_complete_ms: measured.max(MIN_RESPONSE_MS_PER_CHAR * chars),
            char_wpm: self.played_wpm.map(|(char_wpm, _)| char_wpm),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionView {
    pub current: usize,
    pub focused: usize,
    pub sent: Vec<String>,
    pub inputs: Vec<String>,
    pub confirmed: Vec<bool>,
    /// The current group is being sent, or is between two of its sends.
    pub playing: bool,
    pub locked: bool,
    /// Seconds left before the first group is sent, while the session
    /// counts down to it.
    pub countdown: Option<u32>,
    /// Planned sends for the group being played, and how many are done.
    pub repeat_total: u32,
    pub repeat_done: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupSession {
    session_id: SessionId,
    phase: SessionPhase,
    groups: Vec<Group>,
    current_group: usize,
    focused_group: usize,
    settings: TrainingSettings,
}

impl GroupSession {
    pub(crate) fn new(
        session_id: SessionId,
        num_groups: usize,
        settings: TrainingSettings,
    ) -> Self {
        Self {
            session_id,
            phase: SessionPhase::Playing { index: 0 },
            groups: vec![Group::blank(); num_groups.max(1)],
            current_group: 0,
            focused_group: 0,
            settings,
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub fn current_group(&self) -> usize {
        self.current_group
    }

    /// The settings the session was started with. Every group is built from
    /// these, not from whatever the settings screen holds now.
    pub fn settings(&self) -> &TrainingSettings {
        &self.settings
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn group(&self, index: usize) -> Option<&Group> {
        self.groups.get(index)
    }

    pub fn any_confirmed(&self) -> bool {
        self.groups.iter().any(Group::confirmed)
    }

    /// Whether typing into `index` is refused: while its group is being sent,
    /// if the settings lock the box for that, and before anything has been
    /// sent at all.
    pub fn input_locked(&self, index: usize) -> bool {
        let sending = self.settings.playback.lock_input_during_group_playback && self.playing();
        (sending || self.countdown().is_some()) && index == self.current_group
    }

    /// Seconds left before the first group, while the session counts down.
    pub fn countdown(&self) -> Option<u32> {
        match self.phase {
            SessionPhase::Countdown { remaining } => Some(remaining),
            _ => None,
        }
    }

    fn playing(&self) -> bool {
        matches!(
            self.phase,
            SessionPhase::Playing { .. } | SessionPhase::RepeatGap { .. }
        )
    }

    pub fn view(&self) -> SessionView {
        let current = self.groups.get(self.current_group);
        SessionView {
            current: self.current_group,
            focused: self.focused_group,
            sent: self.groups.iter().map(|g| g.sent.clone()).collect(),
            inputs: self.groups.iter().map(|g| g.input.clone()).collect(),
            confirmed: self.groups.iter().map(Group::confirmed).collect(),
            playing: self.playing(),
            locked: self.input_locked(self.current_group),
            countdown: self.countdown(),
            repeat_total: current.map_or(1, |g| g.repeats.max(1)),
            repeat_done: current.map_or(0, |g| g.plays_done),
        }
    }

    pub(crate) fn set_phase(&mut self, phase: SessionPhase) {
        self.phase = phase;
    }

    pub(crate) fn group_repeats(&self, index: usize) -> u32 {
        self.groups.get(index).map_or(1, |g| g.repeats.max(1))
    }

    pub(crate) fn played_wpm(&self, index: usize) -> Option<(f64, f64)> {
        self.groups.get(index)?.played_wpm
    }

    pub(crate) fn group_start(&self, index: usize) -> Option<MonoMs> {
        self.groups.get(index).map(|g| g.start_at)
    }

    /// Fill in what `index` sends, and how many times.
    pub(crate) fn set_group(&mut self, index: usize, text: String, repeats: u32) {
        if let Some(group) = self.groups.get_mut(index)
            && !group.confirmed()
        {
            group.sent = text;
            group.input.clear();
            group.repeats = repeats.max(1);
        }
    }

    pub(crate) fn begin_group(&mut self, index: usize, now: MonoMs) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        group.start_at = now;
        group.plays_done = 0;
        self.current_group = index;
        self.focused_group = index;
    }

    /// Count one finished send of `index`.
    pub(crate) fn note_play_finished(&mut self, index: usize) {
        if let Some(group) = self.groups.get_mut(index) {
            group.plays_done = group.plays_done.saturating_add(1);
        }
    }

    pub(crate) fn end_playback(
        &mut self,
        index: usize,
        now: MonoMs,
        char_wpm: f64,
        effective_wpm: f64,
    ) {
        if let Some(group) = self.groups.get_mut(index) {
            group.end_at = Some(now);
            group.played_wpm =
                (char_wpm > 0.0 && effective_wpm > 0.0).then_some((char_wpm, effective_wpm));
        }
    }

    /// Take what is typed into the current group, tidied: see
    /// [`normalize_answer`].
    pub(crate) fn set_input(&mut self, index: usize, typed: &str) {
        if index != self.current_group {
            return;
        }
        if let Some(group) = self.groups.get_mut(index)
            && !group.confirmed()
        {
            group.input = normalize_answer(&group.sent, typed);
        }
    }

    /// Stamp the moment the answer reached the group's length, the first
    /// time it does; withdraw the stamp when it no longer does.
    pub(crate) fn mark_answer_complete(&mut self, index: usize, complete: bool, now: MonoMs) {
        if let Some(group) = self.groups.get_mut(index)
            && !group.confirmed()
        {
            group.answer_at = if complete {
                group.answer_at.or(Some(now))
            } else {
                None
            };
        }
    }

    /// Answer the current group with what is in its box. Returns what was
    /// copied, the first time only.
    pub(crate) fn confirm(&mut self, index: usize, now: MonoMs) -> Option<&CopyTally> {
        if index != self.current_group {
            return None;
        }
        let last = self.groups.len().saturating_sub(1);
        let group = self.groups.get_mut(index)?;
        if group.confirmed() {
            return None;
        }
        group.answer_at = group.answer_at.or(Some(now));
        self.focused_group = (index + 1).min(last);
        Some(group.tally.insert(CopyTally::of(&group.sent, &group.input)))
    }

    /// The finished session, scored, as it is stored. `finished_at` is wall
    /// time, the moment it ended; `date` the local day it counts towards.
    pub fn result(&self, finished_at: u64, date: String) -> SessionResult {
        let scored: Vec<&Group> = self
            .groups
            .iter()
            .filter(|group| group.confirmed() && !group.sent.is_empty())
            .collect();
        let mut tally = CopyTally::default();
        for copied in scored.iter().filter_map(|group| group.tally.as_ref()) {
            tally.merge(copied);
        }
        let accuracy = tally.accuracy();
        let sent: Vec<&str> = scored.iter().map(|g| g.sent.as_str()).collect();
        let timings: Vec<SessionTiming> = scored.iter().map(|g| g.timing()).collect();
        let per_char: Vec<f64> = scored
            .iter()
            .zip(&timings)
            .map(|(group, timing)| timing.time_to_complete_ms / group.sent.chars().count() as f64)
            .collect();
        let avg_response_ms = compute_average_response_ms(&per_char);
        let total_chars = calculate_total_chars(&sent);
        let score = compute_session_score(
            calculate_effective_alphabet_size(&sent),
            accuracy,
            avg_response_ms,
            total_chars,
            ScoreConstants::default(),
        );
        let played: Vec<(f64, f64)> = scored.iter().filter_map(|g| g.played_wpm).collect();
        let playback = &self.settings.playback;
        let (char_wpm, effective_wpm) = if played.is_empty() {
            (
                playback.char_wpm_min,
                playback.effective_wpm_min.min(playback.char_wpm_min),
            )
        } else {
            let n = played.len() as f64;
            (
                played.iter().map(|(c, _)| c).sum::<f64>() / n,
                played.iter().map(|(_, e)| e).sum::<f64>() / n,
            )
        };

        SessionResult {
            date,
            timestamp: finished_at,
            groups: scored
                .iter()
                .map(|group| GroupResult {
                    sent: group.sent.clone(),
                    received: group.input.clone(),
                    correct: group.sent == group.input,
                })
                .collect(),
            group_timings: timings,
            accuracy,
            tally,
            avg_response_ms,
            total_chars,
            score,
            // One level per mode, and `level` is the one this mode was run at —
            // callsigns need no second field the way Mixed needs `digits_level`.
            level: self.settings.active_level(),
            digits_level: self.settings.level(LevelAxis::Digits),
            char_set_mode: self.settings.curriculum.char_set_mode,
            char_wpm,
            effective_wpm,
            alphabet_fingerprint: self.settings.alphabet_fingerprint(),
        }
    }
}

/// An answer as it is kept: upper case, with no whitespace — a group never
/// has any, so a stray space is neither an error nor a reason the length
/// does not match — and no longer than [`LONGEST_ANSWER`] allows.
fn normalize_answer(sent: &str, typed: &str) -> String {
    let longest = (2 * sent.chars().count()).max(LONGEST_ANSWER);
    typed
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .take(longest)
        .collect()
}

/// Whether `received` is as long as `sent`, not counting whitespace: when an
/// answer is complete enough to confirm by itself.
pub fn answer_length_matches(sent: &str, received: &str) -> bool {
    !sent.is_empty()
        && received.chars().filter(|c| !c.is_whitespace()).count() == sent.chars().count()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupResult {
    pub sent: String,
    pub received: String,
    pub correct: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTiming {
    /// From the end of the send to the answer, never quicker than
    /// [`MIN_RESPONSE_MS_PER_CHAR`] a character.
    pub time_to_complete_ms: f64,
    #[serde(default)]
    pub char_wpm: Option<f64>,
}

fn session_level_default() -> u32 {
    LEVEL_MIN
}

fn session_mode_legacy() -> CharSetMode {
    CharSetMode::Koch
}

/// Sessions saved before the speed was recorded were sent at the default.
fn session_wpm_default() -> f64 {
    PlaybackSettings::default().char_wpm_min
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResult {
    pub date: String,
    /// Wall time the session finished, in milliseconds: what history is
    /// ordered by.
    pub timestamp: u64,
    pub groups: Vec<GroupResult>,
    pub group_timings: Vec<SessionTiming>,
    pub accuracy: f64,
    /// What was copied, character by character.
    #[serde(flatten)]
    pub tally: CopyTally,
    pub avg_response_ms: f64,
    pub total_chars: u32,
    pub score: f64,
    /// Saves from the first versions called it the Koch level.
    #[serde(alias = "kochLevel", default = "session_level_default")]
    pub level: u32,
    #[serde(default = "session_level_default")]
    pub digits_level: u32,
    #[serde(default = "session_mode_legacy")]
    pub char_set_mode: CharSetMode,
    #[serde(default = "session_wpm_default")]
    pub char_wpm: f64,
    #[serde(default = "session_wpm_default")]
    pub effective_wpm: f64,
    /// Full progress alphabet at the time of the session. Empty on legacy saves.
    #[serde(default)]
    pub alphabet_fingerprint: String,
}

impl SessionResult {
    /// How each character sent was copied. Sessions saved before that was
    /// recorded have it worked out again from their groups.
    pub fn letter_accuracy(&self) -> Cow<'_, BTreeMap<char, LetterAccuracy>> {
        if !self.tally.sent.is_empty() || self.groups.is_empty() {
            return Cow::Borrowed(&self.tally.sent);
        }
        let mut tally = CopyTally::default();
        for group in &self.groups {
            tally.add(&group.sent, &group.received);
        }
        Cow::Owned(tally.sent)
    }

    /// Whether this session's letter stats should seed sampling for `settings`.
    pub fn usable_for_sampling(&self, settings: &TrainingSettings) -> bool {
        if self.char_set_mode != settings.curriculum.char_set_mode {
            return false;
        }
        self.alphabet_fingerprint.is_empty()
            || self.alphabet_fingerprint == settings.alphabet_fingerprint()
    }

    /// A Koch session with nothing in it, for tests to fill in.
    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        Self {
            date: "2026-09-01".into(),
            timestamp: 0,
            groups: Vec::new(),
            group_timings: Vec::new(),
            accuracy: 0.0,
            tally: CopyTally::default(),
            avg_response_ms: 0.0,
            total_chars: 1,
            score: 0.0,
            level: 1,
            digits_level: 1,
            char_set_mode: CharSetMode::Koch,
            char_wpm: 18.0,
            effective_wpm: 18.0,
            alphabet_fingerprint: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session of `copies`: each group sent, its send ending half a second
    /// in at 18 WPM, then answered as typed `ms` later.
    fn answered(settings: TrainingSettings, copies: &[(&str, &str, u64)]) -> GroupSession {
        let mut session = GroupSession::new(SessionId::new(1), copies.len(), settings);
        let mut now = MonoMs(1_000);
        for (index, (sent, typed, ms)) in copies.iter().enumerate() {
            session.set_group(index, (*sent).into(), 1);
            session.begin_group(index, now);
            now = now.plus(500);
            session.end_playback(index, now, 18.0, 18.0);
            session.set_input(index, typed);
            now = now.plus(*ms);
            session.confirm(index, now);
        }
        session
    }

    fn result(session: &GroupSession) -> SessionResult {
        session.result(3_000, "2026-09-01".into())
    }

    #[test]
    fn perfect_copy_is_full_accuracy() {
        let session = answered(
            TrainingSettings::default(),
            &[("KM", "KM", 400), ("UK", "uk", 400)],
        );
        let result = result(&session);
        assert!((result.accuracy - 1.0).abs() < 1e-9);
        assert!(result.groups.iter().all(|g| g.correct));
        assert!((result.char_wpm - 18.0).abs() < 1e-9);
        assert_eq!(result.timestamp, 3_000);
    }

    /// Characters typed that were never sent cost accuracy. Without that a
    /// padded answer, or a doubled key, scored as a perfect copy — and the
    /// score and the level both follow this accuracy.
    #[test]
    fn padding_an_answer_is_not_a_perfect_copy() {
        let session = answered(
            TrainingSettings::default(),
            &[("KMURE", "KMUURE", 400), ("KM", "KMN", 400)],
        );
        let result = result(&session);
        assert!(
            (result.accuracy - 7.0 / 9.0).abs() < 1e-9,
            "{}",
            result.accuracy
        );
        assert_eq!(result.tally.inserted.values().sum::<u32>(), 2);
    }

    #[test]
    fn unconfirmed_and_empty_groups_are_not_scored() {
        let mut session = answered(TrainingSettings::default(), &[("KM", "KM", 400)]);
        session.groups.push(Group::blank());
        session.groups[1].sent = "RS".into();
        let scored = result(&session);
        assert_eq!(scored.groups.len(), 1);
        assert_eq!(scored.group_timings.len(), 1);

        // A confirmed group with nothing in it scores nothing at all.
        let silent = answered(TrainingSettings::default(), &[("", "", 400)]);
        assert!(result(&silent).groups.is_empty());
    }

    #[test]
    fn an_empty_answer_is_scored_wrong() {
        let result = result(&answered(TrainingSettings::default(), &[("KM", "", 400)]));
        assert_eq!(result.groups.len(), 1);
        assert!(!result.groups[0].correct);
        assert!(result.accuracy < 0.01);
    }

    /// Answers typed while the group is still being sent used to be timed at
    /// a millisecond, and the speed term made them worth ten to twenty times
    /// an answer a second after the send. They are as fast as a person types.
    #[test]
    fn an_answer_that_beats_the_send_scores_as_a_fast_typist_not_as_instant() {
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KMURE".into(), 1);
        session.begin_group(0, MonoMs(1_000));
        session.set_input(0, "KMURE");
        session.confirm(0, MonoMs(1_400));
        let early = result(&session);
        assert_eq!(
            early.group_timings[0].time_to_complete_ms,
            5.0 * MIN_RESPONSE_MS_PER_CHAR
        );
        assert_eq!(early.avg_response_ms, MIN_RESPONSE_MS_PER_CHAR);

        let slower = result(&answered(
            TrainingSettings::default(),
            &[("KMURE", "KMURE", 1_500)],
        ));
        assert!(
            early.score <= slower.score * 2.0,
            "{} against {}",
            early.score,
            slower.score
        );
    }

    #[test]
    fn the_answer_is_timed_from_the_first_moment_it_was_complete() {
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KMURE".into(), 1);
        session.begin_group(0, MonoMs(1_000));
        session.end_playback(0, MonoMs(2_000), 20.0, 18.0);
        session.mark_answer_complete(0, true, MonoMs(3_000));
        session.mark_answer_complete(0, true, MonoMs(3_500));
        session.confirm(0, MonoMs(4_000));
        assert_eq!(
            result(&session).group_timings[0].time_to_complete_ms,
            1_000.0
        );

        // A stamp withdrawn by a backspace is gone; the confirm is the answer.
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KMURE".into(), 1);
        session.end_playback(0, MonoMs(2_000), 20.0, 18.0);
        session.mark_answer_complete(0, true, MonoMs(2_500));
        session.mark_answer_complete(0, false, MonoMs(2_600));
        session.confirm(0, MonoMs(4_000));
        let timing = &result(&session).group_timings[0];
        assert_eq!(timing.time_to_complete_ms, 2_000.0);
        assert_eq!(timing.char_wpm, Some(20.0));
    }

    /// Typing is kept the way a group is written: no spaces, upper case, and
    /// not without end.
    #[test]
    fn an_answer_is_tidied_and_capped_on_the_way_in() {
        let mut session = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        session.set_group(0, "KM".into(), 1);
        session.set_input(0, " k m\t");
        assert_eq!(session.group(0).map(Group::input), Some("KM"));
        session.set_input(0, &"E".repeat(100_000));
        assert_eq!(
            session.group(0).map(|g| g.input().len()),
            Some(LONGEST_ANSWER)
        );
        let mut long = GroupSession::new(SessionId::new(1), 1, TrainingSettings::default());
        long.set_group(0, "KMUREKMUREKMUREKMURE".into(), 1);
        long.set_input(0, &"E".repeat(100));
        assert_eq!(long.group(0).map(|g| g.input().len()), Some(40));
    }

    #[test]
    fn input_is_only_accepted_for_the_current_unconfirmed_group() {
        let mut session = GroupSession::new(SessionId::new(1), 2, TrainingSettings::default());
        session.set_group(0, "KM".into(), 1);
        session.set_group(1, "UR".into(), 1);
        session.set_input(1, "nope");
        assert_eq!(session.group(1).map(Group::input), Some(""));
        session.set_input(0, "km");
        assert_eq!(session.group(0).map(Group::input), Some("KM"));
        assert!(session.confirm(0, MonoMs(1_200)).is_some());
        assert!(session.confirm(0, MonoMs(1_300)).is_none(), "only once");
        session.set_input(0, "later");
        assert_eq!(session.group(0).map(Group::input), Some("KM"));
        // A confirmed group keeps its text too.
        session.set_group(0, "XX".into(), 1);
        assert_eq!(session.group(0).map(Group::sent), Some("KM"));
    }

    #[test]
    fn confirming_only_takes_the_current_group_and_moves_the_focus_on() {
        let mut session = GroupSession::new(SessionId::new(1), 2, TrainingSettings::default());
        session.set_group(0, "KM".into(), 1);
        assert!(session.confirm(1, MonoMs(1_100)).is_none());
        assert!(session.confirm(9, MonoMs(1_100)).is_none());
        assert!(session.confirm(0, MonoMs(1_100)).is_some());
        assert!(session.any_confirmed());
        assert_eq!(session.view().focused, 1);

        // The last group keeps the focus.
        session.begin_group(1, MonoMs(2_000));
        assert!(session.confirm(1, MonoMs(2_100)).is_some());
        assert_eq!(session.view().focused, 1);
    }

    #[test]
    fn the_view_mirrors_the_current_group() {
        let mut settings = TrainingSettings::default();
        settings.playback.lock_input_during_group_playback = true;
        let mut session = GroupSession::new(SessionId::new(3), 2, settings);
        session.set_group(0, "KM".into(), 3);
        session.set_group(1, "UR".into(), 0);
        session.begin_group(0, MonoMs(1_000));
        session.note_play_finished(0);
        let view = session.view();
        assert!(view.playing && view.locked);
        assert_eq!(
            (view.current, view.repeat_total, view.repeat_done),
            (0, 3, 1)
        );
        assert_eq!(view.sent, vec!["KM", "UR"]);
        assert_eq!(view.confirmed, vec![false, false]);
        // Repeats are never below one.
        assert_eq!(session.group_repeats(1), 1);
        assert_eq!(session.group_repeats(9), 1);

        session.set_phase(SessionPhase::AwaitingAnswer { index: 0 });
        assert!(!session.view().playing);
        assert!(!session.input_locked(0));
        // A session always has at least one group, even if asked for none.
        let empty = GroupSession::new(SessionId::new(1), 0, TrainingSettings::default());
        assert_eq!(empty.group_count(), 1);
    }

    #[test]
    fn the_focus_moves_with_the_group_being_sent() {
        let mut session = GroupSession::new(SessionId::new(1), 2, TrainingSettings::default());
        assert_eq!(session.view().focused, 0);
        session.begin_group(1, MonoMs(10));
        assert_eq!(session.view().focused, 1);
        // Beginning a group that is not there changes nothing.
        session.begin_group(9, MonoMs(20));
        assert_eq!(session.current_group(), 1);
    }

    #[test]
    fn an_answer_matches_only_at_the_same_length() {
        assert!(answer_length_matches("KM", "km"));
        assert!(answer_length_matches("KM", " k m "));
        assert!(!answer_length_matches("KM", "k"));
        assert!(!answer_length_matches("KM", "kmu"));
        assert!(!answer_length_matches("", ""));
    }

    #[test]
    fn the_result_records_the_speed_it_was_sent_at() {
        let mut settings = TrainingSettings::default();
        settings.playback.effective_wpm_min = 18.0;
        let mut session = GroupSession::new(SessionId::new(1), 1, settings.clone());
        session.set_group(0, "KM".into(), 1);
        session.end_playback(0, MonoMs(500), 25.0, 12.0);
        session.confirm(0, MonoMs(1_000));
        let played = result(&session);
        assert!((played.char_wpm - 25.0).abs() < 1e-9);
        assert!((played.effective_wpm - 12.0).abs() < 1e-9);
        assert_eq!(played.alphabet_fingerprint, settings.alphabet_fingerprint());

        // A session whose sends never reported a speed records the settings'.
        settings.playback.link_char_to_effective = false;
        settings.playback.char_wpm_min = 22.0;
        settings.playback.effective_wpm_min = 14.0;
        let mut silent = GroupSession::new(SessionId::new(1), 1, settings);
        silent.set_group(0, "KM".into(), 1);
        silent.confirm(0, MonoMs(10));
        let unplayed = result(&silent);
        assert_eq!((unplayed.char_wpm, unplayed.effective_wpm), (22.0, 14.0));
    }

    #[test]
    fn a_digits_session_records_the_digits_level() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Digits;
        settings.curriculum.digits_level = 4;
        let result = result(&answered(settings, &[("01", "01", 10)]));
        assert_eq!(result.level, 4);
        assert_eq!(result.digits_level, 4);
        assert_eq!(result.alphabet_fingerprint, "0123456789");
    }

    #[test]
    fn history_is_reusable_only_for_the_same_alphabet() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        let result = result(&answered(settings.clone(), &[("KM", "KM", 10)]));
        assert!(result.usable_for_sampling(&settings));

        // A different character set never seeds another one's sampling.
        let mut other = settings.clone();
        other.curriculum.char_set_mode = CharSetMode::Digits;
        assert!(!result.usable_for_sampling(&other));

        // Same mode, different alphabet.
        let mut resequenced = settings;
        resequenced.curriculum.custom_sequence = vec!['A', 'B', 'C'];
        assert!(!result.usable_for_sampling(&resequenced));

        // A legacy session carries no fingerprint and is accepted.
        let mut legacy = result;
        legacy.alphabet_fingerprint.clear();
        assert!(legacy.usable_for_sampling(&resequenced));
    }

    /// The tally is saved as the flat keys a session always had, plus the
    /// characters typed but never sent when there were any.
    #[test]
    fn a_session_tally_round_trips_through_its_saved_keys() {
        let result = result(&answered(TrainingSettings::default(), &[("KM", "KMN", 10)]));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["letterAccuracy"]["K"]["correct"], 1);
        assert_eq!(json["inserted"]["N"], 1);
        assert!(json.get("tally").is_none());
        let back: SessionResult = serde_json::from_value(json).unwrap();
        assert_eq!(back, result);

        let mut clean = result;
        clean.tally.inserted.clear();
        let json = serde_json::to_value(&clean).unwrap();
        assert!(json.get("inserted").is_none());
    }

    /// Older saves carry fields this version no longer writes, and lack ones
    /// it added. Both still load.
    #[test]
    fn legacy_session_json_fills_new_fields() {
        let timing: SessionTiming =
            serde_json::from_str(r#"{"timeToCompleteMs":1,"perCharMs":1}"#).unwrap();
        assert_eq!(timing.char_wpm, None);
        let raw = r#"{
            "date":"2026-01-01","timestamp":1,"startedAt":0,"finishedAt":1,
            "groups":[],"groupTimings":[],"accuracy":1,
            "letterAccuracy":{"K":{"correct":1,"total":2}},
            "alphabetSize":0,"avgResponseMs":0,"totalChars":0,
            "effectiveAlphabetSize":0,"score":0,"kochLevel":3
        }"#;
        let result: SessionResult = serde_json::from_str(raw).unwrap();
        assert_eq!(
            result.tally.sent.get(&'K'),
            Some(&crate::alignment::LetterAccuracy {
                correct: 1,
                total: 2
            })
        );
        assert!(result.tally.inserted.is_empty());
        assert_eq!(result.level, 3);
        assert_eq!(result.digits_level, 1);
        assert_eq!(result.char_set_mode, CharSetMode::Koch);
        assert_eq!(result.char_wpm, 18.0);
        assert_eq!(result.effective_wpm, 18.0);
        assert!(result.alphabet_fingerprint.is_empty());

        let json = serde_json::to_value(&result).unwrap();
        for gone in [
            "startedAt",
            "finishedAt",
            "alphabetSize",
            "effectiveAlphabetSize",
            "perCharMs",
        ] {
            assert!(json.get(gone).is_none(), "{gone} is still written");
        }
    }

    #[test]
    fn a_monotonic_moment_never_runs_backwards() {
        assert_eq!(MonoMs(1_500).since(MonoMs(1_000)), 500);
        assert_eq!(MonoMs(1_000).since(MonoMs(1_500)), 0);
        assert_eq!(MonoMs(u64::MAX).plus(5), MonoMs(u64::MAX));
    }
}
