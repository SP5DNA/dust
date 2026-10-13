//! The settings screen in the whole app: what a learner types and clicks,
//! and what the screen holds afterwards.
//!
//! These read the page rather than the store, so they say what the learner
//! sees; the store is written from the same settings the page shows.

use crate::testing::{Ui, run, test_settings};

/// The value an input is showing.
fn value_of(ui: &Ui, id: &str) -> String {
    let tag = ui.input_tag(id);
    tag.split("value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("{id} shows no value: {tag}"))
        .to_string()
}

/// A number is typed through its prefixes. Retyping the top of a range from
/// 25 to 30 passes through 3, and that must not drag the bottom down with it.
#[test]
fn retyping_one_end_of_a_range_leaves_the_other_alone() {
    run(|| async {
        let mut settings = test_settings();
        settings.playback.char_wpm_min = 20.0;
        settings.playback.char_wpm_max = 25.0;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.type_into("to-character-speed", "3");
        ui.type_into("to-character-speed", "30");
        ui.commit("to-character-speed", "30");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "from-character-speed"), "20");
        assert_eq!(value_of(&ui, "to-character-speed"), "30");
        assert!(ui.has("20–30 WPM"), "{}", ui.html());
    });
}

/// The side tone has no stored link: a range whose ends meet is a fixed
/// pitch. The first digit of a new top used to make them meet, which swapped
/// the two boxes for one and took away the box being typed in.
#[test]
fn typing_into_one_end_of_the_side_tone_keeps_its_box() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.type_into("to-side-tone", "4");
        assert!(ui.shows("to-side-tone"), "the box being typed in went away");
        assert!(!ui.shows("fixed-side-tone"));
        ui.type_into("to-side-tone", "700");
        ui.commit("to-side-tone", "700");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "from-side-tone"), "400");
        assert_eq!(value_of(&ui, "to-side-tone"), "700");
    });
}

/// A number committed past what the field allows comes back as the most it
/// allows, even when that is what the setting already was. Until then the
/// 999 is the box's own: nothing is written over it while it is typed.
#[test]
fn a_number_committed_out_of_bounds_shows_where_it_landed() {
    run(|| async {
        let mut settings = test_settings();
        settings.playback.group_pause_sec = cw_core::GROUP_PAUSE_MAX_SEC;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        let field = "number-pause-after-each-group";
        let _ = ui.take_work();
        ui.type_into(field, "999");
        assert_eq!(ui.take_work(), 0, "typing wrote over the box");
        ui.commit(field, "999");
        // The setting did not move, so only the field itself can write the
        // most it allows back over the 999.
        assert!(ui.take_work() > 0, "the commit left 999 in the box");
        assert_eq!(
            value_of(&ui, field),
            cw_core::GROUP_PAUSE_MAX_SEC.to_string()
        );
        ui.click("less-number-pause-after-each-group");
        assert_eq!(
            value_of(&ui, field),
            (cw_core::GROUP_PAUSE_MAX_SEC - 0.5).to_string()
        );
    });
}

/// A number typed faster than the page draws is committed whole.
///
/// On the desktop and phone builds a redraw reaches the box a frame after it
/// was made, and it sends the box its `value` whether or not that changed. A
/// field that redrew for every key wrote each key's text back over whatever
/// had been typed since, and a quick 120 came out as 10. The harness draws
/// at once, so this plays the webview's part, at its worst: every redraw
/// lands a key late.
#[test]
fn a_number_typed_faster_than_the_page_draws_is_committed_whole() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        let _ = ui.take_work();
        // What the box on screen holds, and a redraw still on its way to it.
        let mut screen = String::new();
        let mut on_its_way: Option<String> = None;
        for key in ['1', '2', '0'] {
            screen.push(key);
            ui.type_into(field, &screen);
            if let Some(late) = on_its_way.take() {
                screen = late;
            }
            if ui.take_work() > 0 {
                on_its_way = Some(value_of(&ui, field));
            }
        }
        if let Some(late) = on_its_way {
            screen = late;
        }
        assert_eq!(screen, "120");
        assert_eq!(
            ui.stored_settings().await.curriculum.num_groups,
            2,
            "nothing is taken before it is committed"
        );
        ui.commit(field, &screen);
        assert_eq!(value_of(&ui, field), "120");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 120);
    });
}

