//! Giving an area, a group or an instance another id: every place the layout names it is rewritten by one operation, and a rename that something outside the layout would still address by the old id is refused with what names it.

use std::collections::BTreeSet;
use std::fmt;

use config::RuleConfig;
use util::report::{Message, Report};

use crate::library::{Library, komponent_path};
use crate::model::*;
use crate::ops::{LayoutOp, OpError, sites, sites_mut};
use crate::resolve::{chain_of, layout_path};
use crate::store::BUILT_IN;
use crate::trust::{komponent_chains, layout_chains};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Named {
    Area {
        layer: LayerKind,
        id: AreaId,
    },
    Group {
        layer: LayerKind,
        area: AreaId,
        id: GroupId,
    },
    Instance(InstanceId),
}

impl Named {
    pub fn id(&self) -> &str {
        match self {
            Named::Area { id, .. } => id.as_str(),
            Named::Group { id, .. } => id.as_str(),
            Named::Instance(id) => id.as_str(),
        }
    }

    pub fn called(&self, id: &str) -> Named {
        match self {
            Named::Area { layer, .. } => Named::Area {
                layer: *layer,
                id: AreaId::new(id),
            },
            Named::Group { layer, area, .. } => Named::Group {
                layer: *layer,
                area: area.clone(),
                id: GroupId::new(id),
            },
            Named::Instance(_) => Named::Instance(InstanceId::new(id)),
        }
    }

    pub fn renamed_to(&self, to: &str) -> LayoutOp {
        match self {
            Named::Area { layer, id } => LayoutOp::RenameArea {
                layer: *layer,
                id: id.clone(),
                to: AreaId::new(to),
            },
            Named::Group { layer, area, id } => LayoutOp::RenameGroup {
                layer: *layer,
                area: area.clone(),
                id: id.clone(),
                to: GroupId::new(to),
            },
            Named::Instance(id) => LayoutOp::RenameInstance {
                id: id.clone(),
                to: InstanceId::new(to),
            },
        }
    }

    fn missing(&self) -> OpError {
        match self {
            Named::Area { id, .. } => OpError::NoArea(id.clone()),
            Named::Group { id, .. } => OpError::NoGroup(id.clone()),
            Named::Instance(id) => OpError::NoInstance(id.clone()),
        }
    }

    fn scope(&self) -> Scope<'_> {
        match self {
            Named::Area { layer, .. } => Scope::Area(*layer),
            Named::Group { layer, area, .. } => Scope::Group(*layer, area),
            Named::Instance(_) => Scope::Instance,
        }
    }

    fn written_in(&self, layout: &Layout) -> bool {
        sites(layout).any(|(site, layer)| match self {
            Named::Area { layer: kind, id } => {
                site.layer == *kind && layer.areas.iter().any(|area| area.id == *id)
            }
            Named::Group {
                layer: kind,
                area,
                id,
            } => {
                site.layer == *kind
                    && layer
                        .areas
                        .iter()
                        .filter(|held| held.id == *area)
                        .any(|held| held.groups.iter().any(|group| group.id == *id))
            }
            Named::Instance(id) => layer
                .areas
                .iter()
                .flat_map(|area| &area.groups)
                .any(|group| group.children.iter().any(|child| child.id == *id)),
        })
    }
}

#[derive(Clone, Copy)]
enum Scope<'a> {
    Area(LayerKind),
    Group(LayerKind, &'a AreaId),
    Instance,
}

