[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::KeyNavConfig;

let (config, path) = source();
let vim = signal(config.keynav.vim);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let vim = vim.clone();
    move || persist(&path, "keynav", &KeyNavConfig { vim: vim.peek() })
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.keynav")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.vim"))) value:$vim
    save_row label:(Reactive::of(|| telar::t!("settings.save.keynav"))) on_press:save
