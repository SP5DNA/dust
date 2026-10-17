use cw_core::answer_length_matches;
use dioxus::core::Runtime;
use dioxus::prelude::*;

use crate::session_runtime::AnswerOutbox;
use crate::time::sleep_ms;
use crate::ui::paddle::PaddleKeys;
use crate::ui::widgets::{
    CharacterComparison, Icon, IconName, ProgressHeader, control_id, html_bool,
};

/// Incomplete answers wait this long before the session sees them. Short
/// enough that a pause still lands well before auto-confirm would have cared,
/// long enough that a burst of keys is one Input, not one per character.
pub(crate) const INPUT_COMMIT_DEBOUNCE_MS: u32 = 48;

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

/// The answer being typed into the open box.
///
/// Typing stays in the box, and waits in the [`AnswerOutbox`]. The session is
/// handed it when its length matches the group, or once it has sat still for
/// the debounce — or, by the runtime, the moment anything else happens to the
/// session first: Enter, End session, Exit, the answer window closing.
///
/// A key redraws nothing. The box is a controlled input, and drawing it writes
/// its `value` again whether or not it changed; on the desktop and mobile
/// builds that write reaches the page a frame later, over whatever was typed
/// in the meantime, and the key typed in between is lost for good. What is
/// typed is in the box already, so while it is open the box is drawn only for
/// what it has not seen: a letter the paddles keyed. Its card is drawn without
/// it ([`AnswerBox`]), and a group that is over is drawn from what the session
/// holds.
#[derive(Clone, Copy, PartialEq)]
struct AnswerDraft {
    /// The text in the box. Not reactive: a key is in the box before it is
    /// here, and drawing the box for it would only write it back.
    text: CopyValue<String>,
    /// Moved on when `text` changes in a way the box has not seen, which
    /// draws the box again. Only the open box reads it, so it redraws that
    /// box alone.
    repaint: Signal<u64>,
    /// The group `text` is the answer to.
    at: CopyValue<Option<usize>>,
    outbox: AnswerOutbox,
    /// Moved on by every key and every new group: a debounce that finds it
    /// moved stands down.
    epoch: CopyValue<u64>,
    /// The view the debounce belongs to, so one still waiting ends with it,
    /// whoever started it — a key in a card, or the paddles.
    owner: ScopeId,
    on_change: EventHandler<(usize, String)>,
}

impl AnswerDraft {
    /// A draft that starts out as `held`: the group and answer the box opens
    /// on, if it opens on anything.
    fn use_new(
        outbox: AnswerOutbox,
        on_change: EventHandler<(usize, String)>,
        held: impl FnOnce() -> Option<(usize, String)>,
    ) -> Self {
        use_hook(|| {
            let (at, text) = held().map_or((None, String::new()), |(idx, text)| (Some(idx), text));
            Self {
                text: CopyValue::new(text),
                repaint: Signal::new(0),
                at: CopyValue::new(at),
                outbox,
                epoch: CopyValue::new(0),
                owner: dioxus::core::current_scope_id(),
                on_change,
            }
        })
    }

    /// What the box for `idx` shows while the draft is its own. Reading it
    /// subscribes the caller to the draft's rewrites — before the first one
    /// too, since a paddle letter is what can make the draft this box's.
    fn read_for(self, idx: usize) -> Option<String> {
        let _ = self.repaint.read();
        self.peek_for(idx)
    }

    fn peek_for(self, idx: usize) -> Option<String> {
        (*self.at.peek() == Some(idx)).then(|| self.text.peek().clone())
    }

