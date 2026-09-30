//! `hogar-shell layout` — reading the layouts on disk, choosing which one the shell draws, and editing it.
//!
//! `list`, `show` and `check` answer in the CLI process, because a layout that stops the shell from starting is exactly the one a user needs to be able to look at (F-10.5). Everything else goes to the shell, which is what owns the store: `use` writes the active name to machine state and asks for a reload, and the verbs that change a layout commit a transaction against the store the shell is drawing from (`surfaces::layouts`).
//!
//! **Every edit here is one transaction**, so `layout undo` takes back one command whatever else made the edit before it — a gesture, a popover or another line of this. The layout an edit lands in is the one being drawn, except that the first edit to the built-in layout forks it: the shipped one is read-only so that a user who has broken theirs always has one that works.
//!
//! **What a verb refuses is as much the point as what it does.** A module nothing answers to, a representation it cannot be drawn as, an area that holds no instances, a control placed on the lock layer: each is a message naming what there is instead of an edit that draws a placeholder the user then has to find.

use std::collections::BTreeSet;

use layout::ops::{areas_at, placement_of, site_of_area, sites};
use layout::reset::Target as Aim;
use layout::{
    Action, Area, AreaId, AreaKind, BUILT_IN, Catalogue, Group, GroupId, Instance, InstanceId,
    LayerKind, Layout, LayoutId, LayoutOp, LayoutStore, NOMINAL_OUTPUT, Representation, Site, Spot,
    Trigger,
};

use super::args::arg;
use super::{Command, Target};
use surfaces::layouts;

pub(crate) const LAYOUT: Target = Target {
    name: "layout",
    commands: &[
        Command {
            name: "list",
            args: "",
            help: "every layout this shell can use",
            run: |_| Ok(list()),
        },
        Command {
            name: "show",
            args: "[name]",
            help: "print a layout as it is stored",
            run: |args| show(args.first().copied()),
        },
        Command {
            name: "check",
            args: "[name]",
            help: "report what is wrong with a layout, without applying it",
            run: |args| check(args.first().copied()),
        },
        Command {
            name: "use",
            args: "<name>",
            help: "draw this layout from now on",
            run: |args| use_layout(args.first().copied()),
        },
        Command {
            name: "undo",
            args: "",
            help: "take back the last edit, whatever made it",
            run: |_| undo(),
        },
        Command {
            name: "redo",
            args: "",
            help: "make the edit that was last taken back again",
            run: |_| redo(),
        },
        Command {
            name: "add",
            args: "<module> <area> [group]",
            help: "place a module in an area of the layout being drawn",
            run: add,
        },
        Command {
            name: "remove",
            args: "<id>",
            help: "take a placed module, or a whole area, out of the layout",
            run: |args| remove(arg(args, 0, "id")?),
        },
        Command {
            name: "move",
            args: "<id> <group> [index]",
            help: "put a placed module in another group, or elsewhere in its own",
            run: move_instance,
        },
        Command {
            name: "set",
            args: "<instance> <key> <value>",
            help: "change one property of a placed module",
            run: set,
        },
        Command {
            name: "reset",
            args: "<id|layer|all>",
            help: "put a part of the layout back to what the layout it extends says, or the built-in one",
            run: |args| reset(arg(args, 0, "id|layer|all")?),
        },
        Command {
            name: "edit",
            args: "<background|desktop|top|overlay|lock|off> [output]",
            help: "edit one layer on one screen (the focused one unless named), or stop",
            run: edit,
        },
    ],
};

/// Switches the edit mode of one layer on, on the screen named or the focused one, or switches whichever is up off. One mode at a time (DEC-2): entering one leaves the last. Lock mode is a preview, refused while the session is locked (TA-8), and every mode is refused under `--safe-layout` (F-10.35).
fn edit(args: &[&str]) -> Result<String, String> {
    let which = arg(args, 0, "background|desktop|top|overlay|lock|off")?;
    if which == "off" {
        return Ok(match editor::mode::leave() {
            Some(left) => format!("stopped editing {} on {}", left.layer, left.output),
            None => "nothing was being edited".to_string(),
        });
    }
    let layer = LayerKind::from_name(which).ok_or_else(|| {
        format!("'{which}' is not a layer (try: background, desktop, top, overlay, lock, off)")
    })?;
    let mode = editor::mode::enter(layer, args.get(1).copied())?;
    Ok(match mode.refused {
        Some(why) => format!("previewing {} on {}: {why}", mode.layer, mode.output),
        None => format!("editing {} on {}", mode.layer, mode.output),
    })
}

/// Makes `name` the layout the shell draws, and asks whatever is running to pick it up.
///
/// Which layout is active is machine state, not a config key (TA-2): it is a choice about this installation rather than a description of one. So this writes `state.json` and reloads, and the running shell reads the name back on the next pass — one direction, no second copy of the answer in the shell's memory to fall out of step.
fn use_layout(name: Option<&str>) -> Result<String, String> {
    let name = name.filter(|name| !name.trim().is_empty()).ok_or_else(|| {
        format!(
            "say which layout to use — `hogar-shell layout list` names them\n{}",
            list()
        )
    })?;
    let (store, _) = LayoutStore::load(layouts::dir());
    let id = LayoutId::new(name);
    if store.get(&id).is_none() {
        return Err(format!("there is no layout called `{name}`\n{}", list()));
    }
    services::state::update(|state| state.layout = Some(name.to_string()));
    config::request_reload();
    Ok(format!("drawing `{name}`"))
}

/// The layouts on disk, with the built-in one and the one being drawn marked.
///
/// These three read files and answer from them, so they run in the CLI process the way `config check` does: a layout that stops the shell from starting is exactly the one a user needs to be able to look at. Which one is active comes from `state.json` for the same reason — asking the shell would fail in precisely the case this is for.
fn list() -> String {
    let (store, report) = LayoutStore::load(layouts::dir());
    let active = services::state::get().layout.unwrap_or_default();
    let mut lines: Vec<String> = store
        .names()
        .map(|id| {
            let mut notes = Vec::new();
            if id.as_str() == BUILT_IN {
                notes.push("built-in, read-only");
            }
            if id.as_str() == active || (active.is_empty() && id.as_str() == BUILT_IN) {
                notes.push("drawing");
            }
            match notes.is_empty() {
                true => id.to_string(),
                false => format!("{id}\t{}", notes.join(", ")),
            }
        })
        .collect();
    if !report.is_clean() {
        lines.push(report.render());
    }
    lines.join("\n")
}

fn show(name: Option<&str>) -> Result<String, String> {
    let (store, _) = LayoutStore::load(layouts::dir());
    let id = LayoutId::new(name.unwrap_or(BUILT_IN));
    let found = store
        .get(&id)
        .ok_or_else(|| format!("there is no layout called '{id}'"))?;
    toml::to_string_pretty(found).map_err(|why| why.to_string())
}

