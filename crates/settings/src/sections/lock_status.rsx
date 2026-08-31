[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::LockStatusConfig;

let (config, path) = source();
let l = config.lock_status;
let caps = signal(l.caps);
let num = signal(l.num);
let hide_inactive = signal(l.hide_inactive);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (caps, num, hide_inactive) = (caps.clone(), num.clone(), hide_inactive.clone());
    move || {
        let value = LockStatusConfig {
            caps: caps.peek(),
            num: num.peek(),
            hide_inactive: hide_inactive.peek(),
        };
        persist(&path, "lock_status", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.lock_status")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.caps"))) value:$caps
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.num"))) value:$num
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.hide_inactive"))) value:$hide_inactive
    save_row label:(Reactive::of(|| telar::t!("settings.save.lock_status"))) on_press:save
