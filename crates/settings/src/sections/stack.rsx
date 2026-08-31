[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{
    ALIGNS, EDGES, align_str, edge_str, parse_align, parse_edge, parse_f32, parse_u32, parse_u64,
    persist, source,
};
use ::config::StackConfig;

let (config, path) = source();
let s = &config.stack;
let base = *s;
let edge = signal(edge_str(s.edge).to_string());
let align = signal(align_str(s.align).to_string());
let width = signal(s.width.to_string());
let max_visible = signal(s.max_visible.to_string());
let timeout = signal(s.timeout_ms.to_string());
let clear_threshold = signal(s.clear_threshold.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (edge, align, width) = (edge.clone(), align.clone(), width.clone());
    let (max_visible, timeout) = (max_visible.clone(), timeout.clone());
    let clear_threshold = clear_threshold.clone();
    move || {
        let value = StackConfig {
            edge: parse_edge(&edge.peek()),
            align: parse_align(&align.peek()),
            width: parse_f32(&width.peek(), base.width),
            max_visible: parse_u32(&max_visible.peek(), base.max_visible),
            timeout_ms: parse_u64(&timeout.peek(), base.timeout_ms),
            clear_threshold: parse_f32(&clear_threshold.peek(), base.clear_threshold),
        };
        persist(&path, "stack", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.stack")))
    enum_row label:(Reactive::of(|| telar::t!("settings.field.edge"))) value:$edge options:EDGES
    enum_row label:(Reactive::of(|| telar::t!("settings.field.align"))) value:$align options:ALIGNS
    text_row label:(Reactive::of(|| telar::t!("settings.field.width"))) value:$width placeholder:"380"
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_visible"))) value:$max_visible placeholder:"4"
    text_row label:(Reactive::of(|| telar::t!("settings.field.timeout_ms"))) value:$timeout placeholder:"3000"
    text_row label:(Reactive::of(|| telar::t!("settings.field.clear_threshold"))) value:$clear_threshold placeholder:"0.35"
    save_row label:(Reactive::of(|| telar::t!("settings.save.stack"))) on_press:save
