//! The config section a module reads its settings from, named by type, so the schema, the validator and a module's own build all read one declaration rather than a second description of the same keys.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Config;
use crate::sections::*;

/// A config section that is some module's options.
pub trait ModuleOptions: Clone + Serialize + DeserializeOwned + 'static {
    /// The section's key in `config.toml`, which is also its name in `hogar-shell config schema`.
    const SECTION: &'static str;

    fn section(config: &Config) -> &Self;

    /// The section with an instance's own `options` over it: TA-2's cascade, module defaults under the instance's. Only the keys the section declares are read, so an instance's presentation keys (`accent`, `drawer_width`, …) are left to [`Config::presentation`].
    fn with_options(config: &Config, options: &toml::Table) -> Self {
        let section = Self::section(config);
        if options.is_empty() {
            return section.clone();
        }
        overlaid(section, options, crate::fields::keys_of(Self::SECTION))
    }
}

/// `base` with each of `options` written over it, a table deep into a table, for the keys in `keys` alone.
///
/// Key by key, so one value of the wrong type costs only itself: it is reported and skipped, and the rest of what the instance says still applies (principle 1). `layout check` names the bad key where it was written; this is what keeps the shell drawing in the meantime.
pub(crate) fn overlaid<T: Clone + Serialize + DeserializeOwned>(
    base: &T,
    options: &toml::Table,
    keys: &[&str],
) -> T {
    let Ok(toml::Value::Table(mut merged)) = toml::Value::try_from(base) else {
        return base.clone();
    };
    for (key, value) in options {
        if !keys.contains(&key.as_str()) {
            continue;
        }
        let mut tried = merged.clone();
        merge(&mut tried, key, value);
        match toml::Value::Table(tried.clone()).try_into::<T>() {
            Ok(_) => merged = tried,
            Err(why) => {
                tracing::warn!("option `{key}` = {value} is not one this module takes: {why}")
            }
        }
    }
    toml::Value::Table(merged)
        .try_into()
        .unwrap_or_else(|_| base.clone())
}

fn merge(into: &mut toml::Table, key: &str, value: &toml::Value) {
    match (into.get_mut(key), value) {
        (Some(toml::Value::Table(inner)), toml::Value::Table(over)) => {
            for (nested, value) in over {
                merge(inner, nested, value);
            }
        }
        _ => {
            into.insert(key.to_string(), value.clone());
        }
    }
}

macro_rules! module_options {
    ($($field:ident: $options:ty),* $(,)?) => {
        $(
            impl ModuleOptions for $options {
                const SECTION: &'static str = stringify!($field);

                fn section(config: &Config) -> &Self {
                    &config.$field
                }
            }
        )*

        #[cfg(test)]
        pub(crate) const MODULE_OPTIONS: &[(&str, &str)] = &[$((stringify!($field), stringify!($options))),*];
    };
}

module_options! {
    active_window: ActiveWindowConfig,
    audio: AudioConfig,
    battery: BatteryConfig,
    bluetooth: BluetoothConfig,
    brightness: BrightnessConfig,
    clock: ClockConfig,
    dashboard: DashboardConfig,
    general: GeneralConfig,
    gpu: GpuConfig,
    launcher: LauncherConfig,
    lock_status: LockStatusConfig,
    media: MediaConfig,
    network: NetworkConfig,
    notifications: NotificationsConfig,
    paths: PathsConfig,
    recorder: RecorderConfig,
    stack: StackConfig,
    status_icons: StatusIconsConfig,
    temperature: TemperatureConfig,
    tray: TrayConfig,
    utilities: UtilitiesConfig,
    visualiser: VisualiserConfig,
    weather: WeatherConfig,
    workspaces: WorkspacesConfig,
}
