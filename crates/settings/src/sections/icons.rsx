[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{persist, source};
use ::config::IconsConfig;

let (config, path) = source();
let i = &config.icons;
let provider = signal(i.provider.clone());
let default_set = signal(i.default_set.clone());
let app_icon_theme = signal(i.app_icon_theme.clone());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (provider, default_set, app_icon_theme) = (
        provider.clone(),
        default_set.clone(),
        app_icon_theme.clone(),
    );
    move || {
        let value = IconsConfig {
            provider: provider.peek(),
            default_set: default_set.peek(),
            app_icon_theme: app_icon_theme.peek(),
        };
        persist(&path, "icons", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.icons")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.provider"))) value:$provider placeholder:"https://api.iconify.design"
    text_row label:(Reactive::of(|| telar::t!("settings.field.default_set"))) value:$default_set placeholder:"lucide"
    text_row label:(Reactive::of(|| telar::t!("settings.field.app_icon_theme"))) value:$app_icon_theme placeholder:"auto"
    save_row label:(Reactive::of(|| telar::t!("settings.save.icons"))) on_press:save
