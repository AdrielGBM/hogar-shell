//! The config section a module reads its settings from, named by type, so the schema, the validator and a module's own build all read one declaration rather than a second description of the same keys.

use crate::Config;
use crate::sections::*;

/// A config section that is some module's options.
pub trait ModuleOptions: 'static {
    /// The section's key in `config.toml`, which is also its name in `hogar-shell config schema`.
    const SECTION: &'static str;

    fn section(config: &Config) -> &Self;
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
    widgets: WidgetsConfig,
    workspaces: WorkspacesConfig,
}
