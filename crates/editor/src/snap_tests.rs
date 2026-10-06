//! Snap guides: a moved or resized rectangle snapping to its siblings and its box within a few pixels, region edges and cuts snapping to the main grid's cell lines and staying on the region grid, the lines drawn while a drag goes, and Alt dragging free — through the math and through real pointer events.

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, DismissRegistration, DrawCommand, Event,
        LayoutItem, LayoutStyle, ModifiersState, Paint, PointerButton, PointerSource,
        compute_layout, signal, use_theme,
    };

    use config::theme::NordTheme;
    use layout::{
        Area, AreaId, AreaKind, Arrange, GroupId, GroupKind, InstanceId, LayerKind, Layout, Rect,
        ResolvedAreaKind, Within,
    };
    use surfaces::menu::Pointed;
    use surfaces::rects::{self, Node};
    use surfaces::transient;

    use crate::mode::{self, Compositor};
    use crate::modes::grid::Room;
    use crate::modes::regions::{self, Cut, Plan};
    use crate::modes::widgets::{self, Geometry};
    use crate::modes::{background, lock as lock_mode};
    use crate::rig::{Rig, SCREEN, rig_with};
    use crate::session;
    use crate::snap::{self, Axis, Guide, Motion, Moving, Snapped};
    use crate::{host, select};

    const SIZE: (f32, f32) = (1920.0, 1080.0);

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn close_rect(a: Rect, b: Rect) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.w, b.w) && close(a.h, b.h)
    }

    const BOX: (f32, f32) = (1000.0, 1000.0);

    /// A body carried near a sibling's start lands its end on it; carried near the box's middle, its centre lands there; each way on its own, and each line it landed on is a guide.
    #[test]
    fn a_carried_rectangle_snaps_its_edges_and_centre_to_siblings_and_the_box() {
        let sibling = rect(0.55, 0.6, 0.2, 0.2);
        let to_sibling = snap::snap(
            rect(0.303, 0.2, 0.2, 0.1),
            &[sibling],
            BOX,
            Moving::BODY,
            false,
        );
        assert!(
            close_rect(to_sibling.rect, rect(0.3, 0.2, 0.2, 0.1)),
            "{to_sibling:?}"
        );
        assert_eq!(
            to_sibling.guides,
            vec![Guide {
                axis: Axis::X,
                at: 0.5
            }]
        );

        let centred = snap::snap(
            rect(0.402, 0.648, 0.2, 0.1),
            &[sibling],
            BOX,
            Moving::BODY,
            false,
        );
        assert!(
            close_rect(centred.rect, rect(0.4, 0.65, 0.2, 0.1)),
            "its centre on the box's middle, its top on the sibling's middle: {centred:?}"
        );
        assert_eq!(centred.guides.len(), 2);
        assert!(close(centred.guides[1].at, 0.7) && centred.guides[1].axis == Axis::Y);
    }

    /// The tolerance is six logical pixels of the box: the same fraction snaps on a small box and not on a large one, and the nearest line wins.
    #[test]
    fn the_tolerance_is_six_pixels_and_the_nearest_line_wins() {
        let near = rect(0.105, 0.3, 0.2, 0.2);
        let siblings = [rect(0.1, 0.9, 0.05, 0.05), rect(0.109, 0.9, 0.05, 0.05)];
        let small = snap::snap(near, &siblings, (1000.0, 1000.0), Moving::BODY, false);
        assert!(
            close(small.rect.x, 0.109),
            "4 px from one, 5 from the other: {small:?}"
        );
        let large = snap::snap(near, &siblings[..1], (2000.0, 2000.0), Moving::BODY, false);
        assert!(
            close(large.rect.x, 0.105),
            "10 px away on a larger box: {large:?}"
        );
        let off = snap::snap(rect(0.307, 0.31, 0.2, 0.2), &[], BOX, Moving::BODY, false);
        assert_eq!(off.rect, rect(0.307, 0.31, 0.2, 0.2), "7 px is too far");
        assert!(off.guides.is_empty());
    }

    /// A resize snaps only the edge it drags: its end grows or shrinks onto a line, its start moves onto one while the end stays put.
    #[test]
    fn a_resize_snaps_only_the_edge_it_drags() {
        let end = snap::snap(
            rect(0.002, 0.1, 0.495, 0.2),
            &[],
            BOX,
            Moving {
                x: Some(Motion::End),
                y: None,
            },
            false,
        );
        assert!(
            close_rect(end.rect, rect(0.002, 0.1, 0.498, 0.2)),
            "the start stays off the box's edge: {end:?}"
        );
        let start = snap::snap(
            rect(0.4, 0.004, 0.2, 0.3),
            &[],
            BOX,
            Moving {
                x: None,
                y: Some(Motion::Start),
            },
            false,
        );
        assert!(
            close_rect(start.rect, rect(0.4, 0.0, 0.2, 0.304)),
            "{start:?}"
        );
        assert_eq!(
            start.guides,
            vec![Guide {
                axis: Axis::Y,
                at: 0.0
            }]
        );
    }

    /// Alt: nothing snaps and no guide is drawn.
    #[test]
    fn free_leaves_the_rectangle_where_it_was_put() {
        let put = rect(0.303, 0.498, 0.2, 0.1);
        assert_eq!(
            snap::snap(put, &[rect(0.5, 0.5, 0.1, 0.1)], BOX, Moving::BODY, true),
            Snapped {
                rect: put,
                guides: Vec::new()
            }
        );
    }

    /// A region edge goes to the nearest cell line within its tolerance, then onto the region grid; free, or with no line near, it stays where it was put, on the region grid.
    #[test]
    fn a_region_line_snaps_to_the_nearest_cell_line_and_onto_the_region_grid() {
        let lines = [0.1, 1.0 / 3.0, 0.345];
        assert_eq!(snap::nearest_line(0.34, &lines, 0.012), Some(0.345));
        assert_eq!(snap::nearest_line(0.2, &lines, 0.012), None);
        let snapped = snap::region_line(0.325, &lines, snap::EDGE_TOLERANCE, false);
        assert_eq!(snapped, regions::snap(1.0 / 3.0));
        assert_eq!(snapped % regions::GRID, 0.0, "on the region grid");
        assert_eq!(
            snap::region_line(0.325, &lines, snap::EDGE_TOLERANCE, true),
            regions::snap(0.325)
        );
        assert_eq!(
            snap::region_line(0.3, &lines, snap::EDGE_TOLERANCE, false),
            regions::snap(0.3),
            "3.3 % from the nearest line"
        );
        assert_eq!(
            snap::region_line(0.3, &lines, snap::CUT_TOLERANCE, false),
            regions::snap(1.0 / 3.0),
            "a cut reaches further"
        );
    }

    /// A grid's cell lines are in the middle of the gaps between its cells, one before the first cell and one after the last, as fractions of the box and only those inside it.
    #[test]
    fn cell_lines_are_mid_gap() {
        let geometry = Geometry {
            area: AreaId::new("grid"),
            region: telar::Rect::new(0.0, 0.0, 1000.0, 500.0),
            origin: (100.0, 5.0),
            cell: 80.0,
            pitch: 100.0,
            room: Room { cols: 3, rows: 4 },
        };
        let bounds = telar::Rect::new(0.0, 0.0, 1000.0, 500.0);
        let across = snap::cell_lines(&geometry, Axis::X, bounds);
        assert!(
            across.len() == 4
                && [0.09, 0.19, 0.29, 0.39]
                    .iter()
                    .zip(&across)
                    .all(|(a, b)| close(*a, *b)),
            "{across:?}"
        );
        let down = snap::cell_lines(&geometry, Axis::Y, bounds);
        assert!(
            down.len() == 4
                && [0.19, 0.39, 0.59, 0.79]
                    .iter()
                    .zip(&down)
                    .all(|(a, b)| close(*a, *b)),
            "the line before the first row is off the box: {down:?}"
        );
        let guide = Guide {
            axis: Axis::Y,
            at: 0.5,
        }
        .line(telar::Rect::new(10.0, 20.0, 300.0, 200.0));
        assert_eq!(guide, telar::Rect::new(10.0, 120.0, 300.0, 0.0));
    }

    fn region_area(id: &str, at: Rect, within: Option<Within>) -> Area {
        Area {
            id: AreaId::new(id),
            kind: Some(AreaKind::WallpaperRegion {
                rect: Some(at),
                source: None,
                fit: None,
                transition: None,
            }),
            within,
            ..Area::default()
        }
    }

    fn texture_area(id: &str, at: Rect, within: Option<Within>) -> Area {
        Area {
            id: AreaId::new(id),
            kind: Some(AreaKind::Texture {
                rect: Some(at),
                image: None,
                gradient: Some(crate::modes::texture::first_gradient()),
                tile: None,
                blend: None,
                opacity: None,
            }),
            within,
            ..Area::default()
        }
    }

    /// Three columns, every rectangle written out.
    fn columns(layout: &mut Layout) {
        let background = &mut layout.outputs[0].layers.background;
        if let Some(AreaKind::WallpaperRegion { rect: at, .. }) = &mut background.areas[0].kind {
            *at = Some(rect(0.0, 0.0, 0.25, 1.0));
        }
        background
            .areas
            .push(region_area("middle", rect(0.25, 0.0, 0.5, 1.0), None));
        background
            .areas
            .push(region_area("right", rect(0.75, 0.0, 0.25, 1.0), None));
    }

    /// An area's siblings are the regions, textures and free areas measured in its box, itself left out; a free child's are the other children of its container.
    #[test]
    fn siblings_share_the_box_or_the_free_container() {
        let _rig = rig_with("snap-siblings", |layout| {
            columns(layout);
            let background = &mut layout.outputs[0].layers.background;
            background
                .areas
                .push(texture_area("wash", rect(0.1, 0.2, 0.3, 0.4), None));
            background.areas.push(texture_area(
                "inside",
                rect(0.5, 0.5, 0.1, 0.1),
                Some(Within::Usable),
            ));
            let areas = &mut layout.outputs[0].layers.desktop.areas;
            let own = areas.pop().expect("the clock's own area");
            let mut board = own.groups.into_iter().next().expect("its group");
            board.id = GroupId::new("board");
            board.kind = Some(GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 4,
                row_span: 4,
            });
            board.arrange = Some(Arrange::Free);
            let template = board.children[0].clone();
            board.children = [
                ("one", Some(rect(0.0, 0.0, 0.5, 0.5))),
                ("two", Some(rect(0.5, 0.5, 0.25, 0.25))),
                ("three", None),
            ]
            .into_iter()
            .map(|(id, at)| {
                let mut child = template.clone();
                child.id = InstanceId::new(id);
                child.rect = at;
                child
            })
            .collect();
            areas[0].groups.push(board);
        });
        let desktop = surfaces::reconcile::desktops()[0].clone();
        let layer = desktop
            .resolved
            .layer(LayerKind::Background)
            .expect("the background layer");
        assert_eq!(
            snap::area_siblings(layer, &AreaId::new("middle")),
            vec![
                rect(0.0, 0.0, 0.25, 1.0),
                rect(0.75, 0.0, 0.25, 1.0),
                rect(0.1, 0.2, 0.3, 0.4)
            ],
            "not itself, nor the texture measured in the usable box"
        );
        assert_eq!(
            snap::area_siblings(layer, &AreaId::new("inside")),
            Vec::<Rect>::new()
        );

        let board = widgets::grids(SCREEN, LayerKind::Desktop)
            .into_iter()
            .flat_map(|(_, area)| area.groups)
            .find(|group| group.id.as_str() == "board")
            .expect("the free container");
        let first = board.children[0].id.clone();
        let siblings = snap::free_siblings(&board, &first);
        assert_eq!(siblings.len(), 2, "{siblings:?}");
        assert_eq!(siblings[0], rect(0.5, 0.5, 0.25, 0.25));
    }

    /// An owner for what a test builds, disposed when it ends.
    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            hold_alt(false);
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn enter(layer: LayerKind) -> DismissRegistration {
        mode::enter_as(
            layer,
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

    fn hold_alt(held: bool) {
        telar::observe_keyboard(&Event::ModifiersChanged {
            modifiers: ModifiersState {
                is_alt: held,
                ..ModifiersState::default()
            },
        });
    }

    /// The selection tool, which draws what a drag shows, under `tools`, over the whole screen, inside the root every window has.
    struct Screen(ComponentList);

    impl Screen {
        fn new(tools: Vec<Box<dyn LayoutItem>>) -> Self {
            let current = mode::current().expect("the mode is up");
            let mut children = vec![select::tool(&current).expect("the selection builds")];
            children.extend(tools);
            let root = Pointed::new(Box::new(
                Container::new(LayoutStyle::new().width(SIZE.0).height(SIZE.1), children)
                    .expect("a page"),
            ));
            let node = root.layout_node();
            let tree = ComponentList::new(root);
            compute_layout(
                node,
                AvailableSpace::Definite(SIZE.0),
                AvailableSpace::Definite(SIZE.1),
            )
            .expect("the tools lay out");
            Self(tree)
        }

        fn send(&mut self, event: Event) {
            telar::observe_keyboard(&event);
            if !telar::dispatch_overlays(&event) {
                self.0.on_event(&event);
            }
            for _ in 0..3 {
                telar::relayout_if_dirty();
            }
        }

        fn press(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerMoved {
                x: x.into(),
                y: y.into(),
                source: PointerSource::Mouse,
            });
            self.send(Event::PointerPressed {
                x: x.into(),
                y: y.into(),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
        }

        fn move_to(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerMoved {
                x: x.into(),
                y: y.into(),
                source: PointerSource::Mouse,
            });
        }

        fn release(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerReleased {
                x: x.into(),
                y: y.into(),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
        }

        /// Every guide drawn: a box filled in the accent no wider, or no taller, than a pixel.
        fn guides(&self) -> Vec<telar::Rect> {
            let accent = Paint::Solid(use_theme::<NordTheme>().accent);
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.0.commands(), |command, [a, b, c, d, e, f]| {
                if let DrawCommand::Rect { rect, style } = command
                    && style.fill == Some(accent)
                    && (rect.width <= 1.0 || rect.height <= 1.0)
                {
                    found.push(telar::Rect::new(
                        a * rect.x + c * rect.y + e,
                        b * rect.x + d * rect.y + f,
                        rect.width,
                        rect.height,
                    ));
                }
            });
            found
        }
    }

    fn stored(rig: &Rig) -> Layout {
        rig.store.borrow().active().clone()
    }

    fn stored_region(rig: &Rig, id: &str) -> Rect {
        let (resolved, _) = layout::resolve(&stored(rig), &crate::written::known(), SCREEN, None);
        resolved
            .layer(LayerKind::Background)
            .and_then(|layer| layer.areas.iter().find(|area| area.id.as_str() == id))
            .and_then(|area| match area.kind {
                ResolvedAreaKind::WallpaperRegion { rect, .. } => Some(rect),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the region {id}"))
    }

    /// The desktop grid's cell lines across the screen, the one the background's regions line up with.
    fn desktop_lines() -> Vec<f32> {
        let desktop = surfaces::reconcile::desktops()[0].clone();
        let bounds = desktop.reserved.box_of(Within::Output, desktop.size);
        let lines = snap::grid_lines(&desktop, LayerKind::Background, Axis::X, bounds);
        assert!(
            lines.len() > 2,
            "the desktop grid has cell lines: {lines:?}"
        );
        lines
    }

    fn nearest(lines: &[f32], to: f32) -> f32 {
        lines
            .iter()
            .copied()
            .min_by(|a, b| (a - to).abs().total_cmp(&(b - to).abs()))
            .expect("a line")
    }

    /// The edge between two columns, dragged to a little past a cell line of the desktop grid, lands on that line — with the grid's lines drawn while it goes and gone after; with Alt held it lands where it was let go.
    #[test]
    fn a_region_edge_snaps_to_the_desktop_grid() {
        edge_dragged(false);
    }

    #[test]
    fn a_region_edge_goes_free_with_alt() {
        edge_dragged(true);
    }

    fn edge_dragged(alt: bool) {
        let rig = rig_with(&format!("snap-edge-{alt}"), columns);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Background);
        let lines = desktop_lines();
        let line = nearest(&lines, 0.4);
        let current = mode::current().expect("the mode is up");
        let mut screen = Screen::new(vec![
            background::tool(&current).expect("the region tools build"),
        ]);
        let to = (line * SIZE.0 + 12.0, 540.0);

        hold_alt(alt);
        screen.press((480.0, 540.0));
        screen.move_to((520.0, 540.0));
        screen.move_to(to);
        let drawn = screen.guides();
        assert!(
            drawn
                .iter()
                .any(|guide| (guide.x - line * SIZE.0).abs() < 0.5 && guide.height == SIZE.1),
            "the cell lines are drawn across the screen: {drawn:?}"
        );
        screen.release(to);
        hold_alt(false);

        let wanted = match alt {
            false => regions::snap(line),
            true => regions::snap(to.0 / SIZE.0),
        };
        assert_eq!(stored_region(&rig, "background").w, wanted, "alt {alt}");
        assert_eq!(stored_region(&rig, "middle").x, wanted, "alt {alt}");
        assert_eq!(screen.guides(), Vec::new(), "the guides go with the drag");
    }

    /// A split button dragged places the cut on the cell line it is let go near, reaching further than an edge does; with Alt held the cut is where it was let go.
    #[test]
    fn a_dragged_split_cuts_on_a_cell_line() {
        split_dragged(false);
    }

    #[test]
    fn a_dragged_split_cuts_free_with_alt() {
        split_dragged(true);
    }

    fn split_dragged(alt: bool) {
        let rig = rig_with(&format!("snap-cut-{alt}"), columns);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Background);
        let line = nearest(&desktop_lines(), 0.6);
        let current = mode::current().expect("the mode is up");
        let mut screen = Screen::new(vec![
            background::tool(&current).expect("the region tools build"),
        ]);
        let button = (960.0 - (host::HOTSPOT / 2.0 + 4.0), 540.0);
        let to = (line * SIZE.0 + 30.0, 540.0);

        hold_alt(alt);
        screen.press(button);
        screen.move_to((button.0 + 40.0, 540.0));
        screen.move_to(to);
        assert!(!screen.guides().is_empty(), "the cell lines are drawn");
        screen.release(to);
        hold_alt(false);

        let cut = match alt {
            false => regions::snap(line),
            true => regions::snap(to.0 / SIZE.0),
        };
        assert_eq!(
            stored_region(&rig, "middle"),
            rect(0.25, 0.0, cut - 0.25, 1.0),
            "alt {alt}"
        );
        assert_eq!(stored_region(&rig, "middle-2").x, cut, "alt {alt}");
    }

    /// `s` splits on the cell line nearest the region's middle, where one is within a cut's reach, and the halves still join back bit for bit.
    #[test]
    fn s_splits_on_the_cell_line_nearest_the_middle() {
        let rig = rig_with("snap-split-key", columns);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Background);
        let before = stored(&rig);
        let line = nearest(&desktop_lines(), 0.5);
        let layout = session::draft().peek();
        let plan = Plan {
            layout: &layout,
            desktop: surfaces::reconcile::desktops()[0].clone(),
            layer: LayerKind::Background,
        };
        let ops = plan
            .split_in_half(&AreaId::new("middle"), Cut::SideBySide)
            .expect("the middle splits");
        crate::context::commit("split".to_string(), ops).expect("committed");
        let cut = match (line - 0.5).abs() < snap::CUT_TOLERANCE {
            true => regions::snap(line),
            false => 0.5,
        };
        assert_eq!(stored_region(&rig, "middle-2").x, cut);
        let joined = Plan {
            layout: &session::draft().peek(),
            desktop: surfaces::reconcile::desktops()[0].clone(),
            layer: LayerKind::Background,
        }
        .join(&AreaId::new("middle"), &AreaId::new("middle-2"))
        .expect("the halves join");
        crate::context::commit("join".to_string(), joined).expect("committed");
        assert_eq!(stored(&rig), before);
    }

    fn prompt() -> Node {
        Node::area(Some(SCREEN), LayerKind::Lock, &AreaId::new("prompt"))
    }

    fn stored_prompt(rig: &Rig) -> Rect {
        let (resolved, _) = layout::resolve(&stored(rig), &crate::written::known(), SCREEN, None);
        resolved
            .layer(LayerKind::Lock)
            .and_then(|layer| {
                layer.areas.iter().find_map(|area| match area.kind {
                    ResolvedAreaKind::Prompt { rect } => Some(rect),
                    _ => None,
                })
            })
            .expect("the prompt")
    }

    /// Lock mode's prompt handle and the selection over a prompt drawn where the layout places it.
    fn prompt_screen(rig: &Rig) -> (Screen, Rect, (f32, f32)) {
        let at = stored_prompt(rig);
        let drawn = telar::Rect::new(at.x * SIZE.0, at.y * SIZE.1, at.w * SIZE.0, at.h * SIZE.1);
        rects::track_spanning(prompt(), vec![signal(drawn)]);
        let current = mode::current().expect("the mode is up");
        let screen = Screen::new(vec![
            lock_mode::tool(&current).expect("the prompt's handle builds"),
        ]);
        let middle = (drawn.x + drawn.width / 2.0, drawn.y + drawn.height / 2.0);
        (screen, at, middle)
    }

    /// The shipped prompt is centred across; nudged a few pixels off, its centre snaps back onto the screen's middle with a guide there, and with Alt held it stays where it was put.
    #[test]
    fn the_prompt_snaps_to_the_middle_of_the_screen() {
        prompt_nudged(false);
    }

    #[test]
    fn the_prompt_goes_free_with_alt() {
        prompt_nudged(true);
    }

    fn prompt_nudged(alt: bool) {
        let rig = rig_with(&format!("snap-prompt-{alt}"), |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let (mut screen, at, middle) = prompt_screen(&rig);
        assert!(close(at.x + at.w / 2.0, 0.5), "{at:?}");
        let to = (middle.0 + 5.0, middle.1);

        hold_alt(alt);
        screen.press(middle);
        screen.move_to(to);
        let drawn = screen.guides();
        screen.release(to);
        hold_alt(false);

        let now = stored_prompt(&rig);
        match alt {
            false => {
                assert!(close(now.x, at.x), "{now:?}");
                assert!(
                    drawn
                        .iter()
                        .any(|guide| (guide.x - SIZE.0 / 2.0).abs() < 0.5),
                    "a guide down the middle: {drawn:?}"
                );
            }
            true => {
                assert!(close(now.x, at.x + 5.0 / SIZE.0), "{now:?}");
                assert_eq!(drawn, Vec::new(), "nothing snapped");
            }
        }
    }

    /// The prompt is carried by the whole of the pointer's travel from the press, the part before the drag threshold included: one move far past it lands it that far.
    #[test]
    fn the_prompt_moves_by_the_whole_travel_from_the_press() {
        let rig = rig_with("snap-prompt-travel", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let (mut screen, at, middle) = prompt_screen(&rig);
        let to = (middle.0 + 100.0, middle.1 + 30.0);

        hold_alt(true);
        screen.press(middle);
        screen.move_to(to);
        screen.release(to);
        hold_alt(false);

        let now = stored_prompt(&rig);
        assert!(
            close(now.x, at.x + 100.0 / SIZE.0) && close(now.y, at.y + 30.0 / SIZE.1),
            "{now:?} from {at:?}"
        );
    }
}
