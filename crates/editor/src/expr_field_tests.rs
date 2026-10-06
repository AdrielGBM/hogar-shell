//! The expression field, driven as a person drives it — a press into the field, keys typed, Enter, Esc, a press on a completion, Ctrl+Z — over a battery reading the test feeds by hand. And the parts that decide what it offers and what it writes, on their own.

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use telar::{
        ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle, ModifiersState,
        NamedKey, NodeId, Paint, Rect, TextStyle, use_theme,
    };
    use telar_expression::{Reference, Span, Type, Value};

    use automation::{Environment, UserSources};
    use config::theme::NordTheme;
    use layout::{AreaId, Expr, GroupId, InstanceId, LayerKind};
    use surfaces::rects::Node;
    use surfaces::transient;
    use ui::descriptor::{
        Built, Category, ChipDef, FieldDef, FieldType, Input, ModuleDescriptor, Privacy,
        Representations, SourceDef,
    };
    use ui::host::Host;

    use crate::expr_field::{
        self, Candidate, Checked, Kind, Wanted, Word, complete, ranked, word_at,
    };
    use crate::keys::{self, Press};
    use crate::mode::{self};
    use crate::popover;
    use crate::popover::bindings::{Step, plan, removal};
    use crate::rig::{Rig, SCREEN, rig, rig_with};

    fn face(_: &Host) -> Built {
        Ok(Box::new(Container::new(
            LayoutStyle::new().width(20.0).height(20.0),
            Vec::new(),
        )?))
    }

    thread_local! {
        static LEVEL: Cell<f64> = const { Cell::new(50.0) };
        static SINKS: RefCell<Vec<ui::descriptor::Sink>> = const { RefCell::new(Vec::new()) };
    }

    fn feed(mut sink: ui::descriptor::Sink) {
        sink(Arc::from([Value::Number(LEVEL.with(Cell::get))]));
        SINKS.with(|sinks| sinks.borrow_mut().push(sink));
    }

    /// What the battery reads from now on, delivered to everything reading it.
    fn charge(level: f64) {
        LEVEL.with(|now| now.set(level));
        SINKS.with(|sinks| {
            for sink in sinks.borrow_mut().iter_mut() {
                sink(Arc::from([Value::Number(level)]));
            }
        });
    }

    static FIELDS: [FieldDef; 1] = [FieldDef {
        name: "level",
        privacy: Privacy::Public,
        ty: FieldType::Number,
    }];

    static SOURCES: [SourceDef; 1] = [SourceDef {
        id: "battery",
        fields: &FIELDS,
        feed,
    }];

    static PROBES: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "battery",
        name: "Battery",
        icon: "battery",
        category: Category::System,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(face, Input::ReadOnly)),
            ..Representations::NONE
        },
        actions: &[],
        sources: &SOURCES,
    }];

    /// An owner for what a test builds, disposed when it ends, with the battery installed and charged at 50.
    struct Scope(telar::OwnerGuard);

    impl Scope {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            LEVEL.with(|now| now.set(50.0));
            SINKS.with(|sinks| sinks.borrow_mut().clear());
            Self(telar::owner_scope())
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    const NONE: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: false,
        is_alt: false,
        is_meta: false,
    };

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

        /// Lays the popover out as the runner does each frame, until it settles: its card takes the height its rows were laid out at, which is known only once they are.
        fn lay_out(&self) {
            crate::rig::lay_out(self.node, (1920.0, 1080.0));
        }

        /// As the runner does: the keyboard's state first, then the overlays and the dismiss stack, then the tree.
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

        fn key(&mut self, key: Key) {
            self.route(&Event::KeyPressed {
                key,
                modifiers: NONE,
            });
        }

        fn named(&mut self, key: NamedKey) {
            self.key(Key::Named(key));
        }

        fn type_in(&mut self, text: &str) {
            for c in text.chars() {
                match c {
                    ' ' => self.named(NamedKey::Space),
                    c => self.key(Key::Char(c)),
                }
            }
        }

        /// Selects everything in the focused field and types `text` over it.
        fn retype(&mut self, text: &str) {
            self.route(&Event::KeyPressed {
                key: Key::Char('a'),
                modifiers: ModifiersState {
                    is_ctrl: true,
                    ..NONE
                },
            });
            self.type_in(text);
        }

        /// Every text drawn, where it is on screen, with the style it is drawn in.
        fn texts(&self) -> Vec<(String, Rect, TextStyle)> {
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.tree.commands(), |command, matrix| {
                if let DrawCommand::Text {
                    text, rect, style, ..
                } = command
                {
                    found.push((
                        text.to_string(),
                        on_screen(*rect, matrix),
                        (**style).clone(),
                    ));
                }
            });
            found
        }

        /// Every rectangle painted in `color`, where it is on screen.
        fn painted(&self, color: telar::Color) -> Vec<Rect> {
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.tree.commands(), |command, matrix| {
                if let DrawCommand::Rect { rect, style } = command
                    && style.fill == Some(Paint::Solid(color))
                {
                    found.push(on_screen(*rect, matrix));
                }
            });
            found
        }

        fn shows(&self, wanted: &str) -> bool {
            self.texts()
                .iter()
                .any(|(text, _, _)| text.contains(wanted))
        }

        fn where_is(&self, wanted: &str) -> (Rect, TextStyle) {
            self.texts()
                .into_iter()
                .find(|(text, _, _)| text == wanted)
                .map(|(_, rect, style)| (rect, style))
                .unwrap_or_else(|| {
                    let all: Vec<String> = self.texts().into_iter().map(|(t, _, _)| t).collect();
                    panic!("{wanted:?} is drawn: {all:?}")
                })
        }

        /// Turns the wheel over the card until the text `wanted` is in the rows it shows.
        fn reveal(&mut self, wanted: &str) {
            let card = crate::rig::card_of(&self.tree);
            let over = (card.x + card.width / 2.0, card.y + card.height / 2.0);
            for _ in 0..80 {
                let (rect, _) = self.where_is(wanted);
                let Some(pixels) = crate::rig::wheel_toward(card, rect) else {
                    return;
                };
                self.route(&crate::rig::wheel_at(over, pixels));
            }
        }

        /// A press on the first text `wanted` drawn below the text `caption`: one field among several that read the same.
        fn press_below(&mut self, caption: &str, wanted: &str) {
            self.reveal(caption);
            let (above, _) = self.where_is(caption);
            let rect = self
                .texts()
                .into_iter()
                .filter(|(text, rect, _)| text == wanted && rect.y > above.y)
                .map(|(_, rect, _)| rect)
                .min_by(|a, b| a.y.total_cmp(&b.y))
                .unwrap_or_else(|| panic!("{wanted:?} is drawn below {caption:?}"));
            self.click((rect.x + 20.0, rect.y + rect.height / 2.0));
        }

        /// A press in the middle of the text `wanted` is drawn in.
        fn press_on(&mut self, wanted: &str) {
            self.reveal(wanted);
            let (rect, _) = self.where_is(wanted);
            self.click((
                rect.x + rect.width.min(40.0) / 2.0,
                rect.y + rect.height / 2.0,
            ));
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

    const PLACEHOLDER: &str = "An expression, like $battery.level < 20";

    fn widgets() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"))
    }

    /// What the stored layout's desktop grid shows when.
    fn stored_visible(rig: &Rig) -> Option<Expr> {
        let store = rig.store.borrow();
        let (resolved, _) = layout::resolve(store.active(), store.all(), SCREEN, None);
        resolved
            .area(LayerKind::Desktop, &AreaId::new("widgets"))
            .and_then(|area| area.visible.clone())
            .map(|visible| visible.expr)
    }

    /// What the edit in progress shows the grid when, preview included.
    fn previewed_visible() -> Option<Expr> {
        surfaces::reconcile::desktops()[0]
            .resolved
            .area(LayerKind::Desktop, &AreaId::new("widgets"))
            .and_then(|area| area.visible.clone())
            .map(|visible| visible.expr)
    }

    fn opened_on_widgets() -> Screen {
        popover::open_area(widgets()).expect("the grid's popover opens");
        let mut screen = Screen::of_popover();
        screen.press_on(PLACEHOLDER);
        screen
    }

    /// The acceptance: `$battery.level < 20` says `false` and then `true` as the battery drains, live, and a typo is underlined where it is, with what is wrong — and is never written.
    #[test]
    fn a_condition_shows_its_value_live_and_a_typo_is_flagged_where_it_is() {
        let _rig = rig("expr-live");
        let _scope = Scope::new();
        let mut screen = opened_on_widgets();
        screen.type_in("$battery.level < 20");
        assert!(screen.shows("= false · bool"), "50 is not under 20");
        assert_eq!(
            previewed_visible(),
            Some(Expr("$battery.level < 20".into())),
            "what checks is previewed on the grid"
        );

        charge(12.0);
        screen.lay_out();
        assert!(
            screen.shows("= true · bool"),
            "and 12 is, as soon as it reads 12"
        );

        screen.retype("$batery.level < 20");
        assert!(
            screen.shows("nothing is called `$batery`"),
            "the typo says what is wrong"
        );
        let (field, style) = screen.where_is("$batery.level < 20");
        let error = use_theme::<NordTheme>().error;
        let underlined = screen.painted(error);
        let measure = |text: &str| telar::measure_text(text, None, 1.0e6, &style).0;
        let under = underlined
            .iter()
            .find(|rect| rect.height == 2.0)
            .expect("the typo is underlined");
        assert!(
            (under.x - field.x).abs() < 0.5,
            "from where the name starts"
        );
        assert!(
            (under.width - measure("$batery.level")).abs() < 0.5,
            "to where it ends, and not under ` < 20`: {under:?}"
        );
        assert_eq!(
            previewed_visible(),
            None,
            "what does not check is never written: the grid is back as the layout has it"
        );
    }

    /// F-7: Enter in the field commits the popover as one entry, and Ctrl+Z takes it back.
    #[test]
    fn enter_keeps_the_expression_as_one_entry_and_ctrl_z_takes_it_back() {
        let rig = rig("expr-enter");
        let _scope = Scope::new();
        let _host = crate::rig::open_mode(LayerKind::Desktop);
        let mut screen = opened_on_widgets();
        screen.type_in("$battery.level < 20");
        screen.named(NamedKey::Enter);
        assert_eq!(popover::current(), None, "Enter closed the popover");
        assert_eq!(rig.undo_label().as_deref(), Some("Customize widgets"));
        assert_eq!(
            stored_visible(&rig),
            Some(Expr("$battery.level < 20".into()))
        );

        let undo = Key::Char('z');
        let ctrl = ModifiersState {
            is_ctrl: true,
            ..NONE
        };
        let event = Event::KeyPressed {
            key: undo.clone(),
            modifiers: ctrl,
        };
        telar::observe_keyboard(&event);
        assert!(telar::dispatch_overlays(&event) || keys::press_as(&undo, ctrl, Press::First));
        assert_eq!(
            stored_visible(&rig),
            None,
            "one undo takes the whole edit back"
        );
        assert_eq!(rig.undo_label(), None);
    }

    /// F-7: the first Esc puts the field back as the popover opened it and leaves it; the second reverts the popover, which records nothing.
    #[test]
    fn escape_puts_the_field_back_and_then_reverts_the_popover() {
        let rig = rig("expr-escape");
        let _scope = Scope::new();
        let mut screen = opened_on_widgets();
        screen.type_in("$battery.level < 20");
        assert!(previewed_visible().is_some());

        screen.named(NamedKey::Escape);
        assert!(screen.shows(PLACEHOLDER), "the field is empty again");
        assert_eq!(previewed_visible(), None);
        assert!(
            popover::current().is_some(),
            "and the popover is still open"
        );

        screen.named(NamedKey::Escape);
        assert_eq!(popover::current(), None);
        assert_eq!(rig.undo_label(), None, "nothing was recorded");
        assert_eq!(stored_visible(&rig), None);
    }

    /// An expression left broken when the popover closes is not written: the popover keeps what the layout had.
    #[test]
    fn a_popover_closed_on_a_broken_expression_writes_nothing() {
        let rig = rig("expr-broken");
        let _scope = Scope::new();
        let mut screen = opened_on_widgets();
        screen.type_in("$battery.level <");
        assert!(screen.shows("expected"), "the field says what is missing");
        transient::close(popover::ID);
        assert_eq!(popover::current(), None);
        assert_eq!(rig.undo_label(), None);
        assert_eq!(stored_visible(&rig), None);
    }

    /// The completions for `$bat` offer the battery's reading with what it reads now; the arrow and Enter take one, and so does a press on it.
    #[test]
    fn a_completion_is_taken_with_the_keys_or_a_press() {
        let _rig = rig("expr-complete");
        let _scope = Scope::new();
        let mut screen = opened_on_widgets();
        screen.type_in("$bat");
        assert!(
            screen.shows("$battery.level  source"),
            "the reading is offered"
        );
        assert!(screen.shows("50"), "with what it reads now");
        screen.named(NamedKey::Enter);
        assert!(screen.shows("$battery.level"), "Enter took it");
        assert!(
            popover::current().is_some(),
            "rather than closing the popover"
        );
        assert!(
            !screen.shows("$battery.level  source"),
            "and closed the list"
        );

        screen.type_in(" < 20 && rou");
        screen.press_on("round(  function");
        assert!(
            screen.shows("$battery.level < 20 && round("),
            "a press took the function, ready for its argument"
        );
    }

    /// An instance's binding: a row added for its accent, an expression typed into it previewed on the instance, and kept by Enter.
    #[test]
    fn a_binding_added_to_an_instance_is_previewed_and_kept() {
        let rig = rig("expr-binding");
        let _scope = Scope::new();
        let clock = Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
            .instance(&GroupId::new("center"), &InstanceId::new("clock"));
        popover::open_instance(clock).expect("the clock's popover opens");
        let mut screen = Screen::of_popover();
        screen.press_on("Add binding");
        screen.press_on(PLACEHOLDER);
        screen.type_in("if($battery.level < 20, #ff0000, #00ff00)");
        assert!(screen.shows("= #00ff00 · colour"));
        let bound = popover::instance_draft()
            .expect("the clock's draft")
            .instance()
            .peek()
            .bindings;
        assert_eq!(
            bound.get("accent"),
            Some(&Expr("if($battery.level < 20, #ff0000, #00ff00)".into()))
        );
        screen.named(NamedKey::Enter);
        assert_eq!(rig.undo_label().as_deref(), Some("Customize clock"));
    }

    fn bar() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
    }

    /// What the stored layout repeats the bar's centre over.
    fn stored_repeat(rig: &Rig) -> Option<Expr> {
        let store = rig.store.borrow();
        let (resolved, _) = layout::resolve(store.active(), store.all(), SCREEN, None);
        resolved
            .area(LayerKind::Top, &AreaId::new("bar-top"))?
            .groups
            .iter()
            .find(|group| group.id.as_str() == "center")?
            .repeat
            .as_ref()
            .map(|repeat| repeat.expr.clone())
    }

    /// DEC-23: a bar's zone repeats its children over a list typed into its row, which refuses anything but a list; a grid's cells, whose footprint is fixed, have no such row.
    #[test]
    fn a_group_repeats_over_a_list_typed_into_its_row_and_never_on_a_cell() {
        let rig = rig("expr-repeat");
        let _scope = Scope::new();
        popover::open_area(widgets()).expect("the grid's popover opens");
        assert!(
            !Screen::of_popover().shows("one copy per item"),
            "a grid's cells do not repeat"
        );
        popover::close();

        popover::open_area(bar()).expect("the bar's popover opens");
        let mut screen = Screen::of_popover();
        let caption = "center: one copy per item";
        screen.press_below(caption, PLACEHOLDER);
        screen.type_in("1");
        assert!(
            screen.shows("expected a list to repeat"),
            "a number is no list"
        );
        screen.retype("{1, 2}");
        assert!(screen.shows("= 1, 2 · list of number"));
        screen.named(NamedKey::Enter);
        assert_eq!(stored_repeat(&rig), Some(Expr("{1, 2}".into())));
        assert_eq!(rig.undo_label().as_deref(), Some("Customize bar-top"));
    }

    /// DEC-23: a child of a repeated group is customized through any of its copies, and its bindings are checked and completed with what a copy reads of its own.
    #[test]
    fn a_copys_binding_reads_its_item_and_index() {
        let rig = rig_with("expr-copy", |layout| {
            let top = &mut layout.outputs[0].layers.top.areas;
            let bar = top
                .iter_mut()
                .find(|area| area.id.as_str() == "bar-top")
                .expect("the bar");
            let center = bar
                .groups
                .iter_mut()
                .find(|group| group.id.as_str() == "center")
                .expect("its centre");
            center.repeat = Some(Expr("{\"a\", \"b\"}".into()));
        });
        let _scope = Scope::new();
        let copy = bar().instance(&GroupId::new("center"), &InstanceId::new("clock#1"));
        popover::open_instance(copy).expect("a copy opens the child's popover");
        let mut screen = Screen::of_popover();
        screen.press_on("Add binding");
        screen.press_on(PLACEHOLDER);
        screen.type_in("$it");
        assert!(
            screen.shows("$item  this copy"),
            "the copy's item is offered"
        );
        screen.retype("if($item == \"a\" && $index == 0, #ff0000, #00ff00)");
        assert!(!screen.shows("nothing is called"), "and both read");
        assert!(
            screen.shows("`$item` has no reading: no copy of its group is drawn"),
            "what a copy reads is read where the screen draws it, which this rig does not"
        );
        screen.named(NamedKey::Enter);
        let store = rig.store.borrow();
        let (resolved, _) = layout::resolve(store.active(), store.all(), SCREEN, None);
        let clock = resolved
            .instances()
            .find(|instance| instance.id.as_str() == "clock")
            .expect("the child as written");
        assert!(clock.bindings.contains_key("accent"));
    }

    fn candidate(kind: Kind, name: &str) -> Candidate {
        Candidate {
            kind,
            name: name.to_string(),
            detail: String::new(),
            summary: String::new(),
            reference: None,
        }
    }

    fn names(ranked: Vec<&Candidate>) -> Vec<&str> {
        ranked
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect()
    }

    fn word(text: &str) -> Word {
        word_at(text, text.len()).expect("a word is being written")
    }

    /// A match from a name's start beats one from a dotted part, which beats one inside it; among equals, readings come before variables, events, functions and constants, then the shorter name.
    #[test]
    fn completions_rank_a_prefix_first_then_readings_before_functions() {
        let all = vec![
            candidate(Kind::Constant, "level_max"),
            candidate(Kind::Function, "len"),
            candidate(Kind::Var, "level_cap"),
            candidate(Kind::Source, "battery.level"),
            candidate(Kind::Event, "event.low_level"),
            candidate(Kind::Function, "sample"),
            candidate(Kind::Source, "level"),
        ];
        assert_eq!(
            names(ranked(&all, &word("le"))),
            [
                "level",
                "level_cap",
                "len",
                "level_max",
                "battery.level",
                "event.low_level",
                "sample"
            ]
        );
        assert_eq!(
            names(ranked(&all, &word("$le"))),
            ["level", "level_cap", "battery.level", "event.low_level"],
            "after `$` only readings"
        );
        assert_eq!(
            names(ranked(&all, &word("LEN"))),
            ["len"],
            "case is ignored"
        );
    }

    #[test]
    fn the_word_completed_is_the_name_at_the_caret_and_nothing_else() {
        let text = "$battery.level < 20";
        assert_eq!(
            word_at(text, 8),
            Some(Word {
                start: 0,
                end: 14,
                typed: "battery".to_string(),
                reading: true,
            })
        );
        assert_eq!(word_at(text, text.len()), None, "a number");
        assert_eq!(word_at("\"le", 3), None, "inside a text");
        assert_eq!(word_at("#ff", 3), None, "a colour");
        assert_eq!(word_at("a < ", 4), None, "nothing typed");
        assert_eq!(
            word_at("\"x\" + le", 8).map(|word| word.typed),
            Some("le".to_string()),
            "after a closed text"
        );
        assert!(word_at("$", 1).is_some_and(|word| word.reading && word.typed.is_empty()));
    }

    #[test]
    fn taking_a_completion_replaces_the_whole_name_and_reuses_a_bracket() {
        let reading = candidate(Kind::Source, "battery.level");
        let typed = "$bat.x < 20";
        let at = word_at(typed, 4).expect("a word");
        assert_eq!(
            complete(typed, &at, &reading),
            ("$battery.level < 20".to_string(), 14)
        );
        let function = candidate(Kind::Function, "round");
        let typed = "ro(1)";
        let at = word_at(typed, 2).expect("a word");
        assert_eq!(complete(typed, &at, &function), ("round(1)".to_string(), 6));
        let typed = "1 + ro";
        assert_eq!(
            complete(typed, &word(typed), &function),
            ("1 + round(".to_string(), 10)
        );
    }

    /// What the field offers comes from the environment: a module's readings as sources with their type, the shell's functions with their signatures.
    #[test]
    fn the_candidates_are_what_the_environment_names() {
        let env = Environment::new(SOURCES.iter(), UserSources::default());
        let all = expr_field::candidates(&env);
        let level = all
            .iter()
            .find(|candidate| candidate.name == "battery.level")
            .expect("the battery's reading");
        assert_eq!(level.kind, Kind::Source);
        assert_eq!(level.detail, "number");
        assert_eq!(level.reference, Some(Reference::new("battery", ["level"])));
        let round = all
            .iter()
            .find(|candidate| candidate.name == "round")
            .expect("a function");
        assert_eq!(round.kind, Kind::Function);
        assert!(round.detail.starts_with("round("), "{}", round.detail);
        assert!(all.iter().any(|candidate| candidate.kind == Kind::Constant));
        assert!(all.iter().any(|candidate| candidate.kind == Kind::Event));
        let locals = layout::Locals::of_copy(Type::Text);
        let copy = env.with_locals(automation::Local::typed_all(&locals));
        let all = expr_field::candidates(&copy);
        let item = all
            .iter()
            .find(|candidate| candidate.name == "item")
            .expect("a copy's item");
        assert_eq!((item.kind, item.detail.as_str()), (Kind::Local, "text"));
        let typed = word("$i");
        assert_eq!(
            names(ranked(&all, &typed))[..2],
            ["item", "index"],
            "a copy's own names come first"
        );
    }

    /// A typo is refused with the span of the name it is about, and a condition of the wrong type over all of it.
    #[test]
    fn a_mistake_is_located() {
        let env = Environment::new(SOURCES.iter(), UserSources::default());
        let Checked::Invalid(errors) =
            expr_field::check(&env, "$batery.level < 20", &Wanted::Exactly(Type::Bool))
        else {
            panic!("a typo does not check");
        };
        assert_eq!(errors[0].span, Span::new(0, 13));
        let Checked::Invalid(errors) =
            expr_field::check(&env, "$battery.level", &Wanted::Exactly(Type::Bool))
        else {
            panic!("a number is not a condition");
        };
        assert_eq!(errors[0].span, Span::new(0, 14));
        assert!(matches!(
            expr_field::check(&env, "  ", &Wanted::Exactly(Type::Bool)),
            Checked::Empty
        ));
        assert!(matches!(
            expr_field::check(&env, "{1, 2}", &Wanted::AnyList),
            Checked::Valid(_)
        ));
        let Checked::Invalid(errors) = expr_field::check(&env, "1", &Wanted::AnyList) else {
            panic!("a number is no list to repeat over");
        };
        assert_eq!(errors[0].span, Span::new(0, 1));
    }

    /// A variable the shell has not set yet is waited for rather than refused — the field writes it, as `layout check` only warns of it — and checks as its type once it is set.
    #[test]
    fn a_variable_not_set_yet_is_written_and_checks_once_it_is_set() {
        automation::vars::remove("expr_test_later");
        let env = Environment::new(SOURCES.iter(), UserSources::default());
        let wanted = Wanted::Exactly(Type::Bool);
        let Checked::Awaiting(errors) = expr_field::check(&env, "$expr_test_later", &wanted) else {
            panic!("a variable not set yet is waited for");
        };
        assert_eq!(errors[0].span, Span::new(0, 16));
        assert!(matches!(
            expr_field::check(&env, "$expr_test_later && $batery.level", &wanted),
            Checked::Invalid(_)
        ));
        automation::vars::set("expr_test_later", services::state::Var::Bool(true)).unwrap();
        assert!(matches!(
            expr_field::check(&env, "$expr_test_later", &wanted),
            Checked::Valid(_)
        ));
        automation::vars::remove("expr_test_later");
    }

    /// TA-8: what a lock-layer popover checks and shows is what the lock screen may read, and every other layer reads as the signed-in user.
    #[test]
    fn a_lock_layer_field_reads_what_the_lock_screen_may_show() {
        let _scope = Scope::new();
        assert_eq!(
            expr_field::environment(LayerKind::Lock).audience(),
            ui::host::Audience::Anyone
        );
        for layer in LayerKind::SESSION {
            assert_eq!(
                expr_field::environment(layer).audience(),
                ui::host::Audience::Owner
            );
        }
    }

    fn touched(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    fn bind(path: &str, text: &str) -> Step {
        Step::Bind(path.to_string(), text.to_string())
    }

    fn keep(path: &str) -> Step {
        Step::Keep(path.to_string())
    }

    fn unbind(path: &str) -> Step {
        Step::Unbind(path.to_string())
    }

    /// A row writes what checks at its target, takes itself off where it started when it moves, and asks for nothing at all when it does not check or is back as it was.
    #[test]
    fn a_binding_row_writes_only_what_checks() {
        let started = ("accent".to_string(), "#f00".to_string());
        assert_eq!(
            plan(
                Some(&started),
                &touched(&["accent"]),
                "accent",
                Some("#0f0")
            ),
            [bind("accent", "#0f0")]
        );
        assert_eq!(
            plan(Some(&started), &touched(&["accent"]), "accent", None),
            [keep("accent")],
            "a broken expression keeps what the layout writes"
        );
        assert_eq!(
            plan(
                Some(&started),
                &touched(&["accent"]),
                "accent",
                Some("#f00")
            ),
            [keep("accent")],
            "back as it was is no change"
        );
        assert_eq!(
            plan(
                Some(&started),
                &touched(&["accent", "show_date"]),
                "show_date",
                Some("true")
            ),
            [unbind("accent"), bind("show_date", "true")],
            "moved to another option"
        );
        assert_eq!(
            plan(Some(&started), &touched(&["accent"]), "accent", Some("")),
            [unbind("accent")],
            "emptied"
        );
        assert_eq!(
            plan(
                None,
                &touched(&["accent", "show_date"]),
                "show_date",
                Some("")
            ),
            [keep("accent"), keep("show_date")],
            "a new row left empty asks for nothing"
        );
        assert_eq!(
            removal(Some(&started), &touched(&["accent", "show_date"])),
            [unbind("accent"), keep("show_date")]
        );
    }

    #[test]
    fn a_long_text_value_is_shown_quoted_and_cut_short() {
        let long = expr_field::shown(&Value::text("x".repeat(60)));
        assert_eq!(long, format!("\"{}…\"", "x".repeat(39)));
        assert_eq!(expr_field::shown(&Value::text("short")), "\"short\"");
        assert_eq!(expr_field::shown(&Value::Number(2.5)), "2.5");
    }

    /// DEC-27: the field speaks the shell's language. What is wrong with an expression, and the type of what it gives, are in Spanish in a Spanish session, and the mistake stays underlined where it is.
    #[test]
    fn a_mistake_is_said_in_the_language_the_shell_speaks() {
        let _rig = rig("expr-spanish");
        let _scope = Scope::new();
        let mut screen = opened_on_widgets();
        telar::set_locale("es");
        screen.retype("$batery.level < 20");
        screen.lay_out();
        let spanish = screen.shows("nada se llama `$batery`");
        let underlined = !screen.painted(use_theme::<NordTheme>().error).is_empty();
        screen.retype("$battery.level < 20");
        screen.lay_out();
        let typed = screen.shows("= false · booleano");
        telar::set_locale("en");
        assert!(spanish, "the typo says what is wrong, in Spanish");
        assert!(underlined, "and is underlined as in any language");
        assert!(typed, "the type of what it gives is named in Spanish too");
    }

    #[test]
    fn a_checked_mistake_carries_what_it_says_rather_than_words() {
        let env = Environment::new(SOURCES.iter(), UserSources::default());
        let Checked::Invalid(errors) =
            expr_field::check(&env, "$batery.level < 20", &Wanted::Exactly(Type::Bool))
        else {
            panic!("a typo does not check");
        };
        assert_eq!(errors[0].message.key(), Some("expression.nothing_called"));
        assert_eq!(errors[0].message.english(), "nothing is called `$batery`");
        assert_eq!(errors[0].message.render_in("es"), "nada se llama `$batery`");
        let Checked::Invalid(errors) = expr_field::check(&env, "1", &Wanted::AnyList) else {
            panic!("a number is no list to repeat over");
        };
        assert_eq!(
            errors[0].message.render_in("es"),
            "se esperaba una lista sobre la que repetir los hijos del grupo, pero esto da número"
        );
    }
}
