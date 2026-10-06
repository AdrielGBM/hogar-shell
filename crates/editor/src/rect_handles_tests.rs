//! Rectangles handled on the screen, through real pointer events: each corner of a texture, a free area, a grid and the lock prompt resizes it, snapping the edges it drags unless Alt is held; a texture's body carries it; a free area's body pins what it holds by ninths and its dots pin it exactly; the prompt never gets smaller than a prompt may be nor leaves its screen; and the keys that move and resize them still do.

#[cfg(test)]
mod tests {
    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, Paint, PointerButton, PointerSource, compute_layout, use_theme,
    };

    use config::theme::NordTheme;
    use layout::{
        Anchor, Area, AreaId, AreaKind, LayerKind, Layout, Rect, ResolvedAreaKind, SMALLEST_PROMPT,
    };
    use surfaces::menu::Pointed;
    use surfaces::rects::Node;
    use surfaces::transient;

    use crate::keys::{self, Press};
    use crate::mode;
    use crate::modes::gesture;
    use crate::modes::rect_handles;
    use crate::modes::{background, lock as lock_mode, widgets};
    use crate::popover::handles::{CORNERS, Corner};
    use crate::rig::{Rig, SCREEN, close, enter, hold_alt, rig_with, stored};
    use crate::select;
    use crate::session::{self, Selection};

    const SIZE: (f32, f32) = (1920.0, 1080.0);
    /// Inside the six pixels a line pulls an edge from.
    const NEAR: f32 = 4.0;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn close_rect(a: Rect, b: Rect) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.w, b.w) && close(a.h, b.h)
    }

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

    /// The selection tool, then the mode's `tools` in the order the host stacks them, over the whole screen inside the root every window has.
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

        fn move_to(&mut self, (x, y): (f32, f32)) {
            self.send(Event::PointerMoved {
                x: x.into(),
                y: y.into(),
                source: PointerSource::Mouse,
            });
        }

        fn button(&mut self, (x, y): (f32, f32), pressed: bool) {
            let (x, y) = (x.into(), y.into());
            let (button, source) = (PointerButton::Primary, PointerSource::Mouse);
            self.send(match pressed {
                true => Event::PointerPressed {
                    x,
                    y,
                    button,
                    source,
                },
                false => Event::PointerReleased {
                    x,
                    y,
                    button,
                    source,
                },
            });
        }

        fn click(&mut self, at: (f32, f32)) {
            self.move_to(at);
            self.button(at, true);
            self.button(at, false);
        }

        /// A press at `from`, a move a little way off and then to `to`, and a release there; answers what the drag showed at the pointer just before it was let go.
        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) -> Shown {
            self.move_to(from);
            self.button(from, true);
            self.move_to((from.0 + 10.0, from.1 + 10.0));
            self.move_to(to);
            let shown = Shown {
                tag: gesture::hint().peek().and_then(|hint| hint.tag),
                guides: self.guides(),
            };
            self.button(to, false);
            shown
        }

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

    struct Shown {
        tag: Option<String>,
        guides: Vec<telar::Rect>,
    }

    fn stored_kind(rig: &Rig, layer: LayerKind, id: &str) -> ResolvedAreaKind {
        let (resolved, _) = layout::resolve(&stored(rig), &crate::written::known(), SCREEN, None);
        resolved
            .area(layer, &AreaId::new(id))
            .map(|area| area.kind.clone())
            .unwrap_or_else(|| panic!("the area {id}"))
    }

    fn stored_rect(rig: &Rig, layer: LayerKind, id: &str) -> Rect {
        stored_kind(rig, layer, id)
            .rect()
            .expect("a rectangle places it")
    }

    fn area(layer: LayerKind, id: &str) -> Node {
        Node::area(Some(SCREEN), layer, &AreaId::new(id))
    }

    /// Where the area `node` names is on screen now, in pixels.
    fn drawn(node: &Node) -> telar::Rect {
        let desktop = surfaces::reconcile::desktop_now(Some(SCREEN)).expect("the screen");
        let area = desktop
            .resolved
            .area(node.layer, &node.area)
            .expect("the area is drawn");
        let bounds = desktop.reserved.box_of(area.within, desktop.size);
        surfaces::area::within(area.kind.rect().expect("a rectangle"), bounds)
    }

    fn corner_of(node: &Node, corner: Corner) -> (f32, f32) {
        let at = drawn(node);
        corner.of((at.x, at.y, at.width, at.height))
    }

    fn select(node: &Node) {
        assert!(session::select(Selection::Area(node.clone())), "{node:?}");
    }

    fn region(id: &str, at: Rect) -> Area {
        Area {
            id: AreaId::new(id),
            kind: Some(AreaKind::WallpaperRegion {
                rect: Some(at),
                source: None,
                fit: None,
                transition: None,
            }),
            ..Area::default()
        }
    }

    /// Three columns of regions, their edges at a quarter and three quarters across, and a texture over the middle one.
    fn textured(layout: &mut Layout, wash: Rect) {
        let background = &mut layout.outputs[0].layers.background;
        if let Some(AreaKind::WallpaperRegion { rect: at, .. }) = &mut background.areas[0].kind {
            *at = Some(rect(0.0, 0.0, 0.25, 1.0));
        }
        background
            .areas
            .push(region("middle", rect(0.25, 0.0, 0.5, 1.0)));
        background
            .areas
            .push(region("right", rect(0.75, 0.0, 0.25, 1.0)));
        background.areas.push(Area {
            id: AreaId::new("wash"),
            kind: Some(AreaKind::Texture {
                rect: Some(wash),
                image: None,
                gradient: Some(crate::modes::texture::first_gradient()),
                tile: None,
                blend: None,
                opacity: None,
            }),
            ..Area::default()
        });
    }

    fn background_screen() -> Screen {
        let current = mode::current().expect("the mode is up");
        Screen::new(vec![
            rect_handles::bodies(&current).expect("the bodies build"),
            background::tool(&current).expect("the region tools build"),
            rect_handles::corners(&current).expect("the corners build"),
        ])
    }

    fn desktop_screen() -> Screen {
        let current = mode::current().expect("the mode is up");
        Screen::new(vec![
            rect_handles::bodies(&current).expect("the bodies build"),
            widgets::tool(&current).expect("the widget tools build"),
            rect_handles::corners(&current).expect("the corners build"),
        ])
    }

    /// A corner dragged to `to`, in fractions of the box, drags its edges there and keeps the opposite corner; neither side gets shorter than the least.
    #[test]
    fn a_corner_moves_its_own_edges_and_keeps_the_opposite_corner() {
        let at = rect(0.2, 0.2, 0.4, 0.4);
        assert_eq!(
            rect_handles::resized(at, Corner::TopLeft, (0.1, 0.3), 0.02),
            rect(0.1, 0.3, 0.5, 0.3)
        );
        assert!(close_rect(
            rect_handles::resized(at, Corner::BottomRight, (0.7, 0.9), 0.02),
            rect(0.2, 0.2, 0.5, 0.7)
        ));
        assert!(close_rect(
            rect_handles::resized(at, Corner::TopRight, (0.1, -1.0), 0.02),
            rect(0.2, 0.0, 0.02, 0.6)
        ));
        assert!(close_rect(
            rect_handles::resized(at, Corner::BottomLeft, (0.9, 2.0), 0.05),
            rect(0.55, 0.2, 0.05, 0.8)
        ));
    }

    /// Each corner of a texture, dragged to a few pixels inside a region edge across and the screen's edge down, lands on them, with the lines drawn and its size said at the pointer; with Alt held it lands where it was let go.
    #[test]
    fn a_texture_resizes_from_each_corner_snapping_and_free_with_alt() {
        for corner in CORNERS {
            for alt in [false, true] {
                texture_corner_dragged(corner, alt);
            }
        }
    }

    fn texture_corner_dragged(corner: Corner, alt: bool) {
        let wash = rect(0.3, 0.3, 0.3, 0.3);
        let rig = rig_with(&format!("rect-texture-{corner:?}-{alt}"), |layout| {
            textured(layout, wash)
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Background);
        let node = area(LayerKind::Background, "wash");
        select(&node);
        let mut screen = background_screen();
        let left = matches!(corner, Corner::TopLeft | Corner::BottomLeft);
        let top = matches!(corner, Corner::TopLeft | Corner::TopRight);
        let (line_x, line_y) = (if left { 0.25 } else { 0.75 }, if top { 0.0 } else { 1.0 });
        let to = (
            line_x * SIZE.0 + if left { NEAR } else { -NEAR },
            line_y * SIZE.1 + if top { NEAR } else { -NEAR },
        );

        hold_alt(alt);
        let shown = screen.drag(corner_of(&node, corner), to);
        hold_alt(false);

        let (x, y) = match alt {
            false => (line_x, line_y),
            true => (to.0 / SIZE.0, to.1 / SIZE.1),
        };
        let (left_x, right_x) = if left { (x, 0.6) } else { (0.3, x) };
        let (top_y, bottom_y) = if top { (y, 0.6) } else { (0.3, y) };
        let wanted = rect(left_x, top_y, right_x - left_x, bottom_y - top_y);
        let now = stored_rect(&rig, LayerKind::Background, "wash");
        assert!(close_rect(now, wanted), "{corner:?} alt {alt}: {now:?}");
        assert_eq!(
            shown.tag,
            Some(rect_handles::size_said(now)),
            "{corner:?} alt {alt}"
        );
        assert_eq!(
            shown.guides.is_empty(),
            alt,
            "{corner:?} alt {alt}: {:?}",
            shown.guides
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Resize wash"));
    }

    /// A texture dragged by its body is carried by the whole travel of the pointer, its left edge snapping onto the region edge it comes within a few pixels of; with Alt held it goes where it was taken.
    #[test]
    fn a_texture_body_moves_snapping_as_a_whole() {
        texture_carried(false);
    }

    #[test]
    fn a_texture_body_moves_free_with_alt() {
        texture_carried(true);
    }

    fn texture_carried(alt: bool) {
        let wash = rect(0.3, 0.3, 0.2, 0.2);
        let rig = rig_with(&format!("rect-texture-body-{alt}"), |layout| {
            textured(layout, wash)
        });
        let _owner = Owner::new();
        let _host = enter(LayerKind::Background);
        let mut screen = background_screen();
        let from = (0.4 * SIZE.0, 0.4 * SIZE.1);
        let by = -(0.05 * SIZE.0) + NEAR;

        hold_alt(alt);
        let shown = screen.drag(from, (from.0 + by, from.1));
        hold_alt(false);

        let x = match alt {
            false => 0.25,
            true => 0.3 + by / SIZE.0,
        };
        let now = stored_rect(&rig, LayerKind::Background, "wash");
        assert!(
            close_rect(now, rect(x, 0.3, 0.2, 0.2)),
            "alt {alt}: {now:?}"
        );
        assert_eq!(
            session::selected(),
            Selection::Area(area(LayerKind::Background, "wash"))
        );
        assert!(
            alt || shown
                .guides
                .iter()
                .any(|guide| (guide.x - 0.25 * SIZE.0).abs() < 0.5),
            "a guide down the region edge: {:?}",
            shown.guides
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Move wash"));
    }

    /// The desktop's free area, made a quarter of the screen in from each side.
    fn quartered(layout: &mut Layout) {
        for held in &mut layout.outputs[0].layers.desktop.areas {
            if let Some(AreaKind::Free { rect: at, .. }) = &mut held.kind {
                *at = Some(rect(0.25, 0.25, 0.5, 0.5));
            }
        }
    }

    fn anchor_of(rig: &Rig) -> Anchor {
        match stored_kind(rig, LayerKind::Desktop, "centre") {
            ResolvedAreaKind::Free { anchor, .. } => anchor,
            other => panic!("not a free area: {other:?}"),
        }
    }

    /// A free area dragged by its body while it is selected pins what it holds to the ninth of it let go over, said at the pointer; a drag across it with nothing of it selected moves nothing; and a dot pressed pins it there.
    #[test]
    fn a_free_area_pins_by_ninths_and_by_its_dots() {
        let rig = rig_with("rect-free-anchor", quartered);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let node = area(LayerKind::Desktop, "centre");
        let mut screen = desktop_screen();
        assert_eq!(anchor_of(&rig), Anchor::Center);
        let inside = (700.0, 400.0);
        let top_left = (560.0, 330.0);

        session::clear_selection();
        screen.drag(inside, top_left);
        assert_eq!(
            anchor_of(&rig),
            Anchor::Center,
            "nothing of it was selected"
        );

        select(&node);
        let shown = screen.drag(inside, top_left);
        assert_eq!(anchor_of(&rig), Anchor::TopLeft);
        assert_eq!(
            shown.tag,
            Some(crate::modes::overlay::anchor_name(Anchor::TopLeft))
        );
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Anchor what centre holds")
        );

        select(&node);
        let dot = surfaces::pinned::point(drawn(&node), Anchor::BottomRight);
        screen.click(dot);
        assert_eq!(anchor_of(&rig), Anchor::BottomRight);
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Pin what centre holds to the bottom right")
        );
    }

    /// A free area's corner resizes it like any other rectangle: with Alt held, by exactly the pointer's travel.
    #[test]
    fn a_free_area_resizes_from_its_corner() {
        let rig = rig_with("rect-free-corner", quartered);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let node = area(LayerKind::Desktop, "centre");
        select(&node);
        let mut screen = desktop_screen();
        let from = corner_of(&node, Corner::BottomRight);

        hold_alt(true);
        screen.drag(from, (from.0 + 96.0, from.1 + 54.0));
        hold_alt(false);

        let now = stored_rect(&rig, LayerKind::Desktop, "centre");
        assert!(close_rect(now, rect(0.25, 0.25, 0.55, 0.55)), "{now:?}");
    }

    /// The desktop grid's corner resizes the grid: its right edge snaps onto the middle of the screen, and with Alt held its top left corner follows the pointer exactly.
    #[test]
    fn a_grid_resizes_from_its_corners() {
        let rig = rig_with("rect-grid-corner", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let node = area(LayerKind::Desktop, "widgets");
        select(&node);
        let mut screen = desktop_screen();
        let before = stored_rect(&rig, LayerKind::Desktop, "widgets");
        let bounds = {
            let at = drawn(&node);
            (at.width / before.w, at.height / before.h)
        };

        let from = corner_of(&node, Corner::BottomRight);
        let shown = screen.drag(from, (SIZE.0 / 2.0 + NEAR, from.1));
        let now = stored_rect(&rig, LayerKind::Desktop, "widgets");
        assert!(close(now.x + now.w, 0.5), "{now:?}");
        assert!(!shown.guides.is_empty());

        select(&node);
        let from = corner_of(&node, Corner::TopLeft);
        hold_alt(true);
        screen.drag((from.0 + 2.0, from.1 + 2.0), (from.0 + 98.0, from.1 + 56.0));
        hold_alt(false);
        let moved = stored_rect(&rig, LayerKind::Desktop, "widgets");
        assert!(
            close(moved.x, now.x + 96.0 / bounds.0) && close(moved.y, now.y + 54.0 / bounds.1),
            "{moved:?} from {now:?}"
        );
        assert!(close(moved.x + moved.w, 0.5), "its right edge stays");
    }

    fn prompt() -> Node {
        area(LayerKind::Lock, "prompt")
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

    /// The prompt's corner shrinks it no further than a prompt may be, however far past its other corner it is dragged, and a corner dragged off the screen leaves it on it.
    #[test]
    fn the_prompt_keeps_its_least_size_and_its_screen() {
        let rig = rig_with("rect-prompt", |_| {});
        let _owner = Owner::new();
        let _host = enter(LayerKind::Lock);
        let node = prompt();
        select(&node);
        let current = mode::current().expect("the mode is up");
        let mut screen = Screen::new(vec![
            rect_handles::bodies(&current).expect("the bodies build"),
            lock_mode::tool(&current).expect("the prompt's handle builds"),
            rect_handles::corners(&current).expect("the corners build"),
        ]);
        let at = stored_prompt(&rig);

        hold_alt(true);
        screen.drag(corner_of(&node, Corner::BottomRight), (10.0, 10.0));
        let shrunk = stored_prompt(&rig);
        assert!(
            close_rect(shrunk, rect(at.x, at.y, SMALLEST_PROMPT, SMALLEST_PROMPT)),
            "{shrunk:?} from {at:?}"
        );

        select(&node);
        screen.drag(corner_of(&node, Corner::TopLeft), (-400.0, -400.0));
        hold_alt(false);
        let grown = stored_prompt(&rig);
        assert!(
            close(grown.x, 0.0) && close(grown.y, 0.0),
            "kept on the screen: {grown:?}"
        );
        assert!(
            close(grown.x + grown.w, at.x + SMALLEST_PROMPT),
            "{grown:?}"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Resize prompt"));
    }

    fn arrow_right(modifiers: ModifiersState) {
        let key = Key::Named(NamedKey::ArrowRight);
        telar::observe_keyboard(&Event::KeyPressed {
            key: key.clone(),
            modifiers,
        });
        assert!(keys::press_as(&key, modifiers, Press::First), "taken");
        telar::observe_keyboard(&Event::KeyReleased { key, modifiers });
        keys::settle_released();
    }

    const SHIFT: ModifiersState = ModifiersState {
        is_shift: true,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };

    const CTRL: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: true,
        is_alt: false,
        is_meta: false,
    };

    /// Shift+arrows move a texture and a free area a step, and Ctrl+arrows make them a step wider, as the corners and bodies do by pointer.
    #[test]
    fn the_keys_still_move_and_resize_rectangles() {
        let rig = rig_with("rect-keys", |layout| {
            textured(layout, rect(0.3, 0.3, 0.2, 0.2));
            quartered(layout);
        });
        let _owner = Owner::new();
        {
            let _host = enter(LayerKind::Background);
            select(&area(LayerKind::Background, "wash"));
            arrow_right(SHIFT);
            arrow_right(CTRL);
            let now = stored_rect(&rig, LayerKind::Background, "wash");
            assert!(close_rect(now, rect(0.31, 0.3, 0.21, 0.2)), "{now:?}");
            mode::leave();
        }
        let _host = enter(LayerKind::Desktop);
        select(&area(LayerKind::Desktop, "centre"));
        arrow_right(SHIFT);
        arrow_right(CTRL);
        let now = stored_rect(&rig, LayerKind::Desktop, "centre");
        assert!(close_rect(now, rect(0.26, 0.25, 0.51, 0.5)), "{now:?}");
    }

    /// While the radius or padding tool is up its handles are the only ones over the selection: a drag from where the rectangle's corner is moves nothing, and the corner is a handle again once the tool is put away.
    #[test]
    fn the_corners_give_way_to_the_radius_and_padding_tools() {
        let rig = rig_with("rect-corners-tool", quartered);
        let _owner = Owner::new();
        let _host = enter(LayerKind::Desktop);
        let node = area(LayerKind::Desktop, "centre");
        select(&node);
        let mut screen = desktop_screen();
        let from = corner_of(&node, Corner::BottomRight);
        let to = (from.0 + 96.0, from.1 + 54.0);
        let before = stored_rect(&rig, LayerKind::Desktop, "centre");

        for tool in [crate::tools::Tool::Radius, crate::tools::Tool::Padding] {
            crate::tools::toggle(tool).expect("the tool is offered");
            assert_eq!(crate::tools::active(), Some(tool));
            screen.drag(from, to);
            assert_eq!(
                stored_rect(&rig, LayerKind::Desktop, "centre"),
                before,
                "{tool:?} is up"
            );
            crate::tools::put_away();
        }

        screen.drag(from, to);
        assert_ne!(stored_rect(&rig, LayerKind::Desktop, "centre"), before);
    }
}
