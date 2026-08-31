[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{join_csv, parse_u32, persist, source, split_csv};
use ::config::WallpaperConfig;

let (config, path) = source();
let w = &config.wallpaper;
let base = w.clone();
let enabled = signal(w.enabled);
let recursive = signal(w.recursive);
let max_entries = signal(w.max_entries.to_string());
let thumbnail_size = signal(w.thumbnail_size.to_string());
let extensions = signal(join_csv(&w.extensions));

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, recursive) = (enabled.clone(), recursive.clone());
    let (max_entries, thumbnail_size, extensions) = (
        max_entries.clone(),
        thumbnail_size.clone(),
        extensions.clone(),
    );
    move || {
        let value = WallpaperConfig {
            enabled: enabled.peek(),
            recursive: recursive.peek(),
            max_entries: parse_u32(&max_entries.peek(), base.max_entries),
            thumbnail_size: parse_u32(&thumbnail_size.peek(), base.thumbnail_size),
            extensions: split_csv(&extensions.peek()),
        };
        persist(&path, "wallpaper", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.wallpaper")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.recursive"))) value:$recursive
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_entries"))) value:$max_entries placeholder:"2000"
    text_row label:(Reactive::of(|| telar::t!("settings.field.thumbnail_size"))) value:$thumbnail_size placeholder:"320"
    text_row label:(Reactive::of(|| telar::t!("settings.field.extensions"))) value:$extensions placeholder:"png, jpg"
    save_row label:(Reactive::of(|| telar::t!("settings.save.wallpaper"))) on_press:save
