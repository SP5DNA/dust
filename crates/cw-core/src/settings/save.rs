//! How settings are read back from a save, older ones included.

use serde::Deserialize;

use super::{
    AutoLevelSettings, BandSettings, CurriculumSettings, PlaybackSettings, PracticeWindow,
    TrainingSettings,
};

/// A save as it is read: today's settings, and the practice window as older
/// saves kept it.
///
/// The legacy keys come first. A flattened section takes the keys it knows
/// as it reads them, so `practiceWindow` is read here, as an option, before
/// the curriculum's own non-optional field could see it.
#[derive(Deserialize)]
struct SettingsSave {
    #[serde(flatten)]
    window: WindowSave,
    #[serde(flatten)]
    curriculum: CurriculumSettings,
    #[serde(flatten)]
    playback: PlaybackSettings,
    #[serde(flatten)]
    band: BandSettings,
    #[serde(flatten)]
    auto_level: AutoLevelSettings,
}

/// Before the practice window was named, a save kept the first and last
/// unlocked character it drew from, counting from one.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct WindowSave {
    practice_window: Option<PracticeWindow>,
    sliding_window_start: Option<u32>,
    sliding_window_end: Option<u32>,
}

impl From<SettingsSave> for TrainingSettings {
    fn from(save: SettingsSave) -> Self {
        let mut settings = Self {
            curriculum: save.curriculum,
            playback: save.playback,
            band: save.band,
            auto_level: save.auto_level,
        };
        let window = &save.window;
        settings.curriculum.practice_window = match (
            window.practice_window,
            window.sliding_window_start,
            window.sliding_window_end,
        ) {
            (Some(named), _, _) => named,
            (None, Some(start), Some(end)) => {
                window_from_bounds(crate::pool::unlocked_practice_count(&settings), start, end)
            }
            (None, _, _) => PracticeWindow::All,
        };
        settings
    }
}

/// The named window an old pair of bounds meant, over `unlocked`
/// characters. Bounds that match none of them were a window this version
/// cannot draw, and come back as everything.
fn window_from_bounds(unlocked: usize, start: u32, end: u32) -> PracticeWindow {
    let n = u32::try_from(unlocked).unwrap_or(u32::MAX).max(1);
    let start = start.clamp(1, n);
    let end = end.clamp(1, n);
    let (low, high) = (start.min(end), start.max(end));
    if low == 1 && high == n {
        PracticeWindow::All
    } else if n >= 3 && low == n - 2 && high == n {
        PracticeWindow::Last3
    } else if n >= 5 && low == n - 4 && high == n {
        PracticeWindow::Last5
    } else {
        PracticeWindow::All
    }
}

impl<'de> Deserialize<'de> for TrainingSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        SettingsSave::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CharSetMode;

    fn load(json: serde_json::Value) -> TrainingSettings {
        serde_json::from_value(json).expect("a save should load")
    }

    /// Level 5 unlocks six Koch characters, and old saves said which of them
    /// they drew from as a pair of bounds.
    #[test]
    fn an_old_save_without_a_named_window_is_read_from_its_bounds() {
        let save = |start: u32, end: u32| {
            load(serde_json::json!({
                "charSetMode": "koch", "level": 5,
                "slidingWindowStart": start, "slidingWindowEnd": end,
            }))
            .curriculum
            .practice_window
        };
        assert_eq!(save(4, 6), PracticeWindow::Last3);
        assert_eq!(save(2, 6), PracticeWindow::Last5);
        assert_eq!(save(1, 6), PracticeWindow::All);
        // Reversed bounds are read the right way round.
        assert_eq!(save(6, 4), PracticeWindow::Last3);
        // Bounds that match no named window fall back to everything.
        assert_eq!(save(3, 5), PracticeWindow::All);
    }

    #[test]
    fn a_named_window_wins_over_old_bounds() {
        let settings = load(serde_json::json!({
            "level": 5, "practiceWindow": "last5",
            "slidingWindowStart": 4, "slidingWindowEnd": 6,
        }));
        assert_eq!(settings.curriculum.practice_window, PracticeWindow::Last5);
        // A window saved as nothing at all, before it was ever set, is
        // everything.
        let settings = load(serde_json::json!({ "practiceWindow": null }));
        assert_eq!(settings.curriculum.practice_window, PracticeWindow::All);
    }

    #[test]
    fn the_old_bounds_are_not_written_back() {
        let json = serde_json::to_value(load(serde_json::json!({
            "slidingWindowStart": 4, "slidingWindowEnd": 6,
        })))
        .expect("settings serialize");
        assert!(json.get("slidingWindowStart").is_none());
        assert!(json.get("slidingWindowEnd").is_none());
        assert_eq!(json["practiceWindow"], "all");
    }

    /// Every default lives in a `Default` impl, and an empty save is exactly
    /// those.
    #[test]
    fn an_empty_save_is_the_defaults() {
        assert_eq!(load(serde_json::json!({})), TrainingSettings::default());
    }

    /// The first versions named two settings after the Koch method. Their
    /// saves still load.
    #[test]
    fn the_first_versions_names_still_load() {
        let settings = load(serde_json::json!({
            "charSetMode": "koch", "kochLevel": 5, "autoAdjustKoch": false,
        }));
        assert_eq!(settings.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(settings.curriculum.level, 5);
        assert!(!settings.auto_level.auto_adjust_level);
    }
}
