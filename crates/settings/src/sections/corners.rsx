[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{opt_string, persist, source};
use ::config::CornersConfig;

let (config, path) = source();
let c = &config.corners;
let tl = signal(c.top_left.clone().unwrap_or_default());
let tr = signal(c.top_right.clone().unwrap_or_default());
let bl = signal(c.bottom_left.clone().unwrap_or_default());
let br = signal(c.bottom_right.clone().unwrap_or_default());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (tl, tr, bl, br) = (tl.clone(), tr.clone(), bl.clone(), br.clone());
    move || {
        let value = CornersConfig {
            top_left: opt_string(&tl.peek()),
            top_right: opt_string(&tr.peek()),
            bottom_left: opt_string(&bl.peek()),
            bottom_right: opt_string(&br.peek()),
        };
        persist(&path, "corners", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.corners")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.top_left"))) value:$tl placeholder:"module id"
    text_row label:(Reactive::of(|| telar::t!("settings.field.top_right"))) value:$tr placeholder:"module id"
    text_row label:(Reactive::of(|| telar::t!("settings.field.bottom_left"))) value:$bl placeholder:"module id"
    text_row label:(Reactive::of(|| telar::t!("settings.field.bottom_right"))) value:$br placeholder:"module id"
    save_row label:(Reactive::of(|| telar::t!("settings.save.corners"))) on_press:save
