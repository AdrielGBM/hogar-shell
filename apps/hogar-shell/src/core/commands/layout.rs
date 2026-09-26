//! `hogar-shell layout` — reading the layouts on disk, choosing which one the shell draws, and editing it.
//!
//! `list`, `show` and `check` answer in the CLI process, because a layout that stops the shell from starting is exactly the one a user needs to be able to look at (F-10.5). Everything else goes to the shell, which is what owns the store: `use` writes the active name to machine state and asks for a reload, and the verbs that change a layout commit a transaction against the store the shell is drawing from (`crate::core::layouts`).
//!
//! **Every edit here is one transaction**, so `layout undo` takes back one command whatever else made the edit before it — a gesture, a popover or another line of this. The layout an edit lands in is the one being drawn, except that the first edit to the built-in layout forks it: the shipped one is read-only so that a user who has broken theirs always has one that works.
//!
//! **What a verb refuses is as much the point as what it does.** A module nothing answers to, a representation it cannot be drawn as, an area that holds no instances, a control placed on the lock layer: each is a message naming what there is instead of an edit that draws a placeholder the user then has to find.

use std::collections::BTreeSet;

use layout::{
    Action, Area, AreaId, AreaKind, BUILT_IN, Catalogue, Group, GroupId, Instance, InstanceId,
    LayerKind, Layout, LayoutId, LayoutOp, LayoutStore, NOMINAL_OUTPUT, Representation, Site, Spot,
    Trigger,
};

