[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{EDGES, edge_str, parse_edge, parse_u32, persist, source};
use ::config::SidebarConfig;

let (config, path) = source();
let s = &config.sidebar;
let base = s.clone();
let edge = signal(edge_str(s.edge).to_string());
let size = signal(s.size.to_string());
let show_toggles = signal(s.show_toggles);
let show_history = signal(s.show_history);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (edge, size) = (edge.clone(), size.clone());
    let (show_toggles, show_history) = (show_toggles.clone(), show_history.clone());
    move || {
        let value = SidebarConfig {
            edge: parse_edge(&edge.peek()),
            size: parse_u32(&size.peek(), base.size),
            show_toggles: show_toggles.peek(),
            show_history: show_history.peek(),
        };
        persist(&path, "sidebar", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.sidebar")))
    enum_row label:(Reactive::of(|| telar::t!("settings.field.edge"))) value:$edge options:EDGES
    text_row label:(Reactive::of(|| telar::t!("settings.field.size"))) value:$size placeholder:"400"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_toggles"))) value:$show_toggles
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_history"))) value:$show_history
    save_row label:(Reactive::of(|| telar::t!("settings.save.sidebar"))) on_press:save
