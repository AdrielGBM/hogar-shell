[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::PathsConfig;

let (config, path) = source();
let p = &config.paths;
let wallpapers = signal(p.wallpapers.clone());
let lyrics = signal(p.lyrics.clone());
let recordings = signal(p.recordings.clone());
let screenshots = signal(p.screenshots.clone());
let assets = signal(p.assets.clone());

// Each hint is the directory the shell would use if the field is left empty, resolved against this machine — so the form shows where things actually land rather than a generic example.
let show = |dir: std::path::PathBuf| dir.to_string_lossy().into_owned();
let wallpapers_hint = show(config.wallpaper_dir());
let lyrics_hint = show(config.lyrics_dir());
let recordings_hint = show(config.recordings_dir());
let screenshots_hint = show(config.screenshot_dir());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (wallpapers, lyrics, recordings) =
        (wallpapers.clone(), lyrics.clone(), recordings.clone());
    let (screenshots, assets) = (screenshots.clone(), assets.clone());
    move || {
        let value = PathsConfig {
            wallpapers: wallpapers.peek(),
            lyrics: lyrics.peek(),
            recordings: recordings.peek(),
            screenshots: screenshots.peek(),
            assets: assets.peek(),
        };
        persist(&path, "paths", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.paths")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.wallpapers"))) value:$wallpapers placeholder:wallpapers_hint
    text_row label:(Reactive::of(|| telar::t!("settings.field.lyrics"))) value:$lyrics placeholder:lyrics_hint
    text_row label:(Reactive::of(|| telar::t!("settings.field.recordings"))) value:$recordings placeholder:recordings_hint
    text_row label:(Reactive::of(|| telar::t!("settings.field.screenshots"))) value:$screenshots placeholder:screenshots_hint
    text_row label:(Reactive::of(|| telar::t!("settings.field.assets"))) value:$assets
    save_row label:(Reactive::of(|| telar::t!("settings.save.paths"))) on_press:save
