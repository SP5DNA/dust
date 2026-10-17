//! The settings screen in the whole app: what a learner types and clicks,
//! and what the screen holds afterwards.
//!
//! These read the page rather than the store, so they say what the learner
//! sees; the store is written from the same settings the page shows.

use cw_core::{CharSetMode, Experience, GroupSizeTarget, RangeSetting, RangeValues};

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

/// − and + show the number they step to over text that was typed and never
/// committed.
///
/// A key typed straight after Enter can be written over by the commit's
/// redraw, a frame late. The box is then back as it was committed, so
/// leaving it commits nothing, but the field still holds the text with the
/// key in it. A step drew that text back over the new number: the box said
/// 405 while the setting had gone to 41.
#[test]
fn a_step_shows_the_number_stepped_to_over_text_never_committed() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        ui.type_into(field, "40");
        ui.commit(field, "40");
        // The 5, which the commit's redraw wrote over.
        ui.type_into(field, "405");
        ui.click("more-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "41");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 41);
        // A phone can keep the box focused through a tap on −, so what is
        // typed before it is never committed either.
        ui.type_into(field, "7");
        ui.click("less-number-groups-per-session");
        assert_eq!(value_of(&ui, field), "40");
        assert_eq!(ui.stored_settings().await.curriculum.num_groups, 40);
    });
}

/// A step up to the field's top shows the top, not the text typed past it.
#[test]
fn a_step_to_the_top_shows_the_top_over_text_never_committed() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.num_groups = cw_core::NUM_GROUPS_MAX - 1;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        let field = "number-groups-per-session";
        let more = "more-number-groups-per-session";
        ui.type_into(field, "1995");
        ui.click(more);
        assert_eq!(value_of(&ui, field), cw_core::NUM_GROUPS_MAX.to_string());
        assert!(
            ui.input_tag(more).contains("disabled"),
            "{}",
            ui.input_tag(more)
        );
        assert_eq!(
            ui.stored_settings().await.curriculum.num_groups,
            cw_core::NUM_GROUPS_MAX
        );
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

/// Adaptive group size is off until asked for. What it aims for shows only
/// once it is on, both are stored, and a group size of one length says why
/// there is nothing to adapt.
#[test]
fn adaptive_group_size_is_opt_in_and_its_target_follows_the_switch() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.max_group_size = 6;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(ui.shows("switch-adapt-group-size-to-me"));
        assert!(ui.has("Your copy span is under Stats."));
        assert!(!ui.shows("seg-balanced"), "no target until it is on");

        ui.commit("switch-adapt-group-size-to-me", "true");
        assert!(ui.has("Balanced aims for 70% of groups copied whole."));
        ui.click("seg-stretch");
        assert!(ui.has("Stretch aims for 50% of groups copied whole."));
        let stored = ui.stored_settings().await;
        assert!(stored.curriculum.adaptive_group_size);
        assert_eq!(
            stored.curriculum.group_size_target,
            GroupSizeTarget::Stretch
        );

        ui.click("link-group-size");
        assert!(ui.has("one size leaves nothing to adapt"));
        assert!(!ui.shows("seg-stretch"));
    });
}

/// A callsign is as long as it is, so there is no size to adapt and no switch.
#[test]
fn callsigns_have_no_adaptive_group_size() {
    run(|| async {
        let mut settings = test_settings();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        let (mut ui, _recorder) = Ui::app_with_settings(settings);
        ui.click("nav-settings");
        assert!(!ui.shows("switch-adapt-group-size-to-me"));
    });
}

/// The experience dropdown's markup.
fn experience_picker(ui: &Ui) -> String {
    ui.html()
        .split("id=\"select-experience\"")
        .nth(1)
        .and_then(|rest| rest.split("</select>").next())
        .expect("the experience dropdown is on the page")
        .to_string()
}

/// The option the experience dropdown shows as chosen.
fn chosen_experience(ui: &Ui) -> String {
    experience_picker(ui)
        .split("<option")
        .find(|option| option.contains("selected"))
        .and_then(|option| option.split('>').nth(1))
        .and_then(|text| text.split('<').next())
        .unwrap_or_default()
        .to_string()
}

/// Choosing an experience sets the trainer up for it, through the same
/// controls the rest of the page shows: their values follow at once.
#[test]
fn choosing_an_experience_sets_the_trainer_up_for_it() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        assert_eq!(chosen_experience(&ui), "Custom");
        assert!(ui.has("Sets session length, group size, speed and level to start from."));

        ui.commit("select-experience", "intermediate");
        assert_eq!(chosen_experience(&ui), "Intermediate");
        assert!(
            !experience_picker(&ui).contains(">Custom<"),
            "Custom is offered over a preset"
        );
        assert!(
            ui.has("20 groups of 2–3 · 15–25 WPM · from level 8"),
            "{}",
            ui.html()
        );
        assert_eq!(value_of(&ui, "from-group-size"), "2");
        assert_eq!(value_of(&ui, "to-group-size"), "3");
        assert_eq!(value_of(&ui, "from-character-speed"), "15");
        assert_eq!(value_of(&ui, "to-character-speed"), "25");
        assert_eq!(value_of(&ui, "number-groups-per-session"), "20");
        assert_eq!(value_of(&ui, "field-level"), "8");
        let stored = ui.stored_settings().await;
        assert_eq!(Experience::of(&stored), Some(Experience::Intermediate));

        // A fixed size and speed show as one value each.
        ui.commit("select-experience", "novice");
        assert!(
            ui.has("20 groups of 2 · 15 WPM · from level 1"),
            "{}",
            ui.html()
        );
        assert_eq!(value_of(&ui, "fixed-group-size"), "2");
        assert_eq!(value_of(&ui, "fixed-character-speed"), "15");
        let stored = ui.stored_settings().await;
        assert_eq!(
            stored.range(RangeSetting::CharWpm),
            RangeValues {
                min: 15.0,
                max: 15.0,
                linked: true
            }
        );
    });
}

/// A change to anything a preset set makes the settings your own; the
/// level moving does not, as a preset only starts it.
#[test]
fn a_change_to_what_a_preset_set_shows_the_settings_as_your_own() {
    run(|| async {
        let (mut ui, _recorder) = Ui::app_with_settings(test_settings());
        ui.click("nav-settings");
        ui.commit("select-experience", "expert");
        assert_eq!(chosen_experience(&ui), "Expert");
        ui.commit("field-level", "20");
        assert_eq!(
            chosen_experience(&ui),
            "Expert",
            "the level took it off the preset"
        );
        ui.commit("number-groups-per-session", "30");
        assert_eq!(chosen_experience(&ui), "Custom");
        assert!(ui.has("Sets session length, group size, speed and level to start from."));
        // And a preset picked again puts it back.
        ui.commit("select-experience", "expert");
        assert_eq!(chosen_experience(&ui), "Expert");
        assert_eq!(value_of(&ui, "number-groups-per-session"), "20");
    });
}
