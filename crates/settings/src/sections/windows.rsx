[logic]
use crate::form_section::{form_section, FormSectionProps};
use ::ui::form::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::WindowsConfig;

let (config, path) = source();
let titles = signal(config.windows.titles);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let titles = titles.clone();
    move || {
        let value = WindowsConfig {
            titles: titles.peek(),
        };
        persist(&path, "windows", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.windows")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.titles"))) value:$titles
    save_row label:(Reactive::of(|| telar::t!("settings.save.windows"))) on_press:save