/// − and + move the box as well as the setting, before anything is typed
/// and after.
#[test]
fn the_step_buttons_move_the_number_in_the_box() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        ui.click("more-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "3");
        ui.type_into(field, "12");
        ui.commit(field, "12");
        assert_eq!(value_of(&ui, field), "12");
        ui.click("more-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "13");
        ui.click("less-number-groups-per-session");
        ui.click("less-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "11");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 11);
    });
}

/// One click to look at another character set must not cost the level the
/// learner had reached in this one.
#[test]
fn trying_another_character_set_costs_no_level() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.level = 38;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.click("seg-mixed");
        ui.click("seg-koch");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "field-level"), "38");

        let mut settings = test_settings();
        settings.curriculum.level = 20;
        settings.curriculum.custom_set = vec!['A', 'B', 'C'];
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.click("seg-custom");
        assert_eq!(value_of(&ui, "field-level"), "2");
        ui.click("seg-koch");
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "field-level"), "20");
        ui.click("nav-practice");
        assert!(ui.has("Level 20"), "{}", ui.html());
    });
}

/// The unlock order can be emptied and typed out again. It is only read
/// once it is committed, so an empty box is not refilled with LCWO's order
/// under the cursor, and typing LCWO's order back is LCWO again.
#[test]
fn the_unlock_order_can_be_cleared_and_typed_again() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.level = 20;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.type_into("field-sequence-order", "");
        assert_eq!(value_of(&ui, "field-sequence-order"), "");
        ui.type_into("field-sequence-order", "KM");
        assert_eq!(value_of(&ui, "field-sequence-order"), "KM");
        assert_eq!(value_of(&ui, "field-level"), "20", "nothing is read yet");

        let lcwo: String = cw_core::SEQUENCE_PRESETS[0].sequence.iter().collect();
        ui.type_into("field-sequence-order", &lcwo);
        ui.commit("field-sequence-order", &lcwo);
        ui.advance(50).await;
        assert_eq!(value_of(&ui, "field-sequence-order"), lcwo);
        assert_eq!(value_of(&ui, "field-level"), "20");
        assert!(ui.input_tag("pill-lcwo").contains("active"));
        assert!(!ui.input_tag("pill-custom").contains("active"));

        // Committed with stray characters, it is cleaned once, on commit.
        ui.commit("field-sequence-order", "km k?");
        assert_eq!(value_of(&ui, "field-sequence-order"), "KM?");
        assert!(ui.input_tag("pill-custom").contains("active"));
    });
}

#[test]
fn a_custom_alphabet_is_cleaned_when_it_is_committed() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.char_set_mode = cw_core::CharSetMode::Custom;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        ui.type_into("field-custom-alphabet", "abca");
        assert_eq!(value_of(&ui, "field-custom-alphabet"), "abca");
        ui.commit("field-custom-alphabet", "abca");
        assert_eq!(value_of(&ui, "field-custom-alphabet"), "ABC");
    });
}

/// Tap the dit paddle `times` times, a letter space apart: that many Es.
/// Keys go to the focused control and bubble up to the app's root.
async fn tap_dits(ui: &mut Ui, on: &str, times: usize) {
    use dioxus::prelude::{Code, Key};
    for _ in 0..times {
        ui.press_key(on, Key::Character("[".into()), Code::BracketLeft);
        ui.advance(30).await;
        ui.release_key(on, Key::Character("[".into()), Code::BracketLeft);
        ui.advance(250).await;
    }
}

/// The practice pad shows what the paddles key on Settings, and starts
/// clean whenever Settings is opened again.
#[test]
fn the_practice_pad_shows_what_was_keyed_until_settings_is_left() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        tap_dits(&mut ui, "seg-iambic-a", 2).await;
        assert!(ui.has(">EE<"), "{}", ui.html());
        ui.click("nav-practice");
        tap_dits(&mut ui, "btn-start-training", 1).await;
        ui.click("nav-settings");
        assert!(!ui.has(">EE<") && !ui.has(">E<"), "{}", ui.html());
        assert!(ui.has("· −"));
    });
}

/// A setting that moves redraws its own card, not the page. Every card used
/// to read the whole of the settings, so one slider tick redrew all of them
/// — about thirty mutations a tick, while being dragged.
#[test]
fn a_slider_tick_redraws_its_own_card_only() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.advance(50).await;
        for (control, most) in [
            ("more-slider-qrn-intensity", 10),
            ("more-number-key-speed", 6),
            ("more-slider-rise-time", 16),
        ] {
            let _ = ui.take_work();
            ui.click(control);
            let work = ui.take_work();
            assert!(
                work <= most,
                "{control} cost {work} mutations; its card alone is at most {most}"
            );
        }
    });
}
