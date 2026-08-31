[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::enum_row::{enum_row, EnumRowProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{
    TEMPERATURE_UNITS, parse_f32, parse_temperature_unit, persist, source, temperature_unit_str,
};
use ::config::TemperatureConfig;

let (config, path) = source();
let t = &config.temperature;
let base = t.clone();
let unit = signal(temperature_unit_str(t.unit).to_string());
let sensor = signal(t.sensor.clone());
let warn = signal(t.warn.to_string());
let critical = signal(t.critical.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (unit, sensor) = (unit.clone(), sensor.clone());
    let (warn, critical) = (warn.clone(), critical.clone());
    move || {
        let value = TemperatureConfig {
            unit: parse_temperature_unit(&unit.peek()),
            sensor: sensor.peek().trim().to_string(),
            warn: parse_f32(&warn.peek(), base.warn),
            critical: parse_f32(&critical.peek(), base.critical),
        };
        persist(&path, "temperature", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.temperature")))
    enum_row label:(Reactive::of(|| telar::t!("settings.field.unit"))) value:$unit options:TEMPERATURE_UNITS
    text_row label:(Reactive::of(|| telar::t!("settings.field.sensor"))) value:$sensor placeholder:"(hottest)"
    text_row label:(Reactive::of(|| telar::t!("settings.field.warn"))) value:$warn placeholder:"70"
    text_row label:(Reactive::of(|| telar::t!("settings.field.critical"))) value:$critical placeholder:"85"
    save_row label:(Reactive::of(|| telar::t!("settings.save.temperature"))) on_press:save