/// Parses, validates and resolves one layout, and says what is wrong with it.
///
/// Resolution is per output, and which outputs exist is a question only a running compositor answers, so this resolves against one nominal output. That catches everything that does not depend on a monitor's name — a missing prompt, an area with no kind, an instance with no module — and leaves the per-monitor half to the running shell's own notice.
fn check(name: Option<&str>) -> Result<String, String> {
    let (store, mut report) = LayoutStore::load(layouts::dir());
    let id = LayoutId::new(name.unwrap_or(BUILT_IN));
    let found = store
        .get(&id)
        .ok_or_else(|| format!("there is no layout called '{id}'"))?;

    let path = store.path_of(&id);
    if let Ok(text) = std::fs::read_to_string(&path) {
        report.merge(layout::check_unknown_keys(&text, &id));
    }
    report.merge(layout::validate(found, &catalogue()));

    let (resolved, resolving) = layout::resolve(found, store.all(), NOMINAL_OUTPUT, None);
    report.merge(resolving);
    report.merge(layout::validate_resolved(
        &resolved,
        &path.display().to_string(),
        &lock_theme(),
    ));
    report.merge(scanout(&resolved, &path.display().to_string()));

    match report.is_clean() {
        true => Ok(report.summary()),
        false => Err(report.render()),
    }
}

/// What each area above fullscreen costs its output, said once per area (DEC-17): it lives in the overlay window, and an output with an overlay surface cannot scan a fullscreen window out directly (F-6.1). A warning rather than an error, because it is a cost the user chose.
///
/// Resolved against [`NOMINAL_OUTPUT`] it is every output a `*` rule reaches; the running shell says it again for each output it draws.
pub(crate) fn scanout(resolved: &layout::Resolved, file: &str) -> util::report::Report {
    let mut report = util::report::Report::default();
    let output = match resolved.output.as_str() {
        NOMINAL_OUTPUT => telar::t!("layout.every_output"),
        named => named.to_string(),
    };
    for (layer, area) in resolved.areas().filter(|(_, area)| area.above_fullscreen) {
        report.warn(util::report::Finding::new(
            file,
            match resolved.output.as_str() {
                NOMINAL_OUTPUT => format!("layers.{layer}.areas.{}.above_fullscreen", area.id),
                named => format!(
                    "layers.{layer} on {named}.areas.{}.above_fullscreen",
                    area.id
                ),
            },
            telar::t!("layout.scanout", area = area.id, output = output),
        ));
    }
    report
}

/// The theme the lock screen would be drawn with, which is what decides whether its prompt can be read. The running config where there is one, and the file on disk where this answers in the CLI.
pub(crate) fn lock_theme() -> config::theme::NordTheme {
    config::config()
        .unwrap_or_else(|| {
            std::sync::Arc::new(config::Config::load_or_default(
                &config::Config::default_path(),
            ))
        })
        .resolve_theme()
}

fn undo() -> Result<String, String> {
    layouts::undo().map(|label| format!("took back `{label}`"))
}

fn redo() -> Result<String, String> {
    layouts::redo().map(|label| format!("made `{label}` again"))
}

/// Places a module in an area, at the end of one of its groups.
///
/// The representation is not asked for: an area says how it holds things — a bar holds chips, a grid holds widgets — so the size is the first one the area can hold that the module declares. A module that declares none of them is refused by name, which is the answer for `user` on a bar: it is a reading with no chip.
fn add(args: &[&str]) -> Result<String, String> {
    let module = arg(args, 0, "module")?.to_string();
    let area = AreaId::new(arg(args, 1, "area")?);
    let group = args.get(2).map(GroupId::new);

    known_module(&module)?;
    let label = format!("Add `{module}`");
    layouts::edit(&label, move |layout, _| {
        let at = area_in(layout, &area)?;
        let found = area_of(layout, &at, &area)?;
        if !found.kind.as_ref().is_some_and(holds_instances) {
            return Err(format!(
                "`{area}` is {}, which holds no modules",
                describe(found.kind.as_ref())
            ));
        }
        let group = match group {
            Some(named) => named,
            None => first_group(found, &area)?,
        };
        let spot = Spot {
            site: at.clone(),
            area: area.clone(),
            group: group.clone(),
        };
        let landing = group_of(found, &group)
            .ok_or_else(|| format!("`{area}` has no group called `{group}`{}", groups_of(found)))?;
        let representation = fits(found.kind.as_ref(), &module).ok_or_else(|| {
            format!(
                "`{module}` cannot be drawn in {}",
                describe(found.kind.as_ref())
            )
        })?;
        allowed_on(at.layer, &module, representation)?;
        let id = free_instance_id(layout, &module);

        Ok((
            vec![LayoutOp::InsertInstance {
                spot,
                index: landing.children.len(),
                instance: Box::new(Instance {
                    id: id.clone(),
                    module: Some(module.clone()),
                    representation: Some(representation),
                    ..Instance::default()
                }),
            }],
            format!("added `{id}` to `{group}` as `{}`", representation.as_str()),
        ))
    })
}

/// Takes a placed module out of the layout, or the whole area when the id names one.
///
/// Both, because both are addressed by id and a user who asks to remove `bar-top` means the bar. Which it was is in the reply, so an id that happens to name both kinds of thing does not act silently on the wrong one. The lock screen's prompt is the one area it refuses, since a lock with nothing to type a password into is a lockout (TA-8).
fn remove(id: &str) -> Result<String, String> {
    let instance = InstanceId::new(id);
    let area = AreaId::new(id);
    let label = format!("Remove `{id}`");
    let id = id.to_string();
    layouts::edit(&label, move |layout, _| {
        if let Some(at) = placement_of(layout, &instance) {
            return Ok((
                vec![LayoutOp::DeleteInstance {
                    spot: at.spot,
                    id: instance.clone(),
                }],
                format!("removed the module `{instance}`"),
            ));
        }
        if let Ok(site) = area_in(layout, &area) {
            return Ok((
                vec![LayoutOp::DeleteArea {
                    site,
                    id: area.clone(),
                }],
                format!("removed the area `{area}`"),
            ));
        }
        Err(nothing_called(layout, &id))
    })
}

