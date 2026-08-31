[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_f32, parse_u64, persist, source};
use ::config::PopoutsConfig;

let (config, path) = source();
let p = config.popouts;
let enabled = signal(p.enabled);
let open_delay = signal(p.open_delay.to_string());
let close_delay = signal(p.close_delay.to_string());
let width = signal(p.width.to_string());
let max_height = signal(p.max_height.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, open_delay, close_delay) =
        (enabled.clone(), open_delay.clone(), close_delay.clone());
    let (width, max_height) = (width.clone(), max_height.clone());
    move || {
        let value = PopoutsConfig {
            enabled: enabled.peek(),
            open_delay: parse_u64(&open_delay.peek(), p.open_delay),
            close_delay: parse_u64(&close_delay.peek(), p.close_delay),
            width: parse_f32(&width.peek(), p.width),
            max_height: parse_f32(&max_height.peek(), p.max_height),
        };
        persist(&path, "popouts", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.popouts")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    text_row label:(Reactive::of(|| telar::t!("settings.field.open_delay"))) value:$open_delay placeholder:"280"
    text_row label:(Reactive::of(|| telar::t!("settings.field.close_delay"))) value:$close_delay placeholder:"200"
    text_row label:(Reactive::of(|| telar::t!("settings.field.width"))) value:$width placeholder:"264"
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_height"))) value:$max_height placeholder:"300"
    save_row label:(Reactive::of(|| telar::t!("settings.save.popouts"))) on_press:save
