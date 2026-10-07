#[cfg(test)]
mod tests {

    use telar::{Key, ModifiersState, NamedKey};

    use config::theme::FontRole;
    use config::{Config, IconMask, ThemeConfig, presets};
    use layout::{AreaId, LayerKind};
    use surfaces::{reconcile, transient};

    use crate::mode::{self};
    use crate::modes::lock as lock_mode;
    use crate::rig::{Card, SCREEN, enter, rig, rig_with, stored, tap};
    use crate::theme::{self, Controls, Look};

    const NONE: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };

    static RUNNING_FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Owner {
        _scope: telar::OwnerGuard,
        _file: std::sync::MutexGuard<'static, ()>,
    }

    impl Owner {
        fn new() -> Self {
            let file = RUNNING_FILE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            theme::pending().set(None);
            Self {
                _scope: telar::owner_scope(),
                _file: file,
            }
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            theme::pending().set(None);
            transient::close(theme::ID);
            theme::pending().set(None);
            mode::leave();
        }
    }

    /// The popover as the transient draws it, with the controls it was built with.
    fn opened() -> (Card, Controls) {
        theme::open().expect("it opens in a mode");
        assert!(transient::is_open(theme::ID));
        let config = Config::default();
        let controls = Controls::of(&Look::of(&config), &config);
        let card = theme::card(SCREEN, controls).expect("the card builds");
        (Card::of(card), controls)
    }

    fn shown_radius() -> f32 {
        reconcile::desktop_now(Some(SCREEN))
            .expect("the screen is drawn")
            .config
            .resolve_theme()
            .radius
    }

    fn running_file() -> Option<String> {
        let running = Config::default_path();
        let _ = Config::load_or_default(&running);
        std::fs::read_to_string(&running).ok()
    }

    /// The theme popover is a strip action, so it is on every mode's strip and in every in-mode context menu.
    #[test]
    fn the_theme_popover_is_a_strip_action() {
        let _rig = rig("theme-strip");
        let _owner = Owner::new();
        assert!(theme::open().is_err(), "no mode, no theme popover");
        let _host = enter(LayerKind::Top);
        let (_, press) = crate::host::strip_actions()
            .into_iter()
            .find(|(label, _)| label() == "Theme…")
            .expect("the strip offers the theme");
        press();
        assert!(transient::is_open(theme::ID));
    }

    /// Changing the base radius previews it on every window at once — the config the windows draw with says it — while nothing is written yet.
    #[test]
    fn changing_the_radius_previews_it_before_it_is_saved() {
        let _rig = rig("theme-radius");
        let _owner = Owner::new();
        let before = running_file();
        let _host = enter(LayerKind::Top);
        let was = shown_radius();
        let (_card, controls) = opened();

        controls.radius.set(was + 7.0);
        assert_eq!(shown_radius(), was + 7.0);
        assert_eq!(
            theme::pending().peek().and_then(|look| look.radius),
            Some((was + 7.0) as u32)
        );
        controls.opacity.set(0.6);
        controls.font.set(1.2);
        let shown = reconcile::desktop_now(Some(SCREEN))
            .expect("the screen is drawn")
            .config;
        assert_eq!(shown.opacity(), 0.6);
        assert_eq!(shown.theme.scale.font, 1.2);
        assert_eq!(
            running_file(),
            before,
            "nothing is written while it is open"
        );
    }

    /// Esc puts back the theme as it was when the popover opened, ends the preview and writes nothing; Cancel does the same.
    #[test]
    fn esc_and_cancel_restore_the_theme_it_opened_with() {
        let _rig = rig("theme-esc");
        let _owner = Owner::new();
        let before = running_file();
        let _host = enter(LayerKind::Top);
        let was = shown_radius();

        let (_card, controls) = opened();
        controls.radius.set(was + 5.0);
        controls.name.set("rose-pine".to_string());
        assert_eq!(shown_radius(), was + 5.0);
        assert!(tap(Key::Named(NamedKey::Escape), NONE));
        assert!(!transient::is_open(theme::ID));
        assert_eq!(theme::pending().peek(), None);
        assert_eq!(shown_radius(), was);
        assert!(
            reconcile::shown_config(&std::sync::Arc::new(Config::default()))
                .0
                .is_none()
        );
        assert!(
            mode::current().is_some(),
            "Esc closed the popover, not the mode"
        );
        assert_eq!(running_file(), before);

        let (mut card, controls) = opened();
        controls.radius.set(was + 3.0);
        assert_eq!(shown_radius(), was + 3.0);
        card.click_on("Cancel (Esc)");
        assert!(!transient::is_open(theme::ID));
        assert_eq!(theme::pending().peek(), None);
        assert_eq!(shown_radius(), was);
        assert_eq!(running_file(), before);
    }

    /// A kept theme is written into the `[theme]` of a config, around everything else the file says, comments included.
    #[test]
    fn a_kept_theme_is_written_and_the_comments_survive() {
        let path = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("editor-theme-write")
            .join("config.toml");
        let _ = std::fs::create_dir_all(path.parent().expect("a directory"));
        std::fs::write(
            &path,
            "# mine\n[theme]\nname = \"nord\"\n\n# how many tries\n[lock]\nmax_tries = 7\n",
        )
        .expect("a config");
        theme::write(
            &path,
            &Look {
                name: "rose-pine".to_string(),
                accent: "#ff8800".to_string(),
                radius: Some(12),
                opacity: 0.8,
                font: 1.2,
                weights: [None, Some(650), None, None],
                mask: IconMask::Circle,
                ..Look::of(&Config::default())
            },
        )
        .expect("it writes");
        let written = std::fs::read_to_string(&path).expect("the config");
        assert!(written.starts_with("# mine"), "{written}");
        assert!(written.contains("# how many tries"), "{written}");
        assert!(written.contains("max_tries = 7"), "{written}");
        let read = Config::load(&path).expect("it still loads");
        assert_eq!(read.theme.name, "rose-pine");
        assert_eq!(read.theme.accent, "#ff8800");
        assert_eq!(read.theme.radius, Some(12));
        assert_eq!(read.theme.opacity, 0.8);
        assert_eq!(read.theme.scale.font, 1.2);
        assert_eq!(read.theme.fonts.spec(FontRole::ALL[1]).weight, Some(650));
        assert_eq!(read.icons.mask, IconMask::Circle);
        assert_eq!(read.lock.max_tries, 7);

        std::fs::write(&path, "[theme]\nname = \"nord\"\n").expect("a config");
        theme::write(&path, &Look::of(&Config::default())).expect("it writes");
        let written = std::fs::read_to_string(&path).expect("the config");
        assert!(
            !written.contains("[icons]"),
            "icons the look leaves as they were are not written: {written}"
        );
    }

    /// A pending theme the lock prompt would be unreadable on is reported in the popover before it is saved: the prompt's own fill stays put while the palette's text flips from light to dark.
    #[test]
    fn a_theme_that_makes_the_prompt_unreadable_is_reported() {
        let _rig = rig_with("theme-lock", |layout| {
            let prompt = layout.outputs[0]
                .layers
                .lock
                .areas
                .iter_mut()
                .find(|area| area.id == AreaId::new("prompt"))
                .expect("the built-in lock has a prompt");
            prompt.style.fill = Some("#2e3440".to_string());
        });
        let _owner = Owner::new();
        let config = Config::default();
        let dawn = Look {
            name: "rose-pine-dawn".to_string(),
            ..Look::of(&config)
        };
        assert_eq!(lock_mode::falls_back_with(&config, &config), None);
        assert!(lock_mode::falls_back_with(&config, &dawn.on(&config)).is_some());

        let _host = enter(LayerKind::Top);
        let (card, controls) = opened();
        let warned = |said: Vec<String>| said.iter().any(|line| line.contains("would fall back"));
        assert!(!warned(card.said()), "{:?}", card.said());
        controls.name.set("rose-pine-dawn".to_string());
        assert!(warned(card.said()), "{:?}", card.said());
        controls.name.set("nord".to_string());
        assert!(!warned(card.said()), "{:?}", card.said());
    }

    struct Restore(std::path::PathBuf, Option<String>);

    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.1 {
                Some(kept) => {
                    let _ = std::fs::write(&self.0, kept);
                }
                None => {
                    let _ = std::fs::remove_file(&self.0);
                }
            }
        }
    }

    /// Done keeps what the popover chose: it is written into the config, nothing of it enters the layout's undo history, and the layout itself is as it was.
    #[test]
    fn done_writes_the_theme_and_the_layouts_history_is_untouched() {
        let rig = rig("theme-done");
        let _owner = Owner::new();
        let _restore = Restore(Config::default_path(), running_file());
        let _host = enter(LayerKind::Top);
        let layout = stored(&rig);
        let was = shown_radius();

        let (mut card, controls) = opened();
        controls.radius.set(was + 6.0);
        card.click_on("Done");

        assert!(!transient::is_open(theme::ID));
        let kept = Config::load(&Config::default_path()).expect("the config is written");
        assert_eq!(kept.theme.radius, Some((was + 6.0) as u32));
        assert_eq!(
            rig.undo_label(),
            None,
            "the theme is outside the undo history"
        );
        assert_eq!(stored(&rig), layout);
        assert!(mode::current().is_some(), "and the mode stays up");
    }
    /// The presets beside the running config, gone again when the test ends.
    struct Presets;

    impl Drop for Presets {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(presets::dir(&Config::default_path()));
        }
    }

    fn shown() -> Config {
        (*reconcile::desktop_now(Some(SCREEN))
            .expect("the screen is drawn")
            .config)
            .clone()
    }

    fn role(wanted: FontRole) -> usize {
        FontRole::ALL
            .iter()
            .position(|role| *role == wanted)
            .expect("every role has a row")
    }

    /// A weight per role, the app icon theme and the app icon shape preview on every window as they are chosen, and Esc puts every one of them back without writing anything.
    #[test]
    fn weights_and_the_icon_rows_preview_live_and_esc_puts_them_back() {
        let _rig = rig("theme-type");
        let _owner = Owner::new();
        let before = running_file();
        let _host = enter(LayerKind::Top);
        let (_card, controls) = opened();

        controls.weights[role(FontRole::Body)].set("600".to_string());
        controls.weights[role(FontRole::Title)].set("800".to_string());
        controls.mask.set("squircle".to_string());
        controls.app_icon_theme.set("Papirus".to_string());
        let now = shown();
        assert_eq!(now.theme.fonts.body.weight, Some(600));
        assert_eq!(
            now.resolve_theme()
                .text_over(
                    telar::TextStyle::new(0.0, telar::Color::WHITE),
                    FontRole::Title,
                    telar::Color::WHITE,
                )
                .font_weight,
            800
        );
        assert_eq!(now.theme.fonts.caption.weight, None);
        assert_eq!(now.icons.mask, IconMask::Squircle);
        assert_eq!(now.icons.app_icon_theme, "Papirus");
        assert_eq!(
            running_file(),
            before,
            "nothing is written while it is open"
        );

        assert!(tap(Key::Named(NamedKey::Escape), NONE));
        let now = shown();
        assert_eq!(now.theme.fonts.body.weight, None);
        assert_eq!(now.theme.fonts.title.weight, None);
        assert_eq!(now.icons.mask, IconMask::None);
        assert_eq!(now.icons.app_icon_theme, "");
        assert_eq!(running_file(), before);
    }

    /// The family row previews the family it is set to on every window as it is chosen, and Esc puts back the one the popover opened with without writing anything.
    #[test]
    fn the_family_row_previews_live_and_esc_puts_it_back() {
        let _rig = rig("theme-family");
        let _owner = Owner::new();
        let before = running_file();
        let _host = enter(LayerKind::Top);
        let (card, controls) = opened();
        assert!(card.shows("Font family"), "{:?}", card.said());

        controls.family.set("DejaVu Sans".to_string());
        assert_eq!(
            shown().theme.family(),
            Some(telar::FontFamily::from("DejaVu Sans"))
        );
        assert_eq!(
            running_file(),
            before,
            "nothing is written while it is open"
        );

        assert!(tap(Key::Named(NamedKey::Escape), NONE));
        assert_eq!(shown().theme.font_family, None);
        assert_eq!(running_file(), before);
    }

    /// A preset picked in the popover previews the whole `[theme]` it keeps — keys the popover has no row for included — and Esc puts back the theme the popover opened with.
    #[test]
    fn a_preset_picked_previews_its_whole_theme_and_esc_puts_it_back() {
        let _rig = rig("theme-preset-pick");
        let _owner = Owner::new();
        let _presets = Presets;
        let before = running_file();
        let mut dusk = ThemeConfig {
            name: "rose-pine".to_string(),
            radius: Some(19),
            font_family: Some("Inter".to_string()),
            ..ThemeConfig::default()
        };
        dusk.fonts.title.weight = Some(600);
        dusk.colors
            .insert("base".to_string(), "#101010".to_string());
        presets::save(&Config::default_path(), "dusk", &dusk).expect("a preset");
        let _host = enter(LayerKind::Top);
        let was = shown_radius();

        let (mut card, controls) = opened();
        assert!(card.shows("dusk"), "{:?}", card.said());
        card.press("Use");
        let now = shown();
        assert_eq!(now.theme.name, "rose-pine");
        assert_eq!(now.theme.radius, Some(19));
        assert_eq!(now.theme.fonts.title.weight, Some(600));
        assert_eq!(
            now.theme.colors.get("base").map(String::as_str),
            Some("#101010")
        );
        assert_eq!(controls.name.peek(), "rose-pine");
        assert_eq!(controls.radius.peek(), 19.0);
        assert_eq!(now.theme.font_family.as_deref(), Some("Inter"));
        assert_eq!(controls.family.peek(), "Inter");

        assert!(tap(Key::Named(NamedKey::Escape), NONE));
        assert_eq!(shown_radius(), was);
        assert_eq!(shown().theme, Config::default().theme);
        assert_eq!(running_file(), before);
    }

    /// The look the controls say is kept as a preset under a name the rules allow, listed at once, and deleted again from its row.
    #[test]
    fn a_look_is_saved_as_a_preset_and_deleted_from_the_popover() {
        let _rig = rig("theme-preset-save");
        let _owner = Owner::new();
        let _presets = Presets;
        let path = Config::default_path();
        let _host = enter(LayerKind::Top);
        let (mut card, controls) = opened();
        assert!(!card.shows("Delete"), "{:?}", card.said());

        controls.accent.set("#ff8800".to_string());
        controls.preset_name.set("Not Allowed".to_string());
        card.press("Save as preset");
        assert!(card.shows_part_of("lowercase letters"), "{:?}", card.said());
        assert!(presets::list(&path).is_empty());

        controls.preset_name.set("mine".to_string());
        card.press("Save as preset");
        assert_eq!(presets::list(&path), vec!["mine"]);
        assert_eq!(
            presets::load(&path, "mine").expect("it is kept").accent,
            "#ff8800"
        );
        assert!(card.shows("Delete"), "{:?}", card.said());

        card.press("Delete");
        assert!(presets::list(&path).is_empty());
        assert!(!card.shows("Delete"), "{:?}", card.said());
    }
}
