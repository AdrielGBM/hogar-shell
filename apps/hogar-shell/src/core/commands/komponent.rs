//! `hogar-shell komponent` — the saved groups layouts draw from `components/<name>.toml` (TA-6).
//!
//! `list` and `show` read the files and answer in the CLI process, as `layout list` and `layout show` do. `save` and `detach` change the layout being drawn, so they go to the shell, which owns the store: each is one transaction, taken back by one `layout undo`. A saved komponent is a file of its own and outlives an undo of the edit that first used it.

use std::collections::{BTreeMap, BTreeSet};

use layout::components::{self, Candidate};
use layout::{
    AreaId, Expr, GroupId, KomponentId, Layout, LayoutStore, Library, NOMINAL_OUTPUT,
    ResolvedGroup, Site, Zone,
};

use super::args::{Args, Flag, arg, flag};
use super::layout::{catalogue, group_entry, inherited_group_named};
use super::{Command, Target};
use editor::komponent::{Placing, Use, plan_use};
use editor::written::{Written, as_written};
use layout::Catalogue;
use surfaces::layouts;

pub(crate) const KOMPONENT: Target = Target {
    name: "komponent",
    commands: &[
        Command {
            name: "list",
            args: "",
            help: "every komponent, its parameters and the layouts that draw it",
            run: |_| Ok(list()),
        },
        Command {
            name: "show",
            args: "<name>",
            help: "print a komponent as it is stored",
            run: |args| show(arg(args, 0, "name")?),
        },
        Command {
            name: "save",
            args: "<area.group> <name> [parameter...]",
            help: "save a group of the layout being drawn as a komponent and draw it from there, making the options and bindings named into parameters",
            run: save,
        },
        Command {
            name: "use",
            args: "<area>[.<group>] <name> [--zone start|center|end] [parameter=<expr>...]",
            help: "draw a komponent in a new group of an area, or in a group that holds nothing, setting the parameters named; on a bar the new group goes at the end of the zone named, the end zone by default",
            run: use_komponent,
        },
        Command {
            name: "detach",
            args: "<area.group>",
            help: "turn the komponent a group draws back into instances of its own",
            run: |args| detach(arg(args, 0, "area.group")?),
        },
    ],
};

/// The komponents on disk: each with its parameters and the layouts that draw it.
fn list() -> String {
    let (store, report) = LayoutStore::load(layouts::dir());
    let library = store.all();
    let mut lines: Vec<String> = library
        .komponents
        .iter()
        .map(|(id, komponent)| {
            let parameters: Vec<String> = komponent
                .parameters
                .iter()
                .map(|(name, parameter)| format!("{name}: {}", parameter.ty))
                .collect();
            let users: Vec<&str> = library
                .layouts
                .values()
                .filter(|layout| components::komponents_of(layout, library).contains(id))
                .map(|layout| layout.id.as_str())
                .collect();
            let mut line = id.to_string();
            if !parameters.is_empty() {
                line.push_str(&format!("\tparameters: {}", parameters.join(", ")));
            }
            if !users.is_empty() {
                line.push_str(&format!("\tdrawn by: {}", users.join(", ")));
            }
            line
        })
        .collect();
    if lines.is_empty() {
        lines.push(format!(
            "no komponents yet — `hogar-shell komponent save <area.group> <name>` saves one to {}",
            layouts::komponents_dir().display()
        ));
    }
    if !report.is_clean() {
        lines.push(report.render());
    }
    lines.join("\n")
}

fn show(name: &str) -> Result<String, String> {
    let (store, _) = LayoutStore::load(layouts::dir());
    let found = store
        .komponent(&KomponentId::new(name))
        .ok_or_else(|| no_komponent(store.all(), name))?;
    toml::to_string_pretty(found).map_err(|why| why.to_string())
}

fn no_komponent(library: &Library, name: &str) -> String {
    let named: Vec<&str> = library.komponents.keys().map(KomponentId::as_str).collect();
    match named.is_empty() {
        true => format!("there is no komponent called `{name}`"),
        false => format!(
            "there is no komponent called `{name}` (there is: {})",
            named.join(", ")
        ),
    }
}

