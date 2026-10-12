//! `localStorage`: the browser store.

use std::cell::Cell;

use cw_core::{AutoLevelCounters, CountersKey, SessionResult, TrainingSettings};

use super::{HistorySaved, Store, load_settings_from, newest, newest_that_fit, parse_sessions};

const THEME_KEY: &str = "dust_theme";
const SETTINGS_KEY: &str = "dust_settings";
const SESSIONS_KEY: &str = "dust_sessions";
const UNREADABLE_SESSIONS_KEY: &str = "dust_sessions_unreadable";
const AUTO_PREFIX: &str = "dust_auto_adjust_";

/// How much of the site's storage the history may take, in characters.
/// Browsers give a site about five million; the rest is left for the
/// settings, the counters and the room a write needs.
const HISTORY_BUDGET: usize = 3_000_000;

#[derive(Default)]
pub struct WebStore {
    /// The stored history could not be read, nor copied out of the way, so
    /// it is not to be written over.
    history_held: Cell<bool>,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn counters_key(key: &CountersKey) -> String {
    format!("{AUTO_PREFIX}{}", key.as_str())
}

impl Store for WebStore {
    fn load_theme(&self) -> String {
        storage()
            .and_then(|store| store.get_item(THEME_KEY).ok().flatten())
            .unwrap_or_default()
    }

    fn save_theme(&self, theme: &str) {
        if let Some(store) = storage() {
            let _ = store.set_item(THEME_KEY, theme);
        }
    }

    fn load_settings(&self) -> TrainingSettings {
        let raw = storage()
            .and_then(|store| store.get_item(SETTINGS_KEY).ok().flatten())
            .unwrap_or_default();
        load_settings_from(&raw)
    }

    fn save_settings(&self, settings: &TrainingSettings) {
        let Some(store) = storage() else {
            return;
        };
        if let Ok(raw) = serde_json::to_string(settings) {
            let _ = store.set_item(SETTINGS_KEY, &raw);
        }
    }

    fn load_sessions(&self) -> Vec<SessionResult> {
        let Some(store) = storage() else {
            return Vec::new();
        };
        let Ok(Some(raw)) = store.get_item(SESSIONS_KEY) else {
            return Vec::new();
        };
        parse_sessions(&raw).unwrap_or_else(|| {
            // Keep the unreadable copy where the next save cannot reach it —
            // and only once it is safely there take it out of the way. A copy
            // that would not fit is no copy, and the original stays put.
            if store.set_item(UNREADABLE_SESSIONS_KEY, &raw).is_ok() {
                let _ = store.remove_item(SESSIONS_KEY);
            } else {
                self.history_held.set(true);
            }
            Vec::new()
        })
    }

    fn save_sessions(&self, sessions: &[SessionResult]) -> HistorySaved {
        if self.history_held.get() {
            return HistorySaved::Held;
        }
        let Some(store) = storage() else {
            return HistorySaved::Failed("this browser has no storage for it.".into());
        };
        let newest = newest(sessions);
        // The newest that fit the budget; and if the browser has less room
        // than that, the newest that fit in half as much, and so on.
        let mut budget = HISTORY_BUDGET;
        loop {
            let (raw, kept) = newest_that_fit(newest, budget);
            match store.set_item(SESSIONS_KEY, &raw) {
                Ok(()) if kept < newest.len() => return HistorySaved::Squeezed { kept },
                Ok(()) => return HistorySaved::Stored { kept },
                Err(_) if kept > 1 => budget = raw.len() / 2,
                Err(_) => {
                    return HistorySaved::Failed("this browser's storage is full.".into());
                }
            }
        }
    }

    fn load_auto_counters(&self, key: &CountersKey) -> AutoLevelCounters {
        storage()
            .and_then(|store| store.get_item(&counters_key(key)).ok().flatten())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_auto_counters(&self, key: &CountersKey, counters: AutoLevelCounters) {
        let Some(store) = storage() else {
            return;
        };
        if let Ok(raw) = serde_json::to_string(&counters) {
            let _ = store.set_item(&counters_key(key), &raw);
        }
    }

    fn clear_auto_counters(&self, keys: &[CountersKey]) {
        let Some(store) = storage() else {
            return;
        };
        for key in keys {
            let _ = store.remove_item(&counters_key(key));
        }
    }
}
