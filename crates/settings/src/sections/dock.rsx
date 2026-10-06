[logic]
use crate::form_section::{form_section, FormSectionProps};
use ::ui::form::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_f32, persist_with, source};
use ::config::DockConfig;

let (config, path) = source();
let base = config.dock.magnification;
let magnification = signal(base.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let magnification = magnification.clone();
    move || {
        let wanted = parse_f32(&magnification.peek(), base);
        persist_with(&path, "dock", |config| DockConfig {
            magnification: wanted,
            ..config.dock.clone()
        });
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.dock")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.magnification"))) value:$magnification placeholder:"1.5"
    save_row label:(Reactive::of(|| telar::t!("settings.save.dock"))) on_press:save