/// Puts a placed module in another group, or at another position in its own.
///
/// The group is named `<group>` where that is unambiguous and `<area>.<group>` where it is not: a group id is unique inside its area, so two bars can both have an `end`.
fn move_instance(args: &[&str]) -> Result<String, String> {
    let instance = InstanceId::new(arg(args, 0, "id")?);
    let group = arg(args, 1, "group")?.to_string();
    let index: Option<usize> = match args.get(2) {
        Some(raw) => Some(
            raw.parse()
                .map_err(|_| format!("<index> must be a position, got '{raw}'"))?,
        ),
        None => None,
    };

    let label = format!("Move `{instance}`");
    layouts::edit(&label, move |layout, _| {
        let from = placement_of(layout, &instance)
            .ok_or_else(|| nothing_called(layout, instance.as_str()))?;
        let to = group_named(layout, &group)?;
        let landing = spot_of(layout, &to)?;
        let last = match to == from.spot {
            // Its own group is one shorter once it leaves, so the end of that group is one less than the end of any other.
            true => landing.children.len().saturating_sub(1),
            false => landing.children.len(),
        };
        let index = index.unwrap_or(last);
        if index > last {
            return Err(format!(
                "position {index} is past the {} modules `{}` holds",
                landing.children.len(),
                to.group
            ));
        }
        Ok((
            vec![LayoutOp::MoveInstance {
                from: from.spot.clone(),
                to: to.clone(),
                id: instance.clone(),
                index,
            }],
            format!("moved `{instance}` to `{}` at {index}", to.group),
        ))
    })
}

/// `output` narrows the region to the most specific rule that writes it for that screen, so a monitor that overrides it is edited there and every other screen keeps the shared one.
pub(super) fn show_in_region(
    id: AreaId,
    picture: &std::path::Path,
    output: Option<String>,
) -> Result<String, String> {
    let source = picture.display().to_string();
    let label = format!("Show {source} in `{id}`");
    layouts::edit(&label, move |layout, _| {
        let site = match &output {
            Some(output) => region_for(layout, &id, output)?,
            None => area_in(layout, &id)?,
        };
        let area = area_of(layout, &site, &id)?;
        let Some(AreaKind::WallpaperRegion {
            rect,
            fit,
            transition,
            ..
        }) = &area.kind
        else {
            return Err(format!(
                "`{id}` is {}, and only a wallpaper region shows a picture",
                describe(area.kind.as_ref())
            ));
        };
        let kind = AreaKind::WallpaperRegion {
            rect: *rect,
            source: Some(source.clone()),
            fit: *fit,
            transition: *transition,
        };
        Ok((
            vec![LayoutOp::SetAreaKind {
                site,
                id: id.clone(),
                kind: Box::new(Some(kind)),
            }],
            source.clone(),
        ))
    })
}

/// The site of the rule that writes `id` for `output`, preferring the one that names `output` most narrowly.
fn region_for(layout: &Layout, id: &AreaId, output: &str) -> Result<Site, String> {
    sites(layout)
        .filter(|(site, layer)| {
            site.workspace.is_none()
                && site.output.matches(output)
                && layer.areas.iter().any(|area| &area.id == id)
        })
        .max_by_key(|(site, _)| site.output.specificity())
        .map(|(site, _)| site)
        .ok_or_else(|| {
            format!(
                "no rule for {output} writes an area called `{id}`{}",
                listing("areas", area_ids(layout))
            )
        })
}

/// Changes one property of a placed module: which module it shows, how big it is drawn, one of its options, one bound expression, or what a gesture on it runs.
fn set(args: &[&str]) -> Result<String, String> {
    let instance = InstanceId::new(arg(args, 0, "instance")?);
    let key = arg(args, 1, "key")?.to_string();
    let value = args
        .get(2..)
        .map(|rest| rest.join(" "))
        .filter(|value| !value.trim().is_empty())
        .ok_or("missing argument <value>")?;

    let label = format!("Set `{key}` on `{instance}`");
    layouts::edit(&label, move |layout, _| {
        let at = placement_of(layout, &instance)
            .ok_or_else(|| nothing_called(layout, instance.as_str()))?;
        let mut changed = spot_of(layout, &at.spot)?.children[at.index].clone();
        apply_key(&mut changed, &key, &value, at.spot.site.layer)?;
        Ok((
            vec![LayoutOp::SetInstance {
                spot: at.spot.clone(),
                id: instance.clone(),
                instance: Box::new(changed),
            }],
            format!("set `{key}` on `{instance}`"),
        ))
    })
}

/// The keys `set` takes, and what each one does to the instance.
fn apply_key(
    instance: &mut Instance,
    key: &str,
    value: &str,
    layer: LayerKind,
) -> Result<(), String> {
    let module = instance.module.clone().unwrap_or_default();
    match key.split_once('.') {
        None if key == "module" => {
            known_module(value)?;
            let representation = instance.representation.unwrap_or(Representation::Chip);
            drawable(value, representation)?;
            allowed_on(layer, value, representation)?;
            instance.module = Some(value.to_string());
        }
        None if key == "representation" => {
            let representation = Representation::from_name(value).ok_or_else(|| {
                format!(
                    "'{value}' is not a size ({})",
                    named(&Representation::ALL, |it| it.as_str())
                )
            })?;
            drawable(&module, representation)?;
            allowed_on(layer, &module, representation)?;
            instance.representation = Some(representation);
        }
        Some(("options", path)) => {
            put(&mut instance.options, path, as_toml(value))?;
            let problems = catalogue()
                .find(&module)
                .map_or_else(Vec::new, |found| found.option_problems(&instance.options));
            let written = |key: &str| path == key || path.starts_with(&format!("{key}."));
            if let Some((key, why)) = problems.into_iter().find(|(key, _)| written(key)) {
                return Err(format!("`{module}`: `{key}` {why}"));
            }
        }
        Some(("bindings", path)) => {
            instance
                .bindings
                .insert(path.to_string(), layout::Expr(value.to_string()));
        }
        Some(("actions", trigger)) => {
            let trigger = Trigger::from_name(trigger).ok_or_else(|| {
                format!(
                    "'{trigger}' is not a gesture ({})",
                    named(&Trigger::ALL, |it| it.as_str())
                )
            })?;
            if layer == LayerKind::Lock {
                return Err(
                    "the lock layer holds readings, never controls, so a gesture cannot run anything there"
                        .to_string(),
                );
            }
            let chain: Vec<String> = value
                .split(';')
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();
            for line in &chain {
                if !super::resolves(line) {
                    return Err(format!("`{line}` is not a command this shell has"));
                }
            }
            instance.actions.insert(trigger, Action(chain));
        }
        _ => {
            return Err(format!(
                "'{key}' is not a property of a placed module (module, representation, options.<key>, bindings.<key>, actions.<gesture>)"
            ));
        }
    }
    Ok(())
}

/// Writes `value` at a dotted path inside an instance's options, making the tables on the way.
fn put(table: &mut toml::Table, path: &str, value: toml::Value) -> Result<(), String> {
    let (head, rest) = match path.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (path, None),
    };
    if head.is_empty() {
        return Err("an option needs a name after `options.`".to_string());
    }
    match rest {
        None => {
            table.insert(head.to_string(), value);
            Ok(())
        }
        Some(rest) => {
            let nested = table
                .entry(head.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            match nested.as_table_mut() {
                Some(nested) => put(nested, rest, value),
                None => Err(format!("`{head}` is a value, so it holds no `{rest}`")),
            }
        }
    }
}