    /// The box for `idx` now holds `text`, so nothing is drawn for it. The
    /// session is handed it at once when its length matches the group's —
    /// auto-confirm starts from there — and otherwise once typing has paused,
    /// or sooner if anything happens to the session first.
    fn edit(mut self, idx: usize, text: String, sent: &str) {
        let full = answer_length_matches(sent, &text);
        self.at.set(Some(idx));
        self.text.set(text.clone());
        self.outbox.hold(idx, text);
        let epoch = self.cancel_pending();
        if full {
            self.hand_over();
            return;
        }
        // Spawned from inside the view's own scope, which is what ties the
        // pause to it: a session ending on the heels of a key takes the view
        // away, and the pause goes with it rather than waking to a draft that
        // is gone. (`Runtime::spawn` on a scope id does not.)
        Runtime::current().in_scope(self.owner, || {
            spawn(async move {
                sleep_ms(INPUT_COMMIT_DEBOUNCE_MS).await;
                if *self.epoch.peek() == epoch {
                    self.hand_over();
                }
            })
        });
    }

    /// A paddle keyed `letter` into the box for `idx`. The page has not seen
    /// it, so the box is drawn again to show it.
    fn push(self, idx: usize, letter: char, sent: &str) {
        let mut text = self.peek_for(idx).unwrap_or_default();
        text.push(letter);
        self.edit(idx, text, sent);
        self.redraw();
    }

    /// Hand the session whatever is waiting. A debounce still waiting has
    /// nothing left to do.
    fn hand_over(self) {
        self.cancel_pending();
        if let Some(answer) = self.outbox.take() {
            self.on_change.call(answer);
        }
    }

    /// A new group is up, or this one is over: the pause has nothing left to
    /// wait for, and a finished group's draft is gone. Whatever was typed is
    /// with the session already — only the session moves a group on, and it
    /// takes what is waiting first. Nothing is drawn for it: a finished
    /// group's box is closed, and shows what the session holds.
    fn settle(mut self, finished: bool) {
        self.cancel_pending();
        if finished {
            self.at.set(None);
            self.text.set(String::new());
        }
    }

