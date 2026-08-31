[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{
    MEDIA_SCROLLS, media_scroll_str, parse_media_scroll, parse_u32, persist, source,
};
use ::config::MediaConfig;

let (config, path) = source();
let m = &config.media;
// Aliases are map-valued, so they stay hand-edited in the TOML for now, like `theme.colors`; carrying the existing map through means saving this section does not silently drop them.
let base = m.clone();
let preferred = signal(m.preferred_player.clone());
let max_chars = signal(m.max_chars.to_string());
let scroll = signal(media_scroll_str(m.scroll).to_string());
let marquee = signal(m.marquee);
let marquee_speed = signal(m.marquee_speed_ms.to_string());
let seek_seconds = signal(m.seek_seconds.to_string());
let visualiser = signal(m.visualiser);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (preferred, max_chars, scroll) = (preferred.clone(), max_chars.clone(), scroll.clone());
    let (marquee, marquee_speed) = (marquee.clone(), marquee_speed.clone());
    let (seek_seconds, visualiser) = (seek_seconds.clone(), visualiser.clone());
    move || {
        let value = MediaConfig {
            preferred_player: preferred.peek(),
            max_chars: parse_u32(&max_chars.peek(), base.max_chars),
            scroll: parse_media_scroll(&scroll.peek()),
            marquee: marquee.peek(),
            marquee_speed_ms: parse_u32(&marquee_speed.peek(), base.marquee_speed_ms),
            seek_seconds: parse_u32(&seek_seconds.peek(), base.seek_seconds),
            visualiser: visualiser.peek(),
            aliases: base.aliases.clone(),
        };
        persist(&path, "media", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.media")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.preferred_player"))) value:$preferred placeholder:"auto"
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_chars"))) value:$max_chars placeholder:"40"
    enum_row label:(Reactive::of(|| telar::t!("settings.field.scroll"))) value:$scroll options:MEDIA_SCROLLS
    text_row label:(Reactive::of(|| telar::t!("settings.field.seek_seconds"))) value:$seek_seconds placeholder:"5"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.marquee"))) value:$marquee
    text_row label:(Reactive::of(|| telar::t!("settings.field.marquee_speed_ms"))) value:$marquee_speed placeholder:"220"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.cover_visualiser"))) value:$visualiser
    save_row label:(Reactive::of(|| telar::t!("settings.save.media"))) on_press:save