use super::args::arg;
use super::{Command, Target};
use crate::core::layouts;

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
            help: "put a part of the layout back to what the built-in one says",
            run: |args| reset(arg(args, 0, "id|layer|all")?),
        },
    ],
};

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
    ));

    match report.is_clean() {
        true => Ok(report.summary()),
        false => Err(report.render()),
    }
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
/// Both, because both are addressed by id and a user who asks to remove `bar-top` means the bar. Which it was is in the reply, so an id that happens to name both kinds of thing does not act silently on the wrong one.
fn remove(id: &str) -> Result<String, String> {
    let instance = InstanceId::new(id);
    let area = AreaId::new(id);
    let label = format!("Remove `{id}`");
    let id = id.to_string();
    layouts::edit(&label, move |layout, _| {
        if let Some(at) = instance_in(layout, &instance) {
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
        let from = instance_in(layout, &instance)
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
        let at = instance_in(layout, &instance)
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

/// Puts one part of the layout back to what the built-in layout says, or takes it away where the built-in layout has nothing to say about it.
///
/// `all` and a layer name put every area back; an area id or an instance id puts that one back. **Something the layout no longer has is still a reset**, because a reset a user reaches for after removing the bar is exactly the one that has to put it back — so a target missing here is looked for in the shipped layout too, and only an id neither of them knows is refused.
///
/// **The output and workspace rules a layout writes are left where they are**: they are the shape of the file rather than something placed in it, and no operation addresses one, so a reset that claimed to remove them would be claiming more than it does.
fn reset(target: &str) -> Result<String, String> {
    let label = format!("Reset `{target}`");
    let target = target.to_string();
    layouts::edit(&label, move |layout, _| {
        let shipped = layout::built_in();
        let said = format!("reset `{target}`");
        if target == "all" {
            return Ok((refill(layout, &shipped, &LayerKind::ALL), said));
        }
        if let Some(layer) = LayerKind::from_name(&target) {
            return Ok((refill(layout, &shipped, &[layer]), said));
        }
        let area = AreaId::new(&target);
        if let Some(site) = area_in(layout, &area)
            .ok()
            .or_else(|| area_in(&shipped, &area).ok())
        {
            return Ok((reset_area(layout, &shipped, &site, &area), said));
        }
        let instance = InstanceId::new(&target);
        if let Some(at) =
            instance_in(layout, &instance).or_else(|| instance_in(&shipped, &instance))
        {
            return Ok((reset_instance(layout, &shipped, &at, &instance), said));
        }
        Err(nothing_called(layout, &target))
    })
}

/// Empties the named layers of every output rule and fills them again from the built-in layout's rule of the same match, which has nothing to say about a rule the user added — so a monitor rule's areas are taken away rather than replaced.
fn refill(layout: &Layout, shipped: &Layout, layers: &[LayerKind]) -> Vec<LayoutOp> {
    let mut ops = Vec::new();
    for rule in &layout.outputs {
        for kind in layers {
            let site = Site {
                output: rule.matches.clone(),
                layer: *kind,
            };
            for area in &rule.layers.get(*kind).areas {
                ops.push(LayoutOp::DeleteArea {
                    site: site.clone(),
                    id: area.id.clone(),
                });
            }
            for (index, area) in areas_at(shipped, &site).iter().enumerate() {
                ops.push(LayoutOp::InsertArea {
                    site: site.clone(),
                    index,
                    area: Box::new(area.clone()),
                });
            }
        }
    }
    ops
}

/// Takes one area out and puts the shipped one in its place — or only one of the two, when the layout no longer has it or the shipped layout never did.
fn reset_area(layout: &Layout, shipped: &Layout, site: &Site, id: &AreaId) -> Vec<LayoutOp> {
    let here = areas_at(layout, site);
    let at = here.iter().position(|area| &area.id == id);
    let mut ops = Vec::new();
    if at.is_some() {
        ops.push(LayoutOp::DeleteArea {
            site: site.clone(),
            id: id.clone(),
        });
    }
    if let Some(area) = areas_at(shipped, site).iter().find(|area| &area.id == id) {
        let shipped_at = areas_at(shipped, site)
            .iter()
            .position(|it| &it.id == id)
            .unwrap_or(0);
        ops.push(LayoutOp::InsertArea {
            site: site.clone(),
            // Back where it was, or where the shipped layout has it once the layout no longer says.
            index: at.unwrap_or(shipped_at.min(here.len())),
            area: Box::new(area.clone()),
        });
    }
    ops
}

/// The same for one placed module. `at` is wherever it was found — in the layout, or in the shipped one when the layout has taken it out.
fn reset_instance(layout: &Layout, shipped: &Layout, at: &At, id: &InstanceId) -> Vec<LayoutOp> {
    let here = instance_in(layout, id);
    let mut ops = Vec::new();
    if let Some(here) = &here {
        ops.push(LayoutOp::DeleteInstance {
            spot: here.spot.clone(),
            id: id.clone(),
        });
    }
    if let Some(found) = instance_in(shipped, id)
        && let Ok(group) = spot_of(shipped, &found.spot)
    {
        let room = spot_of(layout, &at.spot)
            .map(|group| group.children.len())
            .unwrap_or(0);
        ops.push(LayoutOp::InsertInstance {
            spot: at.spot.clone(),
            index: match &here {
                Some(here) => here.index,
                None => found.index.min(room),
            },
            instance: Box::new(group.children[found.index].clone()),
        });
    }
    ops
}

/// The areas one site holds, and none where the layout has no rule for that output.
fn areas_at<'a>(layout: &'a Layout, site: &Site) -> &'a [Area] {
    layout
        .outputs
        .iter()
        .find(|rule| rule.matches == site.output)
        .map(|rule| rule.layers.get(site.layer).areas.as_slice())
        .unwrap_or_default()
}

/// Where an instance sits: the group it is in, and its position in that group.
struct At {
    spot: Spot,
    index: usize,
}

/// Where `id` is placed in the layout's own rules, or `None`.
///
/// Only the output level, because that is all an operation can address: [`Site`] names an output rule and a layer, and what a workspace rule places is written inside that rule. [`nothing_called`] is what says so when an id is only there.
fn instance_in(layout: &Layout, id: &InstanceId) -> Option<At> {
    for rule in &layout.outputs {
        for (kind, layer) in rule.layers.each() {
            for area in &layer.areas {
                for group in &area.groups {
                    if let Some(index) = group.children.iter().position(|it| &it.id == id) {
                        return Some(At {
                            spot: Spot {
                                site: Site {
                                    output: rule.matches.clone(),
                                    layer: kind,
                                },
                                area: area.id.clone(),
                                group: group.id.clone(),
                            },
                            index,
                        });
                    }
                }
            }
        }
    }
    None
}

/// Which rule and layer an area is written in, or an error naming the areas there are.
fn area_in(layout: &Layout, id: &AreaId) -> Result<Site, String> {
    for rule in &layout.outputs {
        for (kind, layer) in rule.layers.each() {
            if layer.areas.iter().any(|area| &area.id == id) {
                return Ok(Site {
                    output: rule.matches.clone(),
                    layer: kind,
                });
            }
        }
    }
    Err(format!(
        "there is no area called `{id}`{}",
        listing("areas", area_ids(layout))
    ))
}

fn area_of<'a>(layout: &'a Layout, site: &Site, id: &AreaId) -> Result<&'a Area, String> {
    layout
        .outputs
        .iter()
        .find(|rule| rule.matches == site.output)
        .and_then(|rule| {
            rule.layers
                .get(site.layer)
                .areas
                .iter()
                .find(|area| &area.id == id)
        })
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
    for rule in &layout.outputs {
        for (kind, layer) in rule.layers.each() {
            for holder in &layer.areas {
                if area.as_ref().is_some_and(|wanted| &holder.id != wanted) {
                    continue;
                }
                if group_of(holder, &group).is_some() {
                    found.push(Spot {
                        site: Site {
                            output: rule.matches.clone(),
                            layer: kind,
                        },
                        area: holder.id.clone(),
                        group: group.clone(),
                    });
                }
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

/// An id no rule of this layout places, said with whatever the layout does have — and with the one case that looks like a missing id and is not.
fn nothing_called(layout: &Layout, id: &str) -> String {
    if in_a_workspace_rule(layout, id) {
        return format!(
            "`{id}` is placed by a workspace rule, and an edit addresses an output's own layers; a workspace rule is written in the layout file"
        );
    }
    format!(
        "nothing in this layout is called `{id}`{}",
        listing("modules", instance_ids(layout))
    )
}

/// Whether an id appears only inside a workspace rule, which no operation addresses.
fn in_a_workspace_rule(layout: &Layout, id: &str) -> bool {
    layout.outputs.iter().any(|rule| {
        rule.workspaces.iter().any(|workspace| {
            workspace.layers.each().into_iter().any(|(_, layer)| {
                layer.areas.iter().any(|area| {
                    area.id.as_str() == id
                        || area.groups.iter().any(|group| {
                            group
                                .children
                                .iter()
                                .any(|instance| instance.id.as_str() == id)
                        })
                })
            })
        })
    })
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
    for rule in &layout.outputs {
        for (_, layer) in rule.layers.each() {
            for area in &layer.areas {
                of(area, &mut ids);
            }
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
        self.find(module)
            .is_some_and(|found| found.input(drawn_as(representation)).is_some())
    }

    fn is_read_only(&self, module: &str, representation: Representation) -> bool {
        self.find(module)
            .and_then(|found| found.input(drawn_as(representation)))
            .is_some_and(|input| input == ui::descriptor::Input::ReadOnly)
    }

    fn command_resolves(&self, line: &str) -> bool {
        super::resolves(line)
    }
}

/// The layout model's five placeable sizes as the descriptor table names them. The table has two more, `Panel` and `Popout`, which are opened rather than placed and so have no way to appear in a layout.
fn drawn_as(representation: Representation) -> ui::host::Representation {
    use ui::host::{Representation as Drawn, WidgetSize};
    match representation {
        Representation::Chip => Drawn::Chip,
        Representation::WidgetS => Drawn::Widget(WidgetSize::S),
        Representation::WidgetM => Drawn::Widget(WidgetSize::M),
        Representation::WidgetL => Drawn::Widget(WidgetSize::L),
        Representation::Card => Drawn::Card,
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
        ui::descriptor::install(crate::core::modules::MODULES);
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("layout-verbs-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a layouts directory");
        std::fs::write(
            dir.join("mine.toml"),
            toml::to_string_pretty(&layout::built_in()).expect("the shipped layout serializes"),
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

    /// The sprint's own criterion: a layout is editable end to end over IPC, and one undo takes back the last edit whatever made it.
    ///
    /// Through the verbs rather than through the store, because what this has to prove is the whole path — the id a verb picks, the size it chooses for the area, the transaction it commits and the reply it sends — and each of those is a place the store's own tests cannot see.
    #[test]
    fn the_verbs_place_change_move_and_remove_a_module_and_undo_takes_each_back() {
        let store = shell_with("verbs", "mine");

        let said = add(&["battery", "bar-top", "end"]).expect("it places one");
        assert!(said.contains("battery"), "{said}");
        assert_eq!(run_of(&store.borrow(), "end"), ["notes", "battery"]);

        set(&["battery", "options.show_percent", "true"]).expect("it sets an option");
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
                .and_then(|it| it.options.get("show_percent"))
                .and_then(toml::Value::as_bool),
            Some(true)
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

    /// An id that only a workspace rule places looks exactly like a missing one, and the difference is what a user needs to hear: no operation addresses a rule's own layers.
    #[test]
    fn an_id_only_a_workspace_rule_places_says_so() {
        let layout: Layout = toml::from_str(
            r#"
            id = "test"
            [[outputs]]
            match = "*"
            [[outputs.workspaces]]
            match = "games"
            [[outputs.workspaces.layers.top.areas]]
            id = "bar-top"
            [[outputs.workspaces.layers.top.areas.groups]]
            id = "end"
            place = "zone"
            zone = "end"
            [[outputs.workspaces.layers.top.areas.groups.children]]
            id = "battery"
            module = "battery"
            "#,
        )
        .expect("it parses");
        let said = nothing_called(&layout, "battery");
        assert!(said.contains("workspace rule"), "{said}");
        assert!(
            nothing_called(&layout, "nothing-at-all").contains("nothing in this layout"),
            "and an id nothing places at all is still what it was"
        );
    }
}
