[logic]
use crate::form::{persist, source};
use ::config::ClockConfig;

let (config, path) = source();
let c = &config.clock;
let base = c.clone();
let twelve_hour = signal(c.twelve_hour);
// An empty field means "no override", which is what `Option<String>` carries; the placeholder shows what the 12/24-hour switch would produce, so it is clear what leaving it blank does.
let format = signal(c.format.clone().unwrap_or_default());
let show_date = signal(c.show_date);
let date_format = signal(c.date_format.clone());

let save: Box<dyn Fn()> = Box::new({
    let (twelve_hour, format) = (twelve_hour.clone(), format.clone());
    let (show_date, date_format) = (show_date.clone(), date_format.clone());
    move || {
        let typed = format.peek();
        let value = ClockConfig {
            twelve_hour: twelve_hour.peek(),
            format: (!typed.trim().is_empty()).then_some(typed),
            show_date: show_date.peek(),
            date_format: {
                let typed = date_format.peek();
                if typed.trim().is_empty() {
                    base.date_format.clone()
                } else {
                    typed
                }
            },
        };
        persist(&path, "clock", &value);
    }
});

[view]
form_section title(|| telar::t!("settings.section.clock"))
    toggle_row label(|| telar::t!("settings.field.twelve_hour")) value:$twelve_hour
    text_row label(|| telar::t!("settings.field.time_format")) value:$format placeholder:"%H:%M:%S"
    toggle_row label(|| telar::t!("settings.field.show_date")) value:$show_date
    text_row label(|| telar::t!("settings.field.date_format")) value:$date_format placeholder:"%a %d %b"
    save_row label(|| telar::t!("settings.save.clock")) on_press(save)