/// Saves the group `target` names, as the layout being drawn arranges it, as the komponent `name`, with each of the values `args` names after it made a parameter, and makes the group draw it.
fn save(args: &Args<'_>) -> Result<String, String> {
    let target = arg(args, 0, "area.group")?.to_string();
    let name = arg(args, 1, "name")?;
    if !layout::is_komponent_name(name) {
        return Err(format!(
            "`{name}` cannot name a komponent file: use lowercase letters a–z, digits, `-` and `_`"
        ));
    }
    let id = KomponentId::new(name);
    let wanted: Vec<String> = args.iter().skip(2).map(|it| it.to_string()).collect();
    let (library, layout) =
        super::layout::stored(|store| (store.all().clone(), store.active().clone()))?;
    if library.komponent(&id).is_some() {
        return Err(format!("a komponent called `{id}` exists already"));
    }
    let base = layout::reset::base_of(&layout, &library);
    let (area, group) = inherited_group_named(&[&layout, &base], &target)?;
    let site = site_of(&layout, &base, &area)?;
    let drawn = drawn_group(&layout, &library, &site, (&area, &group))?;
    if let Some(used) = &drawn.komponent {
        return Err(format!(
            "`{area}.{group}` draws the komponent `{}` already",
            used.id
        ));
    }
    let catalogue = catalogue();
    let offered = components::candidates(&drawn, &|module, key| {
        catalogue.binding_type(module, key).ok()
    });
    let chosen = chosen(&offered, &wanted)?;
    let komponent = components::saved(&drawn, &chosen);
    let label = format!("Save `{area}.{group}` as `{id}`");
    let (said, uses) = (
        format!("saved `{area}.{group}` as `{id}`, which it draws from now on"),
        id.clone(),
    );
    layouts::with_komponent(
        id,
        komponent,
        |why| why.english(),
        move || {
            super::layout::edit_layout(&label, move |layout| {
                let written = Written::area(
                    layout,
                    Some(&site.output.0),
                    site.layer,
                    &area,
                    site.workspace.as_ref(),
                )
                .map_err(|why| why.english())?;
                let mut changed = written.area.clone();
                let entry = group_entry(&mut changed, &group);
                *entry = components::used(entry, &uses);
                Ok((written.ops(&changed), said))
            })
        },
    )
}

/// Makes the group `target` names draw the komponent `name`, with each `parameter=<expr>` after it set and, on a bar, at the end of the zone `--zone` names, as one transaction a `layout undo` takes back. `target` is an area, which gets a new group with a readable id made from the komponent's name, or `<area>.<group>`, which is that group where it holds nothing or a new group of that name; a group that holds modules is refused, since replacing them is destructive, as is one that draws a komponent already. An expression runs to the next `parameter=` word or `--zone`, so it may hold spaces.
fn use_komponent(args: &Args<'_>) -> Result<String, String> {
    let target = arg(args, 0, "area")?;
    let name = arg(args, 1, "name")?;
    let (area, group) = match target.split_once('.') {
        Some((area, group)) => (AreaId::new(area), Some(GroupId::new(group))),
        None => (AreaId::new(target), None),
    };
    let zone = flag(&args[2..], "--zone", "zone")?;
    let request = Use {
        komponent: KomponentId::new(name),
        parameters: parameters(args, 2, zone.as_ref())?,
    };
    let zone = zone.map(|zone| zone_named(zone.value)).transpose()?;
    let library = super::layout::stored(|store| store.all().clone())?;
    let label = format!("Use `{name}` in `{target}`");
    super::layout::edit_layout(&label, move |layout| {
        let base = layout::reset::base_of(layout, &library);
        let site = site_of(layout, &base, &area)?;
        let (resolved, _) = layout::resolve(layout, &library, &screen_of(&site), None);
        let desktop = surfaces::reconcile::desktops_now()
            .iter()
            .find(|held| {
                site.output
                    .matches(held.output.as_deref().unwrap_or_default())
            })
            .cloned();
        let (ops, group) = plan_use(
            layout,
            &library,
            &catalogue(),
            (&resolved, desktop.as_ref()),
            &Placing {
                layer: site.layer,
                area: &area,
                group: group.as_ref(),
                output: Some(&site.output.0),
                workspace: site.workspace.as_ref(),
                cell: None,
                near: (0, 0),
                zone,
            },
            &request,
        )
        .map_err(|why| why.english())?;
        Ok((
            ops,
            format!("`{area}.{group}` draws `{}`", request.komponent),
        ))
    })
}

