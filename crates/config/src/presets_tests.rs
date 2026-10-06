//! Theme presets: kept under a name beside the config, listed, picked back into `[theme]` around the rest of the file, and deleted.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::presets::{self, PresetError};
    use crate::{Config, FontSpec, ThemeConfig};

    fn scratch(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-presets-{test}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    fn dusk() -> ThemeConfig {
        let mut theme = ThemeConfig {
            name: "rose-pine".to_string(),
            accent: "#ff8800".to_string(),
            mode: "dark".to_string(),
            radius: Some(14),
            opacity: 0.85,
            font_family: Some("Inter".to_string()),
            ..ThemeConfig::default()
        };
        theme.scale.font = 1.1;
        theme.fonts.title = FontSpec {
            weight: Some(600),
            ..FontSpec::default()
        };
        theme
            .colors
            .insert("base".to_string(), "#101010".to_string());
        theme.export.enabled = true;
        theme.export.dir = "/somewhere".to_string();
        theme
    }

    #[test]
    fn a_preset_round_trips_save_list_load_and_delete() {
        let config = scratch("round-trip");
        assert!(presets::list(&config).is_empty());

        presets::save(&config, "dusk", &dusk()).expect("it saves");
        presets::save(&config, "a-day_2", &ThemeConfig::default()).expect("it saves");
        assert!(
            config
                .parent()
                .unwrap()
                .join("themes")
                .join("dusk.toml")
                .is_file()
        );
        assert_eq!(presets::list(&config), vec!["a-day_2", "dusk"]);

        let loaded = presets::load(&config, "dusk").expect("it loads");
        assert_eq!(
            loaded,
            ThemeConfig {
                export: Default::default(),
                ..dusk()
            },
            "everything but the export comes back"
        );

        presets::delete(&config, "dusk").expect("it deletes");
        assert_eq!(presets::list(&config), vec!["a-day_2"]);
        assert!(matches!(
            presets::load(&config, "dusk"),
            Err(PresetError::Missing(_))
        ));
        assert!(matches!(
            presets::delete(&config, "dusk"),
            Err(PresetError::Missing(_))
        ));
        std::fs::remove_dir_all(config.parent().unwrap()).ok();
    }

    #[test]
    fn a_preset_file_says_theme_and_leaves_the_export_out() {
        let config = scratch("file");
        presets::save(&config, "dusk", &dusk()).unwrap();
        let text = std::fs::read_to_string(presets::dir(&config).join("dusk.toml")).unwrap();
        assert!(text.contains("[theme]"), "{text}");
        assert!(!text.contains("export"), "{text}");
        assert!(text.contains("[theme.fonts.title]"), "{text}");
        std::fs::remove_dir_all(config.parent().unwrap()).ok();
    }

    #[test]
    fn a_name_is_lowercase_ascii_letters_digits_dashes_and_underscores() {
        for good in ["dusk", "a-day_2", "9"] {
            assert!(presets::is_name(good), "{good:?} refused");
        }
        for bad in ["", "Dusk", "dusk.toml", "../x", "a b", "nоrd", "dusk/"] {
            assert!(!presets::is_name(bad), "{bad:?} accepted");
        }
        let config = scratch("names");
        assert!(matches!(
            presets::save(&config, "../escape", &ThemeConfig::default()),
            Err(PresetError::Name(_))
        ));
        assert!(!config.parent().unwrap().join("escape.toml").exists());
        std::fs::create_dir_all(presets::dir(&config)).unwrap();
        std::fs::write(presets::dir(&config).join("Shouting.toml"), "").unwrap();
        std::fs::write(presets::dir(&config).join("notes.txt"), "").unwrap();
        assert!(presets::list(&config).is_empty());
        std::fs::remove_dir_all(config.parent().unwrap()).ok();
    }

    #[test]
    fn picking_a_preset_writes_its_theme_around_the_rest_of_the_file() {
        let config = scratch("apply");
        std::fs::write(
            &config,
            "# mine\n[theme]\nname = \"nord\"\nradius = 4\n\n[theme.export]\nenabled = true\ndir = \"/keep\"\n\n# how many tries\n[lock]\nmax_tries = 7\n",
        )
        .unwrap();
        let mut plain = ThemeConfig {
            name: "gruvbox".to_string(),
            ..ThemeConfig::default()
        };
        plain.export.dir = "/elsewhere".to_string();
        presets::save(&config, "plain", &plain).unwrap();

        presets::apply(&config, "plain").expect("it applies");
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.starts_with("# mine"), "{written}");
        assert!(written.contains("# how many tries"), "{written}");
        let read = Config::load(&config).expect("it still loads");
        assert_eq!(read.theme.name, "gruvbox");
        assert_eq!(
            read.theme.radius, None,
            "a key the preset leaves unset goes"
        );
        assert!(read.theme.export.enabled, "the export the config had stays");
        assert_eq!(read.theme.export.dir, "/keep");
        assert_eq!(read.lock.max_tries, 7);

        assert!(matches!(
            presets::apply(&config, "absent"),
            Err(PresetError::Missing(_))
        ));
        std::fs::remove_dir_all(config.parent().unwrap()).ok();
    }
}
