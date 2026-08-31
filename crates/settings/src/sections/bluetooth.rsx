[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::toggle_row::{toggle_row, ToggleRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_u32, persist, source};
use ::config::BluetoothConfig;

let (config, path) = source();
let b = config.bluetooth;
let enabled = signal(b.enabled);
let scan_on_open = signal(b.scan_on_open);
let max_devices = signal(b.max_devices.to_string());
let show_unnamed = signal(b.show_unnamed);

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (enabled, scan_on_open) = (enabled.clone(), scan_on_open.clone());
    let (max_devices, show_unnamed) = (max_devices.clone(), show_unnamed.clone());
    move || {
        let value = BluetoothConfig {
            enabled: enabled.peek(),
            scan_on_open: scan_on_open.peek(),
            max_devices: parse_u32(&max_devices.peek(), b.max_devices),
            show_unnamed: show_unnamed.peek(),
        };
        persist(&path, "bluetooth", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.bluetooth")))
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.enabled"))) value:$enabled
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.scan_on_open"))) value:$scan_on_open
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_devices"))) value:$max_devices placeholder:"12"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.show_unnamed"))) value:$show_unnamed
    save_row label:(Reactive::of(|| telar::t!("settings.save.bluetooth"))) on_press:save
