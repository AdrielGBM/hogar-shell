[logic]
use crate::form_section::{form_section, FormSectionProps};
use ::ui::form::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_f32, persist, source};
use ::config::PanelsConfig;

let (config, path) = source();
let p = &config.panels;
let base = *p;
let drag_threshold = signal(p.drag_threshold.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let drag_threshold = drag_threshold.clone();
    move || {
        let value = PanelsConfig {
            drag_threshold: parse_f32(&drag_threshold.peek(), base.drag_threshold),
        };
        persist(&path, "panels", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.panels")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.drag_threshold"))) value:$drag_threshold placeholder:"48"
    save_row label:(Reactive::of(|| telar::t!("settings.save.panels"))) on_press:save
