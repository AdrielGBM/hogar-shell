[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{
    SHAPES, opt_num, opt_u32, parse_shape, parse_u32, persist, shape_str, source,
};
use ::config::ShapeConfig;

let (config, path) = source();
let s = &config.shape;
let base = s.clone();
let mode = signal(shape_str(s.mode).to_string());
let frame = signal(s.frame);
let gap = signal(s.gap.to_string());
let spacing = signal(opt_num(s.spacing));
let radius = signal(opt_num(s.radius));
let inactive = signal(s.inactive_size.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (mode, frame, gap) = (mode.clone(), frame.clone(), gap.clone());
    let (spacing, radius, inactive) = (spacing.clone(), radius.clone(), inactive.clone());
    move || {
        let value = ShapeConfig {
            mode: parse_shape(&mode.peek()),
            frame: frame.peek(),
            gap: parse_u32(&gap.peek(), base.gap),
            spacing: opt_u32(&spacing.peek()),
            radius: opt_u32(&radius.peek()),
            inactive_size: parse_u32(&inactive.peek(), base.inactive_size),
        };
        persist(&path, "shape", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.shape")))
    enum_row label:(Reactive::of(|| telar::t!("settings.field.mode"))) value:$mode options:SHAPES
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.frame_ring"))) value:$frame
    text_row label:(Reactive::of(|| telar::t!("settings.field.gap"))) value:$gap placeholder:"0"
    text_row label:(Reactive::of(|| telar::t!("settings.field.spacing"))) value:$spacing placeholder:"(theme)"
    text_row label:(Reactive::of(|| telar::t!("settings.field.radius"))) value:$radius placeholder:"(theme)"
    text_row label:(Reactive::of(|| telar::t!("settings.field.inactive_size"))) value:$inactive placeholder:"6"
    save_row label:(Reactive::of(|| telar::t!("settings.save.shape"))) on_press:save
