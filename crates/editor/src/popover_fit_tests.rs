//! A popover's card sits wholly on the screen and scrolls its rows when they are taller than it may be: its last row is reached with the wheel and pressed, and a focus that moves down the rows takes the view with it.

#[cfg(test)]
mod tests {
    use telar::{
        ComponentList, Container, DrawCommand, Event, LayoutItem, LayoutStyle, NodeId, Paint, Rect,
        ScrollDelta, use_theme,
    };

    use config::theme::NordTheme;
    use layout::{Area, AreaId, Expr, LayerKind, Layout, OutputMatch, OutputRule};
    use surfaces::rects::{self, Node};
    use surfaces::transient;

    use crate::popover;
    use crate::rig::{SCREEN, rig_with};

    const SCREEN_WIDTH: f32 = 1920.0;
    const SCREEN_HEIGHT: f32 = 1080.0;

    struct Scope(telar::OwnerGuard);

    impl Scope {
        fn new() -> Self {
            Self(telar::owner_scope())
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    struct Screen {
        tree: ComponentList,
        node: NodeId,
    }

    impl Screen {
        fn of_popover() -> Self {
            let item = popover::tree()
                .expect("a popover is open")
                .expect("its tree builds");
            let page = LayoutStyle::new().width(SCREEN_WIDTH).height(SCREEN_HEIGHT);
            let root = Container::new(page, vec![item]).expect("a page");
            let node = root.layout_node();
            let screen = Self {
                tree: ComponentList::new(root),
                node,
            };
            screen.lay_out();
            screen
        }

        fn lay_out(&self) {
            crate::rig::lay_out(self.node, (SCREEN_WIDTH, SCREEN_HEIGHT));
        }

        fn route(&mut self, event: &Event) {
            telar::observe_keyboard(event);
            if !telar::dispatch_overlays(event) {
                self.tree.on_event(event);
            }
            self.lay_out();
        }

        fn click(&mut self, at: (f32, f32)) {
            for event in crate::rig::click_at(at) {
                self.route(&event);
            }
        }

        fn wheel(&mut self, (x, y): (f32, f32), pixels: f32) {
            self.route(&Event::Scrolled {
                delta: ScrollDelta::Pixels { x: 0.0, y: -pixels },
                x: x.into(),
                y: y.into(),
            });
        }

        fn texts(&self) -> Vec<(String, Rect)> {
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.tree.commands(), |command, matrix| {
                if let DrawCommand::Text { text, rect, .. } = command {
                    found.push((text.to_string(), on_screen(*rect, matrix)));
                }
            });
            found
        }

        fn card(&self) -> Rect {
            let surface = use_theme::<NordTheme>().surface;
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.tree.commands(), |command, matrix| {
                if let DrawCommand::Rect { rect, style } = command
                    && style.fill == Some(Paint::Solid(surface))
                {
                    found.push(on_screen(*rect, matrix));
                }
            });
            found
                .into_iter()
                .max_by(|a, b| a.height.total_cmp(&b.height))
                .expect("the card is drawn")
        }

        fn where_is(&self, wanted: &str) -> Rect {
            self.texts()
                .into_iter()
                .filter(|(text, _)| text == wanted)
                .map(|(_, rect)| rect)
                .next_back()
                .unwrap_or_else(|| panic!("{wanted:?} is drawn"))
        }
    }

    fn on_screen(rect: Rect, [a, b, c, d, e, f]: [f32; 6]) -> Rect {
        Rect::new(
            a * rect.x + c * rect.y + e,
            b * rect.x + d * rect.y + f,
            rect.width,
            rect.height,
        )
    }

    fn inside(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x + inner.width <= outer.x + outer.width
            && inner.y + inner.height <= outer.y + outer.height
    }

