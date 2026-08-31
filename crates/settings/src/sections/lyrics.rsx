[logic]
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::LyricsConfig;
use ::config::theme::{FontRole, NordTheme};

// The folder is `[paths] lyrics`, edited with the other paths rather than duplicated here.
let (config, path) = source();
let l = &config.lyrics;
let enabled = signal(l.enabled);
let online = signal(l.online);

// Cloned into the write rather than moved: `[logic]` runs before `[view]`, so the fields below still need the handles this closure reads.
let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, online) = (enabled.clone(), online.clone());
    move || {
        let value = LyricsConfig {
            enabled: enabled.peek(),
            online: online.peek(),
        };
        persist(&path, "lyrics", &value);
    }
});

[view]
col gap:(::ui::scale::space::md()) width:100%
    text "{telar::t!(\"settings.section.lyrics\")}" color:$theme.text font_size:$theme.font(FontRole::Body) font_weight:700
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.lyrics_online"))) value:$online
    save_row label:(Reactive::of(|| telar::t!("settings.save.lyrics"))) on_press:save