impl Scope<'_> {
    fn reaches(self, layer: LayerKind) -> bool {
        match self {
            Scope::Area(kind) | Scope::Group(kind, _) => kind == layer,
            Scope::Instance => true,
        }
    }

    /// An owner names an instance by its id, or a komponent child as `<area>.<group>/<child>`. A copy of a repeated child (`<id>#<n>`) owns no panel — the panel belongs to the child as written, which is what the editor offers and what resolution keeps — so it is not an owner to rename.
    fn owner(self, owner: &InstanceId, id: &str, to: &str) -> Option<InstanceId> {
        match self {
            Scope::Instance => (owner.as_str() == id).then(|| InstanceId::new(to)),
            Scope::Area(_) => {
                let child = owner.as_str().strip_prefix(id)?.strip_prefix('.')?;
                child
                    .contains(KOMPONENT_MARK)
                    .then(|| InstanceId::new(format!("{to}.{child}")))
            }
            Scope::Group(_, area) => {
                let child = owner
                    .as_str()
                    .strip_prefix(&InstanceId::komponent_prefix(area, &GroupId::new(id)))?;
                Some(InstanceId::new(format!(
                    "{}{child}",
                    InstanceId::komponent_prefix(area, &GroupId::new(to))
                )))
            }
        }
    }
}

trait Id {
    fn text(&self) -> &str;
    fn of(text: &str) -> Self;
}

macro_rules! ids {
    ($($id:ty),*) => {$(
        impl Id for $id {
            fn text(&self) -> &str {
                self.as_str()
            }

            fn of(text: &str) -> Self {
                Self::new(text)
            }
        }
    )*};
}

ids!(AreaId, GroupId, InstanceId);

fn swap<T: Id>(slot: &mut T, id: &str, to: Option<&str>) -> usize {
    if slot.text() != id {
        return 0;
    }
    if let Some(to) = to {
        *slot = T::of(to);
    }
    1
}

fn mentions(layout: &mut Layout, scope: Scope<'_>, id: &str, to: Option<&str>) -> usize {
    let mut found = 0;
    for (site, layer) in sites_mut(layout) {
        match scope {
            Scope::Area(kind) if site.layer == kind => {
                for removed in &mut layer.remove {
                    found += swap(removed, id, to);
                }
                for area in &mut layer.areas {
                    found += swap(&mut area.id, id, to);
                }
            }
            Scope::Group(kind, holder) if site.layer == kind => {
                for area in layer.areas.iter_mut().filter(|area| area.id == *holder) {
                    for removed in &mut area.remove {
                        found += swap(removed, id, to);
                    }
                    for group in &mut area.groups {
                        found += swap(&mut group.id, id, to);
                    }
                }
            }
            Scope::Instance => {
                for group in layer.areas.iter_mut().flat_map(|area| &mut area.groups) {
                    for removed in &mut group.remove {
                        found += swap(removed, id, to);
                    }
                    for child in &mut group.children {
                        found += swap(&mut child.id, id, to);
                    }
                }
            }
            _ => {}
        }
        if !scope.reaches(site.layer) {
            continue;
        }
        for area in &mut layer.areas {
            if let Some(AreaKind::Panel {
                owner: Some(owner), ..
            }) = &mut area.kind
                && let Some(renamed) = scope.owner(owner, id, to.unwrap_or(id))
            {
                found += 1;
                if to.is_some() {
                    *owner = renamed;
                }
            }
        }
    }
    found
}

fn named_in(layout: &Layout, scope: Scope<'_>, id: &str) -> bool {
    mentions(&mut layout.clone(), scope, id, None) > 0
}

/// Refused where `to` is already named in the same scope, so the inverse renames exactly what this renamed.
pub(crate) fn apply_rename(
    layout: &mut Layout,
    named: &Named,
    to: &str,
) -> Result<LayoutOp, OpError> {
    let from = named.id();
    if from != to && named_in(layout, named.scope(), to) {
        return Err(OpError::Taken(to.to_string()));
    }
    if mentions(layout, named.scope(), from, Some(to)) == 0 {
        return Err(named.missing());
    }
    Ok(named.called(to).renamed_to(from))
}

#[derive(Clone, Copy)]
pub struct Outside<'a> {
    pub known: &'a Library,
    pub rules: &'a [RuleConfig],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reference {
    Rule {
        rule: String,
        line: String,
    },
    Action {
        file: String,
        key: String,
        line: String,
    },
    Extending(LayoutId),
}