/// A written value as the TOML it spells, and as a string when it spells nothing else. What lets `set` take `true`, `40`, `0.5` and `"%H:%M"` without a second grammar for types.
fn as_toml(value: &str) -> toml::Value {
    toml::from_str::<toml::Table>(&format!("value = {value}"))
        .ok()
        .and_then(|mut table| table.remove("value"))
        .unwrap_or_else(|| toml::Value::String(value.to_string()))
}

/// Puts one part of the layout back to what the layout it extends says — the built-in one, when it extends none — or takes it away where that has nothing to say about it ([`layout::reset`]).
///
/// `all` and a layer name put every area back; an area id or an instance id puts that one back, and is looked for in the base too, since a reset reached for after removing the bar is exactly the one that has to put it back. Only an id neither of them knows is refused.
fn reset(target: &str) -> Result<String, String> {
    let label = format!("Reset `{target}`");
    let target = target.to_string();
    let known = layouts::read(|store| store.all().clone()).unwrap_or_default();
    layouts::edit(&label, move |layout, _| {
        let base = layout::reset::base_of(layout, &known);
        let (area, instance) = (AreaId::new(&target), InstanceId::new(&target));
        let aimed = match target.as_str() {
            "all" => Aim::All,
            named => match LayerKind::from_name(named) {
                Some(layer) => Aim::Layer(layer),
                None if site_of_area(layout, &area)
                    .or_else(|| site_of_area(&base, &area))
                    .is_some() =>
                {
                    Aim::Area(&area)
                }
                None => Aim::Instance(&instance),
            },
        };
        layout::reset::ops(layout, &base, aimed)
            .map(|ops| (ops, format!("reset `{target}`")))
            .ok_or_else(|| nothing_called(layout, &target))
    })
}

/// Which rule and layer an area is written in, or an error naming the areas there are.
fn area_in(layout: &Layout, id: &AreaId) -> Result<Site, String> {
    site_of_area(layout, id).ok_or_else(|| {
        format!(
            "there is no area called `{id}`{}",
            listing("areas", area_ids(layout))
        )
    })
}

fn area_of<'a>(layout: &'a Layout, site: &Site, id: &AreaId) -> Result<&'a Area, String> {
    areas_at(layout, site)
        .iter()
        .find(|area| &area.id == id)
        .ok_or_else(|| format!("there is no area called `{id}`"))
}

fn group_of<'a>(area: &'a Area, id: &GroupId) -> Option<&'a Group> {
    area.groups.iter().find(|group| &group.id == id)
}

fn spot_of<'a>(layout: &'a Layout, spot: &Spot) -> Result<&'a Group, String> {
    let area = area_of(layout, &spot.site, &spot.area)?;
    group_of(area, &spot.group).ok_or_else(|| {
        format!(
            "`{}` has no group called `{}`{}",
            spot.area,
            spot.group,
            groups_of(area)
        )
    })
}

