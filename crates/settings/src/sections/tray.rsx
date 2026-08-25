[logic]
use crate::form::{persist, source, split_csv};
use ::config::TrayConfig;

let (config, path) = source();
let t = &config.tray;
let base = t.clone();
let enabled = signal(t.enabled);
let compact = signal(t.compact);
let recolour = signal(t.recolour);
let background = signal(t.background);
let hidden = signal(t.hidden.join(", "));

let save: Box<dyn Fn()> = Box::new({
    let (enabled, compact, recolour) = (enabled.clone(), compact.clone(), recolour.clone());
    let (background, hidden) = (background.clone(), hidden.clone());
    move || {
        let value = TrayConfig {
            enabled: enabled.peek(),
            compact: compact.peek(),
            recolour: recolour.peek(),
            background: background.peek(),
            hidden: split_csv(&hidden.peek()),
            // Map-valued, so it stays hand-edited in the TOML like `theme.colors`; carrying it through means saving here does not silently drop the user's icon substitutions.
            icon_subs: base.icon_subs.clone(),
        };
        persist(&path, "tray", &value);
    }
});

[view]
form_section title(|| telar::t!("settings.section.tray"))
    toggle_row label(|| telar::t!("settings.field.enabled")) value:$enabled
    toggle_row label(|| telar::t!("settings.field.compact")) value:$compact
    toggle_row label(|| telar::t!("settings.field.recolour")) value:$recolour
    toggle_row label(|| telar::t!("settings.field.background")) value:$background
    text_row label(|| telar::t!("settings.field.hidden")) value:$hidden placeholder:"steam_app_*"
    save_row label(|| telar::t!("settings.save.tray")) on_press(save)
