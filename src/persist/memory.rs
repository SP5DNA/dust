//! A store that keeps everything in memory, for tests: each test makes its
//! own, so nothing one saves reaches another, or the user's real data.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::{HistorySaved, Store, newest};

#[derive(Default)]
pub struct MemoryStore {
    theme: RefCell<String>,
    tutorial_done: Cell<bool>,
    settings: RefCell<Option<TrainingSettings>>,
    sessions: RefCell<Vec<SessionResult>>,
    counters: RefCell<BTreeMap<CountersKey, AutoLevelCounters>>,
    /// How many times the settings were written, for a test that cares how
    /// often that happens.
    pub settings_saves: Cell<u32>,
    /// Room for only this many sessions, the way a full browser leaves it.
    pub room: Cell<Option<usize>>,
    /// A history that could not be read is in the way of any save.
    pub held: Cell<bool>,
}

impl Store for MemoryStore {
    fn load_theme(&self) -> String {
        self.theme.borrow().clone()
    }

    fn save_theme(&self, theme: &str) {
        *self.theme.borrow_mut() = theme.to_string();
    }

    fn tutorial_done(&self) -> bool {
        self.tutorial_done.get()
    }

    fn mark_tutorial_done(&self) {
        self.tutorial_done.set(true);
    }

    fn load_settings(&self) -> TrainingSettings {
        self.settings.borrow().clone().unwrap_or_default().clamp()
    }

    fn save_settings(&self, settings: &TrainingSettings) {
        self.settings_saves.set(self.settings_saves.get() + 1);
        *self.settings.borrow_mut() = Some(settings.clone());
    }

    fn load_sessions(&self) -> Vec<SessionResult> {
        self.sessions.borrow().clone()
    }

    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved {
        if self.held.get() {
            return HistorySaved::Held;
        }
        let newest = newest(sessions);
        let kept =
            &newest[newest.len() - self.room.get().unwrap_or(usize::MAX).min(newest.len())..];
        *self.sessions.borrow_mut() = kept.to_vec();
        if kept.len() < newest.len() {
            HistorySaved::Squeezed { kept: kept.len() }
        } else {
            HistorySaved::Stored { kept: kept.len() }
        }
    }

    fn load_auto_counters(&self, key: &CountersKey) -> AutoLevelCounters {
        self.counters.borrow().get(key).copied().unwrap_or_default()
    }

    fn save_auto_counters(&self, key: &CountersKey, counters: AutoLevelCounters) {
        self.counters.borrow_mut().insert(key.clone(), counters);
    }

    fn clear_auto_counters(&self, keys: &[CountersKey]) {
        let mut counters = self.counters.borrow_mut();
        for key in keys {
            counters.remove(key);
        }
    }
}
