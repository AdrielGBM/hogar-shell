[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{
    CAPITALIZATIONS, capitalize_str, parse_capitalize, parse_f32, parse_u32, persist, source,
};
use ::config::WorkspacesConfig;

let (config, path) = source();
let w = &config.workspaces;
let base = w.clone();
let shown = signal(w.shown.to_string());
let per_monitor = signal(w.per_monitor);
let show_special = signal(w.show_special);
let window_icons = signal(w.window_icons);
let max_icons = signal(w.max_window_icons.to_string());
let occupied = signal(w.occupied_background);
let indicator = signal(w.indicator);
let indicator_trail = signal(w.indicator_trail.to_string());
let scroll = signal(w.scroll);
let label = signal(w.label.clone());
let occupied_label = signal(w.occupied_label.clone());
let active_label = signal(w.active_label.clone());
let capitalize = signal(capitalize_str(w.capitalize).to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (shown, per_monitor, show_special) =
        (shown.clone(), per_monitor.clone(), show_special.clone());
    let (window_icons, max_icons, occupied) =
        (window_icons.clone(), max_icons.clone(), occupied.clone());
    let (indicator, indicator_trail, scroll) =
        (indicator.clone(), indicator_trail.clone(), scroll.clone());
    let (label, occupied_label, active_label, capitalize) = (
        label.clone(),
        occupied_label.clone(),
        active_label.clone(),
        capitalize.clone(),
    );
    move || {
        let typed = label.peek();
        let value = WorkspacesConfig {
            shown: parse_u32(&shown.peek(), base.shown),
            per_monitor: per_monitor.peek(),
            show_special: show_special.peek(),
            window_icons: window_icons.peek(),
            max_window_icons: parse_u32(&max_icons.peek(), base.max_window_icons),
            occupied_background: occupied.peek(),
            indicator: indicator.peek(),
            indicator_trail: parse_f32(&indicator_trail.peek(), base.indicator_trail),
            scroll: scroll.peek(),
            label: if typed.trim().is_empty() {
                base.label.clone()
            } else {
                typed
            },
            // Empty is meaningful here: it means "render like every other pill".
            occupied_label: occupied_label.peek().trim().to_string(),
            active_label: active_label.peek().trim().to_string(),
            capitalize: parse_capitalize(&capitalize.peek()),
            // Map-valued, so it stays hand-edited in the TOML; carrying it through means saving here does not silently drop the user's scratchpad icons.
            special_icons: base.special_icons.clone(),
        };
        persist(&path, "workspaces", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.workspaces")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.shown"))) value:$shown placeholder:"0"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.per_monitor"))) value:$per_monitor
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_special"))) value:$show_special
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.window_icons"))) value:$window_icons
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_window_icons"))) value:$max_icons placeholder:"4"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.occupied_background"))) value:$occupied
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.indicator"))) value:$indicator
    text_row label:(Reactive::of(|| telar::t!("settings.field.indicator_trail"))) value:$indicator_trail placeholder:"0.35"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.scroll"))) value:$scroll
    text_row label:(Reactive::of(|| telar::t!("settings.field.label"))) value:$label placeholder:"{id}"
    text_row label:(Reactive::of(|| telar::t!("settings.field.occupied_label"))) value:$occupied_label placeholder:"(label)"
    text_row label:(Reactive::of(|| telar::t!("settings.field.active_label"))) value:$active_label placeholder:"(label)"
    enum_row label:(Reactive::of(|| telar::t!("settings.field.capitalize"))) value:$capitalize options:CAPITALIZATIONS
    save_row label:(Reactive::of(|| telar::t!("settings.save.workspaces"))) on_press:save