impl Reference {
    pub fn message(&self) -> Message {
        match self {
            Reference::Rule { rule, line } => {
                util::message!("finding.rename_in_rule", rule = rule, line = line)
            }
            Reference::Action { file, key, line } => util::message!(
                "finding.rename_in_action",
                line = line,
                key = key,
                file = file
            ),
            Reference::Extending(layout) => {
                util::message!("finding.rename_in_extending", file = layout_path(layout))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameError {
    Empty,
    Refused {
        name: String,
        refusal: IdRefusal,
    },
    Taken(Named),
    Inherited {
        id: String,
        layout: LayoutId,
    },
    Referenced {
        id: String,
        references: Vec<Reference>,
    },
    Op(OpError),
}

impl RenameError {
    pub fn message(&self) -> Message {
        match self {
            RenameError::Empty => util::message!("finding.rename_empty"),
            RenameError::Refused { name, refusal } => refusal.message(name),
            RenameError::Taken(named) => match named {
                Named::Area { id, .. } => util::message!("finding.rename_taken_area", id = id),
                Named::Group { area, id, .. } => {
                    util::message!("finding.rename_taken_group", area = area, id = id)
                }
                Named::Instance(id) => util::message!("finding.rename_taken_instance", id = id),
            },
            RenameError::Inherited { id, layout } => util::message!(
                "finding.rename_inherited",
                id = id,
                file = layout_path(layout)
            ),
            RenameError::Referenced { id, references } => {
                let listed = references
                    .iter()
                    .map(Reference::message)
                    .reduce(|head, next| {
                        util::message!("finding.rename_and", head = head, next = next)
                    })
                    .unwrap_or_else(|| Message::verbatim(String::new()));
                util::message!("finding.rename_referenced", id = id, references = listed)
            }
            RenameError::Op(error) => error.message(),
        }
    }
}

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message().english())
    }
}

impl std::error::Error for RenameError {}

impl From<OpError> for RenameError {
    fn from(error: OpError) -> Self {
        RenameError::Op(error)
    }
}

/// The rename reaches every level of `layout` at once, since a level addresses what an earlier one writes by its id; for the same reason it is refused when a layout `layout` extends names the id. Lines that run commands are never rewritten: a `[[rules]]` command, an action line in this layout's family or in a komponent it uses, or a layout extending this one that names the id refuses the rename instead, each listed.
pub fn rename(
    layout: &Layout,
    outside: Outside<'_>,
    named: &Named,
    to: &str,
) -> Result<Vec<LayoutOp>, RenameError> {
    let from = named.id();
    if from == to {
        return Ok(Vec::new());
    }
    readable(to)?;
    let chain = chain_of(layout, outside.known, &mut Report::default());
    let ancestors = &chain[..chain.len().saturating_sub(1)];
    if let Some(parent) = ancestors
        .iter()
        .rev()
        .find(|level| named_in(level, named.scope(), from))
    {
        return Err(RenameError::Inherited {
            id: from.to_string(),
            layout: parent.id.clone(),
        });
    }
    if !named.written_in(layout) {
        return Err(named.missing().into());
    }
    let extending = extending(layout, outside.known);
    let family: Vec<&Layout> = ancestors
        .iter()
        .copied()
        .chain([layout])
        .chain(extending.iter().copied())
        .collect();
    if family
        .iter()
        .any(|level| named_in(level, named.scope(), to))
    {
        return Err(RenameError::Taken(named.called(to)));
    }
    let references = references(&family, &extending, outside, named);
    if !references.is_empty() {
        return Err(RenameError::Referenced {
            id: from.to_string(),
            references,
        });
    }
    Ok(vec![named.renamed_to(to)])
}

/// Everything `layout` writes that `text` could name, as a command line names it: an instance, an area of any layer, and a group as `<group>` or `<area>.<group>`.
pub fn named(layout: &Layout, text: &str) -> Vec<Named> {
    let mut found = BTreeSet::new();
    let (area_part, group_part) = match text.split_once(GROUP_MARK) {
        Some((area, group)) => (Some(area), group),
        None => (None, text),
    };
    for (site, layer) in sites(layout) {
        for area in &layer.areas {
            if area.id.as_str() == text {
                found.insert(Key::Area(site.layer, area.id.clone()));
            }
            if area_part.is_none_or(|wanted| area.id.as_str() == wanted) {
                for group in area.groups.iter().filter(|g| g.id.as_str() == group_part) {
                    found.insert(Key::Group(site.layer, area.id.clone(), group.id.clone()));
                }
            }
            for group in &area.groups {
                for child in group.children.iter().filter(|c| c.id.as_str() == text) {
                    found.insert(Key::Instance(child.id.clone()));
                }
            }
        }
    }
    found.into_iter().map(Key::named).collect()
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Instance(InstanceId),
    Area(LayerKind, AreaId),
    Group(LayerKind, AreaId, GroupId),
}

impl Key {
    fn named(self) -> Named {
        match self {
            Key::Instance(id) => Named::Instance(id),
            Key::Area(layer, id) => Named::Area { layer, id },
            Key::Group(layer, area, id) => Named::Group { layer, area, id },
        }
    }
}

fn readable(to: &str) -> Result<(), RenameError> {
    if to.is_empty() {
        return Err(RenameError::Empty);
    }
    match id_refusal(to) {
        Some(refusal) => Err(RenameError::Refused {
            name: to.to_string(),
            refusal,
        }),
        None => Ok(()),
    }
}

/// The built-in layout is never edited in place — an edit forks it — so nothing extending it sees a rename.
fn extending<'a>(layout: &Layout, known: &'a Library) -> Vec<&'a Layout> {
    if layout.id.as_str() == BUILT_IN {
        return Vec::new();
    }
    known
        .layouts
        .values()
        .filter(|other| other.id != layout.id)
        .filter(|other| {
            chain_of(other, known, &mut Report::default())
                .iter()
                .any(|level| level.id == layout.id)
        })
        .collect()
}

