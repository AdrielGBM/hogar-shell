[logic]
use ::config::Config;

// Readings, not fields — so it has no Save. The compositor and session lines are what a bug report needs first and what a user otherwise has to leave the shell to find.
/// A non-empty environment variable, which is the only kind worth reporting.
fn env_or_unknown(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

let version = env!("CARGO_PKG_VERSION").to_string();
let compositor = env_or_unknown("HYPRLAND_INSTANCE_SIGNATURE").map_or_else(
    || telar::t!("settings.about.not_hyprland"),
    |_| "Hyprland".to_string(),
);
let session = env_or_unknown("XDG_SESSION_TYPE").unwrap_or_else(|| telar::t!("common.unknown"));
let config_file = Config::default_path().display().to_string();

[view]
form_section title(|| telar::t!("settings.section.about"))
    reading_row label(|| telar::t!("settings.field.version")) value(move || version.clone())
    reading_row label(|| telar::t!("settings.field.compositor")) value(move || compositor.clone())
    reading_row label(|| telar::t!("settings.field.session")) value(move || session.clone())
    reading_row label(|| telar::t!("settings.field.config_file")) value(move || config_file.clone())