/// The parameters the words of `args` from `from` on set, `among` (a flag found in those words, counted from `from`) and its value left out, each written `<name>=<expr>`: an expression takes the words up to the next one that starts a parameter or the flag, so `label='a b'` and `level=$battery.level + 5` are one each.
fn parameters(
    args: &Args<'_>,
    from: usize,
    among: Option<&Flag<'_>>,
) -> Result<BTreeMap<String, Expr>, String> {
    let end = args.len();
    let runs = match among {
        Some(flag) => [from..from + flag.at, from + flag.at + 2..end],
        None => [from..end, end..end],
    };
    let mut set = BTreeMap::new();
    for run in runs {
        parameters_in(args, run, &mut set)?;
    }
    Ok(set)
}

fn parameters_in(
    args: &Args<'_>,
    run: std::ops::Range<usize>,
    set: &mut BTreeMap<String, Expr>,
) -> Result<(), String> {
    let mut at = run.start;
    while at < run.end {
        if !starts_parameter(args[at]) {
            return Err(format!(
                "`{}` is not a parameter to set: write it as <name>=<expression>",
                args[at]
            ));
        }
        let end = (at + 1..run.end)
            .find(|next| starts_parameter(args[*next]))
            .unwrap_or(run.end);
        let written = args.text(at..end);
        let (name, text) = written
            .split_once('=')
            .expect("a word that starts a parameter has an `=`");
        if text.trim().is_empty() {
            return Err(format!("`{name}` needs an expression after the `=`"));
        }
        if set
            .insert(name.to_string(), Expr(text.to_string()))
            .is_some()
        {
            return Err(format!("`{name}` is set twice"));
        }
        at = end;
    }
    Ok(())
}

fn starts_parameter(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, rest)| !rest.starts_with('=') && telar_expression::is_identifier(name))
}

/// The zone `word` names, as `--zone` takes it.
fn zone_named(word: &str) -> Result<Zone, String> {
    match word {
        "start" => Ok(Zone::Start),
        "center" => Ok(Zone::Center),
        "end" => Ok(Zone::End),
        other => Err(format!(
            "`{other}` is not a zone of a bar: --zone takes start, center or end"
        )),
    }
}

/// The candidates `wanted` names, each by the name a parameter would have or the key it comes from; every one of them where `wanted` is empty is not what is meant, so nothing is.
fn chosen(offered: &[Candidate], wanted: &[String]) -> Result<Vec<Candidate>, String> {
    let mut chosen = Vec::new();
    for name in wanted {
        let found = offered
            .iter()
            .find(|candidate| candidate.name == *name || candidate.key == *name)
            .ok_or_else(|| {
                let names: BTreeSet<&str> = offered.iter().map(|it| it.name.as_str()).collect();
                format!(
                    "`{name}` is not a value of this group that can be a parameter (there is: {})",
                    names.into_iter().collect::<Vec<_>>().join(", ")
                )
            })?;
        chosen.push(found.clone());
    }
    Ok(chosen)
}

/// Detaches the komponent the group `target` names draws, where the layout being drawn names it.
fn detach(target: &str) -> Result<String, String> {
    let library = super::layout::stored(|store| store.all().clone())?;
    let label = format!("Detach `{target}`");
    let target = target.to_string();
    super::layout::edit_layout(&label, move |layout| {
        let (site, area, group) = using(layout, &target)?;
        let screen = screen_of(&site);
        let ops = components::detach(layout, &library, (&site, &area, &group), (&screen, None))
            .map_err(|why| why.message().english())?;
        Ok((ops, format!("detached the komponent `{target}` drew")))
    })
}

