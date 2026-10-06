//! Where the value a popover row shows comes from — written where the popover writes, inherited from a level under it, overridden by a level after it, or nobody's and so the default — and the Reset that takes a key written here back off, so it inherits again.
//!
//! What is read is the screen as the preview draws it ([`reconcile::desktop`]), so a row says "set here" the moment its control moves and names what it inherits the moment Reset takes it back.

use std::cell::RefCell;
use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use telar::{
    AlignItems, Children, Container, LayoutItem, LayoutStyle, Reactive, ReactiveList, RwSignal,
    SizeDimension, box_item,
};

use layout::{Holder, Layout, LayoutOp, Level, Origin, Resolved, Site};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::Node;
use ui::descriptor::Built;

use crate::session::Edit;

use super::rows::{self, label};

/// Where the value of one key a row edits comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Provenance {
    /// The level the popover writes into writes it.
    Here,
    /// A level the popover's is laid over writes it, so the popover inherits it.
    Inherited(Level),
    /// A level laid over the popover's writes it, so a change written here would not show.
    Overridden(Level),
    /// No level writes it: what is drawn is the default.
    Default,
}

impl Provenance {
    /// What a row says about it.
    pub fn said(&self) -> String {
        match self {
            Provenance::Here => telar::t!("editor.origin.here"),
            Provenance::Inherited(level) => {
                telar::t!(
                    "editor.origin.from",
                    rule = level.rule(),
                    file = level.file()
                )
            }
            Provenance::Overridden(level) => {
                telar::t!(
                    "editor.origin.beyond",
                    rule = level.rule(),
                    file = level.file()
                )
            }
            Provenance::Default => telar::t!("editor.origin.default"),
        }
    }
}

/// The level a popover writes into and the layout it was opened on, which is what every row's [`Provenance`] is measured against.
#[derive(Clone)]
pub(crate) struct Measure {
    edit: Edit,
    node: Node,
    /// The layout as it was when the popover's edit began, once it has: the order of its rules is what decides which level lies over which.
    before: Rc<RefCell<Option<Rc<Layout>>>>,
    /// The popover's screen as drawn now, preview included: read once per change for every row rather than once per row.
    screen: RwSignal<Option<Rc<Resolved>>>,
}

impl Measure {
    pub(crate) fn new(edit: &Edit, node: &Node) -> Self {
        let screen = telar::signal(None);
        let output = node.output.clone();
        telar::effect(move || {
            let drawn =
                reconcile::desktop(output.as_deref()).map(|desktop| Rc::new(desktop.resolved));
            screen.set(drawn);
        });
        Self {
            edit: edit.clone(),
            node: node.clone(),
            before: Rc::default(),
            screen,
        }
    }

    fn layout(&self) -> Rc<Layout> {
        if let Some(held) = self.before.borrow().as_ref() {
            return Rc::clone(held);
        }
        match self.edit.transaction().before() {
            Some(before) => {
                let before = Rc::new(before);
                *self.before.borrow_mut() = Some(Rc::clone(&before));
                before
            }
            None => Rc::new(crate::session::draft().peek()),
        }
    }

    /// Whether what the level `site` names writes is laid over `writer` on the popover's screen ([`layout::lays_over`]).
    pub(crate) fn lays_over(&self, site: &Site, writer: &Origin) -> bool {
        self.lays_over_in(&self.layout(), site, writer)
    }

    fn lays_over_in(&self, before: &Layout, site: &Site, writer: &Origin) -> bool {
        layout::lays_over(
            before,
            self.node.output.as_deref().unwrap_or_default(),
            &level_in(before, site),
            writer,
        )
    }

    /// Where `key` of `holder` comes from on the screen as drawn now, for a popover writing into `site`. Reactive.
    pub(crate) fn provenance(&self, site: &Site, holder: Holder<'_>, key: &str) -> Provenance {
        let writer = self.screen.with(|screen| {
            screen
                .as_ref()?
                .level_of(self.node.layer, holder, key)
                .cloned()
        });
        let Some(writer) = writer else {
            return Provenance::Default;
        };
        let before = self.layout();
        if writer == level_in(&before, site) {
            return Provenance::Here;
        }
        match self.lays_over_in(&before, site, &Origin::Level(writer.clone())) {
            true => Provenance::Inherited(writer),
            false => Provenance::Overridden(writer),
        }
    }

