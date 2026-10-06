//! The Id row of the area, group and instance popovers: the item's id, typed and renamed everywhere the layout names it as one undo entry, with why it keeps its id under the row when it must.

use telar::signal;

use layout::LayoutStore;
use layout::ops::{Named, Outside};
use surfaces::rects::{Node, Part};
use ui::descriptor::Built;

use crate::session::{self, EditError, Selection};

use super::rows::{self, label};

pub(crate) fn row(node: &Node) -> Built {
    let Some(named) = named(node) else {
        return rows::together(Vec::new());
    };
    let typed = signal(named.id().to_string());
    let refused = signal(String::new());
    let field = rows::text(label!("editor.popover.id"), None, typed)?;
    let node = node.clone();
    let button = rows::action(
        || telar::t!("editor.popover.rename"),
        move || {
            let to = typed.peek().trim().to_string();
            refused.set(rename(&node, &named, &to).err().unwrap_or_default());
        },
    )?;
    let why = rows::note(move || refused.get())?;
    rows::together(vec![field, button, why])
}

fn named(node: &Node) -> Option<Named> {
    Some(match &node.part {
        Part::Area => Named::Area {
            layer: node.layer,
            id: node.area.clone(),
        },
        Part::Group(group) => Named::Group {
            layer: node.layer,
            area: node.area.clone(),
            id: group.clone(),
        },
        Part::Instance(_, id) if id.komponent_child().is_some() => return None,
        Part::Instance(_, id) => Named::Instance(id.template()),
    })
}

/// Planned against the draft, which is what the popover's open edit previews ([`session::Edit::preview`]) and so holds a line typed into it that names the id, as its commit will. The popover's own change is kept first, as closing it would keep it, since its rows address the item by the id it is losing.
fn rename(node: &Node, named: &Named, to: &str) -> Result<(), String> {
    let (edit, open) = super::OPEN
        .with(|held| {
            held.borrow()
                .as_ref()
                .map(|open| (open.edit.clone(), open.open))
        })
        .ok_or_else(|| EditError::NotOpen.to_string())?;
    let layout = session::draft().peek();
    let known =
        surfaces::layouts::read(|store: &LayoutStore| store.all().clone()).unwrap_or_default();
    let config = config::config();
    let rules = config
        .as_deref()
        .map(|config| config.rules.as_slice())
        .unwrap_or_default();
    let ops = layout::ops::rename(
        &layout,
        Outside {
            known: &known,
            rules,
        },
        named,
        to,
    )
    .map_err(|why| why.message().render())?;
    if ops.is_empty() {
        return Ok(());
    }
    if edit.is_open() {
        edit.commit().map_err(|why| why.to_string())?;
    }
    let committed = crate::context::commit(
        telar::t!(
            "editor.popover.renamed",
            from = named.id().to_string(),
            to = to.to_string()
        ),
        ops,
    );
    open.set(false);
    match committed {
        Ok(()) => {
            session::select(Selection::of(renamed(node, to)));
        }
        Err(why) => crate::mode::refuse(why),
    }
    Ok(())
}

fn renamed(node: &Node, to: &str) -> Node {
    let part = match &node.part {
        Part::Area => {
            return Node::area(node.output.as_deref(), node.layer, &layout::AreaId::new(to));
        }
        Part::Group(_) => Part::Group(layout::GroupId::new(to)),
        Part::Instance(group, _) => Part::Instance(group.clone(), layout::InstanceId::new(to)),
    };
    Node {
        part,
        ..node.clone()
    }
}
