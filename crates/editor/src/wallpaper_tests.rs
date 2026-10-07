//! A wallpaper region's picture as its popover sets it: picked from the library by a press, its focus set by rows and by a dot on the region the pointer drags and the arrows step, how it answers windows and workspaces, and all of it for one workspace alone.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use telar::{Key, NamedKey};

    use layout::{
        ActiveWorkspace, Area, AreaId, AreaKind, Fit, Focus, LayerKind, Layout, Rect,
        ResolvedAreaKind, Transition, WorkspaceMatch,
    };
    use services::wallpaper::Entry;
    use surfaces::rects::Node;

    use crate::popover::picture::{self, FOCUS_STEP};
    use crate::rig::{
        Card, NONE, SCREEN, Scope, enter, pointer_at, press_at, release_at, rig_on, rig_with,
        stored,
    };
    use crate::{mode, popover, session, variant};

    /// Where the right-hand region is drawn on the rig's screen.
    const RIGHT: telar::Rect = telar::Rect {
        x: 1440.0,
        y: 0.0,
        width: 480.0,
        height: 1080.0,
    };

    fn rect(x: f32, w: f32) -> Option<Rect> {
        Some(Rect {
            x,
            y: 0.0,
            w,
            h: 1.0,
        })
    }

    fn plain(rect: Option<Rect>) -> AreaKind {
        AreaKind::WallpaperRegion {
            rect,
            source: None,
            fit: None,
            transition: None,
            focus: None,
            dim: None,
            blur: None,
            parallax: None,
        }
    }

    /// Two columns: the screen's own region on the left, and one on the right that writes nothing but where it is.
    fn halves(layout: &mut Layout) {
        let background = &mut layout.outputs[0].layers.background;
        if let Some(AreaKind::WallpaperRegion { rect: at, .. }) = &mut background.areas[0].kind {
            *at = rect(0.0, 0.75);
        }
        background.areas.push(Area {
            id: AreaId::new("right"),
            kind: Some(plain(rect(0.75, 0.25))),
            ..Area::default()
        });
    }

    fn right() -> Node {
        Node::area(Some(SCREEN), LayerKind::Background, &AreaId::new("right"))
    }

    fn written(layout: &Layout) -> Option<AreaKind> {
        layout.outputs[0]
            .layers
            .background
            .areas
            .iter()
            .find(|area| area.id.as_str() == "right")
            .and_then(|area| area.kind.clone())
    }

    fn library() -> Vec<Entry> {
        ["dunes", "lake", "harbour"]
            .into_iter()
            .map(|name| Entry {
                path: PathBuf::from(format!("/pictures/library/{name}.png")),
                name: name.to_string(),
                folder: String::new(),
            })
            .collect()
    }

    fn close(a: Focus, b: Focus) -> bool {
        (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4
    }

    /// The library is narrowed by name or folder, and never draws more than the picker holds.
    #[test]
    fn the_library_is_narrowed_by_name_and_folder() {
        let mut entries = library();
        entries[2].folder = "coast".to_string();
        let names = |query: &str| {
            picture::narrowed(&entries, query)
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), ["dunes", "lake", "harbour"]);
        assert_eq!(names(" LAKE "), ["lake"]);
        assert_eq!(names("coast"), ["harbour"]);
        assert!(names("forest").is_empty());
        let many: Vec<Entry> = (0..100).map(|_| entries[0].clone()).collect();
        assert!(picture::narrowed(&many, "").len() < many.len());
    }

    /// A picture pressed in the popover's library becomes the region's own, written into the region as one entry.
    #[test]
    fn a_picture_pressed_in_the_library_becomes_the_regions_own() {
        services::wallpaper::seed_library(library());
        let rig = rig_with("wallpaper-picker", halves);
        let _scope = Scope::new();
        let _host = enter(LayerKind::Background);
        let before = stored(&rig);
        popover::open_area(right()).expect("the region's popover opens");
        let mut card = Card::open();
        card.press("lake");
        assert_eq!(
            popover::shared::<String>("source")
                .expect("its picture")
                .peek(),
            "/pictures/library/lake.png"
        );
        popover::close();

        let layout = stored(&rig);
        assert!(matches!(
            written(&layout),
            Some(AreaKind::WallpaperRegion { source: Some(source), .. })
                if source == "/pictures/library/lake.png"
        ));
        session::undo().expect("the pick is undone");
        assert_eq!(stored(&rig), before, "in one entry");
        mode::leave();
    }

    /// The popover's rows set the focus, how the picture dims and blurs under windows and how far it slides, all written into the region together.
    #[test]
    fn the_popover_rows_set_the_focus_the_veil_and_the_parallax() {
        let rig = rig_with("wallpaper-rows", halves);
        let _scope = Scope::new();
        let _host = enter(LayerKind::Background);
        let before = stored(&rig);
        popover::open_area(right()).expect("the region's popover opens");
        let _card = Card::open();
        popover::shared::<Focus>("focus")
            .expect("its focus")
            .set(Focus { x: 0.25, y: 0.75 });
        popover::shared::<f32>("dim").expect("its dim").set(0.35);
        popover::shared::<f32>("blur").expect("its blur").set(16.0);
        popover::shared::<f32>("parallax")
            .expect("its parallax")
            .set(0.1);
        popover::close();

        assert_eq!(
            written(&stored(&rig)),
            Some(AreaKind::WallpaperRegion {
                rect: rect(0.75, 0.25),
                source: None,
                fit: None,
                transition: None,
                focus: Some(Focus { x: 0.25, y: 0.75 }),
                dim: Some(0.35),
                blur: Some(16.0),
                parallax: Some(0.1),
            })
        );
        let drawn = surfaces::reconcile::desktops()[0]
            .resolved
            .area(LayerKind::Background, &AreaId::new("right"))
            .map(|area| area.kind.clone());
        assert!(matches!(
            drawn,
            Some(ResolvedAreaKind::WallpaperRegion { dim, blur, parallax, .. })
                if dim == 0.35 && blur == 16.0 && parallax == 0.1
        ));
        session::undo().expect("the popover is undone");
        assert_eq!(stored(&rig), before, "in one entry");
        mode::leave();
    }

    /// The dot sits as far across and down the region as the focus is across and down the picture; dragged, it sets the focus to where it is let go, and focused, each arrow steps it, held to the picture's edges.
    #[test]
    fn the_focus_dot_follows_the_pointer_and_steps_with_the_arrows() {
        let rig = rig_with("wallpaper-focus-dot", halves);
        let _scope = Scope::new();
        let _host = enter(LayerKind::Background);
        surfaces::rects::track_spanning(right(), vec![telar::signal(RIGHT)]);
        popover::open_area(right()).expect("the region's popover opens");
        let mut card = Card::open();
        let focus = popover::shared::<Focus>("focus").expect("its focus");

        let start = picture::dot_at(RIGHT, focus.peek());
        assert_eq!(start, (1680.0, 540.0), "the middle of the region");
        let to = (RIGHT.x + 120.0, 810.0);
        card.route(&press_at(start));
        card.route(&pointer_at((start.0 + 8.0, start.1 + 8.0)));
        card.route(&pointer_at(to));
        card.route(&release_at(to));
        assert!(
            close(focus.peek(), Focus { x: 0.25, y: 0.75 }),
            "{:?}",
            focus.peek()
        );

        card.key(Key::Named(NamedKey::ArrowRight), NONE);
        card.key(Key::Named(NamedKey::ArrowUp), NONE);
        assert!(
            close(
                focus.peek(),
                Focus {
                    x: 0.25 + FOCUS_STEP,
                    y: 0.75 - FOCUS_STEP
                }
            ),
            "the arrows step the focused dot: {:?}",
            focus.peek()
        );
        popover::close();
        assert!(matches!(
            written(&stored(&rig)),
            Some(AreaKind::WallpaperRegion { focus: Some(written), .. })
                if close(written, Focus { x: 0.25 + FOCUS_STEP, y: 0.75 - FOCUS_STEP })
        ));
        mode::leave();
    }

    /// A dot drag called off with Esc puts the focus back.
    #[test]
    fn a_focus_drag_cancelled_with_esc_leaves_the_region_unwritten() {
        let rig = rig_with("wallpaper-focus-cancel", halves);
        let _scope = Scope::new();
        let _host = enter(LayerKind::Background);
        surfaces::rects::track_spanning(right(), vec![telar::signal(RIGHT)]);
        let before = stored(&rig);
        popover::open_area(right()).expect("the region's popover opens");
        let mut card = Card::open();
        let start = picture::dot_at(RIGHT, Focus::MIDDLE);
        card.route(&press_at(start));
        card.route(&pointer_at((start.0 + 8.0, start.1 + 8.0)));
        card.route(&pointer_at((RIGHT.x + 40.0, 100.0)));
        assert_ne!(session::draft().peek(), before, "the drag previews");
        card.escape();
        card.route(&release_at((RIGHT.x + 40.0, 100.0)));
        popover::close();
        assert_eq!(stored(&rig), before);
        assert_eq!(rig.undo_label(), None, "nothing is recorded");
        mode::leave();
    }

    /// The focus is a step of the arrows, scaled by the modifiers, and never leaves the picture; a dragged dot is placed by where it is over the region.
    #[test]
    fn the_focus_steps_and_is_read_off_the_region() {
        let middle = Focus::MIDDLE;
        assert_eq!(
            picture::stepped(middle, &Key::Named(NamedKey::ArrowLeft)),
            Some(Focus {
                x: 0.5 - FOCUS_STEP,
                y: 0.5
            })
        );
        assert_eq!(
            picture::stepped(Focus { x: 1.0, y: 0.0 }, &Key::Named(NamedKey::ArrowRight)),
            Some(Focus { x: 1.0, y: 0.0 }),
            "held at the edge"
        );
        assert_eq!(picture::stepped(middle, &Key::Char('x')), None);
        assert_eq!(
            picture::focus_at(RIGHT, (RIGHT.x - 50.0, 2000.0)),
            Focus { x: 0.0, y: 1.0 },
            "a dot dragged off the region holds to its edge"
        );
    }

    /// "This workspace only", switched on in the region's popover, writes the picture it picks into that workspace's rule: the region shows it on that workspace and its own everywhere else.
    #[test]
    fn a_picture_for_one_workspace_is_picked_in_the_regions_popover() {
        services::wallpaper::seed_library(library());
        let rig = rig_on("wallpaper-variant", Some("2"), halves);
        let _scope = Scope::new();
        let _host = enter(LayerKind::Background);
        popover::open_area(right()).expect("the region's popover opens");
        let mut card = Card::open();
        let label = "This workspace only (2)";
        card.press(label);
        let row = card
            .texts()
            .into_iter()
            .find(|(text, _)| text == label)
            .map(|(_, at)| at)
            .expect("the switch is drawn");
        for event in crate::rig::move_and_click((row.x + 150.0, row.y + row.height / 2.0)) {
            card.route(&event);
        }
        assert!(variant::only().peek(), "the popover's switch is on");
        card.press("harbour");
        popover::close();

        let layout = stored(&rig);
        assert_eq!(
            written(&layout),
            Some(plain(rect(0.75, 0.25))),
            "nothing is written for every workspace"
        );
        let rule = &layout.outputs[0].workspaces;
        assert_eq!(rule.len(), 1);
        assert_eq!(rule[0].matches, WorkspaceMatch("2".to_string()));
        let source_on = |workspace: Option<&str>| {
            let active = workspace.map(|name| ActiveWorkspace {
                name: name.to_string(),
                id: None,
                special: None,
            });
            let (resolved, _) = layout::resolve(
                &layout,
                &layout::Library::default(),
                SCREEN,
                active.as_ref(),
            );
            match resolved
                .area(LayerKind::Background, &AreaId::new("right"))
                .map(|area| area.kind.clone())
            {
                Some(ResolvedAreaKind::WallpaperRegion {
                    source,
                    fit,
                    transition,
                    ..
                }) => {
                    assert_eq!((fit, transition), (Fit::Cover, Transition::Fade));
                    Some(source)
                }
                _ => None,
            }
        };
        assert_eq!(
            source_on(Some("2")),
            Some("/pictures/library/harbour.png".to_string())
        );
        assert_eq!(source_on(Some("3")), Some(String::new()));
        assert_eq!(source_on(None), Some(String::new()));
        mode::leave();
    }
}
