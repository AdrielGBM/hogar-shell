[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_f32, parse_u32, persist, source};
use ::config::{DrawerConfig, FloatConfig, PanelsConfig};

let (config, path) = source();
let p = &config.panels;
let base = *p;
let drag_threshold = signal(p.drag_threshold.to_string());
let drawer_w = signal(p.drawer.width.to_string());
let drawer_h = signal(p.drawer.max_height.to_string());
let float_w = signal(p.float.width.to_string());
let float_h = signal(p.float.height.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let drag_threshold = drag_threshold.clone();
    let (drawer_w, drawer_h) = (drawer_w.clone(), drawer_h.clone());
    let (float_w, float_h) = (float_w.clone(), float_h.clone());
    move || {
        let value = PanelsConfig {
            drag_threshold: parse_f32(&drag_threshold.peek(), base.drag_threshold),
            drawer: DrawerConfig {
                width: parse_f32(&drawer_w.peek(), base.drawer.width),
                max_height: parse_f32(&drawer_h.peek(), base.drawer.max_height),
            },
            float: FloatConfig {
                width: parse_u32(&float_w.peek(), base.float.width),
                height: parse_u32(&float_h.peek(), base.float.height),
            },
        };
        persist(&path, "panels", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.panels")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.drawer_width"))) value:$drawer_w placeholder:"320"
    text_row label:(Reactive::of(|| telar::t!("settings.field.drawer_max_height"))) value:$drawer_h placeholder:"280"
    text_row label:(Reactive::of(|| telar::t!("settings.field.float_width"))) value:$float_w placeholder:"360"
    text_row label:(Reactive::of(|| telar::t!("settings.field.float_height"))) value:$float_h placeholder:"240"
    text_row label:(Reactive::of(|| telar::t!("settings.field.drag_threshold"))) value:$drag_threshold placeholder:"48"
    save_row label:(Reactive::of(|| telar::t!("settings.save.panels"))) on_press:save
