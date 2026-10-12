use cw_core::{KeyerMode, answer_length_matches};
use dioxus::core::Runtime;
use dioxus::html::{ScrollBehavior, ScrollLogicalPosition, ScrollToOptions};
use dioxus::prelude::*;

use crate::time::sleep_ms;
use crate::ui::paddle::{PaddleKeys, paddle_hint};
use crate::ui::widgets::{
    CharacterComparison, Icon, IconName, ProgressHeader, control_id, html_bool,
};

/// Incomplete answers wait this long before the session sees them. Short
/// enough that a pause still lands well before auto-confirm would have cared,
/// long enough that a burst of keys is one Input, not one per character.
const INPUT_COMMIT_DEBOUNCE_MS: u32 = 48;

const _: () = assert!(INPUT_COMMIT_DEBOUNCE_MS < cw_core::AUTO_CONFIRM_DELAY_MS);

/// The progress header, kept out of the view that updates on every key.
#[component]
pub fn TrainingProgress(
    focused: usize,
    total: usize,
    playing: bool,
    repeat_total: u32,
    repeat_done: u32,
) -> Element {
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let status = if playing && repeat_total > 1 {
        format!("Sending {send_index} of {repeat_total}")
    } else if playing {
        "Sending".to_string()
    } else {
        "Your turn".to_string()
    };
    rsx! {
        div { style: "display: contents;",
            ProgressHeader { current: focused, total, status, live: playing }
        }
    }
}

/// The answer being typed into the open box, and what the session has heard
/// of it.
///
/// Typing stays in the box. The session hears it when its length matches the
/// group, when it has sat still for the debounce, or when the box is left —
/// Enter, a click anywhere else, End session — and each of those goes through
/// [`AnswerDraft::commit`], so "tell the session, unless it has heard this
/// already" is written once.
#[derive(Clone, Copy, PartialEq)]
struct AnswerDraft {
    /// The text in the box. Only that box reads it, so a key redraws one card.
    text: Signal<String>,
    /// The group `text` is the answer to.
    at: CopyValue<Option<usize>>,
    /// What the session last heard, and for which group.
    heard: CopyValue<Option<(usize, String)>>,
    /// Moved on by every commit and every new group: a debounce that finds
    /// it moved stands down.
    epoch: CopyValue<u64>,
    /// The view that owns the debounce, so one still waiting ends with it,
    /// whoever started it — a key in a card, or the paddles.
    owner: ScopeId,
    on_change: EventHandler<(usize, String)>,
}

impl AnswerDraft {
    fn use_new(on_change: EventHandler<(usize, String)>) -> Self {
        use_hook(|| Self {
            text: Signal::new(String::new()),
            at: CopyValue::new(None),
            heard: CopyValue::new(None),
            epoch: CopyValue::new(0),
            owner: dioxus::core::current_scope_id(),
            on_change,
        })
    }

    /// What the box for `idx` shows while the draft is its own. Reading it
    /// subscribes the caller to every key — before the first one too, which
    /// is what makes the draft this box's.
    fn read_for(self, idx: usize) -> Option<String> {
        let text = self.text.read();
        (*self.at.peek() == Some(idx)).then(|| text.clone())
    }

    fn peek_for(self, idx: usize) -> Option<String> {
        (*self.at.peek() == Some(idx)).then(|| self.text.peek().clone())
    }

    /// The box for `idx` now holds `text`. The session hears it at once when
    /// its length matches the group's, or matched what it heard last — a box
    /// backed off a full answer must stop auto-confirm straight away — and
    /// otherwise once typing has paused.
    fn edit(mut self, idx: usize, text: String, sent: &str) {
        let full_before = matches!(
            &*self.heard.peek(),
            Some((at, heard)) if *at == idx && answer_length_matches(sent, heard)
        );
        let full_now = answer_length_matches(sent, &text);
        self.at.set(Some(idx));
        self.text.set(text);
        if full_before || full_now {
            self.commit(idx);
            return;
        }
        let epoch = self.cancel_pending();
        Runtime::current().spawn(self.owner, async move {
            sleep_ms(INPUT_COMMIT_DEBOUNCE_MS).await;
            if *self.epoch.peek() == epoch {
                self.commit(idx);
            }
        });
    }

    /// A paddle keyed `letter` into the box for `idx`.
    fn push(self, idx: usize, letter: char, sent: &str) {
        let mut text = self.peek_for(idx).unwrap_or_default();
        text.push(letter);
        self.edit(idx, text, sent);
    }

