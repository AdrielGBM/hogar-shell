[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::ActiveWindowConfig;

let (config, path) = source();
let w = config.active_window;
let compact = signal(w.compact);
let show_icon = signal(w.show_icon);
let inverted = signal(w.inverted);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (compact, show_icon) = (compact.clone(), show_icon.clone());
    let inverted = inverted.clone();
    move || {
        let value = ActiveWindowConfig {
            compact: compact.peek(),
            show_icon: show_icon.peek(),
            inverted: inverted.peek(),
        };
        persist(&path, "active_window", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.active_window")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.compact"))) value:$compact
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_icon"))) value:$show_icon
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.inverted"))) value:$inverted
    save_row label:(Reactive::of(|| telar::t!("settings.save.active_window"))) on_press:save
