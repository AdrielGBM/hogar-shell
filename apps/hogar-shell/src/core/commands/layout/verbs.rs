use layout::ops::{Named, Outside};
use layout::{History, Layout, LayoutStore};
use surfaces::layouts;
use surfaces::reconcile::Desktop;
use surfaces::rects::{Node, Part};
use util::report::Message;

use editor::panel::{Given, Shape};
use editor::stacking::Order;

use super::super::args::{Args, arg};
use super::stored;

pub(super) fn duplicate(args: &Args<'_>) -> Result<String, String> {
    let id = arg(args, 0, "id|area.group")?;
    let (desktop, node) = on_screen(id)?;
    let label = format!("Duplicate `{id}`");
    super::edit_layout(&label, |layout| {
        let (ops, copy) =
            editor::duplicate::duplicated(layout, &desktop, &node).map_err(english)?;
        Ok((ops, format!("duplicated `{id}` as `{}`", name_of(&copy))))
    })
}

pub(super) fn order(args: &Args<'_>) -> Result<String, String> {
    let id = arg(args, 0, "id")?;
    let way = arg(args, 1, "up|down|front|back")?;
    let order = match way {
        "up" => Order::Forward,
        "down" => Order::Backward,
        "front" => Order::Front,
        "back" => Order::Back,
        other => {
            return Err(format!(
                "'{other}' is not a way to move along the order (up, down, front, back)"
            ));
        }
    };
    let (desktop, node) = on_screen(id)?;
    let label = format!("Order `{id}` {way}");
    super::edit_layout(&label, |layout| {
        let ops = editor::stacking::restacked(layout, &desktop, &node, order).map_err(english)?;
        Ok((ops, format!("moved `{id}` {way}")))
    })
}

pub(super) fn panel(args: &Args<'_>) -> Result<String, String> {
    let along = args.contains(&"--along");
    let words: Vec<&str> = args.iter().copied().filter(|it| *it != "--along").collect();
    let id = arg(&words, 0, "instance")?;
    if let Some(extra) = words.get(1) {
        return Err(format!("'{extra}' is not something `layout panel` takes"));
    }
    let (desktop, node) = on_screen(id)?;
    if !matches!(node.part, Part::Instance(..)) {
        return Err(format!(
            "`{id}` is not a placed module, and a panel is opened by one"
        ));
    }
    let shape = match along {
        true => Shape::Along,
        false => Shape::Beside,
    };
    let label = format!("Give `{id}` a panel");
    super::edit_layout(&label, |layout| {
        match editor::panel::given(layout, &desktop, &node, shape).map_err(english)? {
            Given::Held(panel) => Err(format!("`{id}` opens the panel `{panel}` already")),
            Given::Made(ops, panel) => Ok((ops, format!("gave `{id}` the panel `{panel}`"))),
        }
    })
}