fn references(
    family: &[&Layout],
    extending: &[&Layout],
    outside: Outside<'_>,
    named: &Named,
) -> Vec<Reference> {
    let id = named.id();
    let mut found: Vec<Reference> = outside
        .rules
        .iter()
        .flat_map(|rule| {
            rule.run
                .iter()
                .filter(|line| names(line, id))
                .map(|line| Reference::Rule {
                    rule: rule.id.clone(),
                    line: line.clone(),
                })
        })
        .collect();
    let mut actions = |file: String, chains: Vec<(String, &Action)>| {
        for (key, action) in chains {
            for line in action.0.iter().filter(|line| names(line, id)) {
                found.push(Reference::Action {
                    file: file.clone(),
                    key: key.clone(),
                    line: line.clone(),
                });
            }
        }
    };
    let mut used = BTreeSet::new();
    for level in family {
        actions(layout_path(&level.id), layout_chains(level));
        used.extend(
            sites(level)
                .flat_map(|(_, layer)| &layer.areas)
                .flat_map(|area| &area.groups)
                .filter_map(|group| group.komponent.clone()),
        );
    }
    for komponent_id in used {
        if let Some(komponent) = outside.known.komponent(&komponent_id) {
            actions(komponent_path(&komponent_id), komponent_chains(komponent));
        }
    }
    found.extend(
        extending
            .iter()
            .filter(|level| named_in(level, named.scope(), id))
            .map(|level| Reference::Extending(level.id.clone())),
    );
    found
}

/// Over-matches on purpose: a line naming `bar-top.end/clock`, `player#2` or `id=value` must not slip through.
fn names(line: &str, id: &str) -> bool {
    let separates = |c: char| is_id_separator(c) || matches!(c, '=' | ',' | '"' | '\'');
    if id.chars().any(separates) {
        return line.contains(id);
    }
    line.split(separates).any(|word| word == id)
}
