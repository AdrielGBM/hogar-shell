//! DEC-26 in the popover: an expression a broader level writes — an area's `visible`, a group's `repeat`, an instance's binding — has a Remove that takes it back where the popover writes, pressed through the pointer, kept as one entry in the history and put back by one undo; an expression the popover's own level writes is still only deleted.

#[cfg(test)]
mod tests {
    use telar::{
        ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle, ModifiersState,
        NamedKey, NodeId, Rect,
    };

    use layout::{
        Area, AreaId, Expr, GroupId, InstanceId, LayerKind, Layout, OutputMatch, OutputRule, Unset,
    };
    use surfaces::rects::Node;
    use surfaces::transient;

    use crate::popover;
    use crate::rig::{Rig, SCREEN, rig_with};
    use crate::session;

    /// An owner for what a test builds, disposed when it ends.
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

    /// The open popover's tree, laid out over the whole screen as its window lays it out.
    struct Screen {
        tree: ComponentList,
        node: NodeId,
    }

    impl Screen {
        fn of_popover() -> Self {
            let item = popover::tree()
                .expect("a popover is open")
                .expect("its tree builds");
            let page = LayoutStyle::new().width(1920.0).height(1080.0);
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
            crate::rig::lay_out(self.node, (1920.0, 1080.0));
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

        fn texts(&self) -> Vec<(String, Rect)> {
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

        fn shows(&self, wanted: &str) -> bool {
            self.texts().iter().any(|(text, _)| text.contains(wanted))
        }

        fn count(&self, wanted: &str) -> usize {
            self.texts()
                .iter()
                .filter(|(text, _)| text.contains(wanted))
                .count()
        }

        /// Turns the wheel over the card until the text `wanted` is in the rows it shows.
        fn reveal(&mut self, wanted: &str) {
            let card = crate::rig::card_of(&self.tree);
            let over = (card.x + card.width / 2.0, card.y + card.height / 2.0);
            for _ in 0..80 {
                let rect = self
                    .texts()
                    .into_iter()
                    .find(|(text, _)| text == wanted)
                    .map(|(_, rect)| rect)
                    .unwrap_or_else(|| panic!("{wanted:?} is drawn"));
                let Some(pixels) = crate::rig::wheel_toward(card, rect) else {
                    return;
                };
                self.route(&crate::rig::wheel_at(over, pixels));
            }
        }

        /// A press on the first `wanted` drawn below the text `caption`, turning the wheel until it is in view.
        fn press_below(&mut self, caption: &str, wanted: &str) {
            self.reveal(caption);
            let card = crate::rig::card_of(&self.tree);
            let over = (card.x + card.width / 2.0, card.y + card.height / 2.0);
            let below = |screen: &Self| {
                let texts = screen.texts();
                let above = texts
                    .iter()
                    .find(|(text, _)| text == caption)
                    .map(|(_, rect)| rect.y)
                    .unwrap_or_else(|| panic!("{caption:?} is drawn: {texts:?}"));
                texts
                    .iter()
                    .filter(|(text, rect)| text == wanted && rect.y > above)
                    .map(|(_, rect)| *rect)
                    .min_by(|a, b| a.y.total_cmp(&b.y))
                    .unwrap_or_else(|| panic!("{wanted:?} is drawn below {caption:?}: {texts:?}"))
            };
            for _ in 0..80 {
                let Some(pixels) = crate::rig::wheel_toward(card, below(self)) else {
                    break;
                };
                self.route(&crate::rig::wheel_at(over, pixels));
            }
            let rect = below(self);
            self.click((rect.x + rect.width / 2.0, rect.y + rect.height / 2.0));
        }

        fn escape(&mut self) {
            self.route(&Event::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                modifiers: ModifiersState::default(),
            });
        }
    }

    const REMOVE: &str = "Remove";
    const INHERITED: &str = "From outputs.* in";
    const BEYOND: &str = "Overridden by outputs.*.workspaces.2 in";

    /// `mine` with `change` made to the area `id` of the `*` rule's `layer`, and a rule for the rig's screen that writes the area by its id alone — the narrowest level that writes it, where the popover writes, under which the `*` rule's expressions are inherited.
    fn inheriting(
        layer: LayerKind,
        id: &'static str,
        change: impl FnOnce(&mut Area),
    ) -> impl FnOnce(&mut Layout) {
        move |mine| {
            let area = mine.outputs[0]
                .layers
                .get_mut(layer)
                .areas
                .iter_mut()
                .find(|area| area.id.as_str() == id)
                .expect("the built-in layout has the area");
            change(area);
            let mut screen = OutputRule {
                matches: OutputMatch(SCREEN.to_string()),
                ..OutputRule::default()
            };
            screen.layers.get_mut(layer).areas.push(Area {
                id: AreaId::new(id),
                ..Area::default()
            });
            mine.outputs.push(screen);
        }
    }

