[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_i32, persist, source};
use ::config::AudioConfig;

let (config, path) = source();
let a = config.audio;
let increment = signal(a.increment.to_string());
let max_volume = signal(a.max_volume.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (increment, max_volume) = (increment.clone(), max_volume.clone());
    move || {
        let value = AudioConfig {
            increment: parse_i32(&increment.peek(), a.increment),
            max_volume: parse_i32(&max_volume.peek(), a.max_volume),
        };
        persist(&path, "audio", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.audio")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.increment"))) value:$increment placeholder:"5"
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_volume"))) value:$max_volume placeholder:"150"
    save_row label:(Reactive::of(|| telar::t!("settings.save.audio"))) on_press:save
