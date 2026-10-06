//! Lock mode: the background and desktop tools working on a preview of the lock screen that is checked the way a real lock is, the prompt moved and restyled but never removed, how much a locked screen may reveal switched live, and no session lock taken or touched by any of it.

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, Text, compute_layout,
    };

    use config::theme::{FontRole, NordTheme};
    use config::{Config, MediaDetail, NotificationDetail};
    use layout::{
        Area, AreaId, AreaKind, Instance, LayerKind, Layout, LayoutOp, Rect, ResolvedAreaKind,
        SMALLEST_PROMPT, Site, Style,
    };
    use modules::lock::LockLayout;
    use surfaces::catalogue::Descriptors;
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{
        Built, Category, FieldDef, Input, ModuleDescriptor, Privacy as Shown, Representations,
        WidgetDef,
    };
    use ui::host::{Host, WidgetSize};

    use crate::keys::{self};
    use crate::mode::{self, Mode};
    use crate::modes::lock::{self as lock_mode, Privacy};
    use crate::modes::{background, desktop, palette, widgets};
    use crate::rig::{SCREEN, close, enter, rig_with, stored, tap};
    use crate::session::{self, Selection};
    use crate::{context, select};

    const SIZE: (f32, f32) = (1920.0, 1080.0);

    /// What the notifications reading says about the applications a waiting notification came from, which `[lock] notification_detail` decides on a locked screen.
    const FROM: &str = "from Bank";

    const APPS: FieldDef = FieldDef {
        name: "apps",
        privacy: Shown::OnLock(|lock| lock.notification_detail == NotificationDetail::Apps),
        ty: ui::descriptor::FieldType::Text,
    };

    fn line(said: String) -> Built {
        let theme = telar::use_theme::<NordTheme>();
        Ok(Box::new(Text::new(
            move || said.clone(),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Body, theme.text),
        )?))
    }

    fn face(_: &Host) -> Built {
        line("reading".to_string())
    }

    fn waiting(host: &Host) -> Built {
        line(format!(
            "3 waiting {}",
            host.reveal(&APPS, FROM.to_string())
        ))
    }

    const fn widget(id: &'static str, build: fn(&Host) -> Built, input: Input) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name: id,
            icon: "circle",
            category: Category::Info,
            options: &[],
            representations: Representations {
                widget: Some(WidgetDef {
                    sizes: &WidgetSize::ALL,
                    build,
                    input,
                }),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    /// The readings the shipped lock layer places, and a control.
    static PROBES: &[ModuleDescriptor] = &[
        widget("clock", face, Input::ReadOnly),
        widget("user", face, Input::ReadOnly),
        widget("media", face, Input::ReadOnly),
        widget("notifications", waiting, Input::ReadOnly),
        widget("mixer", face, Input::Interactive),
    ];

    thread_local! {
        static DRAWN: RefCell<Option<(telar::OwnerId, ComponentList, telar::NodeId)>> = const { RefCell::new(None) };
    }

    /// An owner for what a test builds, disposed when it ends.
    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            lock_mode::chosen().set(None);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            undraw();
            lock_mode::chosen().set(None);
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn the_mode() -> Mode {
        mode::current().expect("the mode is up")
    }

    fn undraw() {
        if let Some((owner, tree, _)) = DRAWN.with(|drawn| drawn.borrow_mut().take()) {
            drop(tree);
            telar::dispose_owner(owner);
        }
    }

    /// Draws the lock preview the host draws, over the whole screen, so what the tools read from the rect registry is there — the rig runs no compositor, so its windows build nothing. What was drawn before is taken down first.
    fn draw() -> Vec<String> {
        undraw();
        let scope = telar::owner_scope();
        let page = Container::new(
            LayoutStyle::new().width(SIZE.0).height(SIZE.1),
            vec![lock_mode::preview(SCREEN).expect("the preview builds")],
        )
        .expect("a screen");
        let root = page.layout_node();
        let tree = ComponentList::new(page);
        DRAWN.with(|drawn| *drawn.borrow_mut() = Some((scope.id(), tree, root)));
        said()
    }

    /// Everything the drawn preview says now, laid out again first.
    fn said() -> Vec<String> {
        DRAWN.with(|drawn| {
            let drawn = drawn.borrow();
            let (_, tree, root) = drawn.as_ref().expect("the preview is drawn");
            compute_layout(
                *root,
                AvailableSpace::Definite(SIZE.0),
                AvailableSpace::Definite(SIZE.1),
            )
            .expect("the preview lays out");
            tree.commands()
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::Text { text, .. } => Some(text.to_string()),
                    _ => None,
                })
                .collect()
        })
    }

    const NONE: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };

    fn prompt() -> Node {
        Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("prompt"))
    }

    fn lock_areas(layout: &Layout) -> Vec<Area> {
        layout.outputs[0].layers.lock.areas.clone()
    }

    fn lock_ids(layout: &Layout) -> Vec<String> {
        lock_areas(layout)
            .iter()
            .map(|area| area.id.to_string())
            .collect()
    }

    fn prompt_rect(layout: &Layout) -> Rect {
        let (resolved, _) = layout::resolve(layout, &crate::written::known(), SCREEN, None);
        resolved
            .layer(LayerKind::Lock)
            .and_then(|layer| {
                layer.areas.iter().find_map(|area| match area.kind {
                    ResolvedAreaKind::Prompt { rect } => Some(rect),
                    _ => None,
                })
            })
            .expect("the lock layer has its prompt")
    }

    /// What a lock taken now would refuse `layout`'s lock layer for.
    fn refused(layout: &Layout) -> Vec<String> {
        LockLayout::problems(
            layout,
            &crate::written::known(),
            &Descriptors::installed(),
            &Config::default().resolve_theme(),
            &[Some(SCREEN)],
            "layouts/mine.toml",
        )
        .errors
        .into_iter()
        .map(|found| found.message.english())
        .collect()
    }

    fn lock_site() -> Site {
        Site {
            output: layout::OutputMatch("*".into()),
            workspace: None,
            layer: LayerKind::Lock,
        }
    }

    /// Lock mode is the background and desktop modes' tools over a preview: every tool builds, the preview's areas, instances and prompt are in the rect registry the tools read, the prompt is selected by a press on it, and the keys the mode answers include the lock's own.
    #[test]
    fn lock_mode_borrows_the_background_and_desktop_tools_over_its_preview() {
        let _rig = rig_with("lock-tools", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let said = draw();
        assert!(said.iter().any(|text| text == "Preview"), "{said:?}");
        assert!(
            !said.iter().any(|text| text.contains("minimal lock")),
            "the shipped lock layer is drawn as it is: {said:?}"
        );

        let at = rects::rect(&prompt()).expect("the prompt is in the registry");
        assert!(
            close(at.x, 0.3 * SIZE.0) && close(at.width, 0.4 * SIZE.0),
            "{at:?}"
        );
        let placed = rects::on(Some(SCREEN), LayerKind::Lock);
        let readings = Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("lock-readings"));
        assert!(
            placed.iter().any(|(node, _)| *node == readings),
            "{placed:?}"
        );
        assert!(
            rects::instance(Some(SCREEN), &layout::InstanceId::new("lock-clock")).is_some(),
            "{placed:?}"
        );

        let mode = the_mode();
        select::tool(&mode).expect("the selection builds");
        background::tool(&mode).expect("the region tools build");
        widgets::tool(&mode).expect("the grid tools build");
        lock_mode::tool(&mode).expect("the prompt's handle builds");

        select::press_at(&prompt(), (at.x + at.width / 2.0, at.y + at.height - 10.0));
        assert_eq!(session::selected(), Selection::Area(prompt()));

        let table = keys::table(LayerKind::Lock);
        for (name, operation) in [
            ("widget-add", "lock-palette"),
            ("grid-create", "lock-grid"),
            ("lock-privacy", "lock-privacy"),
        ] {
            assert!(
                table
                    .iter()
                    .any(|row| row.name == name && row.covers.contains(&operation)),
                "{name}"
            );
        }
        assert!(
            table
                .iter()
                .any(|row| row.name == "region-split" && row.covers.contains(&"lock-regions"))
        );
    }

    /// TA-8: the prompt has no remove affordance that works — Delete is refused, a secondary press opens no menu, the area menu's removal is refused — and each says why in the strip; nothing reaches the history.
    #[test]
    fn the_prompt_cannot_be_deleted_by_the_keyboard_or_the_pointer() {
        let rig = rig_with("lock-prompt-kept", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        draw();
        let before = stored(&rig);

        assert!(session::select(Selection::Area(prompt())));
        assert!(tap(Key::Named(NamedKey::Delete), NONE));
        assert_eq!(stored(&rig), before);
        let why = mode::refusal().peek().expect("the strip says why");
        assert!(why.contains("never removed"), "{why}");

        let at = rects::rect(&prompt()).expect("drawn");
        select::menu_at(&prompt(), (at.x + 10.0, at.y + 10.0));
        assert!(!transient::is_open(context::ID));
        let why = mode::refusal().peek().expect("the strip says why");
        assert!(why.contains("no context menus"), "{why}");

        assert!(context::remove_area(&prompt(), "prompt").is_err());
        assert_eq!(stored(&rig), before);
        assert_eq!(rig.undo_label(), None);
    }

    /// The prompt moves wherever it is put, kept wholly on its screen and no smaller than a prompt may be: dragged off an edge, stepped past one, shrunk past its least.
    #[test]
    fn the_prompt_moves_and_shrinks_kept_wholly_on_its_screen() {
        let rig = rig_with("lock-prompt-moves", |layout| {
            if let Some(area) = layout.outputs[0]
                .layers
                .lock
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == "prompt")
            {
                area.kind = Some(AreaKind::Prompt {
                    rect: Some(Rect {
                        x: 0.92,
                        y: 0.35,
                        w: 0.06,
                        h: 0.3,
                    }),
                });
            }
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);

        let ops = lock_mode::moved_to(
            &session::draft().peek(),
            &prompt(),
            Rect {
                x: 1.4,
                y: -0.5,
                w: 0.06,
                h: 0.3,
            },
        )
        .expect("the prompt is written by the layout");
        let mut dragged = session::draft().peek();
        layout::ops::apply_all(&mut dragged, &ops).expect("the move applies");
        let at = prompt_rect(&dragged);
        assert!(close(at.x, 0.94) && close(at.y, 0.0), "{at:?}");

        assert!(session::select(Selection::Area(prompt())));
        let shift = ModifiersState {
            is_shift: true,
            ..NONE
        };
        for _ in 0..3 {
            tap(Key::Named(NamedKey::ArrowRight), shift);
        }
        let at = prompt_rect(&stored(&rig));
        assert!(close(at.x + at.w, 1.0), "{at:?}");

        let ctrl = ModifiersState {
            is_ctrl: true,
            ..NONE
        };
        for _ in 0..3 {
            tap(Key::Named(NamedKey::ArrowLeft), ctrl);
        }
        let at = prompt_rect(&stored(&rig));
        assert!(close(at.w, SMALLEST_PROMPT), "{at:?}");
        assert!(refused(&stored(&rig)).is_empty());
    }

    /// The prompt's style row says how readable its text is, live; a fill or an opacity the lock would refuse is flagged and never kept — each with the lock's own reason. An area listed after the prompt is kept, since the lock layer resolves with the prompt last.
    #[test]
    fn an_edit_the_lock_would_fall_back_from_is_flagged_and_not_kept() {
        let rig = rig_with("lock-contrast", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let theme = Config::default().resolve_theme();
        let styled = |fill: &str| Style {
            fill: Some(fill.to_string()),
            ..Style::default()
        };
        assert!(lock_mode::contrast_of(&styled("text"), &theme).contains("fall back"));
        assert!(!lock_mode::contrast_of(&styled("surface"), &theme).contains("fall back"));
        assert!(!lock_mode::contrast_of(&Style::default(), &theme).contains("fall back"));

        let before = stored(&rig);
        let restyled = |style: Style| {
            let mut area = lock_areas(&session::draft().peek())
                .into_iter()
                .find(|area| area.id.as_str() == "prompt")
                .expect("the prompt");
            area.style = style;
            context::commit(
                "restyle".to_string(),
                vec![LayoutOp::ReplaceArea {
                    site: lock_site(),
                    id: AreaId::new("prompt"),
                    area: Box::new(area),
                }],
            )
        };
        let unreadable = restyled(styled("text")).expect_err("refused");
        assert!(unreadable.to_string().contains("minimal"), "{unreadable}");
        let faint = restyled(Style {
            opacity: Some(0.5),
            ..Style::default()
        })
        .expect_err("refused");
        assert!(faint.to_string().contains("too faint"), "{faint}");
        assert_eq!(stored(&rig), before);

        context::commit(
            "cover".to_string(),
            vec![LayoutOp::InsertArea {
                site: lock_site(),
                index: lock_areas(&before).len(),
                area: Box::new(Area {
                    id: AreaId::new("over"),
                    kind: Some(AreaKind::Free {
                        rect: Some(Rect::default()),
                        anchor: None,
                    }),
                    ..Area::default()
                }),
            }],
        )
        .expect("an area listed after the prompt is kept, for the prompt is always resolved last");
        assert!(refused(&stored(&rig)).is_empty());

        restyled(styled("surface")).expect("a readable card is kept");
        assert!(refused(&stored(&rig)).is_empty());
    }

    /// The preview is checked the way a real lock is: a lock layer the lock would refuse previews as the minimal lock, and says why.
    #[test]
    fn the_preview_falls_back_as_the_lock_would_and_says_why() {
        let _rig = rig_with("lock-preview-falls-back", |layout| {
            let prompt = layout.outputs[0]
                .layers
                .lock
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == "prompt")
                .expect("the prompt");
            prompt.style.opacity = Some(0.2);
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let said = draw();
        assert!(
            said.iter()
                .any(|text| text.contains("minimal lock") && text.contains("too faint")),
            "{said:?}"
        );
        assert!(said.iter().any(|text| text == "Preview"), "{said:?}");
        assert!(
            !said.iter().any(|text| text == "reading"),
            "none of the refused layer's readings is drawn: {said:?}"
        );
    }

    /// A grid made on the lock layer, and a region split there, go under the prompt, so the lock layer they leave is one the lock draws; a reading lands on the new grid, a control does not.
    #[test]
    fn grids_and_regions_made_on_the_lock_go_under_its_prompt() {
        let rig = rig_with("lock-grid-region", |layout| {
            layout.outputs[0].layers.lock.areas.insert(
                0,
                Area {
                    id: AreaId::new("lock-picture"),
                    kind: Some(AreaKind::WallpaperRegion {
                        rect: Some(Rect::default()),
                        source: None,
                        fit: None,
                        transition: None,
                        focus: None,
                        dim: None,
                        blur: None,
                        parallax: None,
                    }),
                    ..Area::default()
                },
            );
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let under_the_prompt = |layout: &Layout| {
            let ids = lock_ids(layout);
            assert_eq!(ids.last().map(String::as_str), Some("prompt"), "{ids:?}");
        };

        assert!(tap(
            Key::Char('n'),
            ModifiersState {
                is_alt: true,
                ..NONE
            }
        ));
        let made = stored(&rig);
        under_the_prompt(&made);
        assert!(
            lock_ids(&made).iter().any(|id| id.starts_with("widgets")),
            "{:?}",
            lock_ids(&made)
        );
        assert!(refused(&made).is_empty(), "{:?}", refused(&made));

        let picture = Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("lock-picture"));
        assert!(session::select(Selection::Area(picture)));
        assert!(tap(Key::Char('s'), NONE));
        let split = stored(&rig);
        under_the_prompt(&split);
        let regions = lock_areas(&split)
            .iter()
            .filter(|area| matches!(area.kind, Some(AreaKind::WallpaperRegion { .. })))
            .count();
        assert_eq!(regions, 2, "{:?}", lock_ids(&split));
        assert!(refused(&split).is_empty(), "{:?}", refused(&split));
        draw();
        let readings = rects::rect(&Node::area(
            Some(SCREEN),
            LayerKind::Lock,
            &AreaId::new("lock-readings"),
        ))
        .expect("the readings are drawn");
        assert!(
            (readings.y - 0.12 * SIZE.1).abs() < 1.0 && readings.x.abs() < 1.0,
            "each area of the preview is placed on the screen, not after the one before it: {readings:?}"
        );

        let desktop = surfaces::reconcile::desktops()[0].clone();
        let adding = |module| desktop::Adding {
            module,
            representation: layout::Representation::WidgetM,
            at: None,
            near: (0, 0),
        };
        let grid = AreaId::new("widgets");
        let layout = session::draft().peek();
        assert!(
            desktop::added(&layout, &desktop, LayerKind::Lock, &grid, &adding("clock")).is_ok()
        );
        assert!(
            desktop::added(&layout, &desktop, LayerKind::Lock, &grid, &adding("mixer")).is_err()
        );
        let offered: Vec<String> = palette::lines(LayerKind::Lock, "")
            .into_iter()
            .filter_map(|line| match line {
                palette::Line::Entry { name, .. } => Some(name),
                palette::Line::Heading(_) => None,
            })
            .collect();
        assert!(!offered.iter().any(|name| name == "mixer"), "{offered:?}");
        assert!(offered.iter().any(|name| name == "clock"), "{offered:?}");
    }

    /// The privacy keys switch the preview's readings at once: the applications a notification came from are drawn only while `notification_detail` says `apps`, and go again when the choice is put back.
    #[test]
    fn the_privacy_switch_redacts_the_preview_live() {
        let _rig = rig_with("lock-privacy-live", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let shows_apps = |said: Vec<String>| said.iter().any(|text| text.contains(FROM));
        assert!(!shows_apps(draw()), "the count alone is the default");

        lock_mode::chosen().set(Some(Privacy {
            notifications: NotificationDetail::Apps,
            media: MediaDetail::Title,
        }));
        assert!(shows_apps(said()), "switched on the preview already drawn");

        lock_mode::chosen().set(None);
        assert!(!shows_apps(said()));
    }

    /// The privacy popover opens only in lock mode, builds, and Esc closes it with nothing chosen and nothing written; a kept choice is written into the `[lock]` of a config, around everything else the file says.
    #[test]
    fn the_privacy_popover_writes_config_and_esc_writes_nothing() {
        let _rig = rig_with("lock-privacy-popover", |_| {});
        let _owner = Owner::new();
        let running = Config::default_path();
        let _ = Config::load_or_default(&running);
        let before = std::fs::read_to_string(&running).ok();

        assert!(
            lock_mode::open_privacy().is_err(),
            "no lock mode, no privacy popover"
        );
        let _host = enter(LayerKind::Lock);
        lock_mode::open_privacy().expect("it opens in lock mode");
        assert!(transient::is_open(lock_mode::PRIVACY));
        let _card = lock_mode::privacy_card(SCREEN).expect("the card builds");
        assert!(tap(Key::Named(NamedKey::Escape), NONE));
        assert!(!transient::is_open(lock_mode::PRIVACY));
        assert_eq!(lock_mode::chosen().peek(), None);
        assert_eq!(std::fs::read_to_string(&running).ok(), before);

        let path = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("editor-lock-privacy")
            .join("config.toml");
        let _ = std::fs::create_dir_all(path.parent().expect("a directory"));
        std::fs::write(&path, "# mine\n[lock]\nmax_tries = 7\n").expect("a config");
        lock_mode::write(
            &path,
            Privacy {
                notifications: NotificationDetail::Apps,
                media: MediaDetail::State,
            },
        )
        .expect("it writes");
        let written = std::fs::read_to_string(&path).expect("the config");
        assert!(written.starts_with("# mine"), "{written}");
        assert!(written.contains("max_tries = 7"), "{written}");
        assert!(
            written.contains("notification_detail = \"apps\""),
            "{written}"
        );
        assert!(written.contains("media_detail = \"state\""), "{written}");
    }

    /// TA-8: entering lock mode, using every tool it has and leaving it never asks for a session lock.
    #[test]
    fn lock_mode_and_its_tools_never_take_a_session_lock() {
        let _rig = rig_with("lock-no-session", |_| {});
        let _owner = Owner::new();
        let asked = Rc::new(Cell::new(0));
        services::lock::set_session_opener({
            let asked = Rc::clone(&asked);
            move |_| {
                asked.set(asked.get() + 1);
                panic!("lock mode asked for a session lock")
            }
        });

        let _host = enter(LayerKind::Lock);
        draw();
        let mode = the_mode();
        select::tool(&mode).expect("the selection builds");
        background::tool(&mode).expect("the region tools build");
        widgets::tool(&mode).expect("the grid tools build");
        lock_mode::tool(&mode).expect("the prompt's handle builds");
        assert!(tap(Key::Char('a'), NONE));
        assert!(transient::is_open(palette::ID));
        transient::close(palette::ID);
        assert!(tap(Key::Char('p'), NONE));
        assert!(transient::is_open(lock_mode::PRIVACY));
        transient::close(lock_mode::PRIVACY);
        assert!(tap(
            Key::Char('n'),
            ModifiersState {
                is_alt: true,
                ..NONE
            }
        ));
        draw();
        mode::leave();

        assert_eq!(asked.get(), 0);
        assert!(!platform_wayland::session_is_locked());
        assert!(!services::lock::is_locked());
    }

    /// A lock region gets a texture from `t` and from the lock toolbar's button, as the background mode's regions do: laid right over it, under the prompt, and one entry each.
    #[test]
    fn a_lock_region_takes_a_texture_from_its_key_and_the_toolbar() {
        let rig = rig_with("lock-texture", |layout| {
            layout.outputs[0].layers.lock.areas.insert(
                0,
                Area {
                    id: AreaId::new("lock-picture"),
                    kind: Some(AreaKind::WallpaperRegion {
                        rect: Some(Rect::default()),
                        source: None,
                        fit: None,
                        transition: None,
                        focus: None,
                        dim: None,
                        blur: None,
                        parallax: None,
                    }),
                    ..Area::default()
                },
            );
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let picture = Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("lock-picture"));
        assert!(session::select(Selection::Area(picture)));

        assert!(tap(Key::Char('t'), NONE));
        let keyed = stored(&rig);
        assert_eq!(
            lock_ids(&keyed),
            ["lock-picture", "texture", "lock-readings", "prompt"],
            "right over the region, under the prompt"
        );
        assert!(refused(&keyed).is_empty(), "{:?}", refused(&keyed));

        let (_, press) = crate::host::toolbar_of(LayerKind::Lock)
            .into_iter()
            .find(|(label, _)| label() == "Add texture")
            .expect("the lock toolbar adds a texture");
        press();
        let pressed = stored(&rig);
        assert_eq!(
            lock_ids(&pressed),
            [
                "lock-picture",
                "texture-2",
                "texture",
                "lock-readings",
                "prompt"
            ]
        );
        session::undo().expect("the button's texture is undone");
        assert_eq!(stored(&rig), keyed, "each was one entry");
    }

    /// B6: on the lock screen a widget steps only through the sizes it may be drawn at there, so a control placed on it by hand is offered no size at all — by Ctrl+arrows as by the palette — rather than one the lock would refuse.
    #[test]
    fn a_control_on_the_lock_is_offered_no_size() {
        let rig = rig_with("lock-sizes", |layout| {
            let readings = layout.outputs[0]
                .layers
                .lock
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == "lock-readings")
                .expect("the shipped readings");
            let media = readings
                .groups
                .iter_mut()
                .flat_map(|group| group.children.iter_mut())
                .find(|child| child.id.as_str() == "lock-media")
                .expect("the shipped media reading");
            media.module = Some("mixer".to_string());
            media.representation = Some(layout::Representation::WidgetS);
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        assert!(desktop::sizes_of("mixer", LayerKind::Lock).is_empty());
        assert_eq!(
            desktop::sizes_of("mixer", LayerKind::Desktop).len(),
            3,
            "elsewhere it steps through all three"
        );
        assert_eq!(
            desktop::sizes_of("clock", LayerKind::Lock).len(),
            3,
            "a reading steps through all three on the lock too"
        );

        let before = stored(&rig);
        let mixer = Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("lock-readings"))
            .instance(
                &layout::GroupId::new("media"),
                &layout::InstanceId::new("lock-media"),
            );
        assert!(session::select(Selection::Instance(mixer)));
        assert!(tap(
            Key::Named(NamedKey::ArrowRight),
            ModifiersState {
                is_ctrl: true,
                ..NONE
            }
        ));
        assert_eq!(stored(&rig), before);
        assert_eq!(
            mode::refusal().peek(),
            Some("'mixer' is not drawn at another size that way".to_string())
        );
    }

    fn with_readings(layout: &Layout, change: impl FnOnce(&mut Area)) -> Layout {
        let mut changed = layout.clone();
        let readings = changed.outputs[0]
            .layers
            .lock
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "lock-readings")
            .expect("the shipped readings");
        change(readings);
        changed
    }

    fn press() -> std::collections::BTreeMap<layout::Trigger, layout::Action> {
        [(
            layout::Trigger::Press,
            layout::Action(vec!["launch foot".to_string()]),
        )]
        .into()
    }

    /// The lock layer refuses what a locked screen must not answer to — an action on an area or on an instance, a panel, a control — each said by the lock's own check and each kept out of the layout by `kept`.
    #[test]
    fn the_lock_refuses_actions_panels_and_controls() {
        let rig = rig_with("lock-refusals", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let before = stored(&rig);
        assert!(refused(&before).is_empty());

        let on_area = with_readings(&before, |area| area.actions = press());
        let on_instance = with_readings(&before, |area| {
            area.groups[0].children[0].actions = press();
        });
        let panel = {
            let mut layout = before.clone();
            layout.outputs[0].layers.lock.areas.insert(
                0,
                Area {
                    id: AreaId::new("lock-panel"),
                    kind: Some(AreaKind::Panel {
                        owner: Some(layout::InstanceId::new("lock-clock")),
                        along: None,
                        cols: None,
                        rows: None,
                        cell: None,
                        gap: None,
                    }),
                    ..Area::default()
                },
            );
            layout
        };
        let control = with_readings(&before, |area| {
            area.groups[0].children[0].module = Some("mixer".to_string());
        });
        for (name, after) in [
            ("an action on an area", on_area),
            ("an action on an instance", on_instance),
            ("a panel", panel),
            ("a control", control),
        ] {
            assert!(!refused(&after).is_empty(), "{name} is refused");
            assert!(
                lock_mode::kept(&before, &after).is_err(),
                "{name} is not kept"
            );
        }
    }

    /// Containers and the style of an instance are allowed on the lock: they only arrange and paint readings.
    #[test]
    fn containers_and_instance_style_are_allowed_on_the_lock() {
        let rig = rig_with("lock-containers", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let before = stored(&rig);
        let after = with_readings(&before, |area| {
            let group = &mut area.groups[0];
            group.arrange = Some(layout::Arrange::Column);
            group.gap = Some(12.0);
            group.style.fill = Some("surface".to_string());
            let child = &mut group.children[0];
            child.style = Style {
                fill: Some("surface".to_string()),
                radius: Some(layout::Corners::all(12.0)),
                shadow: Some(2),
                border: Some(layout::Border {
                    width: Some(2.0),
                    color: Some("accent".to_string()),
                }),
                ..Style::default()
            };
        });
        assert!(refused(&after).is_empty(), "{:?}", refused(&after));
        assert!(lock_mode::kept(&before, &after).is_ok());
    }

    /// A prompt with a border and a shadow is still a prompt the lock draws, so the edit is kept; an unreadable fill is not.
    #[test]
    fn a_shadowed_bordered_prompt_passes_kept() {
        let rig = rig_with("lock-prompt-edge", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let before = stored(&rig);
        let styled = |style: Style| {
            let mut after = before.clone();
            after.outputs[0]
                .layers
                .lock
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == "prompt")
                .expect("the prompt")
                .style = style;
            after
        };
        let edged = styled(Style {
            shadow: Some(3),
            border: Some(layout::Border {
                width: Some(3.0),
                color: Some("accent".to_string()),
            }),
            ..Style::default()
        });
        assert!(refused(&edged).is_empty(), "{:?}", refused(&edged));
        assert!(lock_mode::kept(&before, &edged).is_ok());
        let unreadable = styled(Style {
            fill: Some("text".to_string()),
            shadow: Some(3),
            ..Style::default()
        });
        assert!(lock_mode::kept(&before, &unreadable).is_err());
    }

    /// Shift+B adds a region over the whole lock screen, at the back where the readings and the prompt stay above it, as one undo entry the lock accepts.
    #[test]
    fn a_full_screen_region_is_added_in_lock_mode() {
        let rig = rig_with("lock-full-region", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        assert!(tap(
            Key::Char('B'),
            ModifiersState {
                is_shift: true,
                ..NONE
            }
        ));
        let made = stored(&rig);
        let ids = lock_ids(&made);
        assert_eq!(ids.first().map(String::as_str), Some("region"), "{ids:?}");
        assert_eq!(ids.last().map(String::as_str), Some("prompt"), "{ids:?}");
        let region = &lock_areas(&made)[0];
        assert!(matches!(
            region.kind,
            Some(AreaKind::WallpaperRegion { rect: Some(Rect { x, y, w, h }), .. })
                if (x, y, w, h) == (0.0, 0.0, 1.0, 1.0)
        ));
        assert!(refused(&made).is_empty(), "{:?}", refused(&made));
        draw();
        let rect = rects::rect(&Node::area(
            Some(SCREEN),
            LayerKind::Lock,
            &AreaId::new("region"),
        ))
        .expect("the region is drawn");
        assert!(
            rect.x.abs() < 1.0
                && (rect.width - SIZE.0).abs() < 1.0
                && (rect.height - SIZE.1).abs() < 1.0,
            "{rect:?}"
        );
        assert!(rig.undo_label().is_some());
    }

    fn with_prompt(layout: &Layout, change: impl FnOnce(&mut Area)) -> Layout {
        let mut changed = layout.clone();
        let prompt = changed.outputs[0]
            .layers
            .lock
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "prompt")
            .expect("the shipped prompt");
        change(prompt);
        changed
    }

    fn holding(child: Instance) -> layout::Group {
        layout::Group {
            id: layout::GroupId::new("inside"),
            kind: Some(layout::GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
            }),
            children: vec![child],
            ..layout::Group::default()
        }
    }

    fn reading(change: impl FnOnce(&mut Instance)) -> Instance {
        let mut child = Instance {
            id: layout::InstanceId::new("inside-clock"),
            module: Some("clock".to_string()),
            representation: Some(layout::Representation::WidgetS),
            ..Instance::default()
        };
        change(&mut child);
        child
    }

    /// Nothing an edit can do makes the prompt invisible, unreachable, too faint or unreadable — each said in the lock's own words and each kept out of the layout by `kept`, down to the boundary of what is still allowed, and for what the prompt holds as for the prompt itself.
    #[test]
    fn the_lock_refuses_a_prompt_that_is_hidden_off_screen_too_small_faint_or_unreadable() {
        let rig = rig_with("lock-prompt-refusals", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let before = stored(&rig);
        let placed = |rect: Rect| {
            with_prompt(&before, |prompt| {
                prompt.kind = Some(AreaKind::Prompt { rect: Some(rect) });
            })
        };
        let faded =
            |opacity: f32| with_prompt(&before, |prompt| prompt.style.opacity = Some(opacity));

        for (name, after) in [
            ("fully opaque", faded(1.0)),
            ("the faintest allowed", faded(layout::FAINTEST_PROMPT)),
            (
                "the smallest allowed",
                placed(Rect {
                    x: 0.4,
                    y: 0.4,
                    w: SMALLEST_PROMPT,
                    h: SMALLEST_PROMPT,
                }),
            ),
            (
                "flush with the edge",
                placed(Rect {
                    x: 0.6,
                    y: 0.6,
                    w: 0.4,
                    h: 0.4,
                }),
            ),
        ] {
            assert!(refused(&after).is_empty(), "{name}: {:?}", refused(&after));
            assert!(lock_mode::kept(&before, &after).is_ok(), "{name}");
        }

        let cases = [
            (
                "just under the faintest",
                faded(layout::FAINTEST_PROMPT - 0.01),
                "too faint",
            ),
            (
                "a visibility expression",
                with_prompt(&before, |prompt| {
                    prompt.visible = Some(layout::Expr("false".to_string()));
                }),
                "visibility expression",
            ),
            (
                "past the right edge",
                placed(Rect {
                    x: 0.7,
                    y: 0.4,
                    w: 0.4,
                    h: 0.2,
                }),
                "does not lie on the output",
            ),
            (
                "just under the smallest",
                placed(Rect {
                    x: 0.4,
                    y: 0.4,
                    w: SMALLEST_PROMPT - 0.01,
                    h: 0.2,
                }),
                "too small",
            ),
            (
                "a card its text cannot be read on",
                with_prompt(&before, |prompt| {
                    prompt.style.fill = Some("text".to_string());
                }),
                "WCAG AA",
            ),
            (
                "a group of it too faint",
                with_prompt(&before, |prompt| {
                    prompt.groups.push(layout::Group {
                        style: Style {
                            opacity: Some(0.5),
                            ..Style::default()
                        },
                        ..holding(reading(|_| {}))
                    });
                }),
                "too faint",
            ),
            (
                "a child of it on an unreadable card",
                with_prompt(&before, |prompt| {
                    prompt.groups.push(holding(reading(|child| {
                        child.style.fill = Some("text".to_string());
                    })));
                }),
                "WCAG AA",
            ),
            (
                "a child of it painted by an expression",
                with_prompt(&before, |prompt| {
                    prompt.groups.push(holding(reading(|child| {
                        child
                            .bindings
                            .insert("style.opacity".to_string(), layout::Expr("0.1".to_string()));
                    })));
                }),
                "painted by an expression",
            ),
        ];
        for (name, after, says) in cases {
            let said = refused(&after).join("\n");
            assert!(said.contains(says), "{name}: {said:?}");
            let why = lock_mode::kept(&before, &after).expect_err(name).english();
            assert!(why.contains("minimal"), "{name}: {why}");
        }
    }

    /// `kept` judges what an edit adds: a lock layer that was already refused is not refused again for something else in it, so it can still be fixed one step at a time, but a new fault is.
    #[test]
    fn a_lock_layer_already_refused_can_be_fixed_in_steps_and_is_refused_a_new_fault() {
        let rig = rig_with("lock-fix-in-steps", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let sound = stored(&rig);
        let broken = with_prompt(&sound, |prompt| {
            prompt.style.opacity = Some(0.5);
            prompt.kind = Some(AreaKind::Prompt {
                rect: Some(Rect {
                    x: 0.8,
                    y: 0.4,
                    w: 0.4,
                    h: 0.2,
                }),
            });
        });
        assert_eq!(refused(&broken).len(), 2, "{:?}", refused(&broken));

        let half_fixed = with_prompt(&broken, |prompt| prompt.style.opacity = Some(0.95));
        assert!(
            lock_mode::kept(&broken, &half_fixed).is_ok(),
            "a step toward a lock layer that holds"
        );
        let decorated = with_prompt(&broken, |prompt| prompt.style.shadow = Some(2));
        assert!(
            lock_mode::kept(&broken, &decorated).is_ok(),
            "an edit that adds no fault"
        );
        let worse = with_prompt(&broken, |prompt| {
            prompt.style.fill = Some("text".to_string());
        });
        assert!(
            lock_mode::kept(&broken, &worse).is_err(),
            "one more fault is one too many"
        );
        let untouched = with_readings(&broken, |area| {
            area.style.shadow = Some(1);
        });
        assert!(
            lock_mode::kept(&broken, &untouched).is_ok(),
            "an edit to the readings that does not touch what is wrong"
        );
    }

    #[test]
    fn fixing_one_fault_of_a_prompt_with_two_is_not_refused_for_the_message_of_the_other() {
        let rig = rig_with("lock-fix-quoted-ratio", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let broken = with_prompt(&stored(&rig), |prompt| {
            prompt.style.opacity = Some(0.5);
            prompt.style.fill = Some("text".to_string());
        });
        let half_fixed = with_prompt(&broken, |prompt| prompt.style.opacity = Some(0.95));
        assert!(lock_mode::kept(&broken, &half_fixed).is_ok());
    }

    /// The shell's own menu belongs to the layers that have a desktop under them; on the lock preview it is refused, says why, and opens nothing.
    #[test]
    fn the_shell_menu_is_refused_on_the_lock_preview() {
        let rig = rig_with("lock-shell-menu", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        draw();
        let before = stored(&rig);

        let why = crate::shell_menu::open(surfaces::menu::ShellAsked {
            output: Some(SCREEN.to_string()),
            at: Some((960.0, 40.0)),
        })
        .expect_err("no shell menu over the lock preview");
        assert!(why.to_string().contains("no context menus"), "{why}");
        assert!(!transient::is_open(context::ID));
        assert_eq!(stored(&rig), before);
        assert_eq!(rig.undo_label(), None);
    }

    /// A reading on the lock is copied by Ctrl+D as it is on any layer, as one undo entry, and the copy is a reading the lock accepts — the keys offer nothing the lock check would refuse.
    #[test]
    fn a_reading_duplicated_on_the_lock_is_one_entry_and_a_lock_the_check_accepts() {
        let rig = rig_with("lock-duplicate", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let before = stored(&rig);
        let clock = Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("lock-readings"))
            .instance(
                &layout::GroupId::new("clock"),
                &layout::InstanceId::new("lock-clock"),
            );
        assert!(session::select(Selection::Instance(clock)));

        assert!(tap(
            Key::Char('d'),
            ModifiersState {
                is_ctrl: true,
                ..NONE
            }
        ));
        let after = stored(&rig);
        let count = |layout: &Layout| {
            lock_areas(layout)
                .iter()
                .flat_map(|area| area.groups.iter())
                .flat_map(|group| group.children.iter())
                .count()
        };
        assert_eq!(count(&after), count(&before) + 1);
        assert!(refused(&after).is_empty(), "{:?}", refused(&after));
        assert_eq!(lock_ids(&after).last().map(String::as_str), Some("prompt"));
        assert!(rig.undo_label().is_some());
        session::undo().expect("one entry");
        assert_eq!(stored(&rig), before);
    }
}