    /// Draw the box showing the draft again, for a change it has not seen.
    fn redraw(mut self) {
        *self.repaint.write() += 1;
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
#[component]
#[allow(clippy::too_many_arguments)]
fn GroupCard(
    idx: usize,
    sent: String,
    /// What the session holds for this group. Blank for the active card,
    /// whose box shows the draft instead.
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
    let live = is_active && !input_locked;
    let correct = value.trim().eq_ignore_ascii_case(&sent);
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
            AnswerBox {
                idx,
                sent: sent.clone(),
                value: value.clone(),
                disabled,
                input_locked,
                is_active,
                is_confirmed,
                placeholder,
                draft,
                on_confirm,
            }
            // The card is keyed on whether its box is live, so a box that
            // opens is a new card, and this mounts with it: mounting is when
            // the box takes the focus — the one place that does. Only a live
            // card carries it, so the others cost no mount event.
            if live {
                span {
                    hidden: true,
                    onmounted: move |_| {
                        // Tests have no page to run it on.
                        #[cfg(not(test))]
                        spawn(async move {
                            let _ = document::eval(&focus_script(idx)).await;
                        });
                    },
                }
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

/// The box a group is answered in.
///
/// A component of its own so that its card can be drawn without it. Drawing
/// the box writes its `value` again, whether or not it changed, and on the
/// desktop and mobile builds that write reaches the page a frame later, over
/// whatever was typed in the meantime. The card is drawn again for things the
/// box has nothing to do with — the send counter, while a group repeats with
/// the box open — and given props to compare, the box is skipped then.
///
/// Only the open box reads the draft, and so only it is drawn for a paddle
/// letter. The others must not: a read would subscribe every box to every
/// letter and quietly undo the split.
#[component]
#[allow(clippy::too_many_arguments)]
fn AnswerBox(
    idx: usize,
    sent: String,
    /// What the session holds for this group. Blank for the open box, which
    /// shows the draft instead.
    value: String,
    disabled: bool,
    input_locked: bool,
    is_active: bool,
    is_confirmed: bool,
    placeholder: &'static str,
    draft: AnswerDraft,
    on_confirm: EventHandler<()>,
) -> Element {
    let class = if input_locked {
        "answer locked"
    } else {
        "answer"
    };
    let closed = disabled || input_locked;
    let shown = if is_active {
        draft.read_for(idx).unwrap_or_default()
    } else {
        value
    };
    rsx! {
        input {
            id: "group-input-{idx}",
            class,
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
            oninput: move |e| {
                if !closed {
                    draft.edit(idx, e.value(), &sent);
                }
            },
            // `[` and `]` are paddles: the app's root keys them and keeps
            // them out of the box.
            onkeydown: move |e| {
                if closed {
                    e.prevent_default();
                    return;
                }
                // What is typed reaches the session ahead of the confirm.
                if e.key() == Key::Enter {
                    on_confirm.call(());
                }
            },
        }
    }
}

/// The room after the last group, which [`focus_script`] sizes.
const GROUP_TAIL: &str = "group-tail";

/// The script that brings the box for `idx` into view and gives it the focus,
/// in one trip to the page.
///
/// Centred, so the group just answered stays in sight above it, and at once:
/// a smooth scroll is an animation running for a few hundred milliseconds just
/// as the group opens and you start typing, and the browser spends it laying
/// out rather than echoing your keys. The focus then must not scroll again.
/// The mounted element's own `scroll_to_with_options` will not do: the desktop
/// and mobile builds hand its alignment to `scrollIntoView` under names it
/// does not know, and the box ends up at the top edge of the list.
///
/// The last groups have no cards below them to scroll past, and stopped at
/// the foot of the list, half under a phone's keyboard. So a list that
/// scrolls ends in [`GROUP_TAIL`], half the list tall, and they come up to
/// the middle like the rest. It is sized here, from the list as it is on
/// screen, and only for a list that scrolls: one that fits keeps no gap.
///
/// Every build runs what it is given to eval as the body of a function, so the
/// script may return early.
fn focus_script(idx: usize) -> String {
    format!(
        r#"const card = document.getElementById("group-card-{idx}");
const input = document.getElementById("group-input-{idx}");
// A box that has closed again since it opened is left where it is.
if (!card || !input || input.disabled || input.readOnly) {{
    return;
}}
const tail = document.getElementById("{GROUP_TAIL}");
const list = tail && tail.parentElement;
if (list && list.scrollHeight > list.clientHeight) {{
    tail.style.height = list.clientHeight / 2 + "px";
}}
// An engine that does not know an instant scroll refuses the options whole;
// it can still centre, and the focus must come either way.
try {{
    card.scrollIntoView({{ block: "center", inline: "nearest", behavior: "instant" }});
}} catch (_) {{
    card.scrollIntoView({{ block: "center", inline: "nearest" }});
}}
input.focus({{ preventScroll: true }});"#
    )
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
    /// Where the box's text waits on its way to the session.
    answer: AnswerOutbox,
    on_change: EventHandler<(usize, String)>,
    on_confirm: EventHandler<()>,
    on_submit: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    let send_index = (repeat_done + 1).min(repeat_total.max(1));
    let current_done = confirmed.get(current).copied().unwrap_or(false);
    // The open box is never drawn from the session, so a view drawn afresh
    // on a session under way — after a hot reload, say — starts its draft
    // from whatever the session holds of the open answer.
    let draft = AnswerDraft::use_new(answer, on_change, || {
        let held = inputs.get(current).filter(|_| !current_done)?;
        (!held.is_empty()).then(|| (current, held.clone()))
    });
    let can_paddle = !playing && !locked && !current_done;
    let sent_now = groups.get(current).cloned().unwrap_or_default();
    // The paddles key into the box that is open, and only while it is.
    let keys = use_context::<PaddleKeys>();
    keys.use_answer_box(can_paddle, move |letter: char| {
        draft.push(current, letter, &sent_now);
    });
    // A letter keyed in full is in the answer when it closes, even with its
    // letter space still running out; and auto-confirm waits for a letter
    // still being keyed, as it would for a key still being typed.
    answer.use_entry(move || keys.finish_letter(), move || keys.letter_open());
    use_effect(use_reactive!(|current, current_done| {
        let _ = current;
        draft.settle(current_done);
        // A letter half keyed for one group is not the next one's.
        keys.stop();
    }));
    // One short line. How to key with the paddles is the settings screen's to
    // say, beside the paddles, not something to read with a group to copy.
    let hint = if playing && locked {
        "Listen — the answer box unlocks when the group finishes."
    } else if playing {
        "Listen, or type along — the box stays open while this group is sent."
    } else {
        "Type what you heard."
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
                                    // The open box shows the draft. What the
                                    // session has heard of it changes nothing
                                    // on screen, and passing it would draw the
                                    // box each time the session hears — and
                                    // write it back over the typing.
                                    value: if is_active {
                                        String::new()
                                    } else {
                                        inputs.get(idx).cloned().unwrap_or_default()
                                    },
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
                    div { id: GROUP_TAIL, class: "group-tail", aria_hidden: "true" }
                }
            }
            div { class: "train-actions",
                button {
                    id: control_id("btn", "end session"),
                    class: "btn btn-primary",
                    onclick: move |_| on_submit.call(()),
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

#[cfg(test)]
mod tests {
    use super::{GROUP_TAIL, focus_script};
    use crate::testing::{Ui, run, test_settings};

    /// The harness never mounts anything on a page, so the script is the one
    /// part of the box taking the focus a test can see.
    #[test]
    fn the_opened_box_is_centred_and_focused_in_one_script() {
        let script = focus_script(7);
        assert!(
            script.contains(r#"getElementById("group-card-7")"#),
            "{script}"
        );
        assert!(
            script.contains(r#"getElementById("group-input-7")"#),
            "{script}"
        );
        // `block`, not the `vertical` the desktop builds send for a mounted
        // element's scroll: `scrollIntoView` ignores that, and top-aligns.
        // Centred on both ways in, and never anywhere else.
        let primary =
            r#"card.scrollIntoView({ block: "center", inline: "nearest", behavior: "instant" });"#;
        let fallback = r#"card.scrollIntoView({ block: "center", inline: "nearest" });"#;
        assert!(script.contains(primary), "{script}");
        assert!(script.contains(fallback), "{script}");
        assert_eq!(script.matches("block:").count(), 2, "{script}");
        assert!(!script.contains("vertical"), "{script}");
        let scroll = script.find(primary).expect("the card is scrolled");
        let focus = script
            .find("input.focus({ preventScroll: true })")
            .expect("the box takes the focus without scrolling again");
        assert!(scroll < focus, "centred first, then focused: {script}");
        // A box closed again by the time the script runs keeps its place.
        let guard = script
            .find("input.disabled || input.readOnly")
            .expect("a closed box is left alone");
        assert!(guard < scroll, "{script}");
        // The room after the last group is made before the card is centred,
        // and only in a list that scrolls.
        assert!(
            script.contains(&format!(r#"getElementById("{GROUP_TAIL}")"#)),
            "{script}"
        );
        let scrolls = script
            .find("list.scrollHeight > list.clientHeight")
            .expect("a list that fits keeps no gap");
        let room = script
            .find(r#"tail.style.height = list.clientHeight / 2 + "px";"#)
            .expect("the room is half the list");
        assert!(
            guard < scrolls && scrolls < room && room < scroll,
            "{script}"
        );
    }

    /// The room the script sizes is the list's last child: after every
    /// group, and before the session's buttons.
    #[test]
    fn the_group_list_ends_in_room_for_its_last_group() {
        run(|| async {
            let mut settings = test_settings();
            settings.curriculum.num_groups = 3;
            let (mut ui, _recorder) = Ui::app_with_settings(settings);
            ui.click("btn-start-training");
            let html = ui.html();
            let at = |needle: &str| {
                html.find(needle)
                    .unwrap_or_else(|| panic!("no {needle} in {html}"))
            };
            let id = format!(r#"id="{GROUP_TAIL}""#);
            let tail = at(&id);
            assert!(at(r#"id="group-card-2""#) < tail, "{html}");
            assert!(tail < at(r#"id="btn-end-session""#), "{html}");
            assert_eq!(html.matches(&id).count(), 1, "{html}");
        });
    }
}
