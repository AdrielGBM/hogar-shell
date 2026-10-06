[logic]
use crate::form_section::{form_section, FormSectionProps};
use ::ui::form::toggle_row::{toggle_row, ToggleRowProps};
use ::ui::form::text_row::{text_row, TextRowProps};
use ::ui::form::enum_row::{enum_row, EnumRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{CURVES, EASINGS, REDUCED_MOTION, parse_f32, parse_u64, persist, source};
use ::config::{AnimationConfig, ReducedMotion};

let (config, path) = source();
let a = &config.animation;
let base = a.clone();
let enabled = signal(a.enabled);
let scale = signal(a.duration_scale.to_string());
let curve = signal(a.curve.clone());
let easing = signal(a.easing.clone());
let panel_ms = signal(a.panel_duration_ms.to_string());
let autohide_ms = signal(a.autohide_duration_ms.to_string());
let reduced = signal(a.reduced.id().to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, scale, curve) = (enabled.clone(), scale.clone(), curve.clone());
    let (easing, panel_ms) = (easing.clone(), panel_ms.clone());
    let (autohide_ms, reduced) = (autohide_ms.clone(), reduced.clone());
    move || {
        let value = AnimationConfig {
            enabled: enabled.peek(),
            duration_scale: parse_f32(&scale.peek(), base.duration_scale),
            curve: curve.peek(),
            easing: easing.peek(),
            panel_duration_ms: parse_u64(&panel_ms.peek(), base.panel_duration_ms),
            autohide_duration_ms: parse_u64(&autohide_ms.peek(), base.autohide_duration_ms),
            reduced: ReducedMotion::from_id(&reduced.peek()).unwrap_or(base.reduced),
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
    text_row label:(Reactive::of(|| telar::t!("settings.field.autohide_duration_ms"))) value:$autohide_ms placeholder:"160"
    enum_row label:(Reactive::of(|| telar::t!("settings.field.reduced_motion"))) value:$reduced options:REDUCED_MOTION
    save_row label:(Reactive::of(|| telar::t!("settings.save.animation"))) on_press:save
