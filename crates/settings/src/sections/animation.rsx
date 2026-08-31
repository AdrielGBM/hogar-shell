[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{CURVES, EASINGS, parse_f32, parse_u64, persist, source};
use ::config::AnimationConfig;

let (config, path) = source();
let a = &config.animation;
let base = a.clone();
let enabled = signal(a.enabled);
let scale = signal(a.duration_scale.to_string());
let curve = signal(a.curve.clone());
let easing = signal(a.easing.clone());
let panel_ms = signal(a.panel_duration_ms.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, scale, curve) = (enabled.clone(), scale.clone(), curve.clone());
    let (easing, panel_ms) = (easing.clone(), panel_ms.clone());
    move || {
        let value = AnimationConfig {
            enabled: enabled.peek(),
            duration_scale: parse_f32(&scale.peek(), base.duration_scale),
            curve: curve.peek(),
            easing: easing.peek(),
            panel_duration_ms: parse_u64(&panel_ms.peek(), base.panel_duration_ms),
        };
        persist(&path, "animation", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.animation")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    text_row label:(Reactive::of(|| telar::t!("settings.field.duration_scale"))) value:$scale placeholder:"1"
    enum_row label:(Reactive::of(|| telar::t!("settings.field.curve"))) value:$curve options:CURVES
    enum_row label:(Reactive::of(|| telar::t!("settings.field.easing"))) value:$easing options:EASINGS
    text_row label:(Reactive::of(|| telar::t!("settings.field.panel_duration_ms"))) value:$panel_ms placeholder:"180"
    save_row label:(Reactive::of(|| telar::t!("settings.save.animation"))) on_press:save
