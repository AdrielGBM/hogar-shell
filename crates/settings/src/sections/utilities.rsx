[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{join_csv, parse_u32, parse_u64, persist, source, split_csv};
use ::config::UtilitiesConfig;

let (config, path) = source();
let u = &config.utilities;
let base = u.clone();
let toggles = signal(join_csv(&u.toggles));
let show_capture = signal(u.show_capture);
let show_recordings = signal(u.show_recordings);
let columns = signal(u.columns.to_string());
let preview = signal(u.window_preview_ms.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (toggles, show_capture) = (toggles.clone(), show_capture.clone());
    let (show_recordings, columns, preview) =
        (show_recordings.clone(), columns.clone(), preview.clone());
    move || {
        let value = UtilitiesConfig {
            toggles: split_csv(&toggles.peek()),
            show_capture: show_capture.peek(),
            show_recordings: show_recordings.peek(),
            columns: parse_u32(&columns.peek(), base.columns),
            window_preview_ms: parse_u64(&preview.peek(), base.window_preview_ms),
        };
        persist(&path, "utilities", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.utilities")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.toggles"))) value:$toggles placeholder:"wifi, bluetooth, mic, dnd"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_capture"))) value:$show_capture
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_recordings"))) value:$show_recordings
    text_row label:(Reactive::of(|| telar::t!("settings.field.columns"))) value:$columns placeholder:"4"
    text_row label:(Reactive::of(|| telar::t!("settings.field.window_preview_ms"))) value:$preview placeholder:"1000"
    save_row label:(Reactive::of(|| telar::t!("settings.save.utilities"))) on_press:save