    fn bar() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
    }

    fn whole_screen() -> Rect {
        Rect::new(0.0, 0.0, SCREEN_WIDTH, SCREEN_HEIGHT)
    }

    fn inheriting_a_repeat(layout: &mut Layout) {
        let bar = layout.outputs[0]
            .layers
            .top
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "bar-top")
            .expect("the bar");
        bar.groups
            .iter_mut()
            .find(|group| group.id.as_str() == "end")
            .expect("its end")
            .repeat = Some(Expr("{1, 2}".into()));
        let mut screen = OutputRule {
            matches: OutputMatch(SCREEN.to_string()),
            ..OutputRule::default()
        };
        screen.layers.top.areas.push(Area {
            id: AreaId::new("bar-top"),
            ..Area::default()
        });
        layout.outputs.push(screen);
    }

    /// The bar's popover is taller than a card may be, so it scrolls: its last row is not where the card ends before the wheel turns, a press on the bar's own Remove at its foot lands once it has, and the card never leaves the screen.
    #[test]
    fn the_last_row_of_a_tall_popover_is_scrolled_to_and_pressed() {
        let rig = rig_with("fit-scroll", inheriting_a_repeat);
        let _scope = Scope::new();
        rects::track_spanning(
            bar(),
            vec![telar::signal(Rect::new(0.0, 0.0, SCREEN_WIDTH, 34.0))],
        );
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut screen = Screen::of_popover();

        let card = screen.card();
        assert!(
            inside(card, whole_screen()),
            "the card is on screen: {card:?}"
        );
        assert!(
            card.height <= SCREEN_HEIGHT * 0.7 + 1.0,
            "and no taller than it may be: {card:?}"
        );
        let remove = screen.where_is("Remove bar-top");
        assert!(
            !inside(remove, card),
            "the row is past the card's end until it is scrolled to: {remove:?} in {card:?}"
        );

        let over_rows = (card.x + card.width / 2.0, card.y + card.height / 2.0);
        for _ in 0..40 {
            screen.wheel(over_rows, 100.0);
        }
        let remove = screen.where_is("Remove bar-top");
        assert!(
            inside(remove, card),
            "scrolled to the end, it is in the card: {remove:?} in {card:?}"
        );
        screen.click((
            remove.x + remove.width / 2.0,
            remove.y + remove.height / 2.0,
        ));
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Remove bar-top"),
            "the press reached Remove"
        );
        assert!(
            surfaces::reconcile::desktops()[0]
                .resolved
                .area(LayerKind::Top, &AreaId::new("bar-top"))
                .is_none()
        );
    }

    /// Keyboard focus moving down the rows takes the view along, so the row it lands on is never behind the card's clip.
    #[test]
    fn focus_moving_down_the_rows_scrolls_them_into_view() {
        let _rig = rig_with("fit-focus", |_| {});
        let _scope = Scope::new();
        rects::track_spanning(
            bar(),
            vec![telar::signal(Rect::new(0.0, 0.0, SCREEN_WIDTH, 34.0))],
        );
        popover::open_area(bar()).expect("the bar's popover opens");
        let screen = Screen::of_popover();
        let card = screen.card();
        let last = screen.where_is("usable");
        assert!(!inside(last, card), "the last row starts out of view");

        for _ in 0..120 {
            telar::focus::focus_next();
            screen.lay_out();
            if inside(screen.where_is("usable"), card) {
                break;
            }
        }
        assert!(
            inside(screen.where_is("usable"), card),
            "walking the focus to the end brought the last row into the card"
        );
    }

    /// A popover for an item at the foot of the screen is placed wholly on it, above the item, and a screen too short for the rows takes a card as tall as it leaves.
    #[test]
    fn a_card_anchored_near_the_bottom_stays_on_screen() {
        use crate::popover::place::{GAP, card_at};
        let usable = Rect::new(0.0, 0.0, SCREEN_WIDTH, SCREEN_HEIGHT);
        let card = (360.0, 600.0);
        for item in [
            Rect::new(900.0, 1046.0, 120.0, 34.0),
            Rect::new(0.0, 1060.0, 60.0, 20.0),
            Rect::new(1900.0, 500.0, 20.0, 20.0),
        ] {
            let (x, y) = card_at(item, None, card, usable);
            let placed = Rect::new(x, y, card.0, card.1);
            assert!(
                inside(
                    placed,
                    Rect::new(
                        GAP,
                        GAP,
                        SCREEN_WIDTH - 2.0 * GAP,
                        SCREEN_HEIGHT - 2.0 * GAP
                    )
                ),
                "{item:?} is given {placed:?}"
            );
        }
    }
}
