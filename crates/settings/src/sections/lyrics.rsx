[logic]
use crate::form::{persist, source};
use ::config::LyricsConfig;
use ::config::theme::{FontRole, NordTheme};

// The folder is `[paths] lyrics`, edited with the other paths rather than duplicated here.
let (config, path) = source();
let l = &config.lyrics;
let enabled = signal(l.enabled);
let online = signal(l.online);

// Cloned into the write rather than moved: `[logic]` runs before `[view]`, so the fields below still need the handles this closure reads.
let save: Box<dyn Fn()> = Box::new({
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
col gap(::ui::scale::space::md()) width:100%
    text "{telar::t!(\"settings.section.lyrics\")}" color:text size:theme.font(FontRole::Body) weight:700
    toggle_row label(|| telar::t!("settings.field.enabled")) value:$enabled
    toggle_row label(|| telar::t!("settings.field.lyrics_online")) value:$online
    save_row label(|| telar::t!("settings.save.lyrics")) on_press(save)
