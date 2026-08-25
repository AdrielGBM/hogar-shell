[logic]
use crate::form::{persist, source};
use ::config::{AppsConfig, GeneralConfig};

let (config, path) = source();
let lang = signal(telar::current_locale().unwrap_or_else(|| config.language()));
let over_fullscreen = signal(config.general.show_over_fullscreen);
let logo = signal(config.general.logo.clone());
let apps = config.general.apps.clone();
let legacy_terminal = config.general.terminal.clone();
// `[general.apps] terminal` is the field's home now; a config still carrying the older top-level key is seeded from it, so editing here moves the value rather than appearing to lose it.
let terminal = signal(if apps.terminal.trim().is_empty() {
    config.general.terminal.clone()
} else {
    apps.terminal.clone()
});
let file_manager = signal(apps.file_manager.clone());
let audio_mixer = signal(apps.audio_mixer.clone());
let media_player = signal(apps.media_player.clone());
let browser = signal(apps.browser.clone());
let editor = signal(apps.editor.clone());

let save: Box<dyn Fn()> = Box::new({
    let (lang, over_fullscreen, logo) = (lang.clone(), over_fullscreen.clone(), logo.clone());
    let (terminal, file_manager, audio_mixer) =
        (terminal.clone(), file_manager.clone(), audio_mixer.clone());
    let (media_player, browser, editor) =
        (media_player.clone(), browser.clone(), editor.clone());
    move || {
        let value = GeneralConfig {
            language: lang.peek(),
            show_over_fullscreen: over_fullscreen.peek(),
            logo: logo.peek(),
            terminal: legacy_terminal.clone(),
            apps: AppsConfig {
                terminal: terminal.peek(),
                file_manager: file_manager.peek(),
                audio_mixer: audio_mixer.peek(),
                media_player: media_player.peek(),
                browser: browser.peek(),
                editor: editor.peek(),
            },
        };
        persist(&path, "general", &value);
    }
});

[view]
form_section title(|| telar::t!("settings.section.general"))
    language_row label(|| telar::t!("settings.field.language")) value:$lang
    toggle_row label(|| telar::t!("settings.field.show_over_fullscreen")) value:$over_fullscreen
    text_row label(|| telar::t!("settings.field.logo")) value:$logo placeholder:"auto"
    text_row label(|| telar::t!("settings.field.terminal")) value:$terminal placeholder:"xterm"
    text_row label(|| telar::t!("settings.field.file_manager")) value:$file_manager placeholder:"xdg-open"
    text_row label(|| telar::t!("settings.field.audio_mixer")) value:$audio_mixer placeholder:"pavucontrol"
    text_row label(|| telar::t!("settings.field.media_player")) value:$media_player placeholder:"xdg-open"
    text_row label(|| telar::t!("settings.field.browser")) value:$browser placeholder:"xdg-open"
    text_row label(|| telar::t!("settings.field.editor")) value:$editor placeholder:"xdg-open"
    save_row label(|| telar::t!("settings.save.general")) on_press(save)
