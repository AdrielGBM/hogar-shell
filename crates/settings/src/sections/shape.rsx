[logic]
use crate::form_section::{form_section, FormSectionProps};
use ::ui::form::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::ShapeConfig;

let (config, path) = source();
let frame = signal(config.shape.frame);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let frame = frame.clone();
    move || {
        let value = ShapeConfig {
            frame: frame.peek(),
        };
        persist(&path, "shape", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.shape")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.frame_ring"))) value:$frame
    save_row label:(Reactive::of(|| telar::t!("settings.save.shape"))) on_press:save