    /// The popover's screen as it draws `ops` laid over the layout its edit began on, resolved here rather than read from the preview, which may not have caught up yet.
    pub(crate) fn drawn_with(&self, ops: &[LayoutOp]) -> Option<Desktop> {
        let mut after = self.edit.transaction().before()?;
        layout::ops::apply_all(&mut after, ops).ok()?;
        let screen = reconcile::desktop_now(self.node.output.as_deref())?;
        Some(screen.resolving(&after, &crate::written::known()))
    }
}

/// The level a popover writing into `site` of `before` writes.
fn level_in(before: &Layout, site: &Site) -> Level {
    Level {
        layout: before.id.clone(),
        output: site.output.clone(),
        workspace: site.workspace.clone(),
    }
}

/// `row` with a line under it saying where its value comes from, and a Reset while what the popover writes holds it.
pub(crate) fn marked(
    row: Box<dyn LayoutItem>,
    provenance: impl Fn() -> Provenance + 'static,
    writes: impl Fn() -> bool + 'static,
    reset: impl Fn() + 'static,
) -> Built {
    let standing = telar::memo(provenance);
    noted(
        row,
        move || standing.get().said(),
        label!("editor.popover.reset"),
        writes,
        reset,
    )
}

/// `row` with `said` under it, and a button called `action` beside that while `offered`, which does `press`.
pub(crate) fn noted(
    row: Box<dyn LayoutItem>,
    said: impl Fn() -> String + 'static,
    action: Reactive<String>,
    offered: impl Fn() -> bool + 'static,
    press: impl Fn() + 'static,
) -> Built {
    let said = rows::note(said)?;
    let press = Rc::new(press);
    let button = ReactiveList::with_style(
        LayoutStyle::new(),
        move || match offered() {
            true => vec![()],
            false => Vec::new(),
        },
        |_: &()| (),
        move |()| {
            let press = Rc::clone(&press);
            telar::button(
                telar::ButtonProps::props()
                    .label(action.clone())
                    .ghost(true)
                    .on_press(Rc::new(move || press()))
                    .build(),
                Children::default(),
            )
        },
    )?;
    let line = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .width(SizeDimension::Percent(1.0)),
        vec![
            box_item(Container::new(
                LayoutStyle::new().flex_grow(1.0),
                vec![said],
            )?),
            Box::new(button),
        ],
    )?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        vec![row, box_item(line)],
    )?))
}

/// What the line under an expression's row says, in the same words as every other row's: set here, else given by `inherited` — a level laid over the popover's where `beyond` says so — else the default.
pub(crate) fn expression_said(
    here: bool,
    inherited: Option<&Origin>,
    beyond: impl FnOnce() -> bool,
) -> String {
    if here {
        return Provenance::Here.said();
    }
    match inherited {
        Some(Origin::Level(level)) => match beyond() {
            true => Provenance::Overridden(level.clone()),
            false => Provenance::Inherited(level.clone()),
        }
        .said(),
        Some(komponent @ Origin::Komponent(_)) => {
            telar::t!("editor.origin.komponent", file = komponent.file())
        }
        None => Provenance::Default.said(),
    }
}

/// `entry` as a layout file writes it, as a table.
pub(crate) fn table_of<T: Serialize>(entry: &T) -> Option<toml::Table> {
    toml::Table::try_from(entry).ok()
}

/// Whether `table` holds a value at the dotted `key`.
pub(crate) fn holds(table: &toml::Table, key: &str) -> bool {
    let mut parts = key.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let mut at = table.get(first);
    for part in parts {
        at = match at {
            Some(toml::Value::Table(inner)) => inner.get(part),
            _ => None,
        };
    }
    at.is_some()
}

/// `entry` without the value at the dotted `key`, a table emptied by taking it out taken out with it: an entry that writes nothing of what it held there, so it inherits it. `None` where `entry` does not hold it.
pub(crate) fn without<T: Serialize + DeserializeOwned>(entry: &T, key: &str) -> Option<T> {
    let mut table = table_of(entry)?;
    let parts: Vec<&str> = key.split('.').collect();
    if !take_out(&mut table, &parts) {
        return None;
    }
    table.try_into().ok()
}

fn take_out(table: &mut toml::Table, parts: &[&str]) -> bool {
    match parts {
        [] => false,
        [last] => table.remove(*last).is_some(),
        [first, rest @ ..] => {
            let Some(toml::Value::Table(inner)) = table.get_mut(*first) else {
                return false;
            };
            let taken = take_out(inner, rest);
            if inner.is_empty() {
                table.remove(*first);
            }
            taken
        }
    }
}
