//! Starting points for four kinds of learner: how long a session is, how
//! long its groups are, how fast they come, and the level to begin at.
//!
//! A preset is applied once, through the same calls the settings screen
//! makes, and leaves everything else as it was. The settings are on a preset
//! for as long as its session, group size and speed are what it set: the
//! level is where it starts, and the trainer moves it from there.

use super::{RangeSetting, RangeValues, TrainingSettings};

/// A kind of learner, and the settings that suit them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Experience {
    Novice,
    Intermediate,
    Advanced,
    Expert,
}

/// Groups in a session, whichever preset.
const GROUPS: u32 = 20;

impl Experience {
    pub const ALL: [Self; 4] = [
        Self::Novice,
        Self::Intermediate,
        Self::Advanced,
        Self::Expert,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Novice => "Novice",
            Self::Intermediate => "Intermediate",
            Self::Advanced => "Advanced",
            Self::Expert => "Expert",
        }
    }

    /// The one-word name a control holds it under.
    pub fn key(self) -> &'static str {
        match self {
            Self::Novice => "novice",
            Self::Intermediate => "intermediate",
            Self::Advanced => "advanced",
            Self::Expert => "expert",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|experience| experience.key() == key)
    }

    /// Groups in a session.
    pub fn groups(self) -> u32 {
        GROUPS
    }

    /// Characters in a group: the fewest and the most. One number is a fixed
    /// size.
    pub fn group_size(self) -> (u32, u32) {
        match self {
            Self::Novice => (2, 2),
            Self::Intermediate => (2, 3),
            Self::Advanced => (2, 5),
            Self::Expert => (3, 5),
        }
    }

    /// Character speed, slowest and fastest, in words a minute.
    pub fn char_wpm(self) -> (f64, f64) {
        match self {
            Self::Novice => (15.0, 15.0),
            Self::Intermediate | Self::Advanced => (15.0, 25.0),
            Self::Expert => (15.0, 35.0),
        }
    }

    /// The level it starts at.
    pub fn level(self) -> u32 {
        match self {
            Self::Novice => 1,
            Self::Intermediate => 8,
            Self::Advanced | Self::Expert => 14,
        }
    }

    /// Put the settings on this preset: the session, the group size, the
    /// speed and the level, the way the settings screen sets each.
    pub fn apply(self, settings: &mut TrainingSettings) {
        settings.curriculum.num_groups = GROUPS;
        let (fewest, most) = self.group_size();
        put(
            settings,
            RangeSetting::GroupSize,
            f64::from(fewest),
            f64::from(most),
        );
        let (slowest, fastest) = self.char_wpm();
        put(settings, RangeSetting::CharWpm, slowest, fastest);
        settings.set_active_level(self.level());
    }

    /// The preset the settings are on, if they are on one.
    pub fn of(settings: &TrainingSettings) -> Option<Self> {
        Self::ALL.into_iter().find(|experience| {
            let (fewest, most) = experience.group_size();
            let (slowest, fastest) = experience.char_wpm();
            settings.curriculum.num_groups == GROUPS
                && holds(
                    settings.range(RangeSetting::GroupSize),
                    f64::from(fewest),
                    f64::from(most),
                )
                && holds(settings.range(RangeSetting::CharWpm), slowest, fastest)
        })
    }
}

/// Set a range to run from `min` to `max`: fixed when they are one number,
/// drawn afresh for each group when they are not.
fn put(settings: &mut TrainingSettings, which: RangeSetting, min: f64, max: f64) {
    settings.set_range_linked(which, min == max);
    // The lower end first: it carries the upper one up with it if it has to,
    // and the upper end then goes where it should.
    settings.set_range_min(which, min);
    settings.set_range_max(which, max);
}

/// Whether a range is the one [`put`] makes.
fn holds(range: RangeValues, min: f64, max: f64) -> bool {
    let same = |a: f64, b: f64| (a - b).abs() < 1e-9;
    if min == max {
        range.linked && same(range.min, min)
    } else {
        !range.linked && same(range.min, min) && same(range.max, max)
    }
}

#[cfg(test)]
mod tests {
    use super::Experience;
    use crate::settings::{CharSetMode, LevelAxis, RangeSetting, RangeValues, TrainingSettings};