    /// The area `id` of `layer` as the stored layout writes it in the rule for the rig's screen.
    fn written_for_screen(rig: &Rig, layer: LayerKind, id: &str) -> Area {
        let store = rig.store.borrow();
        store.active().outputs[1]
            .layers
            .get(layer)
            .areas
            .iter()
            .find(|area| area.id.as_str() == id)
            .cloned()
            .expect("the screen's rule writes the area")
    }

    fn stored(rig: &Rig) -> layout::Resolved {
        let store = rig.store.borrow();
        layout::resolve(store.active(), store.all(), SCREEN, None).0
    }

    fn widgets() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"))
    }

    fn bar() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
    }

    fn clock() -> Node {
        bar().instance(&GroupId::new("center"), &InstanceId::new("clock"))
    }

    fn visible(resolved: &layout::Resolved) -> Option<Expr> {
        resolved
            .area(LayerKind::Desktop, &AreaId::new("widgets"))
            .and_then(|area| area.visible.clone())
            .map(|visible| visible.expr)
    }

    /// An area's inherited `visible`: Remove writes `unset = ["visible"]` where the popover writes, previewed at once; a click outside keeps it as one entry and one undo puts the expression back.
    #[test]
    fn removing_an_inherited_visibility_takes_it_back_as_one_undo_entry() {
        let rig = rig_with(
            "unset-visible",
            inheriting(LayerKind::Desktop, "widgets", |area| {
                area.visible = Some(Expr("1 < 2".into()));
            }),
        );
        let _scope = Scope::new();
        popover::open_area(widgets()).expect("the grid's popover opens");
        let mut screen = Screen::of_popover();
        assert!(screen.shows(INHERITED), "the row says where it comes from");
        let said = screen.count(INHERITED);
        screen.press_below("Shown while", REMOVE);
        assert_eq!(
            screen.count(INHERITED),
            said - 1,
            "the line goes once the expression is taken back"
        );
        assert_eq!(
            visible(&surfaces::reconcile::desktops()[0].resolved),
            None,
            "taken back is previewed on the grid"
        );

        transient::close(popover::ID);
        assert_eq!(rig.undo_label().as_deref(), Some("Customize widgets"));
        let written = written_for_screen(&rig, LayerKind::Desktop, "widgets");
        assert_eq!(written.unset, [Unset::Visible]);
        assert_eq!(written.visible, None);
        assert_eq!(visible(&stored(&rig)), None);

        assert_eq!(session::undo().as_deref(), Ok("Customize widgets"));
        assert_eq!(rig.undo_label(), None, "it was one entry");
        assert_eq!(visible(&stored(&rig)), Some(Expr("1 < 2".into())));
        assert!(
            written_for_screen(&rig, LayerKind::Desktop, "widgets")
                .unset
                .is_empty()
        );
    }

    /// An inherited `visible` a workspace rule gives, while the popover writes for every workspace: the output rule it writes in comes before every workspace rule, so taking it back there would change nothing on screen. Remove is refused, saying which level gives it, and nothing is written.
    #[test]
    fn removing_what_a_later_level_gives_is_refused_where_it_would_change_nothing() {
        let rig = crate::rig::rig_on("unset-beyond", Some("2"), |mine| {
            let mut ruled = layout::WorkspaceRule {
                matches: layout::WorkspaceMatch("2".into()),
                ..layout::WorkspaceRule::default()
            };
            ruled.layers.desktop.areas.push(Area {
                id: AreaId::new("widgets"),
                visible: Some(Expr("1 < 2".into())),
                ..Area::default()
            });
            mine.outputs[0].workspaces.push(ruled);
        });
        let _scope = Scope::new();
        popover::open_area(widgets()).expect("the grid's popover opens");
        let mut screen = Screen::of_popover();
        assert!(screen.shows(BEYOND));
        screen.press_below("Shown while", REMOVE);
        let why = crate::mode::refusal().peek().expect("the strip says why");
        assert!(why.contains("outputs.*.workspaces.2"), "{why}");
        assert!(screen.shows(BEYOND), "the expression is still there");

        transient::close(popover::ID);
        assert_eq!(rig.undo_label(), None, "nothing was written");
    }

    /// Esc after Remove reverts the popover like any other change: nothing is written.
    #[test]
    fn escape_after_remove_writes_nothing() {
        let rig = rig_with(
            "unset-escape",
            inheriting(LayerKind::Desktop, "widgets", |area| {
                area.visible = Some(Expr("1 < 2".into()));
            }),
        );
        let _scope = Scope::new();
        popover::open_area(widgets()).expect("the grid's popover opens");
        let mut screen = Screen::of_popover();
        screen.press_below("Shown while", REMOVE);
        while popover::current().is_some() {
            screen.escape();
        }
        assert_eq!(rig.undo_label(), None);
        assert_eq!(visible(&stored(&rig)), Some(Expr("1 < 2".into())));
        assert!(
            written_for_screen(&rig, LayerKind::Desktop, "widgets")
                .unset
                .is_empty()
        );
    }

    /// A group's inherited `repeat`: Remove writes a partial entry for the group taking it back.
    #[test]
    fn removing_an_inherited_repeat_takes_it_back_in_a_partial_group() {
        let rig = rig_with(
            "unset-repeat",
            inheriting(LayerKind::Top, "bar-top", |area| {
                let start = area
                    .groups
                    .iter_mut()
                    .find(|group| group.id.as_str() == "start")
                    .expect("the bar's start");
                start.repeat = Some(Expr("{1, 2}".into()));
            }),
        );
        let _scope = Scope::new();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut screen = Screen::of_popover();
        screen.press_below("start: one copy per item", REMOVE);
        transient::close(popover::ID);

        let written = written_for_screen(&rig, LayerKind::Top, "bar-top");
        let start = written
            .groups
            .iter()
            .find(|group| group.id.as_str() == "start")
            .expect("a partial entry for the group");
        assert_eq!(start.unset, [Unset::Repeat]);
        assert_eq!(start.repeat, None);
        assert!(
            start.kind.is_none() && start.children.is_empty(),
            "and nothing else"
        );
        let repeat = |resolved: &layout::Resolved| {
            resolved
                .area(LayerKind::Top, &AreaId::new("bar-top"))
                .and_then(|bar| bar.groups.iter().find(|group| group.id.as_str() == "start"))
                .and_then(|group| group.repeat.clone())
                .map(|repeat| repeat.expr)
        };
        assert_eq!(repeat(&stored(&rig)), None);

        assert_eq!(session::undo().as_deref(), Ok("Customize bar-top"));
        assert_eq!(rig.undo_label(), None);
        assert_eq!(repeat(&stored(&rig)), Some(Expr("{1, 2}".into())));
    }

    fn accent(resolved: &layout::Resolved) -> Option<Expr> {
        resolved
            .instances()
            .find(|instance| instance.id.as_str() == "clock")
            .and_then(|instance| instance.bindings.get("accent").cloned())
            .map(|accent| accent.expr)
    }

    /// An instance's inherited binding: Remove writes `unset = ["bindings.accent"]` on the instance where the popover writes, as one undo entry.
    #[test]
    fn removing_an_inherited_binding_takes_it_back_as_one_undo_entry() {
        let rig = rig_with(
            "unset-binding",
            inheriting(LayerKind::Top, "bar-top", |area| {
                let clock = area
                    .groups
                    .iter_mut()
                    .flat_map(|group| group.children.iter_mut())
                    .find(|child| child.id.as_str() == "clock")
                    .expect("the bar's clock");
                clock
                    .bindings
                    .insert("accent".into(), Expr("#ff0000".into()));
            }),
        );
        let _scope = Scope::new();
        popover::open_instance(clock()).expect("the clock's popover opens");
        let mut screen = Screen::of_popover();
        assert!(screen.shows(INHERITED));
        screen.press_below("Driven by expressions", REMOVE);
        assert!(!screen.shows(INHERITED), "the row is gone");
        transient::close(popover::ID);

        assert_eq!(rig.undo_label().as_deref(), Some("Customize clock"));
        let written = written_for_screen(&rig, LayerKind::Top, "bar-top");
        let clock = written
            .groups
            .iter()
            .flat_map(|group| group.children.iter())
            .find(|child| child.id.as_str() == "clock")
            .expect("a partial entry for the clock");
        assert_eq!(clock.unset, [Unset::binding("accent")]);
        assert!(clock.bindings.is_empty());
        assert_eq!(accent(&stored(&rig)), None);

        assert_eq!(session::undo().as_deref(), Ok("Customize clock"));
        assert_eq!(rig.undo_label(), None, "it was one entry");
        assert_eq!(accent(&stored(&rig)), Some(Expr("#ff0000".into())));
    }

    /// A binding the popover's own level writes is deleted by Remove, with nothing taken back.
    #[test]
    fn removing_a_binding_of_its_own_only_deletes_it() {
        let rig = rig_with("unset-own", |mine| {
            let clock = mine.outputs[0]
                .layers
                .top
                .areas
                .iter_mut()
                .flat_map(|area| area.groups.iter_mut())
                .flat_map(|group| group.children.iter_mut())
                .find(|child| child.id.as_str() == "clock")
                .expect("the bar's clock");
            clock
                .bindings
                .insert("accent".into(), Expr("#ff0000".into()));
        });
        let _scope = Scope::new();
        popover::open_instance(clock()).expect("the clock's popover opens");
        let mut screen = Screen::of_popover();
        assert!(!screen.shows(INHERITED));
        screen.press_below("Driven by expressions", REMOVE);
        transient::close(popover::ID);

        let store = rig.store.borrow();
        let clock = store.active().outputs[0]
            .layers
            .top
            .areas
            .iter()
            .flat_map(|area| area.groups.iter())
            .flat_map(|group| group.children.iter())
            .find(|child| child.id.as_str() == "clock")
            .expect("the clock");
        assert!(clock.bindings.is_empty());
        assert!(clock.unset.is_empty(), "nothing to take back");
    }
}
