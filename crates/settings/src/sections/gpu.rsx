[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::GpuConfig;

let (config, path) = source();
let g = &config.gpu;
let enabled = signal(g.enabled);
let backend = signal(g.backend.clone());
let card = signal(g.card.clone());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, backend, card) = (enabled.clone(), backend.clone(), card.clone());
    move || {
        let value = GpuConfig {
            enabled: enabled.peek(),
            backend: backend.peek(),
            card: card.peek(),
        };
        persist(&path, "gpu", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.gpu")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    text_row label:(Reactive::of(|| telar::t!("settings.field.backend"))) value:$backend placeholder:"auto"
    text_row label:(Reactive::of(|| telar::t!("settings.field.card"))) value:$card placeholder:"card1"
    save_row label:(Reactive::of(|| telar::t!("settings.save.gpu"))) on_press:save
