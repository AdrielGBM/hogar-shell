//! What the pointer shows and does over the edited layer, through real pointer events on the desktop mode's tools: the thin outline of what a click would select, a double-click customizing, the size tag under the selection, and the tag and ghost a drag carries at the pointer.

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use telar::{
        AvailableSpace, Color, ComponentList, Container, DismissRegistration, DrawCommand, Event,
        LayoutItem, LayoutStyle, Paint, PointerButton, PointerSource, Rect, RectStyle,
        StyledContainer, compute_layout, signal, use_theme,
    };

    use config::theme::NordTheme;
    use layout::{Area, AreaId, AreaKind, Arrange, GroupId, GroupKind, InstanceId, LayerKind};
    use surfaces::menu::Pointed;
    use surfaces::rects::{self, Node};
    use surfaces::transient;
    use ui::descriptor::{Built, Category, Input, ModuleDescriptor, Representations, WidgetDef};
    use ui::host::{Host, WidgetSize};

    use crate::mode::{self, Compositor};
    use crate::modes::widgets::{self, Geometry};
    use crate::modes::{gesture, grid};
    use crate::rig::{Rig, SCREEN};
    use crate::session::{self, Selection};
    use crate::{host, popover};

    const SIZE: (f32, f32) = (1920.0, 1080.0);

    fn face(_: &Host) -> Built {
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new().width(40.0).height(20.0),
            |_| RectStyle::default(),
            Vec::new(),
        )?))
    }

    static PROBES: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "circle",
        category: Category::Time,
        options: &[],
        representations: Representations {
            widget: Some(WidgetDef {
                sizes: &WidgetSize::ALL,
                build: face,
                input: Input::ReadOnly,
            }),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    }];

    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn widgets_area() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"))
    }

    /// A grid over the whole screen holding the built-in clock on its first cells, a row container and a loose widget beside it.
    fn rig(test: &str) -> Rig {
        crate::rig::rig_with(test, |layout| {
            let areas = &mut layout.outputs[0].layers.desktop.areas;
            let centre: Area = areas.pop().expect("the clock's own area");
            let mut clock = centre.groups.into_iter().next().expect("its group");
            clock.kind = Some(GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
            });
            let copy = |id: &str, col: u32, arrange: Option<Arrange>| {
                let mut group = clock.clone();
                group.id = GroupId::new(id);
                group.kind = Some(GroupKind::Cell {
                    col,
                    row: 0,
                    col_span: 4,
                    row_span: 2,
                });
                group.arrange = arrange;
                for child in &mut group.children {
                    child.id = InstanceId::new(format!("{id}-clock"));
                }
                group
            };
            let shelf = copy("shelf", 8, Some(Arrange::Row));
            let spare = copy("spare", 16, None);
            areas[0].kind = Some(AreaKind::Grid {
                rect: Some(layout::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                }),
                cell: None,
                gap: None,
                anchor: Some(layout::Anchor::TopLeft),
            });
            areas[0].groups.extend([clock, shelf, spare]);
        })
    }

    fn enter() -> DismissRegistration {
        enter_on(LayerKind::Desktop)
    }

    fn enter_on(layer: LayerKind) -> DismissRegistration {
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

    /// The grid as drawn, every group and instance on it recorded where its cells are, as the desktop window records them.
    fn placed() -> Geometry {
        let (geometry, area) = widgets::grids(SCREEN, LayerKind::Desktop)
            .into_iter()
            .next()
            .expect("the grid");
        rects::track_spanning(widgets_area(), vec![signal(geometry.region)]);
        for group in &area.groups {
            let rect = geometry.rect_of(grid::cells_of(group).expect("on cells"));
            rects::track_spanning(widgets_area().group(&group.id), vec![signal(rect)]);
            for child in &group.children {
                rects::track_spanning(
                    widgets_area().instance(&group.id, &child.id),
                    vec![signal(rect)],
                );
            }
        }
        geometry
    }

    fn clock_node() -> Node {
        let area = widgets::grids(SCREEN, LayerKind::Desktop)
            .into_iter()
            .next()
            .expect("the grid")
            .1;
        let group = area
            .groups
            .iter()
            .find(|group| group.id.as_str() == "clock")
            .expect("the clock's group");
        widgets_area().instance(&group.id, &group.children[0].id)
    }

    fn drawn(node: &Node) -> Rect {
        rects::rect(node).expect("recorded")
    }

    fn middle(rect: Rect) -> (f32, f32) {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    /// The desktop mode's tools as its host builds them, over the whole screen.
    struct Screen(ComponentList);

    impl Screen {
        fn new() -> Self {
            let current = mode::current().expect("the mode is up");
            Self::of(vec![host::tools(&current).expect("the tools build")])
        }

        fn of(tools: Vec<Box<dyn LayoutItem>>) -> Self {
            let page = || LayoutStyle::new().width(SIZE.0).height(SIZE.1);
            let root = Pointed::new(Box::new(Container::new(page(), tools).expect("a page")));
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
            self.settle();
        }

        fn settle(&mut self) {
            for _ in 0..3 {
                telar::relayout_if_dirty();
            }
        }

        fn move_to(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerMoved {
                x: x.into(),
                y: y.into(),
                source: PointerSource::Mouse,
            });
        }

        fn press(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerPressed {
                x: x.into(),
                y: y.into(),
                button: PointerButton::Primary,
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

        fn click(&mut self, at: (f32, f32)) {
            self.move_to(at);
            self.press(at);
            self.release(at);
        }

        /// Every box drawn, where it is on the screen, with how it is painted.
        fn boxes(&self) -> Vec<(Rect, RectStyle)> {
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.0.commands(), |command, [a, b, c, d, e, f]| {
                if let DrawCommand::Rect { rect, style } = command {
                    found.push((
                        Rect::new(
                            a * rect.x + c * rect.y + e,
                            b * rect.x + d * rect.y + f,
                            rect.width,
                            rect.height,
                        ),
                        RectStyle::clone(style),
                    ));
                }
            });
            found
        }

        /// Where every box edged in `edge` is.
        fn edged(&self, edge: Color) -> Vec<Rect> {
            self.boxes()
                .into_iter()
                .filter(|(_, style)| {
                    style
                        .border
                        .as_ref()
                        .is_some_and(|border| border.paint == Paint::Solid(edge))
                })
                .map(|(rect, _)| rect)
                .collect()
        }

        /// Where every box filled with `fill` is.
        fn filled(&self, fill: Color) -> Vec<Rect> {
            self.boxes()
                .into_iter()
                .filter(|(_, style)| style.fill == Some(Paint::Solid(fill)))
                .map(|(rect, _)| rect)
                .collect()
        }

        /// Every text drawn, with where it is on the screen.
        fn texts(&self) -> Vec<(String, Rect)> {
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.0.commands(), |command, [a, b, c, d, e, f]| {
                if let DrawCommand::Text { text, rect, .. } = command {
                    found.push((
                        text.to_string(),
                        Rect::new(
                            a * rect.x + c * rect.y + e,
                            b * rect.x + d * rect.y + f,
                            rect.width,
                            rect.height,
                        ),
                    ));
                }
            });
            found
        }

        fn said(&self, wanted: &str) -> Option<Rect> {
            self.texts()
                .into_iter()
                .find(|(text, _)| text == wanted)
                .map(|(_, rect)| rect)
        }
    }

    fn same(a: Rect, b: Rect) -> bool {
        (a.x - b.x).abs() < 0.5
            && (a.y - b.y).abs() < 0.5
            && (a.width - b.width).abs() < 0.5
            && (a.height - b.height).abs() < 0.5
    }

    fn hover_edge() -> Color {
        use_theme::<NordTheme>().accent.with_alpha(0.6)
    }

    fn ghost_fill() -> Color {
        use_theme::<NordTheme>().surface.with_alpha(0.5)
    }

    /// The pointer over a widget outlines it thinly; once it is selected nothing more is drawn over it, while what a click elsewhere would select is still outlined; during a drag nothing is.
    #[test]
    fn the_pointer_outlines_what_a_click_would_select_but_not_the_selection_or_during_a_drag() {
        let _rig = rig("pointer-hover");
        let _owner = Owner::new();
        let _host = enter();
        let geometry = placed();
        let mut screen = Screen::new();
        let clock = drawn(&clock_node());

        screen.move_to(middle(clock));
        let outlined = screen.edged(hover_edge());
        assert!(
            outlined.len() == 1 && same(outlined[0], clock),
            "the clock is outlined: {outlined:?}"
        );

        screen.click(middle(clock));
        assert_eq!(session::selected(), Selection::Instance(clock_node()));
        screen.move_to((clock.x + 8.0, clock.y + 8.0));
        assert_eq!(
            screen.edged(hover_edge()),
            Vec::<Rect>::new(),
            "nothing more over the selection, nor round its group a click there would climb to"
        );

        let empty = middle(geometry.rect_of(grid::Cells {
            col: 2,
            row: 8,
            cols: 1,
            rows: 1,
        }));
        screen.move_to(empty);
        let outlined = screen.edged(hover_edge());
        assert!(
            outlined.len() == 1 && same(outlined[0], geometry.region),
            "the grid a click there would select is outlined: {outlined:?}"
        );

        screen.move_to(middle(clock));
        screen.press(middle(clock));
        screen.move_to((clock.x + clock.width / 2.0 + 30.0, clock.y + 300.0));
        assert_eq!(
            screen.edged(hover_edge()),
            Vec::<Rect>::new(),
            "none during a drag"
        );
        screen.release((clock.x + clock.width / 2.0 + 30.0, clock.y + 300.0));
    }

    /// A double-click on a widget opens its popover; a single click only selects it.
    #[test]
    fn a_double_click_opens_the_popover_of_what_it_selects() {
        let _rig = rig("pointer-double-click");
        let _owner = Owner::new();
        let _host = enter();
        placed();
        let mut screen = Screen::new();
        let at = middle(drawn(&clock_node()));

        screen.click(at);
        assert_eq!(session::selected(), Selection::Instance(clock_node()));
        assert_eq!(popover::current(), None, "one click only selects");
        screen.click(at);
        assert_eq!(popover::current(), Some(clock_node()));
        assert_eq!(
            session::selected(),
            Selection::Instance(clock_node()),
            "the second press customizes rather than climbing to the group"
        );
        popover::close();
    }

    /// Under a selected widget or group its size in cells and in pixels; none under an area, and none during a drag.
    #[test]
    fn the_size_tag_says_cells_and_pixels_under_the_selection() {
        let _rig = rig("pointer-size-tag");
        let _owner = Owner::new();
        let _host = enter();
        placed();
        let mut screen = Screen::new();
        let rect = drawn(&clock_node());
        let cells = grid::cells_of(
            &widgets::grids(SCREEN, LayerKind::Desktop)[0]
                .1
                .groups
                .iter()
                .find(|group| group.id.as_str() == "clock")
                .cloned()
                .expect("the clock's group"),
        )
        .expect("on cells");
        let expected = format!(
            "{} × {} cells · {} × {}",
            cells.cols,
            cells.rows,
            rect.width.round(),
            rect.height.round()
        );
        let tagged = |screen: &Screen| {
            screen
                .texts()
                .into_iter()
                .filter(|(text, _)| text.contains(" × "))
                .collect::<Vec<_>>()
        };

        screen.click(middle(rect));
        let tag = screen
            .said(&expected)
            .unwrap_or_else(|| panic!("{expected:?} under the clock: {:?}", screen.texts()));
        assert!(
            tag.y > rect.y + rect.height && (middle(tag).0 - middle(rect).0).abs() < 1.0,
            "centred under it: {tag:?} {rect:?}"
        );

        assert!(session::select(Selection::Group(
            widgets_area().group(&GroupId::new("clock"))
        )));
        screen.settle();
        assert!(screen.said(&expected).is_some(), "{:?}", screen.texts());

        assert!(session::select(Selection::Area(widgets_area())));
        screen.settle();
        assert_eq!(tagged(&screen), Vec::new(), "none under an area");

        screen.press(middle(rect));
        screen.move_to((middle(rect).0 + 200.0, middle(rect).1 + 300.0));
        assert_eq!(tagged(&screen), Vec::new(), "none during a drag");
        screen.release((middle(rect).0 + 200.0, middle(rect).1 + 300.0));
    }

    /// A widget being dragged has a translucent ghost under the pointer, where it was taken hold of, and a tag beside the pointer saying where it lands — a cell counted from one, into a container, onto a widget to stack — that goes over to the pointer's other side near the right edge and the foot of the screen.
    #[test]
    fn a_drag_tags_where_it_lands_beside_the_pointer_and_carries_a_ghost() {
        let _rig = rig("pointer-drag-tag");
        let _owner = Owner::new();
        let _host = enter();
        let geometry = placed();
        let mut screen = Screen::new();
        let clock = drawn(&clock_node());
        let grab = (6.0, 6.0);
        let start = (clock.x + grab.0, clock.y + grab.1);
        let cell = |col, row| {
            geometry.rect_of(grid::Cells {
                col,
                row,
                cols: 1,
                rows: 1,
            })
        };

        screen.move_to(start);
        screen.press(start);
        let target = cell(5, 3);
        let at = (target.x + grab.0, target.y + grab.1);
        screen.move_to((start.0 + 10.0, start.1));
        screen.move_to(at);
        let tag = screen
            .said("col 6 · row 4")
            .unwrap_or_else(|| panic!("the cell is said: {:?}", screen.texts()));
        assert!(
            tag.x >= at.0 + 14.0 && tag.y >= at.1 + 14.0,
            "below and right of the pointer: {tag:?} {at:?}"
        );
        let ghost = |screen: &Screen| {
            let ghosts = screen.filled(ghost_fill());
            assert_eq!(ghosts.len(), 1, "one ghost: {ghosts:?}");
            ghosts[0]
        };
        let held = ghost(&screen);
        assert!(
            (held.width - clock.width).abs() < 0.5
                && (held.height - clock.height).abs() < 0.5
                && held.contains(at.0, at.1),
            "the ghost is the widget's size, under the pointer: {held:?} {at:?}"
        );
        let further = (at.0 + 37.0, at.1 + 23.0);
        screen.move_to(further);
        let followed = ghost(&screen);
        assert!(
            same(
                followed,
                Rect::new(held.x + 37.0, held.y + 23.0, held.width, held.height)
            ),
            "and follows it: {followed:?}"
        );

        let shelf = drawn(&widgets_area().group(&GroupId::new("shelf")));
        screen.move_to(middle(shelf));
        assert!(
            screen.said("Into the container").is_some(),
            "{:?}",
            screen.texts()
        );
        let spare = drawn(&widgets_area().group(&GroupId::new("spare")));
        screen.move_to(middle(spare));
        assert!(screen.said("Stack").is_some(), "{:?}", screen.texts());

        let corner = (SIZE.0 - 20.0, SIZE.1 - 10.0);
        screen.move_to(corner);
        let (said, tag) = screen
            .texts()
            .into_iter()
            .find(|(text, _)| text.starts_with("col "))
            .unwrap_or_else(|| panic!("a cell is said: {:?}", screen.texts()));
        assert!(
            tag.x + tag.width <= corner.0 - 14.0 && tag.y + tag.height <= corner.1 - 14.0,
            "{said:?} goes over to the pointer's left and above it: {tag:?}"
        );

        screen.release(corner);
        assert!(
            screen
                .texts()
                .iter()
                .all(|(text, _)| !text.starts_with("col ")),
            "the tag goes with the drag"
        );
        assert_eq!(
            screen.filled(ghost_fill()),
            Vec::new(),
            "and so does the ghost"
        );
    }

    /// A widget is held where it was pressed, the travel before the drag threshold included: a first move far past the threshold carries the ghost that far from the widget, and it lands on the cells under the ghost's corner.
    #[test]
    fn a_drag_holds_the_widget_where_it_was_pressed() {
        let _rig = rig("pointer-drag-press");
        let _owner = Owner::new();
        let _host = enter();
        let geometry = placed();
        let mut screen = Screen::new();
        let clock = drawn(&clock_node());
        let start = (clock.x + 6.0, clock.y + 6.0);
        let travel = (geometry.pitch * 3.0, geometry.pitch * 2.0);
        let at = (start.0 + travel.0, start.1 + travel.1);

        screen.move_to(start);
        screen.press(start);
        screen.move_to(at);
        let ghosts = screen.filled(ghost_fill());
        let wanted = Rect::new(
            clock.x + travel.0,
            clock.y + travel.1,
            clock.width,
            clock.height,
        );
        assert!(
            ghosts.len() == 1 && same(ghosts[0], wanted),
            "the ghost moved by the whole travel: {ghosts:?} {wanted:?}"
        );
        assert!(
            screen.said("col 4 · row 3").is_some(),
            "{:?}",
            screen.texts()
        );
        screen.release(at);
    }

    fn two_columns(test: &str) -> Rig {
        crate::rig::rig_with(test, |layout| {
            let background = &mut layout.outputs[0].layers.background;
            let column = |x: f32, w: f32| {
                Some(layout::Rect {
                    x,
                    y: 0.0,
                    w,
                    h: 1.0,
                })
            };
            if let Some(AreaKind::WallpaperRegion { rect, .. }) = &mut background.areas[0].kind {
                *rect = column(0.0, 0.25);
            }
            background.areas.push(Area {
                id: AreaId::new("right"),
                kind: Some(AreaKind::WallpaperRegion {
                    rect: column(0.25, 0.75),
                    source: None,
                    fit: None,
                    transition: None,
                }),
                ..Area::default()
            });
        })
    }

    fn stack_width_handle() -> (Screen, (f32, f32)) {
        let desktop = surfaces::reconcile::desktops()
            .iter()
            .find(|desktop| desktop.output.as_deref() == Some(SCREEN))
            .cloned()
            .expect("the edited screen");
        let stack = crate::modes::overlay::stacks_of(&desktop)
            .into_iter()
            .next()
            .expect("the layout has a stack");
        let node = Node::area(Some(SCREEN), LayerKind::Overlay, &stack.id);
        popover::open_area(node.clone()).expect("its popover opens");
        let item = popover::tree()
            .expect("a popover is open")
            .expect("its tree builds");
        let column = rects::rect(&node).unwrap_or_default();
        let handle = crate::modes::overlay::width_point(column, stack.anchor, stack.width);
        (Screen::of(vec![item]), handle)
    }

    fn drag_by(screen: &mut Screen, from: (f32, f32), by: f32) -> (f32, f32) {
        let to = (from.0 + by, from.1);
        screen.move_to(from);
        screen.press(from);
        screen.move_to((from.0 + 10.0, from.1));
        screen.move_to(to);
        assert!(
            gesture::dragging(),
            "dragging while the pointer travels from {from:?}"
        );
        screen.release(to);
        assert!(!gesture::dragging(), "not dragging once it is let go");
        to
    }

    /// While a region edge is dragged the dragging signal is set, and it is cleared on release.
    #[test]
    fn the_dragging_signal_is_set_during_a_region_edge_drag() {
        let _rig = two_columns("pointer-edge-drag-signal");
        let _owner = Owner::new();
        let _host = enter_on(LayerKind::Background);
        let current = mode::current().expect("the mode is up");
        let mut screen = Screen::of(vec![
            crate::modes::background::tool(&current).expect("the region tools build"),
        ]);

        assert!(!gesture::dragging(), "not dragging before the press");
        let _ = drag_by(&mut screen, (480.0, 540.0), 60.0);
    }

    /// The same width handle of an overlay stack, dragged twice in a row without being rebuilt, sets the dragging signal each time.
    #[test]
    fn a_stack_width_handle_signals_dragging_on_every_drag() {
        let _rig = crate::rig::rig_with("pointer-width-drag-signal", |_| {});
        let _owner = Owner::new();
        let _host = enter_on(LayerKind::Overlay);
        let (mut screen, handle) = stack_width_handle();

        assert!(!gesture::dragging(), "not dragging before the press");
        let moved = drag_by(&mut screen, handle, 40.0);
        drag_by(&mut screen, moved, 40.0);
    }
}
