//! What `config.toml` has to keep doing: the defaults, the monitor overrides, and the resolution rules a surface reads through.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use telar::Color;

    use crate::theme::NordTheme;
    use crate::*;

    /// `[launcher]` carries both an array of tables (`actions`) and a map (`icons`), and TOML requires every scalar to be emitted before either. Field order on the struct is what decides that, so a key added in the wrong place turns every launcher save into a serialize error the user only sees in the log.
    #[test]
    fn a_launcher_with_actions_and_icon_overrides_still_serialises() {
        let mut icons = HashMap::new();
        icons.insert("firefox".to_string(), "firefox-nightly".to_string());
        let launcher = LauncherConfig {
            actions: vec![LauncherAction {
                name: "Reload".to_string(),
                command: "hogar-shell shell reload".to_string(),
                ..LauncherAction::default()
            }],
            icons,
            enable_dangerous_actions: true,
            ..LauncherConfig::default()
        };
        let written = toml::to_string(&launcher).expect("a launcher section serialises");
        let back: LauncherConfig = toml::from_str(&written).expect("round-trips");
        assert_eq!(back.actions.len(), 1);
        assert_eq!(
            back.icons.get("firefox").map(String::as_str),
            Some("firefox-nightly")
        );
        assert!(back.enable_dangerous_actions);
    }

    #[test]
    fn an_icon_override_wins_only_when_it_says_something() {
        let mut icons = HashMap::new();
        icons.insert("firefox".to_string(), "firefox-nightly".to_string());
        // A row cleared back to empty must fall through to the desktop entry rather than blanking the icon.
        icons.insert("code".to_string(), "  ".to_string());
        let launcher = LauncherConfig {
            icons,
            ..LauncherConfig::default()
        };
        assert_eq!(launcher.icon_for("firefox", "firefox"), "firefox-nightly");
        assert_eq!(launcher.icon_for("code", "vscode"), "vscode");
        assert_eq!(launcher.icon_for("gimp", "gimp"), "gimp");
    }

    #[test]
    fn save_section_replaces_one_table_and_preserves_the_rest() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "# hand-written\n[theme]\nname = \"nord\"\naccent = \"cyan\"\n\n[icons]\ndefault_set = \"lucide\"\n",
        )
        .unwrap();

        let theme = ThemeConfig {
            accent: "orange".to_string(),
            ..ThemeConfig::default()
        };
        Config::save_section(&path, "theme", &theme).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# hand-written"), "top comment survives");
        assert!(
            out.contains("[icons]") && out.contains("lucide"),
            "the untouched section survives"
        );
        let reloaded: Config = toml::from_str(&out).unwrap();
        assert_eq!(
            reloaded.theme.accent, "orange",
            "the edited value persisted"
        );
        assert_eq!(
            reloaded.icons.default_set, "lucide",
            "the other section round-trips"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// **The trap in handing the write to a queue.** A save reads the file, replaces one table and writes the whole thing back, and the settings panel saves a section per form — so a save that returned before its bytes were on disk would leave the next one reading the file as it stood *before* it, and the first form's change would be gone. That is why `save_section` waits for the writer rather than queueing and returning: by the time it returns, the file is what the next reader sees, and the staging copy it went through is no longer in the directory the user browses.
    #[test]
    fn a_save_is_on_disk_by_the_time_it_returns_so_the_next_one_reads_it() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-save-run-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "# hand-written\n[theme]\naccent = \"cyan\"\n").unwrap();

        let mut theme = Config::load_or_default(&path).theme;
        theme.accent = "orange".to_string();
        Config::save_section(&path, "theme", &theme).unwrap();
        let mut icons = Config::load_or_default(&path).icons;
        icons.default_set = "lucide".to_string();
        Config::save_section(&path, "icons", &icons).unwrap();

        let reloaded = Config::load(&path).expect("the saved file parses");
        assert_eq!(
            reloaded.theme.accent, "orange",
            "the second save read the file after the first one, so the first one's change is still there"
        );
        assert_eq!(reloaded.icons.default_set, "lucide");

        let left: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(
            left,
            vec![path],
            "a finished save leaves no staging copy beside the config"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_a_section_keeps_its_sub_tables_under_it_instead_of_scattering_them() {
        // What this catches is not a parse failure — the scattered file still parses, which is why nothing saw it. Saving `[theme]` printed `[theme.export]` between unrelated sections, put `[theme.fonts.title]` inside another section's tables, and left `[theme]` itself *after* its own children. For a function whose whole promise is "preserving every other section, key order, and comment", that is the failure.
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-save-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[shape]\ngap = 8\n\n[panels]\ndrag_threshold = 32\n\n[theme]\nname = \"nord\"\n\n[workspaces]\nshown = 10\n",
        )
        .unwrap();

        Config::save_section(&path, "theme", &ThemeConfig::default()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let headers: Vec<&str> = text.lines().filter(|line| line.starts_with('[')).collect();
        let at = |name: &str| {
            headers
                .iter()
                .position(|h| *h == name)
                .unwrap_or_else(|| panic!("{name} missing from\n{text}"))
        };

        assert!(
            at("[theme]") < at("[theme.export]"),
            "a parent precedes its children:\n{text}"
        );
        assert!(at("[theme]") < at("[theme.scale]"), "{text}");
        assert!(at("[shape]") < at("[panels]"), "{text}");
        assert!(at("[panels]") < at("[theme]"), "{text}");
        assert!(
            headers[at("[panels]") + 1] == "[theme]",
            "a section of theme's leaked between [panels] and [theme]:\n{text}"
        );
        assert!(
            at("[workspaces]") > at("[theme.export]"),
            "an unrelated section was pushed in among theme's children:\n{text}"
        );
        let reloaded: Config = toml::from_str(&text).expect("the saved file parses");
        assert_eq!(reloaded.workspaces.shown, 10);
        assert_eq!(reloaded.panels.drag_threshold, 32.0);
        assert_eq!(reloaded.theme.name, "nord");

        std::fs::remove_dir_all(&dir).ok();
    }

    fn save_theme_over(initial: &str, theme: &ThemeConfig) -> String {
        let dir = std::env::temp_dir().join(format!(
            "hogar-shell-save-keys-{}-{}",
            std::process::id(),
            initial.len()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, initial).unwrap();
        Config::save_section(&path, "theme", theme).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        Config::load(&path).expect("the saved file loads");
        std::fs::remove_dir_all(&dir).ok();
        text
    }

    #[test]
    fn saving_a_section_keeps_the_comments_inside_it() {
        let text = save_theme_over(
            "[theme]\n# the palette\nname = \"nord\"\naccent = \"cyan\" # my accent\nmode = \"dark\"\n",
            &ThemeConfig {
                name: "nord".to_string(),
                accent: "orange".to_string(),
                mode: "light".to_string(),
                ..ThemeConfig::default()
            },
        );
        assert!(text.contains("# the palette\nname"), "{text}");
        assert!(text.contains("accent = \"orange\" # my accent"), "{text}");
        assert!(text.contains("mode = \"light\""), "{text}");
    }

    #[test]
    fn saving_a_section_does_not_write_keys_that_stay_unset() {
        let text = save_theme_over(
            "[theme]\nname = \"nord\"\n",
            &ThemeConfig {
                name: "nord".to_string(),
                ..ThemeConfig::default()
            },
        );
        assert!(!text.contains("radius"), "{text}");
        assert!(!text.contains("font_family"), "{text}");
    }

    #[test]
    fn saving_a_section_removes_a_key_the_value_no_longer_writes() {
        let text = save_theme_over(
            "[theme]\nname = \"nord\"\nradius = 6\n",
            &ThemeConfig {
                name: "nord".to_string(),
                radius: None,
                ..ThemeConfig::default()
            },
        );
        assert!(!text.contains("radius"), "{text}");
        assert!(text.contains("name = \"nord\""), "{text}");
    }

    #[test]
    fn saving_a_section_keeps_the_comments_in_its_sub_tables() {
        let text = save_theme_over(
            "[theme]\nname = \"nord\"\n\n# type scale\n[theme.scale]\n# bigger text\nfont = 1.5 # large\n",
            &ThemeConfig {
                name: "gruvbox".to_string(),
                scale: ScaleConfig {
                    font: 1.5,
                    spacing: 1.25,
                    ..ScaleConfig::default()
                },
                ..ThemeConfig::default()
            },
        );
        assert!(text.contains("# type scale\n[theme.scale]"), "{text}");
        assert!(text.contains("# bigger text\nfont = 1.5 # large"), "{text}");
        assert!(text.contains("spacing = 1.25"), "{text}");
        assert!(text.contains("name = \"gruvbox\""), "{text}");
    }

    #[test]
    fn edge_orientation() {
        assert!(Edge::Top.is_horizontal() && Edge::Bottom.is_horizontal());
        assert!(Edge::Left.is_vertical() && Edge::Right.is_vertical());
    }

    #[test]
    fn shape_defaults_reproduce_todays_bar() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.shape.frame);
        let shape = cfg.shape_from(None, None, None, None);
        assert_eq!(shape.mode, Shape::Bar);
        assert_eq!(shape.gap, 0);
        assert_eq!(shape.radius, 0.0, "the nord theme's default radius is 0");
        assert_eq!(cfg.gap_of(&shape), 0);
    }

    #[test]
    fn panels_and_open_mode_defaults() {
        let none = toml::Table::new();
        let cfg: Config = toml::from_str("").unwrap();
        let clock = cfg.presentation("clock", &none);
        assert_eq!(clock.drawer_size(), (320.0, 280.0));
        assert_eq!(clock.float_size("clock"), (360, 240));
        assert_eq!(clock.popout_size(), (264.0, 300.0));
        assert_eq!(clock.open_mode("clock"), OpenMode::Drawer);
        let settings = cfg.presentation("settings", &none);
        assert_eq!(
            settings.open_mode("settings"),
            OpenMode::Float,
            "an application panel floats"
        );
        assert_eq!(settings.float_size("settings"), (920, 680));

        let floaty: Config = toml::from_str(
            "[modules.clock]\nopen = \"float\"\ndrawer_width = 400\nfloat_width = 480\nfloat_height = 320\n\
             [modules.settings]\nvariant = \"filled\"\n",
        )
        .unwrap();
        let clock = floaty.presentation("clock", &none);
        assert_eq!(clock.open_mode("clock"), OpenMode::Float);
        assert_eq!(clock.drawer_size().0, 400.0);
        assert_eq!(clock.float_size("clock"), (480, 320));
        let settings = floaty.presentation("settings", &none);
        assert_eq!(
            settings.open_mode("settings"),
            OpenMode::Float,
            "a `[modules.settings]` that says nothing about `open` leaves it floating"
        );

        let instance: toml::Table =
            toml::from_str("drawer_width = 500\nopen = \"drawer\"\n").unwrap();
        let placed = floaty.presentation("clock", &instance);
        assert_eq!(placed.drawer_size().0, 500.0, "an instance's own size wins");
        assert_eq!(placed.open_mode("clock"), OpenMode::Drawer);
        assert_eq!(
            placed.float_size("clock"),
            (480, 320),
            "and the module's fills in the rest"
        );
    }

    #[test]
    fn starter_config_round_trips_through_toml() {
        // load_or_default writes the starter to disk on first run, so it must serialize and re-parse cleanly.
        let starter = Config::starter();
        let text = toml::to_string_pretty(&starter).expect("starter serializes");
        let parsed: Config = toml::from_str(&text).expect("starter re-parses");
        assert_eq!(parsed.panels.drag_threshold, starter.panels.drag_threshold);
        assert_eq!(
            parsed.notifications.sidebar_size,
            starter.notifications.sidebar_size
        );
        // An unset coordinate is the one field type TOML has no value for, so it is the one that would break the write of a fresh config rather than merely round-trip oddly.
        assert_eq!(parsed.weather.latitude, None);
        assert_eq!(
            parsed.weather.refresh_minutes,
            starter.weather.refresh_minutes
        );
        assert_eq!(parsed.gpu.backend, "auto");
        assert!(parsed.paths.wallpapers.is_empty());
        assert_eq!(parsed.bluetooth.max_devices, starter.bluetooth.max_devices);
        assert_eq!(
            parsed.network.rescan_seconds,
            starter.network.rescan_seconds
        );
        assert_eq!(parsed.media.seek_seconds, starter.media.seek_seconds);
    }

    /// A6: every section that can start a background producer carries `enabled`, defaults it to on, and reads it back off a written config. A section that gained a service but not the flag would have no way to be switched off short of removing the module from the bar.
    #[test]
    fn every_service_section_can_be_switched_off() {
        // Each section's own `Default`, not `Config::default()` — the latter is all-empty by design, since it is what backs serde's missing-field fill.
        for on in [
            NetworkConfig::default().enabled,
            BluetoothConfig::default().enabled,
            GpuConfig::default().enabled,
            WeatherConfig::default().enabled,
        ] {
            assert!(on, "a service section is on unless the user says otherwise");
        }

        let off: Config = toml::from_str(
            "[network]\nenabled=false\n[bluetooth]\nenabled=false\n\
             [gpu]\nenabled=false\n[weather]\nenabled=false\n",
        )
        .expect("parses");
        assert!(!off.network.enabled);
        assert!(!off.bluetooth.enabled);
        assert!(!off.gpu.enabled);
        assert!(!off.weather.enabled);
        // And the flag survives a save, so switching one off in the settings panel sticks.
        let round_tripped: Config =
            toml::from_str(&toml::to_string_pretty(&off).expect("serializes")).expect("re-parses");
        assert!(!round_tripped.weather.enabled);
        assert!(!round_tripped.network.enabled);
    }

    #[test]
    fn theme_config_overrides_colors_and_numbers() {
        let cfg: Config = toml::from_str(
            "[theme]\nname=\"custom\"\nradius=12\nfont_size=16\n[theme.colors]\nbase=\"#101010\"\naccent=\"#ff8800\"\n",
        )
        .unwrap();
        let theme = cfg.resolve_theme();
        assert_eq!(theme.radius, 12.0);
        assert_eq!(theme.font_size, 16.0);
        assert_eq!(theme.base, Color::from_hex("#101010").unwrap());
        assert_eq!(theme.accent, Color::from_hex("#ff8800").unwrap());
        // An unset token keeps the built-in value.
        assert_eq!(theme.text, NordTheme::new().text);
        // The [theme] number override also backs the shape resolution.
        assert_eq!(cfg.shape_from(None, None, None, None).radius, 12.0);
    }

    #[test]
    fn theme_config_parses_font_family_and_icon_stroke() {
        let cfg: Config =
            toml::from_str("[theme]\nfont_family = \"JetBrains Mono\"\nicon_stroke = 1.5\n")
                .unwrap();
        // font_family stays in config (applied process-wide, not carried in the Copy theme struct).
        assert_eq!(cfg.theme.font_family.as_deref(), Some("JetBrains Mono"));
        // icon_stroke flows into the resolved theme so icon_view can read it.
        assert_eq!(cfg.resolve_theme().icon_stroke, Some(1.5));
        let bare: Config = toml::from_str("").unwrap();
        assert_eq!(bare.theme.font_family, None);
        assert_eq!(bare.resolve_theme().icon_stroke, None);
    }

    #[test]
    fn a_weight_per_role_reaches_that_roles_text_and_no_other() {
        use crate::theme::{FONT_WEIGHT_RANGE, FontRole};

        let cfg: Config = toml::from_str(
            "[theme.fonts.title]\nweight = 650\n[theme.fonts.caption]\nweight = 2000\n",
        )
        .unwrap();
        let theme = cfg.resolve_theme();
        let weight = |role| theme.text_style(role, Color::WHITE).font_weight;
        assert_eq!(cfg.theme.fonts.spec(FontRole::Title).weight, Some(650));
        assert_eq!(weight(FontRole::Title), 650);
        assert_eq!(weight(FontRole::Caption), *FONT_WEIGHT_RANGE.end());
        let bare = Config::default().resolve_theme();
        assert_eq!(
            weight(FontRole::Body),
            bare.text_style(FontRole::Body, Color::WHITE).font_weight
        );
        let mut fonts = FontsConfig::default();
        fonts.spec_mut(FontRole::Display).weight = Some(300);
        assert_eq!(fonts.display.weight, Some(300));
    }

    #[test]
    fn the_icon_mask_defaults_to_none_and_parses_each_shape() {
        let bare: Config = toml::from_str("").unwrap();
        assert_eq!(bare.icons.mask, IconMask::None);
        for mask in IconMask::ALL {
            let cfg: Config =
                toml::from_str(&format!("[icons]\nmask = \"{}\"\n", mask.id())).unwrap();
            assert_eq!(cfg.icons.mask, mask);
            assert_eq!(IconMask::from_id(mask.id()), Some(mask));
        }
        assert!(toml::from_str::<Config>("[icons]\nmask = \"star\"\n").is_err());
        assert_eq!(IconMask::from_id("star"), None);
        let written = toml::to_string(&IconsConfig {
            mask: IconMask::Squircle,
            ..IconsConfig::default()
        })
        .unwrap();
        assert!(written.contains("mask = \"squircle\""), "{written}");
    }

    #[test]
    fn spacing_and_radius_fall_back_to_the_theme() {
        let theme = NordTheme::new();
        let bare: Config = toml::from_str("").unwrap();
        let unset = bare.shape_from(None, None, None, None);
        assert_eq!(unset.radius, theme.radius);
        assert_eq!(unset.spacing, theme.spacing);
        let cfg: Config = toml::from_str("[shape]\nradius=10\nspacing=4\n").unwrap();
        let area = cfg.shape_from(None, None, None, Some(2));
        assert_eq!(area.radius, 2.0, "the area's own radius wins");
        assert_eq!(
            area.spacing, theme.spacing,
            "a retired `[shape] spacing` is no fallback: the theme is"
        );
    }

    #[test]
    fn clock_format_follows_the_twelve_hour_switch_unless_overridden() {
        let d = ClockConfig::default();
        assert_eq!(d.time_format(), "%H:%M:%S");

        let twelve: Config = toml::from_str("[clock]\ntwelve_hour = true\n").unwrap();
        assert_eq!(twelve.clock.time_format(), "%I:%M:%S %p");

        let explicit: Config =
            toml::from_str("[clock]\ntwelve_hour = true\nformat = \"%H:%M\"\n").unwrap();
        assert_eq!(
            explicit.clock.time_format(),
            "%H:%M",
            "an explicit pattern wins over the switch"
        );
    }

    #[test]
    fn active_window_defaults_show_the_title_with_its_icon() {
        let d: Config = toml::from_str("").unwrap();
        assert!(d.active_window.show_icon);
        assert!(!d.active_window.compact);

        let cfg: Config = toml::from_str("[active_window]\ncompact = true\n").unwrap();
        assert!(cfg.active_window.compact);
        assert!(
            cfg.active_window.show_icon,
            "unset fields keep their defaults"
        );

        // `max_chars` used to bound the title here and is gone; a config still carrying it must load, not fail.
        let stale: Config =
            toml::from_str("[active_window]\ncompact = true\nmax_chars = 60\n").unwrap();
        assert!(stale.active_window.compact);
    }

    #[test]
    fn audio_and_brightness_steps_are_configurable_and_bounded() {
        let d: Config = toml::from_str("").unwrap();
        assert_eq!(d.audio.step(), 5);
        assert_eq!(d.audio.ceiling(), 150);
        assert_eq!(d.brightness.step(), 5);

        let cfg: Config = toml::from_str(
            "[audio]\nincrement = 2\nmax_volume = 100\n[brightness]\nincrement = 10\n",
        )
        .unwrap();
        assert_eq!(cfg.audio.step(), 2);
        assert_eq!(cfg.audio.ceiling(), 100);
        assert_eq!(cfg.brightness.step(), 10);

        // A typo must not leave the wheel inert, run it backwards, or let one notch cross the whole range.
        let broken: Config = toml::from_str(
            "[audio]\nincrement = 0\nmax_volume = 10\n[brightness]\nincrement = -5\n",
        )
        .unwrap();
        assert_eq!(broken.audio.step(), 1);
        assert_eq!(
            broken.audio.ceiling(),
            100,
            "a sink must reach its own maximum"
        );
        assert_eq!(broken.brightness.step(), 1);
    }

    #[test]
    fn temperature_unit_converts_and_labels_the_reading() {
        let d: Config = toml::from_str("").unwrap();
        assert_eq!(d.temperature.unit, TemperatureUnit::Celsius);
        assert!(
            d.temperature.sensor.is_empty(),
            "empty tracks the hottest sensor"
        );
        assert_eq!(d.temperature.warn, 70.0);
        assert_eq!(d.temperature.critical, 85.0);
        assert_eq!(d.temperature.unit.format(61.5), "62°C");

        let cfg: Config = toml::from_str(
            "[temperature]\nunit = \"fahrenheit\"\nsensor = \"k10temp\"\nwarn = 80\n",
        )
        .unwrap();
        assert_eq!(cfg.temperature.sensor, "k10temp");
        assert_eq!(cfg.temperature.warn, 80.0);
        assert_eq!(
            cfg.temperature.critical, 85.0,
            "unset fields keep their defaults"
        );
        assert_eq!(cfg.temperature.unit.from_celsius(100.0), 212.0);
        assert_eq!(cfg.temperature.unit.format(20.0), "68°F");
    }

    #[test]
    fn a_reading_is_as_severe_as_the_highest_threshold_it_reaches() {
        let d = TemperatureConfig::default();
        assert_eq!(d.severity(69.9), Severity::Normal);
        assert_eq!(
            d.severity(70.0),
            Severity::Warn,
            "the threshold itself warns"
        );
        assert_eq!(d.severity(85.0), Severity::Critical);
        let inverted = TemperatureConfig {
            warn: 90.0,
            critical: 80.0,
            ..TemperatureConfig::default()
        };
        assert_eq!(
            inverted.severity(85.0),
            Severity::Critical,
            "critical wins even over a warn set above it"
        );
    }

    #[test]
    fn lock_status_shows_both_keys_until_told_otherwise() {
        let d: Config = toml::from_str("").unwrap();
        assert!(d.lock_status.caps && d.lock_status.num);
        assert!(
            !d.lock_status.hide_inactive,
            "an indicator nobody can see until they press the key is not discoverable"
        );

        let cfg: Config =
            toml::from_str("[lock_status]\nnum = false\nhide_inactive = true\n").unwrap();
        assert!(cfg.lock_status.caps);
        assert!(!cfg.lock_status.num);
        assert!(cfg.lock_status.hide_inactive);
    }

    #[test]
    fn battery_ships_with_warnings_and_never_acts_unasked() {
        let d: Config = toml::from_str("").unwrap();
        assert!(d.battery.enabled);
        assert_eq!(
            d.battery
                .warn_levels
                .iter()
                .map(|w| w.level)
                .collect::<Vec<_>>(),
            vec![20, 10],
            "a laptop shell that silently runs a battery flat is a bug"
        );
        assert_eq!(
            d.battery.critical_level, 0,
            "suspending the machine is opt-in, not a default"
        );
        assert!(d.battery.critical_action.is_empty());

        let cfg: Config = toml::from_str(
            "[battery]\ncritical_level = 3\ncritical_action = \"suspend\"\n\
             [[battery.warn_levels]]\nlevel = 15\ntitle = \"Low\"\ncritical = true\n",
        )
        .unwrap();
        assert_eq!(cfg.battery.critical_level, 3);
        assert_eq!(cfg.battery.critical_action, "suspend");
        assert_eq!(
            cfg.battery.warn_levels.len(),
            1,
            "declaring thresholds replaces the defaults rather than adding to them"
        );
        assert_eq!(cfg.battery.warn_levels[0].title(15), "Low");
    }

    #[test]
    fn a_section_holding_an_array_of_tables_survives_a_save() {
        // `[[battery.warn_levels]]` is the first list-of-tables in the config, and TOML only accepts a table's scalar keys *before* its arrays of tables — a naive serializer would emit `critical_level` inside the last warning. Both the whole-file write and the format-preserving per-section save must get it right.
        let dir = std::env::temp_dir().join(format!("hogar-shell-aot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "# kept\n[theme]\naccent = \"orange\"\n").unwrap();

        let battery = BatteryConfig {
            critical_level: 4,
            critical_action: "suspend".to_string(),
            ..BatteryConfig::default()
        };
        Config::save_section(&path, "battery", &battery).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# kept"), "the untouched file survives");
        let reloaded: Config = toml::from_str(&out).expect("what was written parses back");
        assert_eq!(reloaded.battery.critical_level, 4);
        assert_eq!(reloaded.battery.critical_action, "suspend");
        assert_eq!(reloaded.battery.warn_levels.len(), 2);
        assert_eq!(reloaded.battery.warn_levels[1].level, 10);
        assert_eq!(
            reloaded.theme.accent, "orange",
            "the other section is untouched"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workspace_labels_specialise_by_state_and_fall_back_to_the_general_one() {
        let cfg = WorkspacesConfig {
            label: "{id}".to_string(),
            occupied_label: "•{id}".to_string(),
            active_label: "[{id}]".to_string(),
            ..WorkspacesConfig::default()
        };
        assert_eq!(cfg.render_label(3, "3", 2, false, false), "3");
        assert_eq!(cfg.render_label(3, "3", 2, true, false), "•3");
        assert_eq!(cfg.render_label(3, "3", 2, true, true), "[3]");
        assert_eq!(
            cfg.render_label(3, "3", 2, false, true),
            "[3]",
            "the active template wins whether or not the workspace holds windows"
        );

        // Setting only `active_label` leaves every other pill rendering the general template.
        let only_active = WorkspacesConfig {
            active_label: "<{id}>".to_string(),
            ..WorkspacesConfig::default()
        };
        assert_eq!(only_active.render_label(2, "2", 1, true, false), "2");
        assert_eq!(only_active.render_label(2, "2", 1, true, true), "<2>");

        // And an active pill with only `occupied_label` set takes that rather than dropping to `label`.
        let only_occupied = WorkspacesConfig {
            occupied_label: "•{id}".to_string(),
            ..WorkspacesConfig::default()
        };
        assert_eq!(only_occupied.render_label(2, "2", 1, true, true), "•2");
    }

    #[test]
    fn capitalisation_applies_after_the_template() {
        let cfg = WorkspacesConfig {
            label: "{name}".to_string(),
            capitalize: Capitalize::Title,
            ..WorkspacesConfig::default()
        };
        assert_eq!(
            cfg.render_label(1, "my WEB workspace", 0, false, false),
            "My Web Workspace"
        );

        assert_eq!(Capitalize::None.apply("mixed Case"), "mixed Case");
        assert_eq!(Capitalize::Upper.apply("code"), "CODE");
        assert_eq!(Capitalize::Lower.apply("CODE"), "code");
        assert_eq!(
            Capitalize::Title.apply("my-notes  2"),
            "My-notes  2",
            "separators and runs of whitespace survive intact"
        );
        assert_eq!(Capitalize::Title.apply(""), "");
    }

    #[test]
    fn a_glob_anchors_both_ends_and_only_a_star_spans() {
        assert!(glob_matches("nm-applet", "nm-applet"));
        assert!(
            !glob_matches("nm-applet", "nm-applet-2"),
            "a pattern without a star is a whole-string match"
        );
        assert!(glob_matches("steam_app_*", "steam_app_12345"));
        assert!(glob_matches("*applet", "nm-applet"));
        assert!(glob_matches("chrome*icon*", "chrome_status_icon_1"));
        assert!(
            !glob_matches("chrome*icon", "chrome_status_icon_1"),
            "a trailing literal anchors the end"
        );
        assert!(glob_matches("*", "anything at all"));
        assert!(
            glob_matches("NM-Applet", "nm-applet"),
            "matching ignores case"
        );

        // The two anchors must not overlap: `a*t` needs at least `at`, not just `a`.
        assert!(glob_matches("a*t", "at"));
        assert!(!glob_matches("nm*applet", "nm-apple"));
    }

    #[test]
    fn tray_hiding_and_icon_substitution_match_ids_as_patterns() {
        let cfg: Config = toml::from_str(
            "[tray]\nhidden = [\"steam_app_*\", \"blueman\"]\n\
             [tray.icon_subs]\n\"nm-applet\" = \"mdi:wifi\"\n\"*\" = \"mdi:apps\"\n",
        )
        .unwrap();
        assert!(cfg.tray.is_hidden("steam_app_440"));
        assert!(cfg.tray.is_hidden("blueman"));
        assert!(!cfg.tray.is_hidden("nm-applet"));

        assert_eq!(
            cfg.tray.icon_sub_for("nm-applet"),
            Some("mdi:wifi"),
            "the specific pattern beats the catch-all whatever the map's order"
        );
        assert_eq!(cfg.tray.icon_sub_for("anything-else"), Some("mdi:apps"));
        assert_eq!(TrayConfig::default().icon_sub_for("nm-applet"), None);
    }

    #[test]
    fn the_tray_is_on_by_default_and_hides_nothing() {
        let d: Config = toml::from_str("").unwrap();
        assert!(d.tray.enabled);
        assert!(d.tray.hidden.is_empty() && d.tray.icon_subs.is_empty());
        assert!(
            !d.tray.recolour,
            "tinting every icon would flatten an application that reports state in colour"
        );
        assert!(!d.tray.compact && !d.tray.background);
    }

    #[test]
    fn general_defaults_detect_the_distribution_logo() {
        let d: Config = toml::from_str("").unwrap();
        assert!(d.general.logo.is_empty(), "an empty logo means auto-detect");
    }

    #[test]
    fn a_parse_error_is_returned_rather_than_swallowed() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[clock\nformat = \"%H\"\n").unwrap();

        let error = Config::load(&path).expect_err("a malformed file must not parse");
        assert!(
            matches!(error, LoadError::Parse(_)),
            "the caller needs to distinguish a typo from a missing file"
        );
        // `load_or_default` is the lossy convenience wrapper — it answers a typo with the starter config, throwing the user's settings away. That is exactly why the running shell uses `load`: so it can keep the last config that worked and report the error instead.
        let lossy = Config::load_or_default(&path);
        assert_eq!(
            lossy.clock.format,
            Config::starter().clock.format,
            "the wrapper substitutes the starter, losing whatever the user had"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_seeds_the_starter_config_on_disk() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let seeded = Config::load(&path).expect("a fresh install is not an error");
        assert_eq!(seeded.clock.format, Config::starter().clock.format);
        assert!(path.exists(), "the starter is written for the user to edit");
        assert!(
            Config::load(&path).is_ok(),
            "and what was written parses back"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_stack_defaults_to_sensible_limits() {
        let d: Config = toml::from_str("").unwrap();
        assert_eq!(d.stack.max_visible, 4);
        assert!(d.notifications.critical_sticky);

        let cfg: Config = toml::from_str("[stack]\nmax_visible = 2\ntimeout_ms = 400\n").unwrap();
        assert_eq!(cfg.stack.max_visible, 2);
        assert!(
            cfg.notifications.critical_sticky,
            "unset fields keep defaults"
        );
    }

    #[test]
    fn the_notification_panel_settings_default_to_grouped_and_silent() {
        let d = NotificationsConfig::default();
        assert!(d.group_by_app);
        assert_eq!(d.group_preview(), 3);
        assert!(
            d.action_on_click,
            "a tap opens what the notification is about"
        );
        assert_eq!(d.body_max_lines(), Some(4));
        assert_eq!(
            d.sound_command(),
            None,
            "a shell that started making noise on upgrade would be a bug"
        );
        assert_eq!(
            d.fullscreen,
            FullscreenPopups::Off,
            "a fullscreen window is not interrupted unless it matters"
        );

        let zeroed = NotificationsConfig {
            group_preview_num: 0,
            body_lines: 0,
            sound: "   ".to_string(),
            ..NotificationsConfig::default()
        };
        assert_eq!(zeroed.group_preview(), 1);
        assert_eq!(zeroed.body_max_lines(), Some(1));
        assert_eq!(
            zeroed.sound_command(),
            None,
            "a whitespace-only command is silent"
        );

        let expanded = NotificationsConfig {
            open_expanded: true,
            ..NotificationsConfig::default()
        };
        assert_eq!(expanded.body_max_lines(), None, "the whole body, uncapped");
    }

    #[test]
    fn the_fullscreen_policy_reads_as_the_three_words_it_writes() {
        let parsed = |value: &str| {
            toml::from_str::<Config>(&format!("[notifications]\nfullscreen = \"{value}\"\n"))
                .unwrap()
                .notifications
                .fullscreen
        };
        assert_eq!(parsed("on"), FullscreenPopups::On);
        assert_eq!(parsed("off"), FullscreenPopups::Off);
        assert_eq!(parsed("never"), FullscreenPopups::Never);

        let round_tripped = toml::to_string(&NotificationsConfig {
            fullscreen: FullscreenPopups::Never,
            ..NotificationsConfig::default()
        })
        .unwrap();
        assert!(
            round_tripped.contains("fullscreen = \"never\""),
            "{round_tripped}"
        );
    }

    /// A config directory with a global file and, optionally, one monitor override.
    fn config_dir(name: &str, global: &str, monitor: Option<(&str, &str)>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hogar-shell-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), global).unwrap();
        if let Some((output, text)) = monitor {
            let out_dir = dir.join("monitors").join(output);
            std::fs::create_dir_all(&out_dir).unwrap();
            std::fs::write(out_dir.join("config.toml"), text).unwrap();
        }
        dir
    }

    #[test]
    fn a_monitor_override_merges_over_the_global_config_key_by_key() {
        let dir = config_dir(
            "monitor-merge",
            r#"
[tray]
enabled = true
compact = true
hidden = ["spotify"]

[theme]
accent = "cyan"
name = "nord"
"#,
            Some((
                "DP-2",
                r#"
[tray]
enabled = false
hidden = ["spotify", "discord"]

[theme]
accent = "orange"
"#,
            )),
        );
        let path = dir.join("config.toml");

        let global = Config::for_output(&path, None).unwrap();
        assert!(global.tray.enabled);
        assert_eq!(global.tray.hidden, ["spotify"]);
        assert_eq!(global.theme.accent, "cyan");

        let overridden = Config::for_output(&path, Some("DP-2")).unwrap();
        assert!(!overridden.tray.enabled, "the override wins");
        assert_eq!(
            overridden.tray.hidden,
            ["spotify", "discord"],
            "an array replaces rather than concatenating"
        );
        assert!(
            overridden.tray.compact,
            "a key the override never mentions keeps the global value"
        );
        assert_eq!(overridden.theme.accent, "orange");
        assert_eq!(
            overridden.theme.name, "nord",
            "merging is per key, not per section"
        );

        let unknown = Config::for_output(&path, Some("HDMI-A-1")).unwrap();
        assert!(
            unknown.tray.enabled,
            "a screen with no file is the global config"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_monitor_override_cannot_change_a_section_one_process_owns() {
        let dir = config_dir(
            "monitor-global-only",
            "[general]\nlanguage = \"en\"\n\n[shape]\nframe = false\n",
            Some((
                "DP-1",
                "[general]\nlanguage = \"es\"\n\n[audio]\nmax_volume = 200\n\n[shape]\nframe = true\n",
            )),
        );
        let path = dir.join("config.toml");
        let cfg = Config::for_output(&path, Some("DP-1")).unwrap();

        assert_eq!(cfg.general.language, "en", "[general] is global-only");
        assert_eq!(
            cfg.audio.max_volume,
            AudioConfig::default().max_volume,
            "[audio] is global-only — one volume service is shared across every output"
        );
        assert!(
            cfg.shape.frame,
            "a visual section is still the monitor's to set"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn animation_durations_scale_together_and_collapse_when_switched_off() {
        let base = Duration::from_millis(200);
        let d = AnimationConfig::default();
        assert_eq!(d.duration(base), base, "the default scale moves nothing");

        let quick = AnimationConfig {
            duration_scale: 0.5,
            ..AnimationConfig::default()
        };
        assert_eq!(quick.duration(base), Duration::from_millis(100));

        let off = AnimationConfig {
            enabled: false,
            duration_scale: 4.0,
            ..AnimationConfig::default()
        };
        assert_eq!(
            off.duration(base),
            Duration::ZERO,
            "off wins over any scale — it is the accessibility answer, not a speed"
        );

        // Bounded, so a `0` cannot make everything instant by accident rather than by the switch that says so.
        let broken = AnimationConfig {
            duration_scale: 0.0,
            ..AnimationConfig::default()
        };
        assert_eq!(broken.duration(base), Duration::from_millis(20));
        let nan = AnimationConfig {
            duration_scale: f32::NAN,
            ..AnimationConfig::default()
        };
        assert_eq!(nan.duration(base), base, "an unusable factor is no factor");
    }

    #[test]
    fn the_autohide_duration_is_the_bars_own_and_scales_with_the_rest() {
        let slow: Config = toml::from_str("[animation]\nautohide_duration_ms = 400\n").unwrap();
        assert_eq!(
            slow.animation.autohide_tween(),
            telar::motion::tween(Duration::from_millis(400), telar::motion::Easing::EaseOut)
        );
        assert_eq!(
            slow.animation.panel_tween().duration,
            Duration::from_millis(180),
            "a panel keeps its own duration"
        );

        let quick = AnimationConfig {
            autohide_duration_ms: 400,
            duration_scale: 0.5,
            easing: "linear".to_string(),
            ..AnimationConfig::default()
        };
        assert_eq!(
            quick.autohide_tween(),
            telar::motion::tween(Duration::from_millis(200), telar::motion::Easing::Linear)
        );
        let off = AnimationConfig {
            enabled: false,
            ..quick
        };
        assert_eq!(off.autohide_tween().duration, Duration::ZERO);
    }

    #[test]
    fn reduced_motion_makes_travel_instant_and_holds_every_fade_short() {
        let config: Config =
            toml::from_str("[animation]\nreduced = \"on\"\npanel_duration_ms = 400\n").unwrap();
        let reduced = config.animation;
        assert!(reduced.is_reduced());
        assert_eq!(
            reduced.panel_tween().duration,
            AnimationConfig::REDUCED_CEILING
        );
        assert_eq!(
            reduced.tween_ms(60, 2_000).duration,
            Duration::from_millis(60),
            "a fade already shorter than the ceiling keeps its length"
        );
        assert_eq!(reduced.autohide_tween().duration, Duration::ZERO);
        assert_eq!(reduced.travel_tween_ms(200, 2_000).duration, Duration::ZERO);

        let kept = AnimationConfig {
            reduced: ReducedMotion::Off,
            panel_duration_ms: 400,
            ..AnimationConfig::default()
        };
        assert!(!kept.is_reduced());
        assert_eq!(kept.panel_tween().duration, Duration::from_millis(400));
        assert_eq!(kept.autohide_tween().duration, Duration::from_millis(160));
    }

    #[test]
    fn auto_follows_the_desktop_and_an_override_does_not() {
        for desktop in [false, true] {
            assert_eq!(ReducedMotion::Auto.applies(desktop), desktop);
            assert!(ReducedMotion::On.applies(desktop));
            assert!(!ReducedMotion::Off.applies(desktop));
        }
        assert_eq!(AnimationConfig::default().reduced, ReducedMotion::Auto);
        for reduced in ReducedMotion::ALL {
            assert_eq!(ReducedMotion::from_id(reduced.id()), Some(reduced));
        }
        assert_eq!(ReducedMotion::from_id("sometimes"), None);
        assert!(toml::from_str::<Config>("[animation]\nreduced = \"sometimes\"\n").is_err());
    }

    #[test]
    fn the_two_named_curve_families_resolve_and_fall_back() {
        let with = |curve: &str, easing: &str| AnimationConfig {
            curve: curve.to_string(),
            easing: easing.to_string(),
            ..AnimationConfig::default()
        };
        assert_eq!(with("snappy", "").spring(), telar::motion::Spring::snappy());
        assert_eq!(with("BOUNCY", "").spring(), telar::motion::Spring::bouncy());
        assert_eq!(
            with("nonsense", "").spring(),
            telar::motion::Spring::gentle(),
            "an unknown name is the default, not a panic"
        );
        assert_eq!(with("", "linear").easing(), telar::motion::Easing::Linear);
        assert_eq!(
            with("", "ease_in_out").easing(),
            telar::motion::Easing::EaseInOut
        );
        assert_eq!(
            with("", "nonsense").easing(),
            telar::motion::Easing::EaseOut
        );
    }

    #[test]
    fn a_per_role_font_override_changes_only_the_role_it_names() {
        use crate::theme::FontRole;

        let cfg: Config = toml::from_str(
            "[theme]\nfont_size = 13.0\n\n[theme.fonts.caption]\nsize = 20.0\nweight = 700\nitalic = true\n",
        )
        .unwrap();
        let theme = cfg.resolve_theme();
        assert_eq!(
            theme.font(FontRole::Caption),
            20.0,
            "the named role takes the override"
        );
        assert_eq!(
            theme.font(FontRole::Body),
            13.0,
            "and every other role is untouched"
        );

        let styled = theme.text_style(FontRole::Caption, theme.text);
        assert_eq!(styled.font_weight, 700);
        assert_eq!(styled.font_style, telar::FontStyle::Italic);
        let plain = theme.text_style(FontRole::Body, theme.text);
        assert_eq!(
            plain.font_weight, 400,
            "a role with no override keeps the default weight"
        );
        assert_eq!(plain.font_style, telar::FontStyle::Normal);

        // Bounded on read: a size a screen cannot render is not a size.
        let absurd: Config = toml::from_str("[theme.fonts.body]\nsize = 100000.0\n").unwrap();
        assert_eq!(absurd.resolve_theme().font(FontRole::Body), 200.0);
    }

    /// One key for the whole shell, and no way to break it apart. `[bars] opacity` and `[panels] opacity` existed and were removed: the only thing they bought was a drawer at an opacity the bar it hangs off does not share, which nobody configures on purpose and which two settings drift into on their own.
    #[test]
    fn one_key_sets_the_opacity_of_every_surface() {
        let translucent = Config {
            theme: ThemeConfig {
                opacity: 0.8,
                ..ThemeConfig::default()
            },
            ..Config::starter()
        };
        assert_eq!(translucent.opacity(), 0.8);
        assert_eq!(translucent.panel_fill().a, 0.8, "and panels, from one key");

        // The floor applies to the general key too, or one number makes the whole shell unusable at once.
        let ghost = Config {
            theme: ThemeConfig {
                opacity: 0.0,
                ..ThemeConfig::default()
            },
            ..Config::starter()
        };
        assert_eq!(ghost.opacity(), 0.2);
        assert_eq!(ghost.panel_fill().a, 0.2);

        let broken = Config {
            theme: ThemeConfig {
                opacity: f32::NAN,
                ..ThemeConfig::default()
            },
            ..Config::starter()
        };
        assert_eq!(broken.opacity(), 1.0, "an unusable value is no value");
    }

    /// A per-surface opacity key is gone rather than deprecated, and a file that still carries one must not take the whole config down with it — an unknown key is ignored, so the shell comes up solid.
    #[test]
    fn a_config_still_naming_a_per_surface_opacity_still_loads() {
        let old: Config =
            toml::from_str("[bars]\nopacity = 0.5\n[panels]\nopacity = 0.75\ngap = 4\n")
                .expect("a removed key is ignored, not an error");
        assert_eq!(old.opacity(), 1.0);
        assert_eq!(old.panel_fill().a, 1.0);
    }

    #[test]
    fn the_panel_background_is_solid_by_default_and_never_fades_past_readable() {
        let solid = Config::starter();
        assert_eq!(
            solid.panel_fill().a,
            1.0,
            "a panel is opaque unless asked otherwise"
        );
        assert_eq!(
            solid.panel_fill().to_rgba8(),
            solid.resolve_theme().surface.to_rgba8(),
            "and it is exactly the surface token, so nothing changes for a config that never sets it"
        );

        let translucent = Config {
            theme: ThemeConfig {
                opacity: 0.75,
                ..ThemeConfig::default()
            },
            ..Config::starter()
        };
        assert_eq!(translucent.panel_fill().a, 0.75);
    }

    #[test]
    fn the_two_drag_thresholds_are_bounded_and_switch_off_at_zero() {
        // Drag-to-open: floored well above the tap slop, so an unsteady click cannot cross it.
        assert_eq!(PanelsConfig::default().drag_threshold(), Some(48.0));
        let off = PanelsConfig {
            drag_threshold: 0.0,
        };
        assert_eq!(off.drag_threshold(), None);
        let tiny = PanelsConfig {
            drag_threshold: 1.0,
        };
        assert_eq!(tiny.drag_threshold(), Some(16.0));
        let nan = PanelsConfig {
            drag_threshold: f32::NAN,
        };
        assert_eq!(nan.drag_threshold(), None);

        // Swipe-to-dismiss: a fraction of the card, never the whole width — an unreachable threshold reads as a card that is stuck rather than as a setting that is wrong. The column's, since every card in it answers to the one gesture.
        let s = StackConfig::default();
        assert_eq!(s.swipe_distance(400.0), Some(140.0));
        let full = StackConfig {
            clear_threshold: 2.0,
            ..StackConfig::default()
        };
        assert_eq!(full.swipe_distance(400.0), Some(360.0));
        let disabled = StackConfig {
            clear_threshold: 0.0,
            ..StackConfig::default()
        };
        assert_eq!(disabled.swipe_distance(400.0), None);
    }

    #[test]
    fn the_appearance_scales_multiply_the_tokens_the_user_already_chose() {
        let plain: Config = toml::from_str("").unwrap();
        let base = plain.resolve_theme();
        assert_eq!(
            plain.theme.scale.rounding, 1.0,
            "a config that never mentions scaling is the config it always was"
        );

        let scaled: Config = toml::from_str(
            "[theme]\nradius = 10\nfont_size = 14.0\n\n[theme.scale]\nrounding = 2.0\nfont = 0.5\n",
        )
        .unwrap();
        let theme = scaled.resolve_theme();
        assert_eq!(
            theme.radius, 20.0,
            "the scale multiplies the pinned radius, not the palette's"
        );
        assert_eq!(theme.font_size, 7.0);
        assert_eq!(
            theme.icon_size, base.icon_size,
            "a scale left at 1 moves nothing"
        );

        let broken: Config = toml::from_str(
            "[theme]\nfont_size = 12.0\nicon_size = 20.0\n\n[theme.scale]\nfont = 0.0\nicon = nan\n",
        )
        .unwrap();
        let theme = broken.resolve_theme();
        assert_eq!(theme.font_size, 3.0, "clamped to the 0.25 floor");
        assert_eq!(theme.icon_size, 20.0, "an unusable factor is no factor");
    }

    #[test]
    fn a_mode_switches_a_family_to_its_other_side_and_leaves_a_one_sided_palette_alone() {
        use crate::scheme::Mode;
        assert_eq!(NordTheme::in_mode("gruvbox", Mode::Light), "gruvbox-light");
        assert_eq!(NordTheme::in_mode("gruvbox-light", Mode::Dark), "gruvbox");
        assert_eq!(
            NordTheme::in_mode("catppuccin-frappe", Mode::Light),
            "catppuccin-latte"
        );
        assert_eq!(
            NordTheme::in_mode("rose_pine_moon", Mode::Light),
            "rose-pine-dawn"
        );
        // Nord has no light sibling anyone drew, and inventing one by inversion would be a palette its author never made.
        assert_eq!(NordTheme::in_mode("nord", Mode::Light), "nord");
        assert_eq!(
            NordTheme::in_mode("tokyo-night", Mode::Light),
            "tokyo-night"
        );
        // Already on the asked-for side: a no-op, not a round trip through the other one.
        assert_eq!(
            NordTheme::in_mode("gruvbox-light", Mode::Light),
            "gruvbox-light"
        );

        let light: Config =
            toml::from_str("[theme]\nname = \"gruvbox\"\nmode = \"light\"\n").unwrap();
        assert_eq!(
            light.resolve_theme().base,
            NordTheme::gruvbox_light().base,
            "the mode reaches the resolved theme, not just the name"
        );
        let auto: Config = toml::from_str("[theme]\nname = \"gruvbox\"\n").unwrap();
        assert_eq!(
            auto.resolve_theme().base,
            NordTheme::gruvbox().base,
            "'auto' keeps whatever the palette already is"
        );
    }

    #[test]
    fn a_dynamic_theme_falls_back_to_a_real_palette_until_a_wallpaper_has_been_read() {
        // Nothing has been quantised in a unit test, which is also the state of a fresh install's first frame.
        let dynamic: Config =
            toml::from_str("[theme]\nname = \"dynamic\"\nfallback = \"catppuccin-latte\"\n")
                .unwrap();
        assert!(dynamic.theme.is_dynamic());
        assert_eq!(
            dynamic.resolve_theme().base,
            NordTheme::catppuccin_latte().base,
            "the fallback is a setting, not a formality"
        );
        let tuned: Config =
            toml::from_str("[theme]\nname = \"dynamic\"\nfallback = \"nord\"\nradius = 14\n")
                .unwrap();
        assert_eq!(tuned.resolve_theme().radius, 14.0);
    }

    #[test]
    fn auto_reads_the_mode_a_dynamic_scheme_should_be_generated_at_off_the_fallback() {
        use crate::scheme::{Mode, Variant};
        let dark: Config =
            toml::from_str("[theme]\nname = \"dynamic\"\nfallback = \"nord\"\n").unwrap();
        assert_eq!(dark.scheme_selection(), (Mode::Dark, Variant::Vibrant));

        // A user whose fallback is a light palette has already said which end of the ramp they live at.
        let light: Config =
            toml::from_str("[theme]\nname = \"dynamic\"\nfallback = \"gruvbox-light\"\n").unwrap();
        assert_eq!(light.scheme_selection().0, Mode::Light);

        let pinned: Config = toml::from_str(
            "[theme]\nname = \"dynamic\"\nfallback = \"gruvbox-light\"\nmode = \"dark\"\nvariant = \"muted\"\n",
        )
        .unwrap();
        assert_eq!(pinned.scheme_selection(), (Mode::Dark, Variant::Muted));
        let nonsense: Config = toml::from_str("[theme]\nvariant = \"sparkly\"\n").unwrap();
        assert_eq!(nonsense.scheme_selection().1, Variant::Vibrant);
    }

    #[test]
    fn a_wallpaper_transition_is_zero_whenever_nothing_should_move() {
        let fading: Config = toml::from_str("[background]\ntransition_ms = 400\n").unwrap();
        assert_eq!(fading.wallpaper_transition(), Duration::from_millis(400));

        let none: Config =
            toml::from_str("[background]\ntransition = \"none\"\ntransition_ms = 400\n").unwrap();
        assert!(none.wallpaper_transition().is_zero());

        let off: Config =
            toml::from_str("[background]\ntransition_ms = 400\n\n[animation]\nenabled = false\n")
                .unwrap();
        assert!(
            off.wallpaper_transition().is_zero(),
            "the global animation switch reaches this like every other duration"
        );

        let scaled: Config = toml::from_str(
            "[background]\ntransition_ms = 400\n\n[animation]\nduration_scale = 2.0\n",
        )
        .unwrap();
        assert_eq!(scaled.wallpaper_transition(), Duration::from_millis(800));

        // An absurd duration is a slow transition, never one that outlives the session.
        let absurd: Config = toml::from_str("[background]\ntransition_ms = 999999999\n").unwrap();
        assert_eq!(absurd.wallpaper_transition(), Duration::from_millis(10_000));
    }

    #[test]
    fn the_background_surface_is_opened_by_anything_that_needs_to_draw_on_it() {
        let bare: Config = toml::from_str("").unwrap();
        assert!(
            !bare.background.is_enabled(),
            "opt-in, so it never clobbers the compositor's own"
        );

        for toml_text in [
            "[background]\nenabled = true\n",
            "[background]\nimage = \"~/wall.png\"\n",
            "[background.monitors]\nDP-1 = \"~/wall.png\"\n",
        ] {
            let config: Config = toml::from_str(toml_text).unwrap();
            assert!(
                config.background.is_enabled(),
                "'{toml_text}' needs the surface"
            );
        }
    }

    #[test]
    fn partial_override_takes_precedence_field_by_field() {
        let cfg: Config = toml::from_str("[theme]\nspacing = 7\nradius = 11\n").unwrap();
        let overridden = cfg.shape_from(Some(Shape::Sections), Some(8), None, None);
        assert_eq!(overridden.mode, Shape::Sections, "mode overridden");
        assert_eq!(overridden.gap, 8, "gap overridden");
        assert_eq!(overridden.spacing, 7.0, "spacing is the theme's");
        assert_eq!(overridden.radius, 11.0, "radius is the theme's");
        let plain = cfg.shape_from(None, None, None, None);
        assert_eq!(plain.mode, Shape::Bar);
        assert_eq!(plain.gap, 0);
    }

    #[test]
    fn derived_padding_and_chip_radius() {
        let s = ResolvedShape {
            mode: Shape::Chips,
            gap: 0,
            spacing: 6.0,
            radius: 12.0,
        };
        assert_eq!(s.padding(), 3.0, "round(6/2)");
        assert_eq!(s.chip_radius(), 9.0, "max(0, 12 - 3)");
        let tight = ResolvedShape {
            mode: Shape::Chips,
            gap: 0,
            spacing: 30.0,
            radius: 4.0,
        };
        assert_eq!(
            tight.chip_radius(),
            0.0,
            "radius floors at 0, never negative"
        );
    }

    #[test]
    fn module_override_parses_variant_and_accent() {
        let cfg: Config =
            toml::from_str("[modules.battery]\nvariant=\"filled\"\naccent=\"orange\"\n").unwrap();
        let none = toml::Table::new();
        let battery = cfg.presentation("battery", &none);
        assert_eq!(battery.variant, Variant::Filled);
        assert_eq!(cfg.accent_name(&battery), "orange");
        let clock = cfg.presentation("clock", &none);
        assert_eq!(clock.variant, Variant::Default);
        assert_eq!(cfg.accent_name(&clock), "cyan");
    }

    #[test]
    fn an_edge_indexes_where_it_stands_in_all_of_them() {
        for (at, edge) in Edge::ALL.into_iter().enumerate() {
            assert_eq!(edge.index(), at, "{edge:?}");
        }
    }

    #[test]
    fn a_dock_pins_nothing_until_told_and_magnifies_within_its_bounds() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.dock.pinned.is_empty());
        assert_eq!(cfg.dock.magnification(), 1.5);
        let cfg: Config =
            toml::from_str("[dock]\npinned = [\"firefox\", \"kitty\"]\nmagnification = 9.0\n")
                .unwrap();
        assert_eq!(cfg.dock.pinned, ["firefox", "kitty"]);
        assert_eq!(cfg.dock.magnification(), DockConfig::MAX_MAGNIFICATION);
        let shrinking = DockConfig {
            magnification: 0.5,
            ..DockConfig::default()
        };
        assert_eq!(shrinking.magnification(), 1.0, "an entry never shrinks");
        let unread = DockConfig {
            magnification: f32::NAN,
            ..DockConfig::default()
        };
        assert_eq!(unread.magnification(), 1.0);
    }

    #[test]
    fn the_windows_strip_writes_titles_unless_told_not_to() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.windows.titles);
        assert!(
            !toml::from_str::<Config>("[windows]\ntitles = false\n")
                .unwrap()
                .windows
                .titles
        );
        let written = toml::to_string_pretty(&Config {
            windows: WindowsConfig { titles: false },
            ..Config::default()
        })
        .unwrap();
        assert!(
            !toml::from_str::<Config>(&written)
                .expect("it parses back")
                .windows
                .titles,
            "{written}"
        );
    }
}
