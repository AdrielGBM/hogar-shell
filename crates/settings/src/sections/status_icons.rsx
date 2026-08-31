[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{join_csv, parse_f32, persist, source, split_csv};
use ::config::StatusIconsConfig;

let (config, path) = source();
let s = &config.status_icons;
let base = s.clone();
let icons = signal(join_csv(&s.icons));
let spacing = signal(s.spacing.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (icons, spacing) = (icons.clone(), spacing.clone());
    move || {
        let value = StatusIconsConfig {
            icons: split_csv(&icons.peek()),
            spacing: parse_f32(&spacing.peek(), base.spacing),
        };
        persist(&path, "status_icons", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.status_icons")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.icons"))) value:$icons placeholder:"volume, mic, network, battery"
    text_row label:(Reactive::of(|| telar::t!("settings.field.spacing"))) value:$spacing placeholder:"0.35"
    save_row label:(Reactive::of(|| telar::t!("settings.save.status_icons"))) on_press:save
