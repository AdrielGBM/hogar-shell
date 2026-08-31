[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{RECORDER_BACKENDS, parse_u32, persist, source};
use ::config::RecorderConfig;

let (config, path) = source();
let r = &config.recorder;
let base = r.clone();
let backend = signal(r.backend.clone());
let audio = signal(r.audio);
let device = signal(r.audio_device.clone());
let fps = signal(r.fps.to_string());
let file_name = signal(r.file_name.clone());
let notify = signal(r.notify);
let max_entries = signal(r.max_entries.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (backend, audio, device) = (backend.clone(), audio.clone(), device.clone());
    let (fps, file_name) = (fps.clone(), file_name.clone());
    let (notify, max_entries) = (notify.clone(), max_entries.clone());
    move || {
        let value = RecorderConfig {
            backend: backend.peek(),
            audio: audio.peek(),
            audio_device: device.peek(),
            fps: parse_u32(&fps.peek(), base.fps),
            file_name: file_name.peek(),
            notify: notify.peek(),
            max_entries: parse_u32(&max_entries.peek(), base.max_entries),
        };
        persist(&path, "recorder", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.recorder")))
    enum_row label:(Reactive::of(|| telar::t!("settings.field.backend"))) value:$backend options:RECORDER_BACKENDS
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.audio"))) value:$audio
    text_row label:(Reactive::of(|| telar::t!("settings.field.audio_device"))) value:$device placeholder:"default_output"
    text_row label:(Reactive::of(|| telar::t!("settings.field.fps"))) value:$fps placeholder:"60"
    text_row label:(Reactive::of(|| telar::t!("settings.field.file_name"))) value:$file_name placeholder:"recording_%Y-%m-%d_%H-%M-%S"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.notify"))) value:$notify
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_entries"))) value:$max_entries placeholder:"12"
    save_row label:(Reactive::of(|| telar::t!("settings.save.recorder"))) on_press:save
