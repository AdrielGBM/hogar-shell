[logic]
use crate::form::{parse_u32, persist, source};
use ::config::WeatherConfig;

let (config, path) = source();
let w = &config.weather;
let base = w.clone();
let enabled = signal(w.enabled);
let location = signal(w.location.clone());
let latitude = signal(w.latitude.map(|v| v.to_string()).unwrap_or_default());
let longitude = signal(w.longitude.map(|v| v.to_string()).unwrap_or_default());
let refresh = signal(w.refresh_minutes.to_string());
let days = signal(w.forecast_days.to_string());

let save: Box<dyn Fn()> = Box::new({
    let (enabled, location) = (enabled.clone(), location.clone());
    let (latitude, longitude) = (latitude.clone(), longitude.clone());
    let (refresh, days) = (refresh.clone(), days.clone());
    move || {
        // A blank coordinate is "not set", not zero: a stray empty field must fall back to the place name rather than pinning the forecast to the Gulf of Guinea.
        let optional = |raw: String| raw.trim().parse::<f32>().ok();
        let value = WeatherConfig {
            enabled: enabled.peek(),
            location: location.peek(),
            latitude: optional(latitude.peek()),
            longitude: optional(longitude.peek()),
            refresh_minutes: parse_u32(&refresh.peek(), base.refresh_minutes),
            forecast_days: parse_u32(&days.peek(), base.forecast_days),
        };
        persist(&path, "weather", &value);
    }
});

[view]
form_section title(|| telar::t!("settings.section.weather"))
    toggle_row label(|| telar::t!("settings.field.enabled")) value:$enabled
    text_row label(|| telar::t!("settings.field.location")) value:$location placeholder:"Madrid"
    text_row label(|| telar::t!("settings.field.latitude")) value:$latitude placeholder:"40.4168"
    text_row label(|| telar::t!("settings.field.longitude")) value:$longitude placeholder:"-3.7038"
    text_row label(|| telar::t!("settings.field.refresh_minutes")) value:$refresh placeholder:"15"
    text_row label(|| telar::t!("settings.field.forecast_days")) value:$days placeholder:"7"
    save_row label(|| telar::t!("settings.save.weather")) on_press(save)