    #[test]
    fn each_preset_sets_what_it_says() {
        let range = |min, max, linked| RangeValues { min, max, linked };
        let cases = [
            (
                Experience::Novice,
                range(2.0, 2.0, true),
                range(15.0, 15.0, true),
                1,
            ),
            (
                Experience::Intermediate,
                range(2.0, 3.0, false),
                range(15.0, 25.0, false),
                8,
            ),
            (
                Experience::Advanced,
                range(2.0, 5.0, false),
                range(15.0, 25.0, false),
                14,
            ),
            (
                Experience::Expert,
                range(3.0, 5.0, false),
                range(15.0, 35.0, false),
                14,
            ),
        ];
        for (experience, size, speed, level) in cases {
            let mut settings = TrainingSettings::default();
            experience.apply(&mut settings);
            assert_eq!(settings.curriculum.num_groups, 20, "{experience:?}");
            assert_eq!(
                settings.range(RangeSetting::GroupSize),
                size,
                "{experience:?}"
            );
            assert_eq!(
                settings.range(RangeSetting::CharWpm),
                speed,
                "{experience:?}"
            );
            assert_eq!(settings.active_level(), level, "{experience:?}");
            assert_eq!(Experience::of(&settings), Some(experience));
            assert_eq!(
                settings.clone().clamp(),
                settings,
                "{experience:?} is out of bounds"
            );
        }
    }

    /// Whichever preset the settings were on, another one lands whole: a
    /// range that has to move both ends past each other gets both there.
    #[test]
    fn any_preset_lands_whole_from_any_other() {
        for from in Experience::ALL {
            for to in Experience::ALL {
                let mut settings = TrainingSettings::default();
                from.apply(&mut settings);
                to.apply(&mut settings);
                assert_eq!(Experience::of(&settings), Some(to), "{from:?} to {to:?}");
                assert_eq!(settings.active_level(), to.level(), "{from:?} to {to:?}");
            }
        }
    }

    /// The rest of the settings are left as they were.
    #[test]
    fn a_preset_leaves_everything_else_alone() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.digits_level = 4;
        settings.band.side_tone_min = 550.0;
        settings.band.side_tone_max = 650.0;
        settings.auto_level.auto_adjust_above_threshold_count = 3;
        settings.playback.group_repeat_max = 2;
        let mut applied = settings.clone();
        Experience::Expert.apply(&mut applied);
        assert_eq!(applied.band, settings.band);
        assert_eq!(applied.auto_level, settings.auto_level);
        assert_eq!(applied.curriculum.char_set_mode, CharSetMode::Koch);
        assert_eq!(applied.level(LevelAxis::Digits), 4);
        assert_eq!(applied.playback.group_repeat_max, 2);
        assert_eq!(applied.playback.keyer_wpm, settings.playback.keyer_wpm);
    }

    /// The level is where a preset starts. The trainer moves it, and the
    /// settings stay on the preset; a change to anything it set takes them
    /// off it.
    #[test]
    fn the_level_moving_keeps_the_preset_and_a_change_drops_it() {
        let mut settings = TrainingSettings::default();
        Experience::Novice.apply(&mut settings);
        settings.set_active_level(5);
        assert_eq!(Experience::of(&settings), Some(Experience::Novice));

        let mut longer = settings.clone();
        longer.curriculum.num_groups = 30;
        assert_eq!(Experience::of(&longer), None);

        let mut faster = settings.clone();
        faster.set_range_min(RangeSetting::CharWpm, 18.0);
        assert_eq!(Experience::of(&faster), None);

        let mut random = settings;
        random.set_range_linked(RangeSetting::GroupSize, false);
        random.set_range_max(RangeSetting::GroupSize, 3.0);
        assert_eq!(Experience::of(&random), None);
    }

    #[test]
    fn the_settings_as_they_ship_are_on_no_preset() {
        assert_eq!(Experience::of(&TrainingSettings::default()), None);
    }

    /// With the speeds tied, the effective speed follows the character speed
    /// a preset sets, as it does when the screen sets it.
    #[test]
    fn a_tied_effective_speed_follows() {
        let mut settings = TrainingSettings::default();
        assert!(settings.playback.link_char_to_effective);
        Experience::Expert.apply(&mut settings);
        let effective = settings.range(RangeSetting::EffectiveWpm);
        assert_eq!((effective.min, effective.max), (15.0, 35.0));
    }

    #[test]
    fn each_preset_is_found_by_its_key() {
        for experience in Experience::ALL {
            assert_eq!(Experience::from_key(experience.key()), Some(experience));
        }
        assert_eq!(Experience::from_key("custom"), None);
        assert_eq!(Experience::from_key(""), None);
    }
}