/// The group a command named, either as `<group>` or as `<area>.<group>`. A bare name that several areas have is refused with the qualified names, rather than acting on whichever came first.
fn group_named(layout: &Layout, name: &str) -> Result<Spot, String> {
    let (area, group) = match name.split_once('.') {
        Some((area, group)) => (Some(AreaId::new(area)), GroupId::new(group)),
        None => (None, GroupId::new(name)),
    };
    let mut found: Vec<Spot> = Vec::new();
    for (site, layer) in sites(layout) {
        for holder in &layer.areas {
            if area.as_ref().is_some_and(|wanted| &holder.id != wanted) {
                continue;
            }
            if group_of(holder, &group).is_some() {
                found.push(Spot {
                    site: site.clone(),
                    area: holder.id.clone(),
                    group: group.clone(),
                });
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "there is no group called `{name}`{}",
            listing("groups", group_ids(layout))
        )),
        _ => Err(format!(
            "`{name}` is a group of several areas; say which as <area>.<group>: {}",
            found
                .iter()
                .map(|spot| format!("{}.{}", spot.area, spot.group))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The group an `add` with no group named lands in: the area's first, which for a bar is the run its own file writes first.
fn first_group(area: &Area, id: &AreaId) -> Result<GroupId, String> {
    area.groups
        .first()
        .map(|group| group.id.clone())
        .ok_or_else(|| {
            format!(
                "`{id}` has no group to put a module in; a group says where in the area its modules sit, and is written in the layout file"
            )
        })
}

/// An id nothing in this layout places, said with whatever the layout does have.
fn nothing_called(layout: &Layout, id: &str) -> String {
    format!(
        "nothing in this layout is called `{id}`{}",
        listing("modules", instance_ids(layout))
    )
}

fn instance_ids(layout: &Layout) -> BTreeSet<String> {
    ids(layout, |area, ids| {
        for group in &area.groups {
            ids.extend(group.children.iter().map(|it| it.id.to_string()));
        }
    })
}

fn area_ids(layout: &Layout) -> BTreeSet<String> {
    ids(layout, |area, ids| {
        ids.insert(area.id.to_string());
    })
}

fn group_ids(layout: &Layout) -> BTreeSet<String> {
    ids(layout, |area, ids| {
        ids.extend(area.groups.iter().map(|it| it.id.to_string()));
    })
}

fn ids(layout: &Layout, mut of: impl FnMut(&Area, &mut BTreeSet<String>)) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (_, layer) in sites(layout) {
        for area in &layer.areas {
            of(area, &mut ids);
        }
    }
    ids
}

/// What there is instead, appended to a refusal. Empty when there is nothing, because "(this layout has: )" is worse than saying nothing.
fn listing(what: &str, ids: BTreeSet<String>) -> String {
    match ids.is_empty() {
        true => String::new(),
        false => format!(
            " (this layout's {what}: {})",
            ids.into_iter().collect::<Vec<_>>().join(", ")
        ),
    }
}

fn named<T: Copy>(all: &[T], spelling: impl Fn(T) -> &'static str) -> String {
    all.iter()
        .copied()
        .map(spelling)
        .collect::<Vec<_>>()
        .join("|")
}

/// An id no instance in this layout has, derived from the module's own name: `clock`, then `clock-2`.
///
/// Readable and unique rather than generated (F-10.3), because a layout file is hand-edited and IPC addresses an instance by this name for as long as it exists.
fn free_instance_id(layout: &Layout, module: &str) -> InstanceId {
    let taken = instance_ids(layout);
    if !taken.contains(module) {
        return InstanceId::new(module);
    }
    let mut nth = 2;
    loop {
        let id = format!("{module}-{nth}");
        if !taken.contains(&id) {
            return InstanceId::new(id);
        }
        nth += 1;
    }
}

/// The size an area holds a module at: the first one the area can arrange that the module declares.
///
/// A bar holds chips, a grid holds widgets, and a free rectangle or a stack holds a card — so the size follows from where the module was put rather than being one more thing to say. The fallbacks matter as much as the first choice: a module with no chip is still worth putting on a bar if it has a small widget.
fn fits(kind: Option<&AreaKind>, module: &str) -> Option<Representation> {
    let wanted: &[Representation] = match kind {
        Some(AreaKind::Bar { .. }) | Some(AreaKind::Dock { .. }) => {
            &[Representation::Chip, Representation::WidgetS]
        }
        Some(AreaKind::Grid { .. }) => &[
            Representation::WidgetM,
            Representation::WidgetS,
            Representation::WidgetL,
            Representation::Chip,
        ],
        _ => &[
            Representation::Card,
            Representation::WidgetM,
            Representation::WidgetS,
            Representation::Chip,
        ],
    };
    wanted
        .iter()
        .copied()
        .find(|it| catalogue().has_representation(module, *it))
}

/// Whether the kind of area holds modules at all, so a group placed in paint is refused rather than written where nothing would draw it.
fn holds_instances(kind: &AreaKind) -> bool {
    !matches!(
        kind,
        AreaKind::WallpaperRegion { .. } | AreaKind::Texture { .. }
    )
}

fn describe(kind: Option<&AreaKind>) -> String {
    match kind {
        Some(kind) => format!("a `{}` area", kind.name()),
        None => "an area with no kind yet".to_string(),
    }
}

fn groups_of(area: &Area) -> String {
    listing(
        "groups",
        area.groups.iter().map(|it| it.id.to_string()).collect(),
    )
}

fn known_module(module: &str) -> Result<(), String> {
    if catalogue().knows_module(module) {
        return Ok(());
    }
    let known: Vec<&str> = crate::core::modules::MODULES
        .iter()
        .map(|it| it.id)
        .collect();
    Err(format!(
        "there is no module called `{module}` (this shell has: {})",
        known.join(", ")
    ))
}

fn drawable(module: &str, representation: Representation) -> Result<(), String> {
    if catalogue().has_representation(module, representation) {
        return Ok(());
    }
    Err(format!(
        "`{module}` cannot be drawn as `{}`",
        representation.as_str()
    ))
}

/// The lock layer's own rule, enforced where an edit is made as well as where a file is validated: only a representation that registers nothing may go on a screen anyone in the room can touch (TA-8).
fn allowed_on(
    layer: LayerKind,
    module: &str,
    representation: Representation,
) -> Result<(), String> {
    if layer != LayerKind::Lock || catalogue().is_read_only(module, representation) {
        return Ok(());
    }
    Err(format!(
        "`{module}` as `{}` can be interacted with, and the lock layer takes readings only",
        representation.as_str()
    ))
}

pub(crate) fn catalogue() -> Descriptors {
    Descriptors(crate::core::modules::MODULES)
}

/// What the layout model has to ask the module table.
///
/// It lives here rather than in `crates/layout` because that crate deliberately knows nothing about modules or the IPC table — the whole point of `Catalogue` is that the model can be validated by a test that states its own three modules. This is the real answer, wired to the descriptors this binary ships.
///
/// **It carries the table rather than reading the installed one.** `ui::descriptor::install` runs in `setup_shell`, and these verbs answer in the CLI process where nothing has run it, so an installed-table lookup answers `None` for every module the binary has — which came out as `layout check` calling `clock` an unknown module. `config check` already takes the table as an argument for the same reason (`check::command`).
pub(crate) struct Descriptors(&'static [ui::descriptor::ModuleDescriptor]);

impl Descriptors {
    fn find(&self, module: &str) -> Option<&'static ui::descriptor::ModuleDescriptor> {
        ui::descriptor::lookup(self.0, module)
    }
}

impl layout::Catalogue for Descriptors {
    fn knows_module(&self, module: &str) -> bool {
        self.find(module).is_some()
    }

    fn has_representation(&self, module: &str, representation: Representation) -> bool {
        self.find(module).is_some_and(|found| {
            found
                .input(surfaces::area::representation(representation))
                .is_some()
        })
    }

    fn is_read_only(&self, module: &str, representation: Representation) -> bool {
        self.find(module)
            .and_then(|found| found.input(surfaces::area::representation(representation)))
            .is_some_and(|input| input == ui::descriptor::Input::ReadOnly)
    }

    fn command_resolves(&self, line: &str) -> bool {
        super::resolves(line)
    }

    fn option_problems(&self, module: &str, options: &toml::Table) -> Vec<(String, String)> {
        self.find(module)
            .map_or_else(Vec::new, |found| found.option_problems(options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_layout_is_listed_and_marked_read_only() {
        let listed = list();
        assert!(
            listed.lines().any(|line| line.starts_with("default\t")),
            "{listed}"
        );
        assert!(listed.contains("read-only"), "{listed}");
    }

    #[test]
    fn showing_a_layout_that_is_not_there_says_so_rather_than_printing_nothing() {
        let refused = show(Some("no-such-layout")).expect_err("it refuses");
        assert!(refused.contains("no-such-layout"), "{refused}");
    }

    #[test]
    fn showing_the_built_in_layout_prints_something_that_parses_back() {
        let printed = show(None).expect("it prints");
        let again: Layout = toml::from_str(&printed).expect("and it parses back");
        assert_eq!(again.outputs.len(), 1);
    }

    /// The check has to pass on the layout the shell falls back to, or the fallback is not one.
    ///
    /// **Deliberately without installing the module table**, which is the process this verb actually runs in: `ui::descriptor::install` happens in `setup_shell`, and a check answers in the CLI. Installing it here is what hid the bug — every module in a real user's layout came back "there is no module called `clock`", because the catalogue looked in an empty table while the binary's own table sat one argument away.
    #[test]
    fn the_built_in_layout_checks_clean_without_the_module_table_installed() {
        telar::set_locale("en");
        assert!(
            ui::descriptor::installed().is_empty(),
            "this test is only worth anything while nothing installed the table on this thread"
        );
        let verdict = check(None);
        assert!(verdict.is_ok(), "{}", verdict.unwrap_err());
    }

    /// DEC-17's cost note: one warning per area above fullscreen and per output it lands on, naming the output — every output a `*` rule reaches when `check` resolves without a compositor — and none for an area without the flag.
    #[test]
    fn an_area_above_fullscreen_is_said_to_keep_its_output_off_direct_scanout() {
        let mut mine = layout::built_in();
        let bar = mine.outputs[0]
            .layers
            .top
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "bar-top")
            .expect("the built-in bar");
        bar.above_fullscreen = Some(true);
        let at = |output: &str| layout::resolve(&mine, &Default::default(), output, None).0;

        telar::set_locale("en");
        let checked = scanout(&at(NOMINAL_OUTPUT), "layouts/mine.toml");
        assert!(checked.errors.is_empty());
        let said: Vec<(&str, &str)> = checked
            .warnings
            .iter()
            .map(|finding| (finding.key.as_str(), finding.message.as_str()))
            .collect();
        assert_eq!(
            said,
            [(
                "layers.top.areas.bar-top.above_fullscreen",
                "`bar-top` is drawn above fullscreen windows, which keeps every output off direct scanout"
            )]
        );
        let live = scanout(&at("DP-1"), "layouts/mine.toml");
        assert_eq!(
            live.warnings[0].message,
            "`bar-top` is drawn above fullscreen windows, which keeps DP-1 off direct scanout"
        );

        telar::set_locale("es");
        assert_eq!(
            scanout(&at("DP-1"), "layouts/mine.toml").warnings[0].message,
            "`bar-top` se dibuja sobre las ventanas a pantalla completa, lo que deja a DP-1 sin escaneo directo"
        );
        telar::set_locale("en");

        let unflagged = layout::resolve(&layout::built_in(), &Default::default(), "DP-1", None).0;
        assert!(scanout(&unflagged, "layouts/default.toml").is_clean());
    }

    /// `check` answers from the files rather than from the shell, so it must refuse a name that is not there instead of reporting a clean layout it never read.
    #[test]
    fn checking_a_layout_that_is_not_there_is_an_error_not_a_clean_report() {
        telar::set_locale("en");
        ui::descriptor::install(crate::core::modules::MODULES);
        let refused = check(Some("no-such-layout")).expect_err("it refuses");
        assert!(refused.contains("no-such-layout"), "{refused}");
    }

    /// A store of one bar with three runs, installed as the one a shell owns, and a handle on it to read the result back.
    ///
    /// Its own directory rather than the user's, so these run beside the verbs that read the real one without either seeing the other's files.
    fn shell_with(test: &str, active: &str) -> std::rc::Rc<std::cell::RefCell<LayoutStore>> {
        shell_holding(test, active, &layout::built_in())
    }

    fn shell_holding(
        test: &str,
        active: &str,
        mine: &Layout,
    ) -> std::rc::Rc<std::cell::RefCell<LayoutStore>> {
        ui::descriptor::install(crate::core::modules::MODULES);
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("layout-verbs-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a layouts directory");
        std::fs::write(
            dir.join("mine.toml"),
            toml::to_string_pretty(mine).expect("the layout serializes"),
        )
        .expect("a layout to edit");

        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store
            .use_layout(&LayoutId::new(active))
            .expect("the store holds it");
        let store = std::rc::Rc::new(std::cell::RefCell::new(store));
        layouts::install(std::rc::Rc::clone(&store), std::rc::Rc::new(|| {}));
        store
    }

    /// What is in one of the bar's runs, which is what every verb below moves around.
    fn run_of(store: &LayoutStore, group: &str) -> Vec<String> {
        store
            .active()
            .outputs
            .iter()
            .flat_map(|rule| rule.layers.top.areas.iter())
            .flat_map(|area| area.groups.iter())
            .filter(|it| it.id.as_str() == group)
            .flat_map(|it| it.children.iter())
            .map(|it| it.id.to_string())
            .collect()
    }

    /// The picture the background region `id` names for itself, as the store now holds it.
    fn source_of(store: &LayoutStore, id: &str) -> Option<String> {
        store
            .active()
            .outputs
            .iter()
            .flat_map(|rule| rule.layers.background.areas.iter())
            .find(|area| area.id.as_str() == id)
            .and_then(|area| match &area.kind {
                Some(AreaKind::WallpaperRegion { source, .. }) => source.clone(),
                _ => None,
            })
    }

    /// `wallpaper set --region` is a layout edit: it writes the region's own `source`, which `layout undo` takes back, and it refuses an area that is not a wallpaper region rather than writing a picture nothing would draw.
    #[test]
    fn a_wallpaper_set_on_a_region_is_a_layout_edit_undo_takes_back() {
        let store = shell_with("wallpaper-region", "mine");
        let picture = util::paths::isolated_root()
            .expect("a scratch root")
            .join("layout-verbs-wallpaper-region")
            .join("sea.png");
        std::fs::write(&picture, b"a picture, as far as a path check goes").expect("a file");
        let written = picture.display().to_string();
        assert_eq!(source_of(&store.borrow(), "background"), None);

        let reply = super::super::dispatch(&format!("wallpaper set {written} --region background"));
        assert_eq!(reply, format!("ok {written}"));
        assert_eq!(
            source_of(&store.borrow(), "background").as_deref(),
            Some(written.as_str()),
            "the region names its own picture now"
        );

        undo().expect("the edit comes back out");
        assert_eq!(
            source_of(&store.borrow(), "background"),
            None,
            "and it follows [background] again"
        );

        let refused = super::super::dispatch(&format!("wallpaper set {written} --region bar-top"));
        assert_eq!(
            refused,
            "err `bar-top` is a `bar` area, and only a wallpaper region shows a picture"
        );
        let refused = super::super::dispatch(&format!("wallpaper set {written} --region"));
        assert_eq!(refused, "err missing argument <area> after --region");
    }

    /// The sprint's own criterion: a layout is editable end to end over IPC, and one undo takes back the last edit whatever made it.
    ///
    /// Through the verbs rather than through the store, because what this has to prove is the whole path — the id a verb picks, the size it chooses for the area, the transaction it commits and the reply it sends — and each of those is a place the store's own tests cannot see.
    #[test]
    fn the_verbs_place_change_move_and_remove_a_module_and_undo_takes_each_back() {
        let store = shell_with("verbs", "mine");

        let said = add(&["battery", "bar-top", "end"]).expect("it places one");
        assert!(said.contains("battery"), "{said}");
        assert_eq!(run_of(&store.borrow(), "end"), ["notes", "battery"]);

        let refused = set(&["battery", "options.show_percent", "true"])
            .expect_err("a key the module does not declare is refused");
        assert_eq!(
            refused,
            "`battery`: `show_percent` is not one of its options"
        );
        let refused = set(&["battery", "options.critical_level", "low"])
            .expect_err("and so is a value its type cannot hold");
        assert_eq!(refused, "`battery`: `critical_level` takes a whole number");
        set(&["battery", "options.critical_level", "15"]).expect("it sets an option");
        assert_eq!(
            store
                .borrow()
                .active()
                .outputs
                .iter()
                .flat_map(|rule| rule.layers.top.areas.iter())
                .flat_map(|area| area.groups.iter())
                .flat_map(|group| group.children.iter())
                .find(|it| it.id.as_str() == "battery")
                .and_then(|it| it.options.get("critical_level"))
                .and_then(toml::Value::as_integer),
            Some(15)
        );

        move_instance(&["battery", "start", "0"]).expect("it moves");
        assert_eq!(run_of(&store.borrow(), "start"), ["battery", "workspaces"]);
        assert_eq!(run_of(&store.borrow(), "end"), ["notes"]);

        let said = remove("battery").expect("it removes");
        assert!(said.contains("module"), "it says what it took out: {said}");
        assert_eq!(run_of(&store.borrow(), "start"), ["workspaces"]);

        for expected in [
            vec!["battery", "workspaces"],
            vec!["workspaces"],
            vec!["workspaces"],
            vec!["workspaces"],
        ] {
            undo().expect("every edit comes back out");
            assert_eq!(run_of(&store.borrow(), "start"), expected);
        }
        assert!(undo().is_err(), "and then there is nothing left to undo");
        assert_eq!(run_of(&store.borrow(), "end"), ["notes"]);

        redo().expect("forward again");
        assert_eq!(run_of(&store.borrow(), "end"), ["notes", "battery"]);
    }

    /// An area that has been emptied out is put back as the shipped layout has it, and one the shipped layout has nothing to say about is taken away instead.
    #[test]
    fn reset_puts_an_area_back_the_way_the_built_in_layout_has_it() {
        let store = shell_with("reset", "mine");

        remove("bar-top").expect("the bar goes");
        assert!(run_of(&store.borrow(), "start").is_empty());

        reset("bar-top").expect("and comes back");
        assert_eq!(run_of(&store.borrow(), "start"), ["workspaces"]);
        assert_eq!(run_of(&store.borrow(), "center"), ["clock"]);

        add(&["battery", "bar-top", "end"]).expect("something the shipped layout does not have");
        reset("battery").expect("is reset by being taken away");
        assert_eq!(run_of(&store.borrow(), "end"), ["notes"]);

        add(&["battery", "bar-top", "end"]).expect("and again");
        reset("all").expect("a whole reset puts every layer back");
        assert_eq!(run_of(&store.borrow(), "end"), ["notes"]);
    }

    /// The shipped layout is read-only, so the first edit to it becomes the user's own copy — and the copy is what the installation draws from then on, `state.json` included.
    #[test]
    fn the_first_edit_to_the_built_in_layout_forks_it() {
        let store = shell_with("fork", "default");
        assert_eq!(store.borrow().active_id().as_str(), BUILT_IN);

        add(&["battery", "bar-top", "end"]).expect("it places one");

        let active = store.borrow().active_id().clone();
        assert_ne!(
            active.as_str(),
            BUILT_IN,
            "the edit did not land on the shipped layout"
        );
        assert_eq!(
            services::state::get().layout.as_deref(),
            Some(active.as_str()),
            "and the copy is what this installation draws"
        );
        assert_eq!(run_of(&store.borrow(), "end"), ["notes", "battery"]);
        assert_eq!(
            layout::built_in().outputs[0].layers.top.areas[0].groups[2]
                .children
                .len(),
            1,
            "while the shipped layout is untouched"
        );
    }

    /// A refusal must leave the layout exactly as it was: the transaction is the unit, and a verb that half applied one would leave a state no undo entry describes.
    #[test]
    fn a_refused_edit_changes_nothing() {
        let store = shell_with("refused", "mine");
        let before = toml::to_string(store.borrow().active()).expect("serializes");

        assert!(add(&["clock", "no-such-area"]).is_err());
        assert!(add(&["nosuchmodule", "bar-top"]).is_err());
        assert!(move_instance(&["clock", "no-such-group"]).is_err());
        assert!(set(&["clock", "nosuchkey", "1"]).is_err());
        assert!(set(&["clock", "representation", "enormous"]).is_err());
        assert!(remove("nothing-called-this").is_err());
        assert!(reset("nothing-called-this").is_err());

        assert_eq!(
            toml::to_string(store.borrow().active()).expect("serializes"),
            before
        );
        assert!(
            store.borrow().undo_label().is_none(),
            "and none of them left an undo entry"
        );
    }

    /// `layout edit` is the mode switch (F-10.33): a line the shell answers, which names what it takes, refuses what is not a layer or not a screen, and says so when there was nothing to switch off.
    #[test]
    fn the_edit_verb_reads_a_layer_and_a_screen() {
        telar::set_locale("en");
        for line in [
            "layout edit desktop",
            "layout edit lock DP-1",
            "layout edit off",
        ] {
            assert!(super::super::resolves(line), "{line}");
        }
        assert_eq!(
            edit(&[]).unwrap_err(),
            "missing argument <background|desktop|top|overlay|lock|off>"
        );
        let refused = edit(&["sideways"]).unwrap_err();
        assert!(
            refused.contains("'sideways'") && refused.contains("overlay"),
            "{refused}"
        );
        assert_eq!(edit(&["off"]), Ok("nothing was being edited".to_string()));
        let nowhere = edit(&["top", "VGA-9"]).unwrap_err();
        assert!(nowhere.contains("'VGA-9' is not a screen"), "{nowhere}");
    }

    /// F-10.35: the recovery flag refuses every edit, and a mode is one waiting to happen, so no layer's mode opens under it.
    #[test]
    fn the_edit_verb_is_refused_under_the_safe_layout() {
        telar::set_locale("en");
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("layout-verbs-safe-edit");
        layouts::install(
            std::rc::Rc::new(std::cell::RefCell::new(LayoutStore::safe(dir))),
            std::rc::Rc::new(|| {}),
        );
        for layer in ["background", "desktop", "top", "overlay", "lock"] {
            let refused = edit(&[layer]).expect_err("refused under --safe-layout");
            assert!(refused.contains("--safe-layout"), "{layer}: {refused}");
        }
    }

    /// Every verb that changes a layout needs the store the shell owns, and in a process without one has to say so rather than answering as though it had edited something.
    #[test]
    fn an_edit_with_no_running_shell_says_what_is_missing_rather_than_answering_ok() {
        for refused in [
            undo(),
            redo(),
            add(&["clock", "bar-top"]),
            remove("clock"),
            move_instance(&["clock", "end"]),
            set(&["clock", "representation", "chip"]),
            reset("all"),
        ] {
            let why = refused.expect_err("there is no store in a test process");
            assert!(why.contains("no running shell"), "{why}");
        }
    }

    /// The refusals are the half of these verbs a user meets most, so each says what there is instead of only what there is not.
    #[test]
    fn a_module_this_shell_does_not_have_is_refused_by_name() {
        let refused = add(&["nosuchmodule", "bar-top"]).expect_err("it refuses");
        assert!(refused.contains("nosuchmodule"), "{refused}");
        assert!(refused.contains("clock"), "and lists real ones: {refused}");
    }

    #[test]
    fn the_size_a_module_is_placed_at_follows_the_area_it_is_placed_in() {
        ui::descriptor::install(crate::core::modules::MODULES);
        let bar = AreaKind::Bar {
            edge: Some(config::Edge::Top),
            thickness: Some(32.0),
            length: None,
            offset: None,
            shape: layout::BarShape::default(),
            autohide: None,
        };
        let grid = AreaKind::Grid {
            rect: None,
            cell: None,
            gap: None,
            anchor: None,
        };
        assert_eq!(fits(Some(&bar), "clock"), Some(Representation::Chip));
        assert_eq!(fits(Some(&grid), "clock"), Some(Representation::WidgetM));
        assert_eq!(
            fits(Some(&bar), "user"),
            Some(Representation::WidgetS),
            "a reading with no chip is still worth a bar at the smallest size it has"
        );
        assert_eq!(
            fits(Some(&bar), "tray"),
            Some(Representation::Chip),
            "and a module with only a chip is that chip wherever it is put"
        );
        assert_eq!(
            fits(Some(&grid), "nosuchmodule"),
            None,
            "a module with nothing an area can draw is refused rather than placed as a placeholder"
        );
    }

    /// The lock layer's rule is enforced where the edit is made, not only where the file is read back: `layout add` refuses a control on it (TA-8).
    ///
    /// The tray is the example because its own build is what takes the presses — one pressable box per application — where a chip that merely *opens* a panel is read-only, since the press belongs to the bar that placed it.
    #[test]
    fn a_control_cannot_be_added_to_the_lock_layer() {
        ui::descriptor::install(crate::core::modules::MODULES);
        assert!(allowed_on(LayerKind::Lock, "clock", Representation::Chip).is_ok());
        let refused = allowed_on(LayerKind::Lock, "tray", Representation::Chip)
            .expect_err("a tray takes presses of its own");
        assert!(refused.contains("readings only"), "{refused}");
        assert!(
            allowed_on(LayerKind::Lock, "media", Representation::Card).is_err(),
            "and a card with a transport on it is refused as surely as the tray"
        );
        assert!(
            allowed_on(LayerKind::Top, "tray", Representation::Chip).is_ok(),
            "it is the lock layer's rule alone"
        );
    }

    /// A written value keeps the type it spells, so `set` needs no second grammar for types — and anything that spells no TOML value is the string it is.
    #[test]
    fn a_set_value_is_read_as_the_toml_it_spells() {
        assert_eq!(as_toml("true"), toml::Value::Boolean(true));
        assert_eq!(as_toml("40"), toml::Value::Integer(40));
        assert_eq!(as_toml("0.5"), toml::Value::Float(0.5));
        assert_eq!(as_toml("\"%H:%M\""), toml::Value::String("%H:%M".into()));
        assert_eq!(
            as_toml("%H:%M"),
            toml::Value::String("%H:%M".into()),
            "a bare format string is what a user types, and it is not TOML"
        );
    }

    #[test]
    fn an_option_is_written_at_the_path_it_names() {
        let mut options = toml::Table::new();
        put(&mut options, "format", as_toml("\"%H\"")).expect("a plain key");
        put(&mut options, "face.hands", as_toml("true")).expect("a nested one");
        assert_eq!(options["format"].as_str(), Some("%H"));
        assert_eq!(
            options["face"]
                .as_table()
                .and_then(|it| it["hands"].as_bool()),
            Some(true)
        );
        let refused = put(&mut options, "format.deeper", as_toml("1")).expect_err("it refuses");
        assert!(refused.contains("holds no"), "{refused}");
    }

    /// A module a workspace rule places is addressed by its id like any other: the verbs find it in the rule that places it, and undo puts it back there.
    #[test]
    fn a_module_a_workspace_rule_places_is_edited_where_it_is_written() {
        let mut mine = layout::built_in();
        mine.outputs[0].workspaces.push(
            toml::from_str(
                r#"
                match = "games"
                [[layers.top.areas]]
                id = "bar-top"
                [[layers.top.areas.groups]]
                id = "end"
                [[layers.top.areas.groups.children]]
                id = "games-battery"
                module = "battery"
                "#,
            )
            .expect("a workspace rule"),
        );
        let store = shell_holding("workspace", "mine", &mine);
        let in_games = |store: &LayoutStore| -> Vec<String> {
            store.active().outputs[0].workspaces[0].layers.top.areas[0].groups[0]
                .children
                .iter()
                .map(|it| it.id.to_string())
                .collect()
        };
        assert_eq!(in_games(&store.borrow()), ["games-battery"]);

        set(&["games-battery", "representation", "chip"]).expect("it is set where it is");
        let said = remove("games-battery").expect("and removed from there");
        assert!(said.contains("module"), "{said}");
        assert!(in_games(&store.borrow()).is_empty());
        assert_eq!(
            run_of(&store.borrow(), "end"),
            ["notes"],
            "the output rule's own bar is untouched"
        );

        undo().expect("the removal comes back out");
        assert_eq!(in_games(&store.borrow()), ["games-battery"]);
        assert!(
            nothing_called(store.borrow().active(), "nothing-at-all")
                .contains("nothing in this layout"),
            "and an id nothing places is still refused by name"
        );
    }

    fn lock_ids(store: &LayoutStore) -> Vec<String> {
        store.active().outputs[0]
            .layers
            .lock
            .areas
            .iter()
            .map(|area| area.id.to_string())
            .collect()
    }

    /// The lock screen's prompt is the one area `layout remove` refuses, and the refusal says what it is rather than failing as though it were not there (TA-8).
    #[test]
    fn the_lock_prompt_cannot_be_removed_over_ipc() {
        let store = shell_with("prompt", "mine");
        let before = store.borrow().active().clone();

        let refused = remove("prompt").expect_err("it refuses");
        assert!(
            refused.contains("password prompt") && refused.contains("never removed"),
            "{refused}"
        );
        assert_eq!(store.borrow().active(), &before, "and changes nothing");
        assert!(store.borrow().undo_label().is_none());

        remove("lock-readings")
            .expect("while the rest of the lock layer is the user's to take away");
        assert_eq!(lock_ids(&store.borrow()), ["prompt"]);
    }

    /// Resetting the lock layer puts the shipped prompt back where the user's stands, rather than taking it away to put it back: an edit that takes the prompt away is refused, however briefly.
    #[test]
    fn reset_puts_the_lock_layer_back_without_taking_the_prompt_away() {
        let store = shell_with("reset-lock", "mine");
        let shipped = layout::built_in().outputs[0].layers.lock.clone();

        remove("lock-readings").expect("a reading goes");
        reset("lock").expect("the layer comes back");
        assert_eq!(store.borrow().active().outputs[0].layers.lock, shipped);

        reset("prompt").expect("the prompt alone resets in place");
        reset("all").expect("and so does everything");
        assert_eq!(store.borrow().active().outputs[0].layers.lock, shipped);

        for _ in 0..4 {
            undo().expect("each reset comes back out");
        }
        assert_eq!(lock_ids(&store.borrow()), ["lock-readings", "prompt"]);
    }
}
