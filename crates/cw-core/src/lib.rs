//! Pure Morse group-training domain: Koch pools, Farnsworth timing, scoring, session machine.
//!
//! No React, no DOM, no audio backend. The WASM/UI crate consumes [`timing::PlaybackPlan`]
//! and drives [`session::GroupSession`].

#![forbid(unsafe_code)]

pub mod alignment;
pub mod auto_level;
pub mod band;
pub mod callsign;
pub mod heatmap;
pub mod keyer;
pub mod level;
pub mod machine;
pub mod morse;
pub mod pool;
pub mod rng;
pub mod sampling;
pub mod score;
pub mod sequences;
pub mod session;
pub mod settings;
#[cfg(test)]
mod soak;
pub mod stats;
pub mod streak;
pub mod timing;

pub use alignment::{AlignmentPair, CopyTally, LetterAccuracy, align_group};
pub use auto_level::{
    AutoLevelChange, AutoLevelCounters, AutoLevelProgress, CountersKey, apply_auto_level,
    auto_level_progress, evaluate_auto_level,
};
pub use callsign::{
    CALLSIGN_TIER_MAX, CALLSIGN_TIER_MIN, CallsignParts, callsign_pool, generate_callsign,
    parse_callsign, tier_examples,
};
pub use heatmap::{HEATMAP_WEEKS, HeatmapCell, HeatmapColorMode, HeatmapGrid, build_heatmap};
pub use keyer::{
    KeyerMode, LETTER_GAP_DITS, Paddle, PaddleDecoder, PaddleKeyer, STRAIGHT_DAH_DITS,
    dit_ms_for_wpm, paddle_from_bracket,
};
pub use level::{LEVEL_MIN, max_level_for_len, unlocked_count_for_level, unlocked_prefix};
pub use machine::{
    AUTO_CONFIRM_DELAY_MS, GroupSource, SessionEffect, SessionEvent, SessionMachine,
};
pub use morse::{
    LCWO_SEQUENCE, MAX_DIGITS_LEVEL, decode_morse_pattern, is_morse_code_prefix, morse_for,
};
pub use pool::{compute_char_pool, current_practice_window, unlocked_practice_count};
pub use rng::{FastrandRng, Rng, weighted_random_pick};
pub use sampling::{CharSamplingState, Sampler, generate_training_group};
pub use sequences::{PresetId, SEQUENCE_PRESETS, SequenceChoice, SequencePreset};
pub use session::{
    Group, GroupResult, GroupSession, MIN_RESPONSE_MS_PER_CHAR, MonoMs, SessionId, SessionPhase,
    SessionResult, SessionTiming, SessionView, answer_length_matches,
};
pub use settings::{
    AutoLevelSettings, BandSettings, CharSetMode, CurriculumSettings, DEFAULT_FILTER_WIDTH_HZ,
    FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, FilterShape, GROUP_REPEAT_MAX, GROUP_REPEAT_MIN, LevelAxis,
    MixedAutoLevelAxis, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB, PILEUP_SPREAD_MAX,
    PILEUP_SPREAD_MIN, PlaybackSettings, PracticeWindow, RangeSetting, RangeValues, STATIONS_MAX,
    STATIONS_MIN, SettingsSection, TrainingSettings,
};
pub use stats::{
    AccuracyPoint, BigramHeatmap, CharacterDiagnostic, ConfusionEntry, GROUP_START_BIGRAM_TOKEN,
    MasteryStatus, MistakeStats, SamplingRow, SessionHistoryRow, accuracy_chart,
    character_diagnostics, mistake_stats, sampling_rows, session_history,
};
pub use streak::{StreakState, StreakStatus, compute_streak_status};
pub use timing::{
    EnvelopePoint, EnvelopeShape, Interferer, PlannedTransmission, PlaybackPlan, StationVoice,
    ToneEvent, Transmission, build_envelope_curve, compute_after_group_gap_ms,
    compute_group_gap_for_wpm, compute_group_gap_ms, dot_seconds, envelope_shape,
    plan_morse_playback, plan_morse_playback_for, plan_transmission, resolve_group_repeats,
    resolve_pileup, resolve_station,
};
