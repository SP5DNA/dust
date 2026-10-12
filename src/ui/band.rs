use cw_core::band::{FilterDesign, heard_snr_db};
use cw_core::{
    FILTER_WIDTH_MAX, FILTER_WIDTH_MIN, FilterShape, PILEUP_LEVEL_MAX_DB, PILEUP_LEVEL_MIN_DB,
    PILEUP_SPREAD_MAX, PILEUP_SPREAD_MIN, QSB_RATE_MAX_HZ, QSB_RATE_MIN_HZ, RangeSetting,
    STATIONS_MAX, STATIONS_MIN, SettingsSection, TrainingSettings,
};
use dioxus::prelude::*;

use crate::ui::widgets::{
    DISCLOSURE, Icon, LinkedRange, SectionReset, Seg, SliderField, Switch, control_id,
};

#[component]
pub fn BandConditionsCard(
    settings: Signal<TrainingSettings>,
    previewing: bool,
    on_preview: EventHandler<()>,
    on_stop: EventHandler<()>,
) -> Element {
    let s = settings();
    let mut show_help = use_signal(|| false);
    // The receiver as it is really built: the side-tone range can widen it
    // past the slider, and its shape decides how much noise it lets in.
    let receiver = FilterDesign::from_settings(&s);
    // As heard through the filter you have set, so narrowing it visibly buys
    // signal-to-noise — which is the whole reason to reach for it.
    let noise_reading = if s.band.noise_enabled && s.band.noise_level > 0.0 {
        format!(
            "S/N {:.0} dB",
            heard_snr_db(s.band.noise_level, receiver.noise_bandwidth_hz())
        )
    } else {
        "Off".to_string()
    };
    let widened = (receiver.width_hz() > s.band.filter_width_hz + 0.5).then(|| {
        format!(
            "{:.0} Hz — widened to {:.0} Hz to pass your pitch range",
            s.band.filter_width_hz,
            receiver.width_hz()
        )
    });
    rsx! {
        div { class: "card stack-sm",
            div { class: "card-head",
                div { class: "card-head-main",
                    span { class: "card-icon", Icon { name: "waves" } }
                    div {
                        h3 { class: "card-title", "Band conditions" }
                        p { class: "card-note", "Fading, static, noise and the receiver's filter" }
                    }
                }
                div { class: "card-tools",
                    if previewing {
                        button {
                            id: control_id("btn", "band stop"),
                            class: "btn btn-secondary btn-sm",
                            onclick: move |_| on_stop.call(()),
                            Icon { name: "stop" }
                            "Stop"
                        }
                    } else {
                        button {
                            id: control_id("btn", "band preview"),
                            class: "btn btn-primary btn-sm",
                            onclick: move |_| on_preview.call(()),
                            Icon { name: "play" }
                            "Live preview"
                        }
                    }
                    SectionReset {
                        label: "band conditions".to_string(),
                        on_reset: move |()| {
                            settings.write().reset_section(SettingsSection::BandConditions);
                        },
                    }
                    button {
                        id: control_id(DISCLOSURE, "band help"),
                        class: if show_help() { "icon-btn open" } else { "icon-btn" },
                        title: "What is this?",
                        aria_label: "What is this?",
                        onclick: move |_| show_help.set(!show_help()),
                        Icon { name: if show_help() { "x" } else { "target" } }
                    }
                }
            }
            if previewing {
                p { class: "muted", style: "margin: 0;",
                    "Looping “CQ” with the current mix. Move a slider and you hear it immediately."
                }
            }
            if show_help() {
                div { class: "tips",
                    span { class: "tips-mark", "QRx" }
                    p { class: "muted", style: "margin: 0;",
                        "Everything you hear passes through one CW filter, the way it does in a real receiver: the band noise is a steady hiss pitched at the filter, and static and every dit ring in it for a moment. QSB slowly fades the signal. QRN is lightning: the crackle of distant storms and the crash of a near one, with the receiver's AGC ducking the band behind it. Turn the noise up and the band reaches the signal."
                    }
                }
            }
            div { class: "field-grid",
                SliderField {
                    label: "Filter width (-6 dB)".to_string(),
                    id: "slider-filter-width".to_string(),
                    value_label: format!("{:.0} Hz", s.band.filter_width_hz),
                    value: s.band.filter_width_hz,
                    min: FILTER_WIDTH_MIN,
                    max: FILTER_WIDTH_MAX,
                    step: 10.0,
                    disabled: false,
                    onchange: move |v| settings.write().band.filter_width_hz = v,
                }
            }
            if let Some(widened) = widened {
                p { id: "filter-widened", class: "muted", style: "margin: 0;", "{widened}" }
            }
            div { class: "field",
                span { class: "field-label", "Filter shape" }
                div { class: "segmented",
                    Seg {
                        label: "Sharp".to_string(),
                        id: "seg-filter-sharp".to_string(),
                        active: s.band.filter_shape == FilterShape::Sharp,
                        onclick: move |_| settings.write().band.filter_shape = FilterShape::Sharp,
                    }
                    Seg {
                        label: "Soft".to_string(),
                        id: "seg-filter-soft".to_string(),
                        active: s.band.filter_shape == FilterShape::Soft,
                        onclick: move |_| settings.write().band.filter_shape = FilterShape::Soft,
                    }
                    Seg {
                        label: "Peaked".to_string(),
                        id: "seg-filter-peaked".to_string(),
                        active: s.band.filter_shape == FilterShape::Peaked,
                        onclick: move |_| settings.write().band.filter_shape = FilterShape::Peaked,
                    }
                }
            }
            p { class: "muted", style: "margin: 0;",
                "Everything you hear comes through this. Narrow it and less noise gets in and the filter rings longer, but a station off your pitch fades with it. Sharp: a modern DSP filter, flat with steep sides. Soft: rounded like an older crystal filter. Peaked: Sharp plus the rig's audio peak filter — a resonance at your pitch; stations off your pitch come out a few dB weaker."
            }
            LinkedRange {
                label: "Stations calling".to_string(),
                unit: "at once".to_string(),
                min_value: f64::from(s.band.stations_min),
                max_value: f64::from(s.band.stations_max),
                linked: s.band.stations_min == s.band.stations_max,
                min_bound: f64::from(STATIONS_MIN),
                max_bound: f64::from(STATIONS_MAX),
                step: 1.0,
                hint: "How many call at once, yours included, drawn afresh for each group. At 1 the band is just your station; above that the others call across it, and the band has its own traffic around it — never more than the rest of the count keying at once. Hold both ends together for the same number every time, or open them up so you never know what you are walking into.".to_string(),
                on_min: move |v| settings.write().set_range_min(RangeSetting::Stations, v),
                on_max: move |v| settings.write().set_range_max(RangeSetting::Stations, v),
                on_link: move |on| settings.write().set_range_linked(RangeSetting::Stations, on),
            }
            if s.band.stations_max > STATIONS_MIN {
                p { class: "muted", style: "margin: 0;",
                    "The others sit either side of the one you want, further out and a good way under it, and only ever where your filter passes them, out onto its skirts — copy the strongest, and narrow the filter until the rest drop away. A narrow filter leaves them little room, so it thins the pile-up: not everyone who calls gets through. Between groups the band carries on: other stations working each other at their own pitches and speeds, coming and going."
                }
                div { class: "field-grid",
                    SliderField {
                        label: "Pile-up spread".to_string(),
                        value_label: format!("±{:.0} Hz", s.band.pileup_spread_hz),
                        value: s.band.pileup_spread_hz,
                        min: PILEUP_SPREAD_MIN,
                        max: PILEUP_SPREAD_MAX,
                        step: 5.0,
                        disabled: false,
                        onchange: move |v| settings.write().band.pileup_spread_hz = v,
                    }
                    SliderField {
                        label: "Pile-up is weaker by".to_string(),
                        value_label: format!("{:.0} dB", s.band.pileup_level_db),
                        value: s.band.pileup_level_db,
                        min: PILEUP_LEVEL_MIN_DB,
                        max: PILEUP_LEVEL_MAX_DB,
                        step: 1.0,
                        disabled: false,
                        onchange: move |v| settings.write().band.pileup_level_db = v,
                    }
                }
            }
            Switch {
                title: "QSB fading".to_string(),
                description: "Slow gain swells on the Morse signal only.".to_string(),
                checked: s.band.qsb_enabled,
                onchange: move |on| settings.write().band.qsb_enabled = on,
            }
            div { class: "field-grid",
                SliderField {
                    label: "Depth".to_string(),
                    value_label: format!("{:.0}%", s.band.qsb_depth * 100.0),
                    value: s.band.qsb_depth,
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                    disabled: !s.band.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_depth = v,
                }
                SliderField {
                    label: "Rate".to_string(),
                    value_label: format!("{:.2} Hz", s.band.qsb_rate_hz),
                    value: s.band.qsb_rate_hz,
                    min: QSB_RATE_MIN_HZ,
                    max: QSB_RATE_MAX_HZ,
                    step: 0.01,
                    disabled: !s.band.qsb_enabled,
                    onchange: move |v| settings.write().band.qsb_rate_hz = v,
                }
            }
            Switch {
                title: "QRN static".to_string(),
                description: "Lightning: distant crackle and the crash of a near storm.".to_string(),
                checked: s.band.qrn_enabled,
                onchange: move |on| settings.write().band.qrn_enabled = on,
            }
            SliderField {
                label: "Intensity".to_string(),
                id: "slider-qrn-intensity".to_string(),
                value_label: format!("{:.0}%", s.band.qrn_level * 100.0),
                value: s.band.qrn_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !s.band.qrn_enabled,
                onchange: move |v| settings.write().band.qrn_level = v,
            }
            Switch {
                title: "Band noise".to_string(),
                description: "The steady hiss of the band, through your filter.".to_string(),
                checked: s.band.noise_enabled,
                onchange: move |on| settings.write().band.noise_enabled = on,
            }
            SliderField {
                label: "Level".to_string(),
                id: "slider-noise-level".to_string(),
                value_label: noise_reading,
                value: s.band.noise_level,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                disabled: !s.band.noise_enabled,
                onchange: move |v| settings.write().band.noise_level = v,
            }
            Switch {
                title: "AGC".to_string(),
                description: "The receiver rides its gain: the band ducks under a strong station and behind a crash.".to_string(),
                checked: s.band.agc_enabled,
                onchange: move |on| settings.write().band.agc_enabled = on,
            }
        }
    }
}
