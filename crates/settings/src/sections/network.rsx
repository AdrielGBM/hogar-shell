[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_u32, persist, source};
use ::config::NetworkConfig;

let (config, path) = source();
let n = config.network;
let enabled = signal(n.enabled);
let rescan = signal(n.rescan_seconds.to_string());
let max_networks = signal(n.max_networks.to_string());
let show_hidden = signal(n.show_hidden);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, rescan) = (enabled.clone(), rescan.clone());
    let (max_networks, show_hidden) = (max_networks.clone(), show_hidden.clone());
    move || {
        let value = NetworkConfig {
            enabled: enabled.peek(),
            rescan_seconds: parse_u32(&rescan.peek(), n.rescan_seconds),
            max_networks: parse_u32(&max_networks.peek(), n.max_networks),
            show_hidden: show_hidden.peek(),
        };
        persist(&path, "network", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.network")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    text_row label:(Reactive::of(|| telar::t!("settings.field.rescan_seconds"))) value:$rescan placeholder:"300"
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_networks"))) value:$max_networks placeholder:"20"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_hidden"))) value:$show_hidden
    save_row label:(Reactive::of(|| telar::t!("settings.save.network"))) on_press:save
