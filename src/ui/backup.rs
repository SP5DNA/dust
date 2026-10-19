//! Backups, on the History tab: a file of the history and the settings the
//! user keeps outside the app, saved and restored from here, and a word when
//! the last one is many sessions old.

use dioxus::html::FileData;
use dioxus::prelude::*;

use crate::persist::backup::{BACKUP_DUE_AFTER, BACKUP_MAX_BYTES, Backup, read_backup};
use crate::ui::format::{counted, day_of, short_date};
use crate::ui::widgets::{CardHead, Icon, IconName, control_id};

/// What the History tab needs to save and restore backups.
#[derive(Clone, PartialEq)]
pub struct Backups {
    /// When the last backup was saved, if ever.
    pub last: Option<u64>,
    /// This build can hand over a backup file; an app on a phone has nowhere
    /// to put one, and can only restore.
    pub can_save: bool,
    pub on_save: EventHandler<()>,
    pub on_restore: EventHandler<Backup>,
}

/// What could be lost without a backup, in the words of where it is kept.
const AT_RISK: &str = if cfg!(target_arch = "wasm32") {
    "if this browser's data is cleared"
} else {
    "if this device's data is lost"
};

/// Read a chosen file as a backup, and hand it on to be restored; or say
/// why it could not be.
async fn restore_file(
    file: FileData,
    on_restore: EventHandler<Backup>,
    mut problem: Signal<Option<String>>,
) {
    let name = file.name();
    if file.size() > BACKUP_MAX_BYTES {
        problem.set(Some(format!(
            "{name} is far larger than any backup: nothing was restored."
        )));
        return;
    }
    let read = match file.read_string().await {
        Ok(text) => read_backup(&text).map_err(|not| not.reason()),
        Err(_) => Err("could not be read"),
    };
    match read {
        Ok(backup) => {
            problem.set(None);
            on_restore.call(backup);
        }
        Err(reason) => problem.set(Some(format!("{name} {reason}: nothing was restored."))),
    }
}

/// Save a backup, or restore one; and when `since` sessions have gone
/// unbacked for long enough, say so.
#[component]
pub fn BackupCard(
    backups: Backups,
    /// The sessions finished since the last backup, or all of them.
    since: usize,
) -> Element {
    let problem = use_signal(|| None::<String>);
    // Moved on by every file chosen, to draw a fresh file input: the same
    // file chosen again in the old one would change nothing.
    let mut picked = use_signal(|| 0u64);
    let Backups {
        last,
        can_save,
        on_save,
        on_restore,
    } = backups;
    let due = can_save && since >= BACKUP_DUE_AFTER;
    let last_day = last.and_then(day_of).map(|day| short_date(&day));
    let status = match (&last_day, since) {
        (Some(day), 0) => format!("Last saved {day} · nothing new since"),
        (Some(day), _) => format!("Last saved {day} · {} since", counted(since, "session")),
        (None, _) => "No backup yet".to_string(),
    };
    let note = if !can_save {
        "Bring in a backup saved by the web app or on a computer: restoring it adds the sessions missing here and takes its settings.".to_string()
    } else if due && last.is_some() {
        format!(
            "{} since your last backup. A new one keeps them safe {AT_RISK}.",
            counted(since, "session")
        )
    } else if due {
        format!(
            "{} and no backup yet. A backup file keeps them safe {AT_RISK}, and moves them to another device.",
            counted(since, "session")
        )
    } else {
        format!(
            "A backup file keeps your history and settings safe {AT_RISK}. Restoring one puts back whatever is missing here."
        )
    };
    rsx! {
        div { class: if due { "card stack-sm backup-card due" } else { "card stack-sm backup-card" },
            CardHead { icon: IconName::Download, title: "Backup", note: status }
            p { class: "backup-note", "{note}" }
            div { class: "backup-actions",
                if can_save {
                    button {
                        id: control_id("btn", "save a backup"),
                        class: if due { "btn btn-primary btn-sm" } else { "btn btn-secondary btn-sm" },
                        r#type: "button",
                        onclick: move |_| on_save.call(()),
                        Icon { name: IconName::Download }
                        "Save a backup"
                    }
                }
                label { class: "btn btn-secondary btn-sm file-button",
                    for fresh in [picked()] {
                        input {
                            key: "{fresh}",
                            id: control_id("file", "backup"),
                            class: "file-input",
                            r#type: "file",
                            accept: ".json,application/json",
                            onchange: move |e| {
                                if let Some(file) = e.files().into_iter().next() {
                                    picked += 1;
                                    spawn(restore_file(file, on_restore, problem));
                                }
                            },
                        }
                    }
                    Icon { name: IconName::Upload }
                    "Restore a backup"
                }
            }
            if let Some(problem) = problem() {
                p { class: "backup-problem", role: "alert", "{problem}" }
            }
        }
    }
}