    /// Tell the session what the box for `idx` holds now, unless it has heard
    /// exactly that already. A debounce still waiting has nothing left to do.
    fn commit(mut self, idx: usize) {
        self.cancel_pending();
        let Some(text) = self.peek_for(idx) else {
            return;
        };
        if matches!(&*self.heard.peek(), Some((at, heard)) if *at == idx && *heard == text) {
            return;
        }
        self.heard.set(Some((idx, text.clone())));
        self.on_change.call((idx, text));
    }

    /// A new group is up, or this one is over: nothing waiting goes out, and
    /// a finished group's draft is gone.
    fn settle(mut self, finished: bool) {
        self.cancel_pending();
        if finished {
            self.at.set(None);
            self.text.set(String::new());
        }
    }

    fn cancel_pending(mut self) -> u64 {
        let next = self.epoch.peek().wrapping_add(1);
        self.epoch.set(next);
        next
    }
}

/// One group: what was sent, the box you answer it in, and the comparison
/// once it is confirmed.
///
/// Its own component for one reason, and it is the whole reason: a key lands
/// in one box, and the twenty other cards on the screen have not changed. As
/// an inline loop body every keystroke rebuilt and re-diffed all of them —
/// twenty cards of roughly twenty nodes each, over the wire to the webview,
/// between the key going down and the letter appearing. Given props to compare
/// instead, the nineteen that did not move are skipped. That holds only while
/// every prop is stable: a handler built afresh in the parent's body is a new
/// prop on every render, and redraws them all.
///
/// The active card may read the draft. The others must not: a read would
/// subscribe every card to every keystroke and quietly undo the split.
#[component]
#[allow(clippy::too_many_arguments)]
fn GroupCard(
    idx: usize,
    sent: String,
    /// What the session holds for this group.
    value: String,
    disabled: bool,
    input_locked: bool,
    is_focused: bool,
    is_active: bool,
    is_confirmed: bool,
    send_badge: Option<String>,
    placeholder: &'static str,
    draft: AnswerDraft,
    on_confirm: EventHandler<()>,
) -> Element {
    let cls = if is_focused {
        "group focused"
    } else if is_confirmed {
        "group done"
    } else {
        "group"
    };
    let input_cls = if input_locked {
        "answer locked"
    } else {
        "answer"
    };
    let live = is_active && !input_locked;
    let closed = disabled || input_locked;
    let shown = if is_active {
        draft.read_for(idx).unwrap_or_else(|| value.clone())
    } else {
        value.clone()
    };
    let correct = value.trim().eq_ignore_ascii_case(&sent);
    let sent_for_input = sent.clone();
    rsx! {
        div { id: "group-card-{idx}", class: cls,
            div { class: "row-between",
                div { class: "row", style: "gap: 0.35rem;",
                    span { class: if is_focused { "badge current" } else { "badge" }, "{idx + 1}" }
                    if let Some(badge) = send_badge {
                        span { class: "badge current", "{badge}" }
                    }
                }
                div { class: "row", style: "gap: 0.4rem;",
                    if is_confirmed {
                        span { class: "sent", "{sent}" }
                        span { class: if correct { "badge good" } else { "badge bad" },
                            Icon { name: if correct { IconName::Check } else { IconName::X } }
                        }
                    } else {
                        span { class: "sent", "•••" }
                    }
                }
            }
            input {
                id: "group-input-{idx}",
                class: input_cls,
                aria_label: "Answer for group {idx + 1}",
                value: "{shown}",
                disabled: html_bool(disabled),
                readonly: html_bool(input_locked || is_confirmed),
                placeholder,
                autocomplete: "off",
                autocorrect: "off",
                autocapitalize: "characters",
                spellcheck: false,
                enterkeyhint: "done",
                inputmode: "text",
                lang: "zxx",
                // The card is keyed on whether its box is live, so a box that
                // opens is a new element, and mounting is when it takes the
                // focus — the one place that does.
                onmounted: move |evt| {
                    if live {
                        spawn(async move {
                            let element = evt.data();
                            // Centred, and at once: a smooth scroll is an
                            // animation running for a few hundred
                            // milliseconds just as the group opens and you
                            // start typing, and the browser spends it laying
                            // out rather than echoing your keys.
                            let _ = element
                                .scroll_to_with_options(ScrollToOptions {
                                    behavior: ScrollBehavior::Instant,
                                    vertical: ScrollLogicalPosition::Center,
                                    horizontal: ScrollLogicalPosition::Nearest,
                                })
                                .await;
                            let _ = element.set_focus(true).await;
                        });
                    }
                },
                onblur: move |_| draft.commit(idx),
                oninput: move |e| {
                    if !closed {
                        draft.edit(idx, e.value(), &sent_for_input);
                    }
                },
                // `[` and `]` are paddles: the app's root keys them and keeps
                // them out of the box.
                onkeydown: move |e| {
                    if closed {
                        e.prevent_default();
                        return;
                    }
                    if e.key() == Key::Enter {
                        draft.commit(idx);
                        on_confirm.call(());
                    }
                },
            }
            if is_confirmed {
                div { style: "margin-top: 0.5rem;",
                    CharacterComparison {
                        sent,
                        received: value.trim().to_ascii_uppercase(),
                    }
                }
            }
        }
    }
}

