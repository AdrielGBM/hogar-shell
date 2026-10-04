//! The background mode's tools (T-7.1): regions split, joined and resized so the screen stays tiled, each change one entry in the history; textures laid over them, sliced and given gradients within what the renderer draws; and an edit made for one workspace written there and resolving there alone.

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, DismissRegistration, Event, Key, LayoutItem,
        LayoutStyle, ModifiersState, NamedKey, PointerButton, PointerSource, compute_layout,
        effect,
    };

    use layout::{
        ActiveWorkspace, Area, AreaId, AreaKind, Blend, Fit, Gradient, GradientStop, LayerKind,
        Layout, Rect, ResolvedAreaKind, Tile, Transition, Within, WorkspaceMatch,
    };
    use surfaces::rects::Node;
    use surfaces::transient;

    use crate::keys::{self, Direction, Press};
    use crate::mode::{self, Compositor};
    use crate::modes::regions::{self, Cut, Plan, SMALLEST, Tile as Region};
    use crate::modes::texture::{self, MOST_STOPS, Refusal};
    use crate::rig::{Rig, SCREEN, rig_on, rig_with};
    use crate::session::{self, Selection};
    use crate::{host, popover, variant};

    const NONE: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn tile(id: &str, at: Rect) -> Region {
        Region {
            id: AreaId::new(id),
            rect: at,
            within: Within::Output,
        }
    }

    fn bits(at: Rect) -> [u32; 4] {
        [
            at.x.to_bits(),
            at.y.to_bits(),
            at.w.to_bits(),
            at.h.to_bits(),
        ]
    }

    /// Halving and halving again, either way and at any grid position, joins back into the very rectangle that was cut — the whole screen, and every region a split makes.
    #[test]
    fn a_split_joined_again_gives_back_the_region_bit_for_bit() {
        let whole = Rect::default();
        let mut cases = vec![whole];
        for cut in [Cut::SideBySide, Cut::Stacked] {
            let (a, b) = regions::split(whole, cut, 0.5).expect("the screen splits in half");
            cases.extend([a, b]);
            let (c, d) = regions::split(a, cut, 0.3).expect("and a half splits again");
            cases.extend([c, d]);
        }
        for region in cases {
            for cut in [Cut::SideBySide, Cut::Stacked] {
                for at in [0.1, 0.2519, 0.5, 0.61803, 0.9] {
                    let Some((first, second)) = regions::split(region, cut, at) else {
                        continue;
                    };
                    let joined = regions::join(first, second).expect("the halves share an edge");
                    assert_eq!(bits(joined), bits(region), "{region:?} cut {cut:?} at {at}");
                    assert_eq!(regions::join(second, first).map(bits), Some(bits(region)));
                }
            }
        }
    }

    #[test]
    fn a_split_leaves_no_half_narrower_than_the_smallest_region() {
        let narrow = rect(0.0, 0.0, 1.5 * SMALLEST, 1.0);
        assert_eq!(
            regions::split(narrow, Cut::SideBySide, 0.75 * SMALLEST),
            None
        );
        assert!(regions::split(narrow, Cut::Stacked, 0.5).is_some());
    }

    /// A left column beside a right one cut in two: the column shares no whole edge with either half, the halves share theirs.
    fn t_junction() -> Vec<Region> {
        vec![
            tile("left", rect(0.0, 0.0, 0.25, 1.0)),
            tile("top", rect(0.25, 0.0, 0.75, 0.5)),
            tile("bottom", rect(0.25, 0.5, 0.75, 0.5)),
        ]
    }

    #[test]
    fn only_regions_sharing_a_whole_edge_join() {
        let tiles = t_junction();
        assert_eq!(regions::join(tiles[0].rect, tiles[1].rect), None);
        assert_eq!(
            regions::join(tiles[1].rect, tiles[2].rect),
            Some(rect(0.25, 0.0, 0.75, 1.0))
        );
        assert_eq!(
            regions::beside(&tiles, &tiles[1], Direction::Down).map(|found| found.id.clone()),
            Some(AreaId::new("bottom"))
        );
        assert_eq!(regions::beside(&tiles, &tiles[0], Direction::Right), None);
    }

    /// No two regions overlap, none is narrower than the smallest one, and together they cover the screen exactly.
    fn assert_tiled(tiles: &[Region]) {
        let mut covered = 0.0f64;
        for (at, one) in tiles.iter().enumerate() {
            assert!(
                one.rect.w >= SMALLEST - 1e-6 && one.rect.h >= SMALLEST - 1e-6,
                "{one:?} is too small"
            );
            covered += f64::from(one.rect.w) * f64::from(one.rect.h);
            for other in &tiles[at + 1..] {
                let across = one.rect.x.max(other.rect.x)
                    < (one.rect.x + one.rect.w).min(other.rect.x + other.rect.w);
                let down = one.rect.y.max(other.rect.y)
                    < (one.rect.y + one.rect.h).min(other.rect.y + other.rect.h);
                assert!(!(across && down), "{one:?} overlaps {other:?}");
            }
        }
        assert!(
            (covered - 1.0).abs() < 1e-6,
            "they cover {covered} of the screen"
        );
    }

    /// Wherever a shared edge is dragged — past either end too — every region on both sides follows it, clamped short of collapsing, and the screen stays tiled.
    #[test]
    fn a_shared_edge_moved_anywhere_keeps_every_region_flush() {
        let tiles = t_junction();
        let lines = regions::lines(&tiles);
        assert_eq!(
            lines.len(),
            2,
            "the upright edge and the level one: {lines:?}"
        );
        for line in &lines {
            let (low, high) = regions::range(&tiles, line);
            for step in -20..=60 {
                let to = step as f32 / 40.0;
                let mut moved = tiles.clone();
                for (id, at) in regions::moved(&tiles, line, to) {
                    let placed = moved
                        .iter_mut()
                        .find(|tile| tile.id == id)
                        .expect("a region that moved is one of them");
                    placed.rect = at;
                }
                assert_tiled(&moved);
                let now = regions::lines(&moved)
                    .into_iter()
                    .find(|moved| moved.key() == line.key())
                    .expect("the edge is still between the same regions");
                assert!(
                    now.at >= low && now.at <= high,
                    "{} past {low}..{high}",
                    now.at
                );
                assert_eq!(now.at, regions::snap(now.at), "and on the grid");
            }
        }
    }

    #[test]
    fn a_gradient_takes_stops_up_to_the_renderers_eight_and_keeps_two() {
        let mut gradient = texture::first_gradient();
        while gradient.stops.len() < MOST_STOPS {
            let (added, at) = texture::with_stop(&gradient, 0.5).expect("room for another");
            assert!(
                added.stops.windows(2).all(|pair| pair[0].at <= pair[1].at),
                "a stop goes in its place along the axis"
            );
            assert_eq!(added.stops[at].at, 0.5);
            gradient = added;
        }
        assert_eq!(texture::with_stop(&gradient, 0.2), Err(Refusal::Most));
        while gradient.stops.len() > 2 {
            gradient = texture::without_stop(&gradient, 1).expect("more than two left");
        }
        assert_eq!(texture::without_stop(&gradient, 0), Err(Refusal::Fewest));
        assert_eq!(
            texture::with_stop(&gradient, 0.9).expect("room").0.stops[1].color,
            gradient.stops[1].color,
            "a stop takes the colour of the one nearest it"
        );
    }

    #[test]
    fn a_dragged_stop_stays_between_its_neighbours() {
        let gradient = Gradient {
            angle: 0.0,
            stops: [0.0, 0.4, 0.8]
                .into_iter()
                .map(|at| GradientStop {
                    at,
                    color: "blue".to_string(),
                })
                .collect(),
        };
        assert_eq!(texture::moved_stop(&gradient, 1, 0.95).stops[1].at, 0.8);
        assert_eq!(texture::moved_stop(&gradient, 1, -1.0).stops[1].at, 0.0);
        assert_eq!(texture::moved_stop(&gradient, 1, 0.5).stops[1].at, 0.5);
    }

    #[test]
    fn the_angle_turns_and_snaps_by_45_degrees() {
        assert_eq!(texture::turned(90.0, 45.0), 135.0);
        assert_eq!(texture::turned(10.0, -45.0), 315.0);
        assert_eq!(texture::snapped(200.0), 180.0);
        let centre = (100.0, 100.0);
        assert_eq!(texture::angle_at(centre, (200.0, 100.0), false), 0.0);
        assert_eq!(texture::angle_at(centre, (100.0, 200.0), false), 90.0);
        assert_eq!(texture::angle_at(centre, (190.0, 150.0), true), 45.0);
        let axis = texture::Axis::of(telar::Rect::new(0.0, 0.0, 200.0, 100.0), 0.0);
        assert_eq!((axis.start, axis.end), ((0.0, 50.0), (200.0, 50.0)));
        assert_eq!(axis.project((150.0, 90.0)), (0.75, 40.0));
    }

    /// F-10.20, F-5.9: a cut is whole pixels, inside the image and short of the cut opposite it; where the image's size is not known yet it is only whole and not negative.
    #[test]
    fn nine_slice_insets_are_whole_pixels_inside_the_image() {
        assert_eq!(
            texture::clamp_insets([500.0, -3.0, 10.6, 12.4], Some((64, 32))),
            [32.0, 0.0, 0.0, 12.0]
        );
        assert_eq!(
            texture::clamp_insets([10.0, 20.0, 30.0, 50.0], Some((64, 32))),
            [10.0, 20.0, 22.0, 44.0]
        );
        assert_eq!(
            texture::clamp_insets([900.4, 0.0, -1.0, 7.5], None),
            [900.0, 0.0, 0.0, 8.0]
        );
        assert_eq!(texture::first_insets(Some((64, 32))), [8.0; 4]);
    }

    /// Three columns, every rectangle written out, the middle one showing its own picture.
    fn columns(layout: &mut Layout) {
        let background = &mut layout.outputs[0].layers.background;
        let column = |x: f32, w: f32| Some(rect(x, 0.0, w, 1.0));
        if let Some(AreaKind::WallpaperRegion { rect, .. }) = &mut background.areas[0].kind {
            *rect = column(0.0, 0.25);
        }
        background.areas.push(Area {
            id: AreaId::new("middle"),
            kind: Some(AreaKind::WallpaperRegion {
                rect: column(0.25, 0.5),
                source: Some("/pictures/middle.png".to_string()),
                fit: Some(Fit::Contain),
                transition: Some(Transition::Slide),
            }),
            ..Area::default()
        });
        background.areas.push(Area {
            id: AreaId::new("right"),
            kind: Some(AreaKind::WallpaperRegion {
                rect: column(0.75, 0.25),
                source: None,
                fit: None,
                transition: None,
            }),
            ..Area::default()
        });
    }

    fn enter() -> DismissRegistration {
        mode::enter_as(
            LayerKind::Background,
            Some(SCREEN),
            &Compositor {
                restack: true,
                locked: false,
                lockable: Ok(()),
            },
        )
        .expect("the mode opens");
        let id = host::transient_id(SCREEN);
        DismissRegistration::new(Rc::new(move || transient::close(&id)))
    }

    fn region(id: &str) -> Node {
        Node::area(Some(SCREEN), LayerKind::Background, &AreaId::new(id))
    }

    /// A key pressed and let go the way the host's window hears it.
    fn tap(key: Key, modifiers: ModifiersState) -> bool {
        telar::observe_keyboard(&Event::KeyPressed {
            key: key.clone(),
            modifiers,
        });
        let taken = telar::dispatch_overlays(&Event::KeyPressed {
            key: key.clone(),
            modifiers,
        }) || keys::press_as(&key, modifiers, Press::First);
        telar::observe_keyboard(&Event::KeyReleased {
            key: key.clone(),
            modifiers,
        });
        keys::settle_released();
        taken
    }

    fn stored(rig: &Rig) -> Layout {
        rig.store.borrow().active().clone()
    }

    fn shown(id: &str) -> Option<Rect> {
        let desktops = surfaces::reconcile::desktops();
        match desktops[0]
            .resolved
            .layer(LayerKind::Background)?
            .areas
            .iter()
            .find(|area| area.id.as_str() == id)?
            .kind
        {
            ResolvedAreaKind::WallpaperRegion { rect, .. } => Some(rect),
            _ => None,
        }
    }

    fn drawn_tiles() -> Vec<Region> {
        regions::tiles_of(
            &surfaces::reconcile::desktops()[0].resolved,
            LayerKind::Background,
        )
    }

    /// `s` splits the selected region, the new half named after it and showing its picture the same way; Alt+→ joins it back into the layout exactly as it was. Each is one entry in the history, and each undo takes one back.
    #[test]
    fn a_split_and_a_join_are_one_undo_entry_each_and_the_join_restores_the_layout() {
        let rig = rig_with("background-split-join", columns);
        let _host = enter();
        let before = stored(&rig);

        assert!(session::select(Selection::Area(region("middle"))));
        assert!(tap(Key::Char('s'), NONE));
        assert_eq!(shown("middle"), Some(rect(0.25, 0.0, 0.25, 1.0)));
        assert_eq!(shown("middle-2"), Some(rect(0.5, 0.0, 0.25, 1.0)));
        let split = stored(&rig);
        let fresh = split.outputs[0]
            .layers
            .background
            .areas
            .iter()
            .find(|area| area.id.as_str() == "middle-2")
            .expect("the new half is written beside the one it came from");
        assert!(matches!(
            &fresh.kind,
            Some(AreaKind::WallpaperRegion {
                source: Some(source),
                fit: Some(Fit::Contain),
                transition: Some(Transition::Slide),
                ..
            }) if source == "/pictures/middle.png"
        ));
        assert_tiled(&drawn_tiles());

        assert!(tap(
            Key::Named(NamedKey::ArrowRight),
            ModifiersState {
                is_alt: true,
                ..NONE
            }
        ));
        assert_eq!(
            stored(&rig),
            before,
            "and the layout is as it was, bit for bit"
        );

        session::undo().expect("the join is undone");
        assert_eq!(stored(&rig), split);
        session::undo().expect("the split is undone");
        assert_eq!(stored(&rig), before);
        mode::leave();
    }

    /// A shared edge moved through the region planner, and a keyboard move of the middle column, each take their neighbours with them.
    #[test]
    fn a_moved_edge_takes_its_neighbours_along_in_one_entry() {
        let rig = rig_with("background-edge", columns);
        let _host = enter();
        let before = stored(&rig);
        let layout = session::draft().peek();
        let desktop = surfaces::reconcile::desktops()[0].clone();
        let plan = Plan {
            layout: &layout,
            desktop,
            layer: LayerKind::Background,
        };
        let key = regions::lines(&drawn_tiles())
            .into_iter()
            .find(|line| line.cut == Cut::SideBySide && line.at == 0.25)
            .expect("the edge between the left and middle columns")
            .key();
        let ops = plan
            .move_line(&AreaId::new("middle"), &key, 0.375)
            .expect("the edge moves");
        crate::context::commit("edge".to_string(), ops).expect("committed");
        let edged = stored(&rig);
        assert_eq!(shown("background"), Some(rect(0.0, 0.0, 0.375, 1.0)));
        assert_eq!(shown("middle"), Some(rect(0.375, 0.0, 0.375, 1.0)));
        assert_tiled(&drawn_tiles());

        assert!(session::select(Selection::Area(region("middle"))));
        assert!(tap(
            Key::Named(NamedKey::ArrowRight),
            ModifiersState {
                is_shift: true,
                ..NONE
            }
        ));
        let moved = shown("middle").expect("still there");
        assert_eq!(moved.w, 0.375, "a move keeps its width");
        assert_eq!(moved.x, 0.375 + regions::STEP);
        assert_tiled(&drawn_tiles());
        session::undo().expect("the move is undone");
        assert_eq!(stored(&rig), edged, "one undo takes the whole move back");
        session::undo().expect("the edge is undone");
        assert_eq!(stored(&rig), before);
        mode::leave();
    }

    /// With "this workspace only" on, a split is written into that workspace's rule — made for it — and resolves there alone.
    #[test]
    fn an_edit_for_one_workspace_lands_in_its_rule_and_resolves_only_there() {
        let rig = rig_on("background-variant", Some("2"), columns);
        let _host = enter();
        variant::set(true).expect("the screen says which workspace is up");
        assert_eq!(variant::editing(), Some(WorkspaceMatch("2".to_string())));
        assert!(session::select(Selection::Area(region("right"))));
        assert!(tap(
            Key::Char('s').clone(),
            ModifiersState {
                is_shift: true,
                ..NONE
            }
        ));

        let layout = stored(&rig);
        let rule = &layout.outputs[0];
        assert_eq!(
            rule.workspaces.len(),
            1,
            "the rule for the workspace was made"
        );
        assert_eq!(rule.workspaces[0].matches, WorkspaceMatch("2".to_string()));
        assert!(
            rule.layers
                .background
                .areas
                .iter()
                .all(|area| area.id.as_str() != "right-2"),
            "nothing of it is written for every workspace"
        );
        let resolved = |workspace: Option<&str>| {
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
            regions::tiles_of(&resolved, LayerKind::Background)
        };
        let on_two = resolved(Some("2"));
        assert!(on_two.iter().any(|tile| tile.id.as_str() == "right-2"));
        assert_tiled(&on_two);
        for elsewhere in [Some("3"), None] {
            let tiles = resolved(elsewhere);
            assert!(tiles.iter().all(|tile| tile.id.as_str() != "right-2"));
            assert_eq!(
                tiles
                    .iter()
                    .find(|tile| tile.id.as_str() == "right")
                    .map(|tile| tile.rect),
                Some(rect(0.75, 0.0, 0.25, 1.0))
            );
        }
        mode::leave();
        assert!(!variant::only().peek(), "leaving the mode switches it off");
    }

    /// Switching "this workspace only" on under an open popover moves what it changed into the workspace's rule, and only what it changed.
    #[test]
    fn a_popover_switched_to_one_workspace_writes_its_change_there() {
        let rig = rig_on("background-variant-popover", Some("2"), columns);
        let _host = enter();
        popover::open_area(region("right")).expect("the region's popover opens");
        let _tree = popover::tree()
            .expect("a popover is open")
            .expect("and it builds");
        let picture = popover::shared::<String>("source").expect("its picture row");
        picture.set("/pictures/right.png".to_string());
        variant::set(true).expect("the screen says which workspace is up");
        popover::close();

        let layout = stored(&rig);
        let everywhere = layout.outputs[0]
            .layers
            .background
            .areas
            .iter()
            .find(|area| area.id.as_str() == "right")
            .expect("still written for every workspace");
        assert!(matches!(
            everywhere.kind,
            Some(AreaKind::WallpaperRegion { source: None, .. })
        ));
        let variant = &layout.outputs[0].workspaces[0].layers.background.areas;
        assert_eq!(variant.len(), 1);
        assert_eq!(
            variant[0].kind,
            Some(AreaKind::WallpaperRegion {
                rect: None,
                source: Some("/pictures/right.png".to_string()),
                fit: None,
                transition: None,
            }),
            "the workspace's entry names what the popover changed and nothing else"
        );
        mode::leave();
    }

    /// A texture laid over a region from its menu sits right above it on its rectangle, a gradient at half strength; `]` turns the gradient, and taking it away is one more entry.
    #[test]
    fn a_texture_over_a_region_is_added_turned_and_taken_away() {
        let rig = rig_with("background-texture", columns);
        let _host = enter();
        let before = stored(&rig);
        crate::modes::background::add_texture(&region("middle")).expect("the texture goes on");
        let added = stored(&rig);
        let layout = added.clone();
        let areas = &layout.outputs[0].layers.background.areas;
        let at = areas
            .iter()
            .position(|area| area.id.as_str() == "middle")
            .expect("the region");
        assert_eq!(areas[at + 1].id.as_str(), "texture");
        assert!(matches!(
            &areas[at + 1].kind,
            Some(AreaKind::Texture {
                rect: Some(covered),
                gradient: Some(_),
                opacity: Some(opacity),
                ..
            }) if *covered == rect(0.25, 0.0, 0.5, 1.0) && *opacity == 0.5
        ));

        assert!(session::select(Selection::Area(region("texture"))));
        assert!(tap(Key::Char(']'), NONE));
        let layout = stored(&rig);
        let turned = layout.outputs[0]
            .layers
            .background
            .areas
            .iter()
            .find_map(|area| match &area.kind {
                Some(AreaKind::Texture {
                    gradient: Some(gradient),
                    ..
                }) => Some(gradient.angle),
                _ => None,
            });
        assert_eq!(turned, Some(135.0));
        assert!(tap(Key::Char('n'), NONE), "the key answers");
        assert_eq!(stored(&rig), layout, "but a gradient has no image to slice");

        crate::context::remove_area(&region("texture"), "texture")
            .expect("the texture is written here");
        assert!(
            !stored(&rig).outputs[0]
                .layers
                .background
                .areas
                .iter()
                .any(|area| area.id.as_str() == "texture")
        );
        for back in [layout, added, before] {
            session::undo().expect("one entry comes back");
            assert_eq!(stored(&rig), back, "each change was one entry");
        }
        mode::leave();
    }

    /// `n` slices an image texture's picture into nine and a second press stops slicing it.
    #[test]
    fn n_slices_an_image_texture_and_takes_the_slice_back() {
        let rig = rig_with("background-slice", |layout| {
            columns(layout);
            layout.outputs[0].layers.background.areas.push(Area {
                id: AreaId::new("frame"),
                kind: Some(AreaKind::Texture {
                    rect: Some(rect(0.0, 0.0, 0.25, 1.0)),
                    image: Some("/pictures/frame.png".to_string()),
                    gradient: None,
                    tile: None,
                    blend: None,
                    opacity: None,
                }),
                ..Area::default()
            });
        });
        let _host = enter();
        let tile = || {
            stored(&rig).outputs[0]
                .layers
                .background
                .areas
                .iter()
                .find_map(|area| match &area.kind {
                    Some(AreaKind::Texture { tile, .. }) => Some(*tile),
                    _ => None,
                })
                .flatten()
        };
        let before = stored(&rig);
        assert!(session::select(Selection::Area(region("frame"))));
        assert!(tap(Key::Char('n'), NONE));
        let sliced = stored(&rig);
        assert!(matches!(tile(), Some(Tile::NineSlice { top, .. }) if top == 16.0));
        assert!(tap(Key::Char('n'), NONE));
        assert_eq!(tile(), Some(Tile::None));
        session::undo().expect("the second press is undone");
        assert_eq!(stored(&rig), sliced);
        session::undo().expect("and the first");
        assert_eq!(stored(&rig), before);
        mode::leave();
    }

    /// A region's and a texture's popovers build, their handles over the item and their rows in the card.
    #[test]
    fn a_regions_and_a_textures_popovers_build() {
        let _rig = rig_with("background-popovers", |layout| {
            columns(layout);
            layout.outputs[0].layers.background.areas.push(Area {
                id: AreaId::new("wash"),
                kind: Some(AreaKind::Texture {
                    rect: Some(rect(0.25, 0.0, 0.5, 1.0)),
                    image: None,
                    gradient: Some(texture::first_gradient()),
                    tile: None,
                    blend: None,
                    opacity: None,
                }),
                ..Area::default()
            });
        });
        let _host = enter();
        for id in ["middle", "wash"] {
            popover::open_area(region(id)).expect("the popover opens");
            popover::tree()
                .expect("a popover is open")
                .expect("and it builds");
            popover::close();
        }
        assert!(
            popover::shared::<layout::Paint>("texture.paint").is_none(),
            "closed"
        );
        mode::leave();
    }

    fn wash(layout: &mut Layout) {
        columns(layout);
        layout.outputs[0].layers.background.areas.push(Area {
            id: AreaId::new("wash"),
            kind: Some(AreaKind::Texture {
                rect: Some(rect(0.25, 0.0, 0.5, 1.0)),
                image: None,
                gradient: Some(texture::first_gradient()),
                tile: None,
                blend: None,
                opacity: None,
            }),
            ..Area::default()
        });
    }

    /// A texture's popover sets how it blends with what is under it and how strongly it paints, written into the texture itself as one entry.
    #[test]
    fn a_textures_popover_sets_its_blend_and_its_own_opacity() {
        let rig = rig_with("background-texture-blend", wash);
        let _host = enter();
        popover::open_area(region("wash")).expect("the texture's popover opens");
        let _tree = popover::tree()
            .expect("a popover is open")
            .expect("and it builds");
        popover::shared::<String>("texture.blend")
            .expect("its blend row")
            .set("multiply".to_string());
        popover::shared::<f32>("texture.opacity")
            .expect("its opacity row")
            .set(0.25);
        popover::close();
        let layout = stored(&rig);
        let written = layout.outputs[0]
            .layers
            .background
            .areas
            .iter()
            .find(|area| area.id.as_str() == "wash")
            .expect("the texture");
        assert!(
            matches!(
                written.kind,
                Some(AreaKind::Texture {
                    blend: Some(Blend::Multiply),
                    opacity: Some(opacity),
                    ..
                }) if opacity == 0.25
            ),
            "{:?}",
            written.kind
        );
        assert!(rig.undo_label().is_some(), "one entry");
        mode::leave();
    }

    /// `t` and the toolbar's button lay a texture over the selected region, each one entry; with nothing selected they say what to select.
    #[test]
    fn t_and_the_toolbar_lay_a_texture_over_the_selected_region() {
        let rig = rig_with("background-texture-key", columns);
        let _host = enter();
        let textures = |layout: &Layout| {
            layout.outputs[0]
                .layers
                .background
                .areas
                .iter()
                .filter(|area| matches!(area.kind, Some(AreaKind::Texture { .. })))
                .count()
        };
        let (label, press) = host::toolbar_of(LayerKind::Background)
            .into_iter()
            .find(|(label, _)| label() == "Add texture")
            .expect("the background toolbar adds a texture");
        assert_eq!(label(), "Add texture");
        press();
        assert_eq!(textures(&stored(&rig)), 0);
        assert_eq!(
            mode::refusal().peek(),
            Some("Select a wallpaper region to lay a texture over".to_string())
        );

        assert!(session::select(Selection::Area(region("middle"))));
        assert!(tap(Key::Char('t'), NONE));
        assert_eq!(textures(&stored(&rig)), 1);
        press();
        assert_eq!(textures(&stored(&rig)), 2);
        session::undo().expect("the button's texture is undone");
        assert_eq!(textures(&stored(&rig)), 1, "each was one entry");
        mode::leave();
    }

    /// A4: dragging an edge grip previews each move once. Planning the edge from the screen the preview redraws would run the preview again for every redraw it made itself, until the runtime gives up.
    #[test]
    fn an_edge_drag_previews_each_move_once() {
        let rig = rig_with("background-edge-drag", columns);
        let _host = enter();
        let mode = mode::current().expect("the mode is up");
        let page = LayoutStyle::new().width(1920.0).height(1080.0);
        let root = Container::new(
            page,
            vec![crate::modes::background::tool(&mode).expect("the tools build")],
        )
        .expect("a page");
        let node = root.layout_node();
        let mut tree = ComponentList::new(root);
        compute_layout(
            node,
            AvailableSpace::Definite(1920.0),
            AvailableSpace::Definite(1080.0),
        )
        .expect("the tools lay out");
        let previews = Rc::new(Cell::new(0usize));
        let counting = Rc::clone(&previews);
        effect(move || {
            surfaces::reconcile::previewing();
            counting.set(counting.get() + 1);
        });
        let mut pointer = |event: Event| {
            tree.on_event(&event);
        };
        let start = previews.get();
        pointer(Event::PointerPressed {
            x: 480.0,
            y: 540.0,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        for x in [560.0, 640.0, 720.0] {
            pointer(Event::PointerMoved {
                x,
                y: 540.0,
                source: PointerSource::Mouse,
            });
        }
        let moved = previews.get() - start;
        assert!(
            (1..=3).contains(&moved),
            "three moves previewed {moved} times"
        );
        assert_eq!(shown("background"), Some(rect(0.0, 0.0, 0.375, 1.0)));
        pointer(Event::PointerReleased {
            x: 720.0,
            y: 540.0,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        assert_eq!(
            stored(&rig).outputs[0].layers.background.areas[0].kind,
            Some(AreaKind::WallpaperRegion {
                rect: Some(rect(0.0, 0.0, 0.375, 1.0)),
                source: None,
                fit: None,
                transition: None,
            }),
            "let go, the edge is where it was dragged to"
        );
        mode::leave();
    }

    /// B7: a stop drag Esc called off puts the stop back and leaves nothing behind for the next one, which reads the stop where it is then: with the texture moved down in between, a small pull on the middle stop keeps it near the middle. The stops, laid over the axis, leave its dots to the pointer.
    #[test]
    fn a_cancelled_stop_drag_starts_the_next_one_where_the_stop_is() {
        let _rig = rig_with("background-stop-cancel", |layout| {
            wash(layout);
            let areas = &mut layout.outputs[0].layers.background.areas;
            if let Some(AreaKind::Texture { gradient, .. }) = &mut areas
                .iter_mut()
                .find(|area| area.id.as_str() == "wash")
                .expect("the texture")
                .kind
            {
                *gradient = Some(Gradient {
                    angle: 90.0,
                    stops: [(0.0, "blue"), (0.5, "teal"), (1.0, "purple")]
                        .into_iter()
                        .map(|(at, color)| GradientStop {
                            at,
                            color: color.to_string(),
                        })
                        .collect(),
                });
            }
        });
        let _host = enter();
        let drawn = telar::signal(telar::Rect::new(480.0, 0.0, 960.0, 1080.0));
        surfaces::rects::track_spanning(region("wash"), vec![drawn]);
        popover::open_area(region("wash")).expect("the texture's popover opens");
        let page = LayoutStyle::new().width(1920.0).height(1080.0);
        let root = Container::new(
            page,
            vec![
                popover::tree()
                    .expect("a popover is open")
                    .expect("and it builds"),
            ],
        )
        .expect("a page");
        let node = root.layout_node();
        let mut tree = ComponentList::new(root);
        let lay_out = || {
            compute_layout(
                node,
                AvailableSpace::Definite(1920.0),
                AvailableSpace::Definite(1080.0),
            )
            .expect("the popover lays out")
        };
        lay_out();
        let mut route = |event: Event| {
            telar::observe_keyboard(&event);
            if !telar::dispatch_overlays(&event) {
                tree.on_event(&event);
            }
        };
        let press = |(x, y): (f32, f32)| Event::PointerPressed {
            x: x.into(),
            y: y.into(),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        };
        let to = |(x, y): (f32, f32)| Event::PointerMoved {
            x: x.into(),
            y: y.into(),
            source: PointerSource::Mouse,
        };
        let release = |(x, y): (f32, f32)| Event::PointerReleased {
            x: x.into(),
            y: y.into(),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        };
        let middle = || match popover::shared::<layout::Paint>("texture.paint")
            .expect("the texture's paint")
            .peek()
        {
            layout::Paint::Gradient(gradient) => gradient.stops[1].at,
            layout::Paint::Image(_) => panic!("a gradient"),
        };

        route(press((960.0, 540.0)));
        route(to((960.0, 640.0)));
        assert!(middle() > 0.55, "the pull moves the stop: {}", middle());
        route(Event::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: NONE,
        });
        assert_eq!(middle(), 0.5, "Esc put it back");
        route(release((960.0, 640.0)));

        drawn.set(telar::Rect::new(480.0, 200.0, 960.0, 1080.0));
        lay_out();
        route(press((960.0, 740.0)));
        route(to((960.0, 750.0)));
        assert!(
            (middle() - 0.5).abs() < 0.05,
            "measured from where the stop is now, a small pull moves it a little: {}",
            middle()
        );
        route(release((960.0, 750.0)));
        let stops = || match popover::shared::<layout::Paint>("texture.paint")
            .expect("the texture's paint")
            .peek()
        {
            layout::Paint::Gradient(gradient) => gradient.stops.len(),
            layout::Paint::Image(_) => 0,
        };
        let dot = (960.0, 200.0 + 1080.0 * 2.5 / 24.0);
        route(to(dot));
        route(press(dot));
        route(release(dot));
        assert_eq!(
            stops(),
            4,
            "a press on the axis between the stops adds one there"
        );
        popover::close();
        mode::leave();
    }
}
