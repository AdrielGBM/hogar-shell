#[cfg(test)]
mod tests {

    use telar::{
        ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle, ModifiersState,
        NamedKey, NodeId, Rect,
    };

    use config::Config;
    use layout::{AreaId, LayerKind};
    use surfaces::menu::Pointed;
    use surfaces::{reconcile, transient};

    use crate::keys::{self, Press};
    use crate::mode::{self};
    use crate::modes::lock as lock_mode;
    use crate::rig::{SCREEN, enter, rig, rig_with};
    use crate::theme::{self, Controls, Look};

    const SIZE: (f32, f32) = (1920.0, 1080.0);

    const NONE: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };

    struct Owner {
        _scope: telar::OwnerGuard,
    }

    impl Owner {
        fn new() -> Self {
            theme::pending().set(None);
            Self {
                _scope: telar::owner_scope(),
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

    fn tap(key: Key) -> bool {
        telar::observe_keyboard(&Event::KeyPressed {
            key: key.clone(),
            modifiers: NONE,
        });
        let taken = telar::dispatch_overlays(&Event::KeyPressed {
            key: key.clone(),
            modifiers: NONE,
        }) || keys::press_as(&key, NONE, Press::First);
        telar::observe_keyboard(&Event::KeyReleased {
            key,
            modifiers: NONE,
        });
        keys::settle_released();
        taken
    }

    /// The popover as the transient draws it, laid out over the screen, the pointer followed as its window follows it.
    struct Card {
        tree: ComponentList,
        root: NodeId,
        controls: Controls,
    }

    impl Card {
        fn open() -> Self {
            theme::open().expect("it opens in a mode");
            assert!(transient::is_open(theme::ID));
            let config = Config::default();
            let start = Look::of(&config);
            let controls = Controls::of(&start, &config);
            let card = theme::card(SCREEN, &start, controls).expect("the card builds");
            let page = Container::new(LayoutStyle::new().width(SIZE.0).height(SIZE.1), vec![card])
                .expect("a screen");
            let root = page.layout_node();
            Self {
                tree: ComponentList::new(Pointed::new(Box::new(page))),
                root,
                controls,
            }
        }

        fn settle(&self) {
            crate::rig::lay_out(self.root, (SIZE.0, SIZE.1));
        }

        fn texts(&self) -> Vec<(String, Rect)> {
            self.settle();
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.tree.commands(), |command, [a, b, c, d, e, f]| {
                if let DrawCommand::Text { text, rect, .. } = command {
                    let at = Rect::new(
                        a * rect.x + c * rect.y + e,
                        b * rect.x + d * rect.y + f,
                        rect.width,
                        rect.height,
                    );
                    found.push((text.to_string(), at));
                }
            });
            found
        }

        fn said(&self) -> Vec<String> {
            self.texts().into_iter().map(|(text, _)| text).collect()
        }

        fn press(&mut self, wanted: &str) {
            let rect = self
                .texts()
                .into_iter()
                .find(|(text, _)| text == wanted)
                .map(|(_, rect)| rect)
                .unwrap_or_else(|| panic!("{wanted:?} is drawn: {:?}", self.said()));
            let at = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            for event in crate::rig::move_and_click(at) {
                if !telar::dispatch_overlays(&event) {
                    self.tree.on_event(&event);
                }
                self.settle();
            }
        }
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
        let card = Card::open();

        card.controls.radius.set(was + 7.0);
        assert_eq!(shown_radius(), was + 7.0);
        assert_eq!(
            theme::pending().peek().and_then(|look| look.radius),
            Some((was + 7.0) as u32)
        );
        card.controls.opacity.set(0.6);
        card.controls.font.set(1.2);
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

        let card = Card::open();
        card.controls.radius.set(was + 5.0);
        card.controls.name.set("rose-pine".to_string());
        assert_eq!(shown_radius(), was + 5.0);
        assert!(tap(Key::Named(NamedKey::Escape)));
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

        let mut card = Card::open();
        card.controls.radius.set(was + 3.0);
        assert_eq!(shown_radius(), was + 3.0);
        card.press("Cancel (Esc)");
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
        assert_eq!(read.lock.max_tries, 7);
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
        let card = Card::open();
        let warned = |said: Vec<String>| said.iter().any(|line| line.contains("would fall back"));
        assert!(!warned(card.said()), "{:?}", card.said());
        card.controls.name.set("rose-pine-dawn".to_string());
        assert!(warned(card.said()), "{:?}", card.said());
        card.controls.name.set("nord".to_string());
        assert!(!warned(card.said()), "{:?}", card.said());
    }
}
