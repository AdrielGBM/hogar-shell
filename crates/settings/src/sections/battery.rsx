[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_i32, persist, source};
use ::config::BatteryConfig;

let (config, path) = source();
let b = &config.battery;
// `warn_levels` is a list of tables, so it stays hand-edited in the TOML like `theme.colors`; carrying it through means saving here does not silently drop the user's thresholds.
let base = b.clone();
let enabled = signal(b.enabled);
let critical_level = signal(b.critical_level.to_string());
let critical_action = signal(b.critical_action.clone());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, critical_level, critical_action) = (
        enabled.clone(),
        critical_level.clone(),
        critical_action.clone(),
    );
    move || {
        let value = BatteryConfig {
            enabled: enabled.peek(),
            warn_levels: base.warn_levels.clone(),
            critical_level: parse_i32(&critical_level.peek(), base.critical_level),
            critical_action: critical_action.peek().trim().to_string(),
        };
        persist(&path, "battery", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.battery")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    text_row label:(Reactive::of(|| telar::t!("settings.field.critical_level"))) value:$critical_level placeholder:"0"
    text_row label:(Reactive::of(|| telar::t!("settings.field.critical_action"))) value:$critical_action placeholder:"suspend"
    save_row label:(Reactive::of(|| telar::t!("settings.save.battery"))) on_press:save
