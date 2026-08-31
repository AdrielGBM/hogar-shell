[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{
    FULLSCREEN_POPUPS, fullscreen_popups_str, parse_f32, parse_fullscreen_popups, parse_u32,
    persist, source,
};
use ::config::NotificationsConfig;

let (config, path) = source();
let n = &config.notifications;
let base = n.clone();
let critical = signal(n.critical_sticky);
let critical_max = signal(n.critical_max_secs.to_string());
let fullscreen = signal(fullscreen_popups_str(n.fullscreen).to_string());
let group_by_app = signal(n.group_by_app);
let group_preview = signal(n.group_preview_num.to_string());
let action_on_click = signal(n.action_on_click);
let body_lines = signal(n.body_lines.to_string());
let open_expanded = signal(n.open_expanded);
let sound = signal(n.sound.clone());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let fullscreen = fullscreen.clone();
    let (critical, group_by_app, action_on_click, open_expanded) = (
        critical.clone(),
        group_by_app.clone(),
        action_on_click.clone(),
        open_expanded.clone(),
    );
    let (group_preview, body_lines, sound) =
        (group_preview.clone(), body_lines.clone(), sound.clone());
    let critical_max = critical_max.clone();
    move || {
        let value = NotificationsConfig {

            critical_sticky: critical.peek(),
            critical_max_secs: parse_u32(&critical_max.peek(), base.critical_max_secs as u32) as u64,

            fullscreen: parse_fullscreen_popups(&fullscreen.peek()),
            group_by_app: group_by_app.peek(),
            group_preview_num: parse_u32(&group_preview.peek(), base.group_preview_num),
            action_on_click: action_on_click.peek(),
            body_lines: parse_u32(&body_lines.peek(), base.body_lines),
            open_expanded: open_expanded.peek(),
            sound: sound.peek(),
        };
        persist(&path, "notifications", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.notifications")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.critical_sticky"))) value:$critical
    text_row label:(Reactive::of(|| telar::t!("settings.field.critical_max_secs"))) value:$critical_max placeholder:"120"
    enum_row label:(Reactive::of(|| telar::t!("settings.field.fullscreen_popups"))) value:$fullscreen options:FULLSCREEN_POPUPS
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.group_by_app"))) value:$group_by_app
    text_row label:(Reactive::of(|| telar::t!("settings.field.group_preview_num"))) value:$group_preview placeholder:"3"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.action_on_click"))) value:$action_on_click
    text_row label:(Reactive::of(|| telar::t!("settings.field.body_lines"))) value:$body_lines placeholder:"4"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.open_expanded"))) value:$open_expanded
    text_row label:(Reactive::of(|| telar::t!("settings.field.sound"))) value:$sound placeholder:"canberra-gtk-play -i message"
    save_row label:(Reactive::of(|| telar::t!("settings.save.notifications"))) on_press:save
