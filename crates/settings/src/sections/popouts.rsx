[logic]
use crate::form_section::{form_section, FormSectionProps};
use ::ui::form::toggle_row::{toggle_row, ToggleRowProps};
use ::ui::form::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_u64, persist, source};
use ::config::PopoutsConfig;

let (config, path) = source();
let p = config.popouts;
let enabled = signal(p.enabled);
let open_delay = signal(p.open_delay.to_string());
let close_delay = signal(p.close_delay.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, open_delay, close_delay) =
        (enabled.clone(), open_delay.clone(), close_delay.clone());
    move || {
        let value = PopoutsConfig {
            enabled: enabled.peek(),
            open_delay: parse_u64(&open_delay.peek(), p.open_delay),
            close_delay: parse_u64(&close_delay.peek(), p.close_delay),
        };
        persist(&path, "popouts", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.popouts")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    text_row label:(Reactive::of(|| telar::t!("settings.field.open_delay"))) value:$open_delay placeholder:"280"
    text_row label:(Reactive::of(|| telar::t!("settings.field.close_delay"))) value:$close_delay placeholder:"200"
    save_row label:(Reactive::of(|| telar::t!("settings.save.popouts"))) on_press:save