pub(super) fn history(_: &Args<'_>) -> Result<String, String> {
    let history = stored(LayoutStore::history)?;
    Ok(editor::history::steps_of(&history)
        .map(|(steps, label)| {
            let label = label.map_or_else(|| editor::history::start().english(), str::to_string);
            format!("{steps}\t{label}")
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

pub(super) fn undo(args: &Args<'_>) -> Result<String, String> {
    let steps = asked(args, |history| history.undo.len())?;
    told(walked(steps, layouts::undo), |labels| {
        format!("took back {}", quoted(labels))
    })
}

pub(super) fn redo(args: &Args<'_>) -> Result<String, String> {
    let steps = asked(args, |history| history.redo.len())?;
    told(walked(steps, layouts::redo), |labels| {
        format!("made {} again", quoted(labels))
    })
}

fn asked(args: &[&str], held: fn(&History) -> usize) -> Result<usize, String> {
    let steps = match args.first() {
        None => 1,
        Some(raw) => raw
            .parse::<usize>()
            .ok()
            .filter(|steps| *steps > 0)
            .ok_or_else(|| format!("[n] must be a number of edits, got '{raw}'"))?,
    };
    if steps > 1 {
        let there = held(&stored(LayoutStore::history)?);
        if steps > there {
            return Err(format!(
                "the history holds {there} of the {steps} edits asked for"
            ));
        }
    }
    Ok(steps)
}

struct Walk {
    taken: Vec<String>,
    stopped: Option<String>,
}

fn walked(steps: usize, mut step: impl FnMut() -> Result<String, Message>) -> Walk {
    let mut taken = Vec::with_capacity(steps);
    for _ in 0..steps {
        match step() {
            Ok(label) => taken.push(label),
            Err(why) => {
                return Walk {
                    taken,
                    stopped: Some(why.english()),
                };
            }
        }
    }
    Walk {
        taken,
        stopped: None,
    }
}

/// A walk that stopped at all is refused, so a script sees it did not get every step it asked for.
fn told(walk: Walk, said: impl Fn(&[String]) -> String) -> Result<String, String> {
    match (walk.stopped, walk.taken.is_empty()) {
        (None, _) => Ok(said(&walk.taken)),
        (Some(why), true) => Err(why),
        (Some(why), false) => Err(format!("{}, then stopped: {why}", said(&walk.taken))),
    }
}

fn quoted(labels: &[String]) -> String {
    labels
        .iter()
        .map(|label| format!("`{label}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn english(why: editor::session::EditError) -> String {
    why.message().english()
}

fn on_screen(id: &str) -> Result<(Desktop, Node), String> {
    let desktops = surfaces::reconcile::desktops_now();
    if desktops.is_empty() {
        return Err(english(editor::session::EditError::no_output()));
    }
    let focused = surfaces::transient::focused_output();
    desktops
        .iter()
        .filter(|desktop| desktop.output == focused)
        .chain(desktops.iter().filter(|desktop| desktop.output != focused))
        .find_map(|desktop| Some((desktop.clone(), named_on(desktop, id)?)))
        .ok_or_else(|| format!("nothing on screen is called `{id}`"))
}

fn named_on(desktop: &Desktop, id: &str) -> Option<Node> {
    let output = desktop.output.as_deref();
    let areas = || desktop.resolved.areas();
    let instance = areas().find_map(|(layer, area)| {
        area.groups.iter().find_map(|group| {
            let child = group
                .children
                .iter()
                .find(|child| child.id.template().as_str() == id)?;
            Some(Node::area(output, layer, &area.id).instance(&group.id, &child.id.template()))
        })
    });
    let area = || {
        areas()
            .find(|(_, area)| area.id.as_str() == id)
            .map(|(layer, area)| Node::area(output, layer, &area.id))
    };
    let group = || {
        let (area, group) = id.split_once('.')?;
        areas()
            .filter(|(_, held)| held.id.as_str() == area)
            .find_map(|(layer, held)| {
                let group = held.groups.iter().find(|it| it.id.as_str() == group)?;
                Some(Node::area(output, layer, &held.id).group(&group.id))
            })
    };
    instance.or_else(area).or_else(group)
}

fn name_of(node: &Node) -> String {
    match &node.part {
        Part::Area => node.area.to_string(),
        Part::Group(group) => format!("{}.{group}", node.area),
        Part::Instance(_, instance) => instance.to_string(),
    }
}

pub(super) fn rename(args: &Args<'_>) -> Result<String, String> {
    let id = arg(args, 0, "id")?.to_string();
    let to = match args.rest(1) {
        "" => return Err("missing argument <new>".to_string()),
        to => to.to_string(),
    };
    let known = stored(|store| store.all().clone())?;
    let config = super::current_config();
    let label = format!("Rename `{id}` to `{to}`");
    super::edit_layout(&label, |layout| {
        let base = layout::reset::base_of(layout, &known);
        let named = one_named(&[layout, &base], &id)?;
        let outside = Outside {
            known: &known,
            rules: &config.rules,
        };
        let ops = layout::ops::rename(layout, outside, &named, &to)
            .map_err(|why| why.message().english())?;
        if ops.is_empty() {
            return Err(format!("`{id}` is called `{to}` already"));
        }
        Ok((ops, format!("renamed `{id}` to `{to}`")))
    })
}

fn one_named(layouts: &[&Layout], id: &str) -> Result<Named, String> {
    let mut found = layouts
        .iter()
        .map(|layout| layout::ops::named(layout, id))
        .find(|found| !found.is_empty())
        .ok_or_else(|| super::nothing_called(layouts[0], id))?;
    if let Some(at) = found
        .iter()
        .position(|named| matches!(named, Named::Instance(_)))
    {
        return Ok(found.swap_remove(at));
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "`{id}` names several things ({}): name a group as <area>.<group>",
            found
                .iter()
                .map(|named| match named {
                    Named::Area { layer, id } => format!("the area `{id}` on {layer}"),
                    Named::Group { area, id, .. } => format!("{area}.{id}"),
                    Named::Instance(id) => id.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walk_refused_partway_says_what_it_took_and_why_it_stopped() {
        let mut left = vec![
            Err(Message::verbatim("the store is gone")),
            Ok("Move `battery`".to_string()),
            Ok("Add `battery`".to_string()),
        ];
        let walk = walked(3, || left.pop().expect("one answer a step"));
        assert_eq!(
            told(walk, |labels| format!("took back {}", quoted(labels))),
            Err(
                "took back `Add `battery``, `Move `battery``, then stopped: the store is gone"
                    .to_string()
            )
        );

        let walk = walked(2, || Err(Message::verbatim("nothing to take back")));
        assert_eq!(told(walk, quoted), Err("nothing to take back".to_string()));
        let walk = walked(2, || Ok("Set".to_string()));
        assert_eq!(told(walk, quoted), Ok("`Set`, `Set`".to_string()));
    }
}
