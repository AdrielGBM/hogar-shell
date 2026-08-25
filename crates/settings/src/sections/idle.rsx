[logic]
use crate::form::{persist, source};
use ::config::IdleConfig;

let (config, path) = source();
let i = &config.idle;
// `stages` is a list of tables, so it stays hand-edited in the TOML — K13. Carried through, so switching idle on from here does not wipe the timeouts it is switching on.
let base = i.clone();
let enabled = signal(i.enabled);
let inhibit_when_audio = signal(i.inhibit_when_audio);
let inhibit_when_charging = signal(i.inhibit_when_charging);
let respect_inhibitors = signal(i.respect_inhibitors);

let save: Box<dyn Fn()> = Box::new({
    let (enabled, inhibit_when_audio) = (enabled.clone(), inhibit_when_audio.clone());
    let (inhibit_when_charging, respect_inhibitors) =
        (inhibit_when_charging.clone(), respect_inhibitors.clone());
    move || {
        let value = IdleConfig {
            enabled: enabled.peek(),
            stages: base.stages.clone(),
            inhibit_when_audio: inhibit_when_audio.peek(),
            inhibit_when_charging: inhibit_when_charging.peek(),
            respect_inhibitors: respect_inhibitors.peek(),
        };
        persist(&path, "idle", &value);
    }
});

[view]
form_section title(|| telar::t!("settings.section.idle"))
    toggle_row label(|| telar::t!("settings.field.enabled")) value:$enabled
    toggle_row label(|| telar::t!("settings.field.inhibit_when_audio")) value:$inhibit_when_audio
    toggle_row label(|| telar::t!("settings.field.inhibit_when_charging")) value:$inhibit_when_charging
    toggle_row label(|| telar::t!("settings.field.respect_inhibitors")) value:$respect_inhibitors
    save_row label(|| telar::t!("settings.save.idle")) on_press(save)
