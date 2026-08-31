[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{SHOT_BACKENDS, persist, source};
use ::config::ScreenshotConfig;

let (config, path) = source();
let s = &config.screenshot;
let copy = signal(s.copy);
let save_to_disk = signal(s.save);
let cursor = signal(s.include_cursor);
let freeze = signal(s.freeze);
let notify = signal(s.notify);
let backend = signal(s.backend.clone());
let file_name = signal(s.file_name.clone());
let annotator = signal(s.annotator.clone());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (copy, save_to_disk, cursor) = (copy.clone(), save_to_disk.clone(), cursor.clone());
    let (freeze, notify, backend) = (freeze.clone(), notify.clone(), backend.clone());
    let (file_name, annotator) = (file_name.clone(), annotator.clone());
    move || {
        let value = ScreenshotConfig {
            copy: copy.peek(),
            save: save_to_disk.peek(),
            include_cursor: cursor.peek(),
            freeze: freeze.peek(),
            notify: notify.peek(),
            backend: backend.peek(),
            file_name: file_name.peek(),
            annotator: annotator.peek(),
        };
        persist(&path, "screenshot", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.screenshot")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.copy"))) value:$copy
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.save"))) value:$save_to_disk
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.include_cursor"))) value:$cursor
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.freeze"))) value:$freeze
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.notify"))) value:$notify
    enum_row label:(Reactive::of(|| telar::t!("settings.field.backend"))) value:$backend options:SHOT_BACKENDS
    text_row label:(Reactive::of(|| telar::t!("settings.field.file_name"))) value:$file_name placeholder:"screenshot_%Y-%m-%d_%H-%M-%S"
    text_row label:(Reactive::of(|| telar::t!("settings.field.annotator"))) value:$annotator placeholder:"satty --filename {file}"
    save_row label:(Reactive::of(|| telar::t!("settings.save.screenshot"))) on_press:save
