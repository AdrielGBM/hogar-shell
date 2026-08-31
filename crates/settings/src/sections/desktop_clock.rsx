[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{PLACEMENTS, opt_string, parse_f32, parse_u32, persist, source};
use ::config::{ClockPlacement, DesktopClockConfig, WidgetsConfig};

// Its own section rather than rows inside `[widgets]`: it is a nested table, and one Save writing both would mean every clock tweak rewrote the visualiser's settings with it.
let (config, path) = source();
let c = &config.widgets.clock;
let base = config.widgets.clone();
let enabled = signal(c.enabled);
let position = signal(c.position.id().to_string());
let scale = signal(c.scale.to_string());
let margin = signal(c.margin.to_string());
let invert = signal(c.invert);
let show_date = signal(c.show_date);
let format = signal(c.format.clone().unwrap_or_default());
let date_format = signal(c.date_format.clone().unwrap_or_default());
let background = signal(c.background);
let opacity = signal(c.background_opacity.to_string());
let blur = signal(c.background_blur.to_string());
let shadow = signal(c.shadow);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, position, scale, margin) = (
        enabled.clone(),
        position.clone(),
        scale.clone(),
        margin.clone(),
    );
    let (invert, show_date, format, date_format) = (
        invert.clone(),
        show_date.clone(),
        format.clone(),
        date_format.clone(),
    );
    let (background, opacity, blur, shadow) = (
        background.clone(),
        opacity.clone(),
        blur.clone(),
        shadow.clone(),
    );
    move || {
        let clock = DesktopClockConfig {
            enabled: enabled.peek(),
            position: ClockPlacement::from_id(&position.peek()).unwrap_or_default(),
            scale: parse_f32(&scale.peek(), base.clock.scale),
            margin: parse_u32(&margin.peek(), base.clock.margin),
            invert: invert.peek(),
            show_date: show_date.peek(),
            format: opt_string(&format.peek()),
            date_format: opt_string(&date_format.peek()),
            background: background.peek(),
            background_opacity: parse_f32(&opacity.peek(), base.clock.background_opacity),
            background_blur: parse_f32(&blur.peek(), base.clock.background_blur),
            shadow: shadow.peek(),
        };
        let value = WidgetsConfig {
            clock,
            ..base.clone()
        };
        persist(&path, "widgets", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.desktop_clock")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    enum_row label:(Reactive::of(|| telar::t!("settings.field.position"))) value:$position options:PLACEMENTS
    text_row label:(Reactive::of(|| telar::t!("settings.field.scale"))) value:$scale placeholder:"3"
    text_row label:(Reactive::of(|| telar::t!("settings.field.margin"))) value:$margin placeholder:"48"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.invert"))) value:$invert
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_date"))) value:$show_date
    text_row label:(Reactive::of(|| telar::t!("settings.field.time_format"))) value:$format placeholder:"(clock)"
    text_row label:(Reactive::of(|| telar::t!("settings.field.date_format"))) value:$date_format placeholder:"(clock)"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.plate"))) value:$background
    text_row label:(Reactive::of(|| telar::t!("settings.field.plate_opacity"))) value:$opacity placeholder:"0.35"
    text_row label:(Reactive::of(|| telar::t!("settings.field.blur"))) value:$blur placeholder:"0"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.shadow"))) value:$shadow
    save_row label:(Reactive::of(|| telar::t!("settings.save.desktop_clock"))) on_press:save
