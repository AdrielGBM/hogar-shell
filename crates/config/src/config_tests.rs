//! What `config.toml` has to keep doing: the defaults, the monitor overrides, and the resolution rules a surface reads through.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use telar::Color;

    use crate::theme::NordTheme;
    use crate::*;

    fn ids(entries: &[ModuleEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn a_zone_reads_bare_ids_and_tables_side_by_side() {
        let cfg: Config = toml::from_str(
            r#"
[bars.top]
start = ["workspaces", { id = "clock", accent = "red" }, { id = "clock", variant = "filled" }]
"#,
        )
        .expect("both entry forms parse in one array");
        assert_eq!(ids(&cfg.bars.top.start), ["workspaces", "clock", "clock"]);
        assert_eq!(cfg.bars.top.start[1].accent.as_deref(), Some("red"));
        assert_eq!(cfg.bars.top.start[2].variant, Some(Variant::Filled));

        // The point of the table form: a `[modules.<id>]` override is keyed by id, so it could only paint both copies the same.
        assert_eq!(cfg.entry_accent_name(&cfg.bars.top.start[1]), "red");
        assert_eq!(
            cfg.entry_variant(&cfg.bars.top.start[2]),
            Variant::Filled,
            "an entry's own variant wins"
        );
        assert_eq!(
            cfg.entry_variant(&cfg.bars.top.start[1]),
            Variant::Default,
            "and an entry that names none falls back rather than inheriting its neighbour's"
        );
    }

    #[test]
    fn a_bare_entry_writes_back_as_the_string_it_was_read_from() {
        let cfg: Config =
            toml::from_str("[bars.top]\nstart = [\"clock\"]\n").expect("config parses");
        let written = toml::to_string_pretty(&cfg.bars.top).expect("serialises");
        assert!(
            written.contains("start = [\"clock\"]"),
            "a bare entry gained a table it never asked for: {written}"
        );
        let back: BarConfig = toml::from_str(&written).expect("round-trips");
        assert_eq!(ids(&back.start), ["clock"]);
    }

    #[test]
    fn an_entry_with_settings_round_trips_through_toml() {
        let cfg: Config =
            toml::from_str("[bars.top]\nstart = [{ id = \"clock\", accent = \"red\" }]\n")
                .expect("config parses");
        let written = toml::to_string_pretty(&cfg.bars.top).expect("serialises");
        let back: BarConfig = toml::from_str(&written).expect("round-trips");
        assert_eq!(back.start, cfg.bars.top.start);
    }

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
        // What this catches is not a parse failure — the scattered file still parses, which is why nothing saw it. Saving `[theme]` printed `[theme.export]` between `[panels]` and `[bars.top]`, put `[theme.fonts.title]` inside the bar definitions, and left `[theme]` itself *after* its own children. For a function whose whole promise is "preserving every other section, key order, and comment", that is the failure.
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

    #[test]
    fn starter_shows_only_a_top_bar() {
        let cfg = Config::starter();
        assert_eq!(ids(&cfg.bars.top.start), ["workspaces"]);
        assert_eq!(ids(&cfg.bars.top.center), ["clock"]);
        assert!(cfg.bars.bottom.is_empty());
        assert!(cfg.bars.left.is_empty());
        assert!(cfg.bars.right.is_empty());
    }

    #[test]
    fn plain_default_is_all_empty() {
        let cfg = Config::default();
        assert!(cfg.bars.top.is_empty() && cfg.bars.left.is_empty());
    }

    #[test]
    fn partial_config_leaves_unlisted_edges_empty() {
        let toml = r#"
[bars.left]
size = 44
start = ["workspaces"]
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.bars.left.size, 44);
        assert_eq!(ids(&cfg.bars.left.start), ["workspaces"]);
        assert!(cfg.bars.top.is_empty());
    }

    #[test]
    fn edge_orientation() {
        assert!(Edge::Top.is_horizontal() && Edge::Bottom.is_horizontal());
        assert!(Edge::Left.is_vertical() && Edge::Right.is_vertical());
    }

    #[test]
    fn shape_defaults_reproduce_todays_bar() {
        let cfg: Config = toml::from_str("[bars.top]\nstart = [\"clock\"]\n").unwrap();
        assert_eq!(cfg.shape.mode, Shape::Bar);
        assert!(!cfg.shape.frame);
        assert_eq!(cfg.shape.gap, 0);
        assert_eq!(
            cfg.shape.radius, None,
            "unset radius falls back to the theme"
        );
        let top = cfg.shape_for(Edge::Top);
        assert_eq!(top.mode, Shape::Bar);
        assert_eq!(top.gap, 0);
        assert_eq!(top.radius, 0.0, "the nord theme's default radius is 0");
        assert!(cfg.hugs(Edge::Top));
        assert!(cfg.bar_surface_opaque(Edge::Top));
    }

    #[test]
    fn zone_of_reflects_bar_zones() {
        let toml = r#"
[bars.top]
start = ["workspaces"]
center = ["clock"]
end = ["battery", "volume"]
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.zone_of(Edge::Top, "workspaces"), Some(Zone::Start));
        assert_eq!(cfg.zone_of(Edge::Top, "clock"), Some(Zone::Center));
        assert_eq!(cfg.zone_of(Edge::Top, "volume"), Some(Zone::End));
        assert_eq!(cfg.zone_of(Edge::Top, "missing"), None);
        assert_eq!(cfg.zone_of(Edge::Bottom, "clock"), None);
    }

    #[test]
    fn panels_and_open_mode_defaults() {
        let cfg: Config = toml::from_str("[bars.top]\ncenter = [\"clock\"]\n").unwrap();
        assert_eq!(cfg.panels.drawer.width, 320.0);
        assert_eq!(cfg.panels.float.width, 360);
        assert_eq!(cfg.panels.float.height, 240);
        assert_eq!(cfg.open_mode_for("clock"), OpenMode::Drawer);

        let floaty: Config = toml::from_str(
            "[modules.clock]\nopen = \"float\"\n[panels.drawer]\nwidth = 400\n[panels.float]\nwidth = 480\nheight = 320\n",
        )
        .unwrap();
        assert_eq!(floaty.open_mode_for("clock"), OpenMode::Float);
        assert_eq!(floaty.panels.drawer.width, 400.0);
        assert_eq!(floaty.panels.float.width, 480);
        assert_eq!(floaty.panels.float.height, 320);
    }

    #[test]
    fn starter_config_round_trips_through_toml() {
        // load_or_default writes the starter to disk on first run, so it must serialize and re-parse cleanly.
        let starter = Config::starter();
        let text = toml::to_string_pretty(&starter).expect("starter serializes");
        let parsed: Config = toml::from_str(&text).expect("starter re-parses");
        assert_eq!(parsed.panels.drawer.width, starter.panels.drawer.width);
        assert_eq!(parsed.panels.float.width, starter.panels.float.width);
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
        assert_eq!(cfg.resolved_radius(Edge::Top), 12.0);
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
    fn spacing_and_radius_fall_back_to_the_theme_then_config_overrides() {
        let theme = NordTheme::new();
        // Nothing set anywhere → the theme's numeric tokens.
        let bare: Config = toml::from_str("[bars.top]\ncenter=[\"clock\"]\n").unwrap();
        assert_eq!(bare.resolved_radius(Edge::Top), theme.radius);
        assert_eq!(bare.resolved_spacing(Edge::Top), theme.spacing);
        // Per-bar wins over [shape], which wins over the theme.
        let cfg: Config = toml::from_str(
            "[shape]\nradius=10\nspacing=4\n[bars.top]\ncenter=[\"clock\"]\n[bars.top.shape]\nradius=2\n[bars.bottom]\nstart=[\"clock\"]\n",
        )
        .unwrap();
        assert_eq!(cfg.resolved_radius(Edge::Top), 2.0, "per-bar override wins");
        assert_eq!(
            cfg.resolved_spacing(Edge::Top),
            4.0,
            "spacing falls to [shape]"
        );
        assert_eq!(
            cfg.resolved_radius(Edge::Bottom),
            10.0,
            "bottom takes [shape]"
        );
    }

    #[test]
    fn panel_radius_matches_the_bar_on_each_edge() {
        // Per-bar radius override on top, global (0) elsewhere: panels inherit the radius of the bar they hang off.
        let cfg: Config = toml::from_str(
            "[shape]\nradius=0\n[bars.top]\ncenter=[\"clock\"]\n[bars.top.shape]\nradius=8\n[bars.left]\nstart=[\"clock\"]\n",
        )
        .unwrap();
        assert_eq!(cfg.panel_radius(Edge::Top), 8.0);
        assert_eq!(
            cfg.panel_radius(Edge::Left),
            0.0,
            "left inherits the global radius"
        );
    }

    #[test]
    fn panel_margin_is_a_uniform_gap_and_never_double_counts_the_bar() {
        // The reservation strip already offsets a panel (exclusive_zone=0) past the bar, so the margin is just the gap — adding the bar's reserved thickness here too would put the panel at double the distance.
        let floating: Config =
            toml::from_str("[shape]\ngap=8\n[bars.top]\nsize=34\ncenter=[\"clock\"]\n").unwrap();
        assert_eq!(floating.panel_gap(Edge::Top), 8);
        assert_eq!(floating.panel_margin(Edge::Top), (8, 8, 8, 8));

        // Hugging bar with no configured gap still gets the default breathing gap, uniformly.
        let hug: Config = toml::from_str("[bars.top]\nsize=34\ncenter=[\"clock\"]\n").unwrap();
        let d = DEFAULT_PANEL_GAP as i32;
        assert_eq!(hug.panel_margin(Edge::Top), (d, d, d, d));
    }

    #[test]
    fn a_panel_keeps_the_gap_its_bar_floats_at_and_there_is_no_key_to_break_that() {
        let derived: Config =
            toml::from_str("[shape]\ngap=20\n[bars.top]\ncenter=[\"clock\"]\n").unwrap();
        assert_eq!(
            derived.panel_gap(Edge::Top),
            20,
            "a panel floats off the bar by exactly what the bar floats off the screen"
        );

        // `[panels] gap` used to pin a fixed distance regardless of the bar. It is gone, and a file that still names it changes nothing: the derivation is the whole rule.
        let pinned: Config = toml::from_str(
            "[shape]\ngap=20\n[panels]\ngap=4\n[bars.top]\ncenter=[\"clock\"]\n[bars.bottom]\nstart=[\"clock\"]\n",
        )
        .unwrap();
        assert_eq!(pinned.panel_gap(Edge::Top), 20);
        assert_eq!(pinned.panel_gap(Edge::Bottom), 20);
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
    fn general_defaults_keep_bars_under_fullscreen_windows() {
        let d: Config = toml::from_str("").unwrap();
        assert!(
            !d.general.show_over_fullscreen,
            "a fullscreen game is meant to cover the bar unless asked otherwise"
        );
        assert!(d.general.logo.is_empty(), "an empty logo means auto-detect");
    }

    #[test]
    fn a_parse_error_is_returned_rather_than_swallowed() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[bars.top\nstart = [\"clock\"]\n").unwrap();

        let error = Config::load(&path).expect_err("a malformed file must not parse");
        assert!(
            matches!(error, LoadError::Parse(_)),
            "the caller needs to distinguish a typo from a missing file"
        );
        // `load_or_default` is the lossy convenience wrapper — it answers a typo with the starter bar, throwing the user's layout away. That is exactly why the running shell uses `load`: so it can keep the last config that worked and report the error instead.
        let lossy = Config::load_or_default(&path);
        assert_eq!(
            lossy.bars.top.start,
            Config::starter().bars.top.start,
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
        assert_eq!(ids(&seeded.bars.top.start), ["workspaces"]);
        assert!(path.exists(), "the starter is written for the user to edit");
        assert!(
            Config::load(&path).is_ok(),
            "and what was written parses back"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_stack_defaults_to_top_right_with_sensible_limits() {
        let d: Config = toml::from_str("").unwrap();
        assert_eq!(d.stack.edge, Edge::Top);
        assert_eq!(d.stack.align, Align::End, "align=end is the right side");
        assert_eq!(d.stack.max_visible, 4);
        assert!(d.notifications.critical_sticky);

        let cfg: Config =
            toml::from_str("[stack]\nmax_visible = 2\ntimeout_ms = 400\nedge = \"bottom\"\n")
                .unwrap();
        assert_eq!(cfg.stack.max_visible, 2);
        assert_eq!(cfg.stack.edge, Edge::Bottom);
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
[bars.top]
size = 34
start = ["workspaces"]
center = ["clock"]

[theme]
accent = "cyan"
name = "nord"
"#,
            Some((
                "DP-2",
                r#"
[bars.top]
size = 44
start = ["cpu", "memory"]

[theme]
accent = "orange"
"#,
            )),
        );
        let path = dir.join("config.toml");

        let global = Config::for_output(&path, None).unwrap();
        assert_eq!(global.bars.top.size, 34);
        assert_eq!(ids(&global.bars.top.start), ["workspaces"]);
        assert_eq!(global.theme.accent, "cyan");

        let overridden = Config::for_output(&path, Some("DP-2")).unwrap();
        assert_eq!(overridden.bars.top.size, 44, "the override wins");
        assert_eq!(
            ids(&overridden.bars.top.start),
            ["cpu", "memory"],
            "an array replaces rather than concatenating"
        );
        assert_eq!(
            ids(&overridden.bars.top.center),
            ["clock"],
            "a key the override never mentions keeps the global value"
        );
        assert_eq!(overridden.theme.accent, "orange");
        assert_eq!(
            overridden.theme.name, "nord",
            "merging is per key, not per section"
        );

        let unknown = Config::for_output(&path, Some("HDMI-A-1")).unwrap();
        assert_eq!(
            unknown.bars.top.size, 34,
            "a screen with no file is the global config"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_monitor_override_cannot_change_a_section_one_process_owns() {
        let dir = config_dir(
            "monitor-global-only",
            "[general]\nlanguage = \"en\"\n\n[shape]\ngap = 0\n",
            Some((
                "DP-1",
                "[general]\nlanguage = \"es\"\n\n[stack]\nmax_visible = 99\n\n[shape]\ngap = 12\n",
            )),
        );
        let path = dir.join("config.toml");
        let cfg = Config::for_output(&path, Some("DP-1")).unwrap();

        assert_eq!(cfg.general.language, "en", "[general] is global-only");
        assert_eq!(
            cfg.stack.max_visible,
            StackConfig::default().max_visible,
            "[stack] is global-only — the column follows the focused screen rather than existing per output"
        );
        assert_eq!(
            cfg.shape.gap, 12,
            "a visual section is still the monitor's to set"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn excluded_screens_match_as_patterns_and_never_catch_an_unnamed_output() {
        let bars = BarsConfig {
            excluded_screens: vec!["HDMI-*".to_string(), "DP-3".to_string()],
            ..BarsConfig::default()
        };
        assert!(bars.excludes(Some("HDMI-A-1")));
        assert!(bars.excludes(Some("DP-3")));
        assert!(!bars.excludes(Some("DP-1")));
        assert!(
            !bars.excludes(None),
            "an output the compositor did not name has nothing to match, so it keeps its bars"
        );
        assert!(
            !BarsConfig::default().excludes(Some("DP-1")),
            "no exclusions is every screen"
        );
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
    fn a_bar_is_solid_by_default() {
        assert_eq!(Config::starter().opacity(), 1.0);
    }

    /// A surface declared opaque is cleared to a solid colour before anything draws, so declaring it while the bar is translucent — or while a frame is painting the strip instead — is how an opacity setting does nothing at all. Both cases were reachable: a side bar in `bar` mode under a frame stayed solid however the opacity was set.
    #[test]
    fn a_bar_that_is_not_solid_never_declares_an_opaque_surface() {
        let hugging = |toml: &str| toml::from_str::<Config>(toml).unwrap();

        let solid = hugging("[shape]\nmode = \"bar\"\ngap = 0\nradius = 0\n");
        assert!(
            solid.bar_surface_opaque(Edge::Left),
            "a hugging bar at full opacity is the one case that may be cleared solid"
        );

        let translucent =
            hugging("[shape]\nmode = \"bar\"\ngap = 0\nradius = 0\n[theme]\nopacity = 0.5\n");
        assert!(!translucent.bar_surface_opaque(Edge::Left));

        let framed = hugging("[shape]\nmode = \"bar\"\ngap = 0\nradius = 0\nframe = true\n");
        assert!(
            !framed.bar_surface_opaque(Edge::Left),
            "the frame's ring covers this strip, so the bar clearing it solid paints the background twice"
        );
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
            ..PanelsConfig::default()
        };
        assert_eq!(off.drag_threshold(), None);
        let tiny = PanelsConfig {
            drag_threshold: 1.0,
            ..PanelsConfig::default()
        };
        assert_eq!(tiny.drag_threshold(), Some(16.0));
        let nan = PanelsConfig {
            drag_threshold: f32::NAN,
            ..PanelsConfig::default()
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

    /// The two surfaces are asked for separately, which is the whole point of `[widgets]` being its own section.
    ///
    /// A clock on a screen with no wallpaper is a clock, not a reason to paint the desktop; and a wallpaper is not a reason to open the surface the visualiser repaints with the music.
    #[test]
    fn the_widgets_surface_is_opened_by_its_own_widgets_and_by_nothing_else() {
        let papered: Config = toml::from_str("[background]\nimage = \"~/wall.png\"\n").unwrap();
        assert!(!papered.widgets.is_enabled());

        for toml_text in [
            "[widgets.clock]\nenabled = true\n",
            "[widgets.visualiser]\nenabled = true\n",
        ] {
            let config: Config = toml::from_str(toml_text).unwrap();
            assert!(
                config.widgets.is_enabled(),
                "'{toml_text}' needs the surface"
            );
            assert!(
                !config.background.is_enabled(),
                "'{toml_text}' does not ask for a wallpaper"
            );
        }
    }

    #[test]
    fn partial_override_takes_precedence_field_by_field() {
        let toml = r#"
[shape]
mode = "bar"
gap = 0
spacing = 6
radius = 10

[bars.top]
center = ["clock"]
[bars.top.shape]
mode = "sections"
gap = 8
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let top = cfg.shape_for(Edge::Top);
        assert_eq!(top.mode, Shape::Sections);
        assert_eq!(top.gap, 8, "gap overridden");
        assert_eq!(top.spacing, 6.0, "spacing inherits the global");
        assert_eq!(top.radius, 10.0, "radius inherits the global");
        let bottom = cfg.shape_for(Edge::Bottom);
        assert_eq!(bottom.mode, Shape::Bar);
        assert_eq!(bottom.gap, 0);
    }

    #[test]
    fn hug_and_opacity_track_gap_and_frame() {
        let toml = r#"
[shape]
gap = 8
radius = 12
[bars.top]
center = ["clock"]
[bars.bottom]
start = ["clock"]
[bars.bottom.shape]
gap = 0
radius = 0
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(!cfg.hugs(Edge::Top));
        assert!(!cfg.bar_surface_opaque(Edge::Top));
        assert!(cfg.hugs(Edge::Bottom));
        assert!(cfg.bar_surface_opaque(Edge::Bottom));
    }

    #[test]
    fn frame_forces_hug_on_every_edge() {
        let toml = r#"
[shape]
frame = true
gap = 8
[bars.top]
center = ["clock"]
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.hugs(Edge::Top), "frame forces hug even at gap>0");
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
        let cfg: Config = toml::from_str(
            "[bars.top]\ncenter=[\"clock\"]\n[modules.battery]\nvariant=\"filled\"\naccent=\"orange\"\n",
        )
        .unwrap();
        assert_eq!(cfg.variant_for("battery"), Variant::Filled);
        assert_eq!(cfg.accent_name_for("battery"), "orange");
        assert_eq!(cfg.variant_for("clock"), Variant::Default);
        assert_eq!(cfg.accent_name_for("clock"), "cyan");
    }

    #[test]
    fn corner_owner_prefers_horizontal_then_vertical() {
        let cfg: Config =
            toml::from_str("[bars.top]\ncenter=[\"clock\"]\n[bars.left]\nstart=[\"workspaces\"]\n")
                .unwrap();
        assert_eq!(
            cfg.corner_owner(Corner::TopLeft),
            Some(Edge::Top),
            "top wins over left"
        );
        assert_eq!(cfg.corner_owner(Corner::BottomLeft), Some(Edge::Left));
        assert_eq!(cfg.corner_owner(Corner::BottomRight), None);
    }

    #[test]
    fn corner_modules_route_to_owning_bar_ends() {
        let cfg: Config = toml::from_str(
            "[bars.top]\ncenter=[\"clock\"]\n[bars.right]\nstart=[\"ws\"]\n\
             [corners]\ntop_left=\"logo\"\nbottom_right=\"tray\"\n",
        )
        .unwrap();
        assert_eq!(cfg.corner_modules_for(Edge::Top), (Some("logo"), None));
        assert_eq!(cfg.corner_modules_for(Edge::Right), (None, Some("tray")));
        assert_eq!(cfg.corner_modules_for(Edge::Left), (None, None));
    }

    #[test]
    fn panel_gap_tracks_the_bar_gap_and_falls_back_when_hugging() {
        let floating: Config =
            toml::from_str("[shape]\ngap=12\n[bars.top]\ncenter=[\"clock\"]\n").unwrap();
        assert_eq!(floating.edge_gap(Edge::Top), 12);
        assert_eq!(
            floating.panel_gap(Edge::Top),
            12,
            "a floating bar's panels float in step"
        );
        assert_eq!(
            floating.edge_reserved(Edge::Top),
            12 + 34,
            "reserved = outer gap + thickness"
        );

        let hugging: Config = toml::from_str("[bars.top]\ncenter=[\"clock\"]\n").unwrap();
        assert_eq!(hugging.edge_gap(Edge::Top), 0);
        assert_eq!(
            hugging.panel_gap(Edge::Top),
            DEFAULT_PANEL_GAP,
            "a hugging bar's panels still get a breathing gap"
        );
        assert_eq!(hugging.edge_reserved(Edge::Top), 34);
    }

    #[test]
    fn frame_edge_reserves_thickness_without_a_gap() {
        let cfg: Config =
            toml::from_str("[shape]\nframe=true\ngap=8\n[bars.top]\ncenter=[\"clock\"]\n").unwrap();
        assert_eq!(
            cfg.edge_gap(Edge::Top),
            0,
            "frame forces a hug, so no outer gap"
        );
        assert_eq!(cfg.edge_reserved(Edge::Top), 34);
        assert_eq!(cfg.panel_gap(Edge::Top), DEFAULT_PANEL_GAP);
    }

    #[test]
    fn frame_gives_empty_edges_inactive_strips() {
        let toml = r#"
[shape]
frame = true
inactive_size = 6
[bars.top]
center = ["clock"]
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.edge_thickness(Edge::Top),
            34,
            "active edge keeps its size"
        );
        assert_eq!(
            cfg.edge_thickness(Edge::Bottom),
            6,
            "empty edge becomes an inactive strip under frame"
        );
    }

    #[test]
    fn only_a_vertical_bar_runs_into_the_strips_beside_it() {
        let cornered: Config =
            toml::from_str("[bars.top]\ncenter=[\"clock\"]\n[bars.left]\nstart=[\"workspaces\"]\n")
                .unwrap();
        assert_eq!(
            bar_ends_abut(&cornered, Edge::Left),
            (true, false),
            "a left bar meets the top bar above it and the bare screen below"
        );
        assert_eq!(
            bar_ends_abut(&cornered, Edge::Top),
            (false, false),
            "a horizontal bar spans its whole edge, so both its ends are the screen's"
        );

        let hiding: Config = toml::from_str(
            "[bars.top]\ncenter=[\"clock\"]\npersistent=false\n[bars.left]\nstart=[\"workspaces\"]\n",
        )
        .unwrap();
        assert_eq!(
            bar_ends_abut(&hiding, Edge::Left),
            (false, false),
            "an auto-hidden top bar reserves nothing, so the left bar runs up to the screen's edge"
        );

        let framed: Config =
            toml::from_str("[shape]\nframe=true\n[bars.left]\nstart=[\"workspaces\"]\n").unwrap();
        assert_eq!(
            bar_ends_abut(&framed, Edge::Right),
            (true, true),
            "under a frame every edge carries a strip, even an empty one"
        );
    }
}
