[logic]
use crate::form::{
    persist, source,
};
use ::config::theme::FontRole;
use ::config::{ToastEvents, ToastsConfig};

let (config, path) = source();
let t = &config.toasts;

let enabled = signal(t.enabled);

let events = t.events;
let config_loaded = signal(events.config_loaded);
let charging = signal(events.charging);
let game_mode = signal(events.game_mode);
let dnd = signal(events.dnd);
let audio_output = signal(events.audio_output);
let audio_input = signal(events.audio_input);
let lock_keys = signal(events.lock_keys);
let kb_layout = signal(events.kb_layout);
let vpn = signal(events.vpn);
let now_playing = signal(events.now_playing);
let screenshot = signal(events.screenshot);
let recording = signal(events.recording);

let save: Box<dyn Fn()> = Box::new({
    let enabled = enabled.clone();
    let (config_loaded, charging, game_mode, dnd) = (
        config_loaded.clone(),
        charging.clone(),
        game_mode.clone(),
        dnd.clone(),
    );
    let (audio_output, audio_input, lock_keys, kb_layout) = (
        audio_output.clone(),
        audio_input.clone(),
        lock_keys.clone(),
        kb_layout.clone(),
    );
    let (vpn, now_playing, screenshot, recording) = (
        vpn.clone(),
        now_playing.clone(),
        screenshot.clone(),
        recording.clone(),
    );
    move || {
        let value = ToastsConfig {
            enabled: enabled.peek(),

            events: ToastEvents {
                config_loaded: config_loaded.peek(),
                charging: charging.peek(),
                game_mode: game_mode.peek(),
                dnd: dnd.peek(),
                audio_output: audio_output.peek(),
                audio_input: audio_input.peek(),
                lock_keys: lock_keys.peek(),
                kb_layout: kb_layout.peek(),
                vpn: vpn.peek(),
                now_playing: now_playing.peek(),
                screenshot: screenshot.peek(),
                recording: recording.peek(),
            },
        };
        persist(&path, "toasts", &value);
    }
});

[view]
form_section title(|| telar::t!("settings.section.toasts"))
    toggle_row label(|| telar::t!("settings.field.enabled")) value:$enabled
    text "{telar::t!(\"settings.subheader.events\")}" color:theme.muted font_size:theme.font(FontRole::Caption) font_weight:700
    toggle_row label(|| telar::t!("settings.field.event_config_loaded")) value:$config_loaded
    toggle_row label(|| telar::t!("settings.field.event_charging")) value:$charging
    toggle_row label(|| telar::t!("settings.field.event_game_mode")) value:$game_mode
    toggle_row label(|| telar::t!("settings.field.event_dnd")) value:$dnd
    toggle_row label(|| telar::t!("settings.field.event_audio_output")) value:$audio_output
    toggle_row label(|| telar::t!("settings.field.event_audio_input")) value:$audio_input
    toggle_row label(|| telar::t!("settings.field.event_lock_keys")) value:$lock_keys
    toggle_row label(|| telar::t!("settings.field.event_kb_layout")) value:$kb_layout
    toggle_row label(|| telar::t!("settings.field.event_vpn")) value:$vpn
    toggle_row label(|| telar::t!("settings.field.event_now_playing")) value:$now_playing
    toggle_row label(|| telar::t!("settings.field.event_screenshot")) value:$screenshot
    toggle_row label(|| telar::t!("settings.field.event_recording")) value:$recording
    save_row label(|| telar::t!("settings.save.toasts")) on_press(save)