/// The group list and the way out of the session.
///
/// The answer box is local: [`AnswerDraft`] says when the session hears it.
/// Typing itself does not clone the session.
#[component]
pub fn TrainingView(
    current: usize,
    groups: Vec<String>,
    inputs: Vec<String>,
    confirmed: Vec<bool>,
    focused: usize,
    playing: bool,
    locked: bool,
    repeat_total: u32,
    repeat_done: u32,
    on_change: EventHandler<(usize, String)>,
    on_confirm: EventHandler<()>,
    on_submit: EventHandler<()>,
    on_stop: EventHandler<()>,
    #[props(default)] keyer_mode: KeyerMode,
) -> Element {
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let draft = AnswerDraft::use_new(on_change);
    let current_done = confirmed.get(current).copied().unwrap_or(false);
    let can_paddle = !playing && !locked && !current_done;
    let sent_now = groups.get(current).cloned().unwrap_or_default();
    // The paddles key into the box that is open, and only while it is.
    let keys = use_context::<PaddleKeys>();
    keys.use_answer_box(can_paddle, move |letter: char| {
        draft.push(current, letter, &sent_now);
    });
    use_effect(use_reactive!(|current, current_done| {
        let _ = current;
        draft.settle(current_done);
        // A letter half keyed for one group is not the next one's.
        keys.stop();
    }));
    let hint = if playing && locked {
        "Listen — the answer box unlocks when the group finishes.".to_string()
    } else if playing {
        "Listen, or type along — the box stays open while this group is sent.".to_string()
    } else {
        format!("Type the letters, or key them. {}", paddle_hint(keyer_mode))
    };
    rsx! {
        div { style: "display: contents;",
            div { class: "card",
                div { class: "row-between",
                    p { class: "muted", style: "margin: 0;", "{hint}" }
                    if repeat_total > 1 {
                        span { class: "chip",
                            Icon { name: IconName::Repeat }
                            "{repeat_total}× per group"
                        }
                    }
                }
                div { class: "group-list", style: "margin-top: 0.85rem;",
                    for (idx, sent) in groups.iter().enumerate() {
                        {
                            let is_focused = focused == idx;
                            let is_confirmed = confirmed.get(idx).copied().unwrap_or(false);
                            let is_active = current == idx && !is_confirmed;
                            let awaiting_play = is_focused && !is_active && !is_confirmed;
                            let disabled = is_confirmed || (!is_active && !awaiting_play);
                            let input_locked =
                                (locked && is_active && !is_confirmed) || awaiting_play;
                            let live = is_active && !input_locked;
                            let placeholder = if awaiting_play {
                                "Waiting…"
                            } else if input_locked {
                                "Listening…"
                            } else if disabled {
                                "Waiting…"
                            } else {
                                "Type the group"
                            };
                            rsx! {
                                GroupCard {
                                    key: "{idx}-{live}",
                                    idx,
                                    sent: sent.clone(),
                                    value: inputs.get(idx).cloned().unwrap_or_default(),
                                    disabled,
                                    input_locked,
                                    is_focused,
                                    is_active,
                                    is_confirmed,
                                    send_badge: (is_focused && playing && repeat_total > 1)
                                        .then(|| format!("Send {send_index}/{repeat_total}")),
                                    placeholder,
                                    draft,
                                    on_confirm,
                                }
                            }
                        }
                    }
                }
            }
            div { class: "train-actions",
                button {
                    id: control_id("btn", "end session"),
                    class: "btn btn-primary",
                    onclick: move |_| {
                        draft.commit(current);
                        on_submit.call(());
                    },
                    Icon { name: IconName::Flag }
                    "End session"
                }
                button {
                    id: control_id("btn", "discard"),
                    class: "btn btn-danger",
                    onclick: move |_| on_stop.call(()),
                    Icon { name: IconName::X }
                    "Discard"
                }
            }
        }
    }
}