/// Where `layout`'s own rules make the group `target` names draw a komponent.
fn using(layout: &Layout, target: &str) -> Result<(Site, AreaId, GroupId), String> {
    let (area, group) = inherited_group_named(&[layout], target)?;
    let found: Vec<Site> = layout::ops::sites(layout)
        .filter(|(_, layer)| {
            layer.areas.iter().any(|held| {
                held.id == area
                    && held
                        .groups
                        .iter()
                        .any(|it| it.id == group && it.komponent.is_some())
            })
        })
        .map(|(site, _)| site)
        .collect();
    match found.as_slice() {
        [site] => Ok((site.clone(), area, group)),
        [] => Err(format!(
            "`{area}.{group}` draws no komponent this layout names; one a layout it extends names is detached there"
        )),
        several => Err(format!(
            "`{area}.{group}` is made to draw a komponent in several rules ({}); edit the file to say which",
            several
                .iter()
                .map(Site::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The screen a level is resolved on to read what it draws: the one it names, or every screen's nominal one for a pattern.
fn screen_of(site: &Site) -> String {
    match site.output.0.contains('*') {
        true => NOMINAL_OUTPUT.to_string(),
        false => site.output.0.clone(),
    }
}

/// The rule of `layout` — or, failing that, of `base` — that writes the area `area`.
fn site_of(layout: &Layout, base: &Layout, area: &AreaId) -> Result<Site, String> {
    layout::ops::site_of_area(layout, area)
        .or_else(|| layout::ops::site_of_area(base, area))
        .ok_or_else(|| format!("there is no area called `{area}`"))
}

/// The group `group` of `area` as the level `site` of `layout` draws it, every level merged, as its files write it ([`as_written`]): a line a bundle holds back is saved with the rest, and the store refuses what would make it the user's own.
fn drawn_group(
    layout: &Layout,
    library: &Library,
    site: &Site,
    (area, group): (&AreaId, &GroupId),
) -> Result<ResolvedGroup, String> {
    let (resolved, _) = layout::resolve(layout, &as_written(library), &screen_of(site), None);
    resolved
        .area(site.layer, area)
        .and_then(|held| held.groups.iter().find(|held| held.id == *group))
        .cloned()
        .ok_or_else(|| format!("`{area}.{group}` is not drawn, so there is nothing to save"))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout::{Expr, GroupId, KomponentId, LayoutId, LayoutStore};
    use surfaces::layouts;

    use crate::core::commands::dispatch;

    /// The shipped layout as `mine`, its clock written with an accent, installed as the running shell's store.
    fn shell(test: &str) -> Rc<RefCell<LayoutStore>> {
        shell_with(test, |_| {})
    }

    /// [`shell`], with `mine` changed by `edit` before it is stored.
    fn shell_with(test: &str, edit: impl FnOnce(&mut layout::Layout)) -> Rc<RefCell<LayoutStore>> {
        ui::descriptor::install(crate::core::modules::MODULES);
        let root = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("komponent-verbs-{test}"));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("layouts");
        std::fs::create_dir_all(&dir).expect("a layouts directory");
        let mut mine = layout::built_in();
        mine.id = LayoutId::new("mine");
        for group in &mut mine.outputs[0].layers.top.areas[0].groups {
            for child in &mut group.children {
                if child.id.as_str() == "clock" {
                    child.options.insert("accent".into(), "#88c0d0".into());
                }
            }
        }
        edit(&mut mine);
        std::fs::write(
            dir.join("mine.toml"),
            toml::to_string_pretty(&mine).expect("the layout serializes"),
        )
        .expect("a layout to edit");
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store.use_layout(&LayoutId::new("mine")).expect("mine");
        let store = Rc::new(RefCell::new(store));
        layouts::install(Rc::clone(&store), Rc::new(|| {}));
        store
    }

    fn center(store: &LayoutStore) -> layout::Group {
        store.active().outputs[0].layers.top.areas[0]
            .groups
            .iter()
            .find(|group| group.id == GroupId::new("center"))
            .cloned()
            .expect("the centre group is written")
    }

    /// The verbs end to end: `save` writes the komponent and makes the group draw it, `layout set` sets a parameter of the use — checked as the type the komponent declares — and `detach` writes the group's instances back, each a transaction one `layout undo` takes back.
    #[test]
    fn a_group_is_saved_set_and_detached_over_ipc() {
        let store = shell("round-trip");
        assert_eq!(
            dispatch("komponent save bar-top.center clock-pill accent"),
            "ok saved `bar-top.center` as `clock-pill`, which it draws from now on"
        );
        let id = KomponentId::new("clock-pill");
        let saved = store.borrow().komponent(&id).cloned().expect("it is saved");
        assert_eq!(saved.parameters["accent"].default, Expr("#88c0d0".into()));
        assert_eq!(center(&store.borrow()).komponent, Some(id.clone()));
        assert!(dispatch("komponent save bar-top.center clock-pill").starts_with("err "));

        assert_eq!(
            dispatch("layout set bar-top.center parameters.accent #ff0000"),
            "ok set `parameters.accent` on `bar-top.center`"
        );
        assert_eq!(
            center(&store.borrow()).parameters.get("accent"),
            Some(&Expr("#ff0000".into()))
        );
        let refused = dispatch("layout set bar-top.center parameters.accent 'red'");
        assert!(
            refused.starts_with("err ") && refused.contains('^'),
            "{refused}"
        );
        let refused = dispatch("layout set bar-top.center parameters.label 'x'");
        assert!(refused.contains("no parameter `label`"), "{refused}");

        assert_eq!(
            dispatch("komponent detach bar-top.center"),
            "ok detached the komponent `bar-top.center` drew"
        );
        let detached = center(&store.borrow());
        assert_eq!(detached.komponent, None);
        assert_eq!(detached.children.len(), 1);
        assert_eq!(
            detached.children[0].bindings.get("accent"),
            Some(&Expr("(#ff0000)".into()))
        );
        assert_eq!(
            dispatch("layout undo"),
            "ok took back `Detach `bar-top.center``"
        );
        assert_eq!(center(&store.borrow()).komponent, Some(id));
    }

    /// A group that draws a komponent takes a look of its own, but its arrangement and its children belong to the komponent's file: `layout set` refuses them and writes nothing.
    #[test]
    fn a_group_drawing_a_komponent_takes_a_look_but_not_an_arrangement_over_ipc() {
        let store = shell("komponent-arrange");
        dispatch("komponent save bar-top.center clock-pill accent");

        assert_eq!(
            dispatch("layout set bar-top.center style.shadow 2"),
            "ok set `style.shadow` on `bar-top.center`"
        );
        assert_eq!(center(&store.borrow()).style.shadow, Some(2));

        let before = store.borrow().active().clone();
        for line in [
            "layout set bar-top.center arrange pages",
            "layout set bar-top.center gap 4",
        ] {
            let refused = dispatch(line);
            assert!(
                refused.starts_with("err ") && refused.contains("draws the komponent `clock-pill`"),
                "{line}: {refused}"
            );
        }
        let refused = dispatch("layout set bar-top.center/clock style.fill surface");
        assert!(
            refused.starts_with("err ") && refused.contains("drawn by the komponent"),
            "{refused}"
        );
        assert_eq!(*store.borrow().active(), before);
    }

    fn bar_group(store: &LayoutStore, id: &str) -> Option<layout::Group> {
        store.active().outputs[0].layers.top.areas[0]
            .groups
            .iter()
            .find(|group| group.id.as_str() == id)
            .cloned()
    }

    /// `use` puts a komponent in a new group of an area under a readable id, in the end zone of a bar, with the parameters it names set as written — even where an expression has spaces — as one transaction a `layout undo` takes back.
    #[test]
    fn a_komponent_is_used_in_a_new_group_and_taken_back_by_one_undo() {
        let store = shell("use-new");
        dispatch("komponent save bar-top.center clock-pill accent");
        let before = store.borrow().active().clone();

        assert_eq!(
            dispatch("komponent use bar-top clock-pill accent=#ff0000"),
            "ok `bar-top.clock-pill` draws `clock-pill`"
        );
        let used = bar_group(&store.borrow(), "clock-pill").expect("a group of its own");
        assert_eq!(used.komponent, Some(KomponentId::new("clock-pill")));
        assert_eq!(used.parameters.get("accent"), Some(&Expr("#ff0000".into())));
        assert!(matches!(
            used.kind,
            Some(layout::GroupKind::Zone {
                zone: layout::Zone::End
            })
        ));
        assert!(used.children.is_empty());

        assert_eq!(
            dispatch("komponent use bar-top clock-pill"),
            "ok `bar-top.clock-pill-2` draws `clock-pill`",
            "a second use gets a group of its own"
        );
        assert_eq!(
            dispatch("layout undo"),
            "ok took back `Use `clock-pill` in `bar-top``"
        );
        assert_eq!(
            dispatch("layout undo"),
            "ok took back `Use `clock-pill` in `bar-top``"
        );
        assert_eq!(*store.borrow().active(), before, "one entry each");
    }

    /// A group that holds nothing is filled where it stands, keeping its place in the bar; the parameters' expressions run over spaces to the next `name=`.
    #[test]
    fn an_empty_group_is_filled_and_a_parameter_may_hold_spaces() {
        let store = shell("use-empty");
        dispatch("komponent save bar-top.center clock-pill accent");
        add_spare_group(&store);
        assert_eq!(
            dispatch("komponent use bar-top.spare clock-pill accent=if(1 < 2, #00ff00, #ff0000)"),
            "ok `bar-top.spare` draws `clock-pill`"
        );
        let used = bar_group(&store.borrow(), "spare").expect("the group");
        assert_eq!(used.komponent, Some(KomponentId::new("clock-pill")));
        assert_eq!(
            used.parameters.get("accent"),
            Some(&Expr("if(1 < 2, #00ff00, #ff0000)".into()))
        );
        assert!(matches!(
            used.kind,
            Some(layout::GroupKind::Zone {
                zone: layout::Zone::Start
            })
        ));
    }

    /// An empty group `spare` in the start zone of the bar, as one transaction.
    fn add_spare_group(store: &Rc<RefCell<LayoutStore>>) {
        let (id, bar, site) = {
            let held = store.borrow();
            let mut bar = held.active().outputs[0].layers.top.areas[0].clone();
            bar.groups.push(layout::Group {
                id: GroupId::new("spare"),
                kind: Some(layout::GroupKind::Zone {
                    zone: layout::Zone::Start,
                }),
                ..layout::Group::default()
            });
            let site =
                layout::ops::site_of_area(held.active(), &bar.id).expect("the bar is written");
            (held.active_id().clone(), bar, site)
        };
        let ops = vec![layout::LayoutOp::ReplaceArea {
            site,
            id: bar.id.clone(),
            area: Box::new(bar),
        }];
        store
            .borrow_mut()
            .commit(layout::Transaction::new("spare", id, ops))
            .expect("a spare group");
    }

    /// What `use` refuses, each with a message that says what to do instead, and nothing written.
    #[test]
    fn a_use_the_layout_would_not_accept_is_refused_with_nothing_written() {
        let store = shell("use-refused");
        dispatch("komponent save bar-top.center clock-pill accent");
        let before = store.borrow().active().clone();
        let refused = |line: &str| {
            let said = dispatch(line);
            assert!(said.starts_with("err "), "{line}: {said}");
            said
        };

        assert!(refused("komponent use bar-top nope").contains("no komponent called `nope`"));
        assert!(
            refused("komponent use bar-top clock-pill colour=#fff")
                .contains("no parameter `colour`")
        );
        let mistyped = refused("komponent use bar-top clock-pill accent='red'");
        assert!(
            mistyped.contains("accent") && mistyped.contains('^'),
            "{mistyped}"
        );
        assert!(refused("komponent use bar-top clock-pill accent").contains("<name>=<expression>"));
        assert!(
            refused("komponent use bar-top clock-pill accent=").contains("needs an expression")
        );
        assert!(
            refused("komponent use bar-top clock-pill accent=#fff accent=#000").contains("twice")
        );
        let occupied = refused("komponent use bar-top.end clock-pill");
        assert!(
            occupied.contains("holds modules of its own")
                && occupied.contains("take its modules out"),
            "{occupied}"
        );
        let drawing = refused("komponent use bar-top.center clock-pill");
        assert!(
            drawing.contains("draws the komponent `clock-pill` already")
                && drawing.contains("detach"),
            "{drawing}"
        );
        assert!(refused("komponent use background clock-pill").contains("holds no komponents"));
        assert!(refused("komponent use nowhere clock-pill").contains("no area called `nowhere`"));
        assert!(
            refused("komponent use widgets clock-pill").contains("depends on the screen"),
            "a grid needs a screen to find free cells on"
        );
        assert_eq!(*store.borrow().active(), before);
        assert_eq!(
            store.borrow().undo_label(),
            Some("Save `bar-top.center` as `clock-pill`")
        );
    }

    /// DEC-30 over IPC: a komponent a bundle brought may be used — the group names it, and its file stays the bundle's, held line by line — but detaching it would write its `shell run` line into the user's layout untrusted, so that is refused with what to do, nothing written, until the line is trusted.
    #[test]
    fn detaching_a_bundles_komponent_over_ipc_waits_for_trust_in_its_lines() {
        let store = shell("detach-held");
        let pill: layout::Komponent = toml::from_str(
            "[[children]]\nid = \"face\"\nmodule = \"clock\"\n[children.actions]\npress = [\"shell run date\"]\n",
        )
        .expect("the komponent parses");
        store
            .borrow_mut()
            .add_komponent(KomponentId::new("pill"), pill)
            .expect("the bundle's komponent");
        let mut trust = layout::Trust::default();
        trust.import("components/pill.toml", "nord");
        store.borrow_mut().set_trust(trust);

        assert!(dispatch("komponent use bar-top pill").starts_with("ok "));
        let before = store.borrow().active().clone();
        let refused = dispatch("komponent detach bar-top.pill");
        assert!(
            refused.starts_with("err ")
                && refused.contains("shell run date")
                && refused.contains("hogar-shell layout trust nord"),
            "{refused}"
        );
        assert_eq!(*store.borrow().active(), before, "nothing written");

        let item = layout::library_items(store.borrow().all())
            .into_iter()
            .find(|item| item.text == "shell run date")
            .expect("the bundle's line");
        let mut trust = store.borrow().all().trust.clone();
        trust.decide(&item, true);
        store.borrow_mut().set_trust(trust);
        assert!(dispatch("komponent detach bar-top.pill").starts_with("ok "));
    }

    /// DEC-30 over IPC: saving a group of a bundle's layout whose chip runs a held `shell run` line would make the line the user's, in a komponent file of their own — refused with what to do and nothing written, never saved without the line; once the line is trusted the komponent is saved with it.
    #[test]
    fn saving_a_group_with_a_held_line_waits_for_trust_and_never_drops_it() {
        const HELD: &str = "shell run date";
        let store = shell_with("save-held", |mine| {
            for group in &mut mine.outputs[0].layers.top.areas[0].groups {
                for child in &mut group.children {
                    if child.id.as_str() == "clock" {
                        child.actions.insert(
                            layout::Trigger::Press,
                            layout::Action(vec![HELD.to_string()]),
                        );
                    }
                }
            }
        });
        let mut trust = layout::Trust::default();
        trust.import("layouts/mine.toml", "nord");
        store.borrow_mut().set_trust(trust);
        let before = store.borrow().active().clone();

        let refused = dispatch("komponent save bar-top.center pill");
        assert!(
            refused.starts_with("err ")
                && refused.contains(HELD)
                && refused.contains("hogar-shell layout trust nord"),
            "{refused}"
        );
        assert!(
            store
                .borrow()
                .komponent(&KomponentId::new("pill"))
                .is_none()
        );
        assert_eq!(*store.borrow().active(), before, "nothing written");

        let item = layout::library_items(store.borrow().all())
            .into_iter()
            .find(|item| item.text == HELD)
            .expect("the bundle's line");
        let mut trust = store.borrow().all().trust.clone();
        trust.decide(&item, true);
        store.borrow_mut().set_trust(trust);
        assert!(dispatch("komponent save bar-top.center pill").starts_with("ok "));
        let saved = store
            .borrow()
            .komponent(&KomponentId::new("pill"))
            .cloned()
            .expect("it is saved");
        assert_eq!(
            saved.children[0].actions.get(&layout::Trigger::Press),
            Some(&layout::Action(vec![HELD.to_string()]))
        );
    }

    /// `--zone` picks the bar zone a new group goes at the end of, as the bar menu's zone submenu does, wherever it stands among the parameters — an expression stops at it — and is refused where there is no zone to pick: an unknown zone, an area that is not a bar, a group that exists already.
    #[test]
    fn use_puts_the_new_group_in_the_zone_named() {
        let store = shell("use-zone");
        dispatch("komponent save bar-top.center clock-pill accent");

        assert_eq!(
            dispatch("komponent use bar-top clock-pill --zone start"),
            "ok `bar-top.clock-pill` draws `clock-pill`"
        );
        let used = bar_group(&store.borrow(), "clock-pill").expect("a group of its own");
        assert!(matches!(
            used.kind,
            Some(layout::GroupKind::Zone {
                zone: layout::Zone::Start
            })
        ));

        assert_eq!(
            dispatch(
                "komponent use bar-top clock-pill accent=if(1 < 2, #00ff00, #ff0000) --zone center"
            ),
            "ok `bar-top.clock-pill-2` draws `clock-pill`"
        );
        let used = bar_group(&store.borrow(), "clock-pill-2").expect("a second group");
        assert!(matches!(
            used.kind,
            Some(layout::GroupKind::Zone {
                zone: layout::Zone::Center
            })
        ));
        assert_eq!(
            used.parameters.get("accent"),
            Some(&Expr("if(1 < 2, #00ff00, #ff0000)".into())),
            "the expression stops at the flag"
        );

        let before = store.borrow().active().clone();
        let refused = |line: &str| {
            let said = dispatch(line);
            assert!(said.starts_with("err "), "{line}: {said}");
            said
        };
        assert!(
            refused("komponent use bar-top clock-pill --zone middle")
                .contains("start, center or end")
        );
        assert!(refused("komponent use bar-top clock-pill --zone").contains("<zone>"));
        assert!(
            refused("komponent use widgets clock-pill --zone start").contains("has no zones"),
            "a grid has cells, not zones"
        );
        assert_eq!(*store.borrow().active(), before, "nothing written");

        add_spare_group(&store);
        let before = store.borrow().active().clone();
        assert!(
            refused("komponent use bar-top.spare clock-pill --zone end")
                .contains("keeps its place")
        );
        assert_eq!(*store.borrow().active(), before, "nothing written");
    }

    #[test]
    fn parameters_run_to_the_next_name() {
        let line = "bar-top pill label='a  b' level=$battery.level + 5 on=1 == 1";
        let args = super::Args::of(line);
        let set = super::parameters(&args, 2, None).expect("they parse");
        assert_eq!(set["label"], Expr("'a  b'".into()));
        assert_eq!(set["level"], Expr("$battery.level + 5".into()));
        assert_eq!(set["on"], Expr("1 == 1".into()));
    }
}
