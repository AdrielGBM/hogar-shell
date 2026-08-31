[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_i32, persist, source};
use ::config::BrightnessConfig;

let (config, path) = source();
let b = config.brightness;
let increment = signal(b.increment.to_string());
let external = signal(b.external);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (increment, external) = (increment.clone(), external.clone());
    move || {
        let value = BrightnessConfig {
            increment: parse_i32(&increment.peek(), b.increment),
            external: external.peek(),
        };
        persist(&path, "brightness", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.brightness")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.increment"))) value:$increment placeholder:"5"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.external_monitors"))) value:$external
    save_row label:(Reactive::of(|| telar::t!("settings.save.brightness"))) on_press:save
