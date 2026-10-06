//! Komponents (TA-6): a group saved to `components/<name>.toml` to be used again, and the two ways a group turns into a use of one and back.
//!
//! **Saving** takes a group as its files write it — every level merged, with nothing trust holds back left out, so a line a bundle has not been trusted for goes into the komponent and the store refuses the save rather than the save dropping it — and writes its children, how it arranges them and its `repeat` to a komponent, turning the values the user picks into parameters: an option becomes a parameter whose default is its value, read back through a binding of the same key, and a binding becomes a parameter whose default is its expression. The group is then written as a use of the komponent, which a level does by naming it, since naming a komponent replaces whatever the group held under it.
//!
//! **Detaching** is the other way round: the use's group gets the komponent's children back as instances of its own, under ids the layout does not use yet, with what each parameter reads at that use written into the expressions that read it. The result draws what the use drew.

use std::collections::{BTreeMap, BTreeSet};

use telar_expression::Type;
use util::report::{Message, Report};

use crate::container::Placement;
use crate::library::Library;
use crate::model::*;
use crate::ops::{
    LayoutOp, Site, apply_all, areas_at, free_id, reowned, sites, taken_instance_ids,
};
use crate::resolve::{ActiveWorkspace, KomponentUse, ResolvedGroup, resolve};
use crate::validate::{Catalogue, Mistake, lock_problems, parameter_errors};

/// The komponents `layout` uses, written at any level of it or of a layout it extends: what has to travel with it for it to draw as it does.
pub fn komponents_of(layout: &Layout, library: &Library) -> BTreeSet<KomponentId> {
    crate::resolve::chain_of(layout, library, &mut Default::default())
        .into_iter()
        .flat_map(|level| sites(level).map(|(_, layer)| layer))
        .flat_map(|layer| layer.areas.iter())
        .flat_map(|area| area.groups.iter())
        .filter_map(|group| group.komponent.clone())
        .collect()
}

/// A value of a group that a save can make into a parameter: an option written as a value, or a binding.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// What the parameter would be called: the key, or the child's id and the key where two children have it.
    pub name: String,
    pub child: InstanceId,
    /// The option's key, or the path a binding drives it by (`style.fill` for a style key).
    pub key: String,
    pub ty: Type,
    /// The value as an expression of `ty`: the option's value spelt as one, or the binding's own.
    pub default: Expr,
    /// Whether it is a binding rather than an option.
    pub bound: bool,
}

/// What of `group` a save can turn into parameters: each binding, and each option or bindable style key (`style.fill`, `style.opacity`, `style.border.color`) whose value an expression can spell as the type a binding of that key would give — what `binding_type` answers for a module and a key. A style colour named by a theme token is spelled as the reading of that token, `$theme.<token>`.
pub fn candidates(
    group: &ResolvedGroup,
    binding_type: &dyn Fn(&str, &str) -> Option<Type>,
) -> Vec<Candidate> {
    let mut found = Vec::new();
    for child in &group.children {
        for (key, value) in &child.options {
            let Some(ty) = binding_type(&child.module, key) else {
                continue;
            };
            if child.bindings.contains_key(key) {
                continue;
            }
            if let Some(default) = spelled(value, &ty) {
                found.push((child.id.clone(), key.clone(), ty, Expr(default), false));
            }
        }
        for key in StyleBinding::ALL {
            let path = key.path();
            let Some(value) = key.written(&child.style) else {
                continue;
            };
            let Some(ty) = binding_type(&child.module, path) else {
                continue;
            };
            if child.bindings.contains_key(path) {
                continue;
            }
            if let Some(default) = spelled(&value, &ty).or_else(|| theme_token(&value, &ty)) {
                found.push((child.id.clone(), path.to_string(), ty, Expr(default), false));
            }
        }
        for (path, bound) in &child.bindings {
            let Some(ty) = binding_type(&child.module, path) else {
                continue;
            };
            found.push((child.id.clone(), path.clone(), ty, bound.expr.clone(), true));
        }
    }
    let shared = |key: &str| found.iter().filter(|held| held.1 == key).count() > 1;
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut candidates = Vec::new();
    for (child, key, ty, default, bound) in &found {
        let stem = match shared(key) {
            true => identifier(&format!("{child}_{key}")),
            false => identifier(key),
        };
        let name = (1..)
            .map(|nth| match nth {
                1 => stem.clone(),
                nth => format!("{stem}_{nth}"),
            })
            .find(|name| !named.contains(name) && !is_local(name))
            .expect("the counting runs out long after the names do");
        named.insert(name.clone());
        candidates.push(Candidate {
            name,
            child: child.clone(),
            key: key.clone(),
            ty: ty.clone(),
            default: default.clone(),
            bound: *bound,
        });
    }
    candidates
}

fn is_local(name: &str) -> bool {
    name == crate::validate::ITEM || name == crate::validate::INDEX
}

/// `key` as a name an expression can read: what is not a letter, a digit or `_` becomes `_`, and a leading digit is kept behind one.
fn identifier(key: &str) -> String {
    let mut name: String = key
        .chars()
        .map(|c| match c.is_alphanumeric() || c == '_' {
            true => c,
            false => '_',
        })
        .collect();
    if name
        .chars()
        .next()
        .is_none_or(|first| first.is_ascii_digit())
    {
        name.insert(0, '_');
    }
    name
}

/// `value`, an option's value, as an expression giving `ty`; `None` where no literal of the language says it.
fn spelled(value: &toml::Value, ty: &Type) -> Option<String> {
    Some(match (ty, value) {
        (Type::Text, toml::Value::String(text)) => quoted(text),
        (Type::Number, toml::Value::Integer(n)) => n.to_string(),
        (Type::Number, toml::Value::Float(n)) if n.is_finite() => n.to_string(),
        (Type::Bool, toml::Value::Boolean(b)) => b.to_string(),
        (Type::Color, toml::Value::String(hex)) if config::theme::parse_hex(hex).is_some() => {
            hex.clone()
        }
        (Type::List(item), toml::Value::Array(items)) => {
            let items: Option<Vec<String>> = items.iter().map(|it| spelled(it, item)).collect();
            format!("{{{}}}", items?.join(", "))
        }
        _ => return None,
    })
}

/// A style colour written as a theme token, as the expression that reads the same colour from the palette being painted.
fn theme_token(value: &toml::Value, ty: &Type) -> Option<String> {
    let toml::Value::String(token) = value else {
        return None;
    };
    (*ty == Type::Color && config::theme::THEME_TOKENS.contains(&token.as_str()))
        .then(|| format!("$theme.{token}"))
}

/// `text` as a text literal of the language.
fn quoted(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!("'{escaped}'")
}

/// The komponent `group` is, as its files write it, with each of `chosen` made a parameter: an option taken out of its child's options and read back by a binding of the same key, a binding's expression made the parameter's default and the binding reading the parameter instead.
pub fn saved(group: &ResolvedGroup, chosen: &[Candidate]) -> Komponent {
    let parameters = chosen
        .iter()
        .map(|candidate| {
            let parameter = Parameter {
                ty: ParameterType(candidate.ty.clone()),
                default: candidate.default.clone(),
            };
            (candidate.name.clone(), parameter)
        })
        .collect();
    let children = group
        .children
        .iter()
        .map(|child| {
            let mut instance = Instance {
                id: child.id.clone(),
                module: Some(child.module.clone()),
                representation: Some(child.representation),
                options: child.options.clone(),
                bindings: child
                    .bindings
                    .iter()
                    .map(|(path, bound)| (path.clone(), bound.expr.clone()))
                    .collect(),
                style: child.style.clone(),
                actions: child.actions.clone(),
                unset: Vec::new(),
                ..placed_by(child.placement)
            };
            for candidate in chosen.iter().filter(|it| it.child == child.id) {
                match StyleBinding::from_path(&candidate.key) {
                    Some(key) => key.forget(&mut instance.style),
                    None => {
                        instance.options.remove(&candidate.key);
                    }
                }
                instance
                    .bindings
                    .insert(candidate.key.clone(), Expr(format!("${}", candidate.name)));
            }
            instance
        })
        .collect();
    let written = group.written();
    Komponent {
        parameters,
        arrange: written.arrange,
        cols: written.cols,
        rows: written.rows,
        gap: written.gap,
        repeat: written.repeat,
        children,
    }
}

/// The keys that put a child where `placement` says, a weight of 1 left out as what a child already has.
fn placed_by(placement: Option<Placement>) -> Instance {
    match placement {
        Some(Placement::Weight(weight)) => Instance {
            weight: (weight != 1.0).then_some(weight),
            ..Instance::default()
        },
        Some(Placement::Cell(cell)) => Instance {
            cell: Some(cell),
            ..Instance::default()
        },
        Some(Placement::Rect(rect)) => Instance {
            rect: Some(rect),
            ..Instance::default()
        },
        None => Instance::default(),
    }
}

/// `written`, a group's entry at one level, made a use of the komponent `id`: naming it replaces whatever the levels under it give the group, so the entry keeps its id, placement and style and drops everything it held.
pub fn used(written: &Group, id: &KomponentId) -> Group {
    Group {
        id: written.id.clone(),
        kind: written.kind,
        komponent: Some(id.clone()),
        style: written.style.clone(),
        ..Group::default()
    }
}

/// Why a komponent cannot be put in a group where it was asked: what it is, where it goes or what the use sets.
#[derive(Clone, Debug, PartialEq)]
pub enum UseError {
    /// The library holds no komponent of that name.
    Unknown(KomponentId),
    /// The komponent declares no parameter of that name.
    NoParameter {
        komponent: KomponentId,
        name: String,
        declared: String,
    },
    /// A parameter's expression is not one of the type the komponent declares, or cannot be read where it is drawn.
    Parameter {
        name: String,
        text: Expr,
        mistakes: Vec<Mistake>,
    },
    /// The komponent repeats its children, which a grid cell's fixed footprint cannot hold.
    CellRepeats,
    /// The komponent arranges its children in a way other than `pages`, which a zone does not take.
    ZoneArrange(Arrange),
    /// The lock layer refuses what the komponent holds.
    Lock(KomponentId, Report),
    /// The area is not on the screen.
    NoArea(AreaId),
    /// The group already holds modules of its own, which the komponent would replace.
    Occupied(AreaId, GroupId),
    /// The group already draws a komponent.
    Drawing(AreaId, GroupId, KomponentId),
    /// The area is of a kind that holds no groups of modules.
    NoGroups(AreaId, &'static str),
    /// A grid takes its free cells from the screen that draws it, and none does.
    NoScreen(AreaId),
    /// A zone was named for a group of an area that is not a bar, which has no zones.
    ZoneOffBar(AreaId, &'static str),
    /// A zone was named for a group that exists, which keeps the place it has.
    ZoneOfExisting(AreaId, GroupId),
}

impl UseError {
    pub fn message(&self) -> Message {
        match self {
            UseError::Unknown(id) => util::message!(
                "finding.use_unknown_komponent",
                komponent = id,
                file = crate::library::komponent_path(id)
            ),
            UseError::NoParameter {
                komponent,
                name,
                declared,
            } => util::message!(
                "finding.unknown_parameter",
                komponent = komponent,
                name = name,
                declared = declared
            ),
            UseError::Parameter { name, mistakes, .. } => mistakes.first().map_or_else(
                || util::message!("finding.use_parameter", name = name),
                |mistake| mistake.message.clone(),
            ),
            UseError::NoArea(id) => util::message!("finding.no_area", id = id),
            UseError::CellRepeats => util::message!("finding.cell_repeats"),
            UseError::ZoneArrange(arrange) => {
                util::message!("finding.zone_arrange", arrange = arrange.as_str())
            }
            UseError::Lock(komponent, report) => report.findings().next().map_or_else(
                || util::message!("finding.use_lock", komponent = komponent),
                |finding| finding.message.clone(),
            ),
            UseError::Occupied(area, group) => {
                util::message!("finding.use_occupied", area = area, group = group)
            }
            UseError::Drawing(area, group, komponent) => util::message!(
                "finding.use_drawing",
                area = area,
                group = group,
                komponent = komponent
            ),
            UseError::NoGroups(area, kind) => {
                util::message!("finding.use_no_groups", area = area, kind = *kind)
            }
            UseError::NoScreen(area) => util::message!("finding.use_no_screen", area = area),
            UseError::ZoneOffBar(area, kind) => {
                util::message!("finding.use_zone_off_bar", area = area, kind = *kind)
            }
            UseError::ZoneOfExisting(area, group) => {
                util::message!("finding.use_zone_of_existing", area = area, group = group)
            }
        }
    }

    /// As the command line says it: in English, an expression's mistakes under the text with a caret at each.
    pub fn english(&self) -> String {
        match self {
            UseError::Parameter {
                name,
                text,
                mistakes,
            } => {
                let shown: Vec<String> = mistakes
                    .iter()
                    .map(|mistake| mistake.render(&text.0))
                    .collect();
                format!("parameter `{name}`:\n{}", shown.join("\n"))
            }
            UseError::Lock(_, report) => report
                .findings()
                .map(|finding| finding.message.english())
                .collect::<Vec<_>>()
                .join("\n"),
            other => other.message().english(),
        }
    }
}

/// The komponent `id` as a group that draws it with `parameters` set may use it on `layer`, `in_cell` where the group is a grid cell: what the library holds under that name, with every parameter declared and given an expression of its type, no `repeat` in a cell, nothing but `pages` in a zone, and on the lock layer nothing that answers the pointer or does an action. It is what [`validate_komponents`] reports for the same use once written.
pub fn check_use<'a>(
    library: &'a Library,
    catalogue: &dyn Catalogue,
    (id, parameters): (&KomponentId, &BTreeMap<String, Expr>),
    layer: LayerKind,
    in_cell: bool,
) -> Result<&'a Komponent, UseError> {
    let komponent = library
        .komponent(id)
        .ok_or_else(|| UseError::Unknown(id.clone()))?;
    for name in parameters.keys() {
        if !komponent.parameters.contains_key(name) {
            return Err(UseError::NoParameter {
                komponent: id.clone(),
                name: name.clone(),
                declared: crate::resolve::declared_names(komponent),
            });
        }
    }
    let on_lock = layer == LayerKind::Lock;
    for (name, expr) in parameters {
        let declared = &komponent.parameters[name];
        let mistakes = parameter_errors(catalogue, &declared.ty.0, expr, on_lock);
        if !mistakes.is_empty() {
            return Err(UseError::Parameter {
                name: name.clone(),
                text: expr.clone(),
                mistakes,
            });
        }
    }
    if in_cell && komponent.repeat.is_some() {
        return Err(UseError::CellRepeats);
    }
    if let Some(arrange) = komponent.arrange
        && !in_cell
        && arrange != Arrange::Pages
    {
        return Err(UseError::ZoneArrange(arrange));
    }
    if on_lock {
        let refused = lock_problems(id, komponent, catalogue);
        if !refused.errors.is_empty() {
            return Err(UseError::Lock(id.clone(), refused));
        }
    }
    Ok(komponent)
}

/// `written`, a group's entry at one level, made a use of the komponent `id` with `parameters` set, as [`used`] does.
pub fn used_with(written: &Group, id: &KomponentId, parameters: BTreeMap<String, Expr>) -> Group {
    Group {
        parameters,
        ..used(written, id)
    }
}

/// Why a group's komponent cannot be detached where it was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum DetachError {
    /// The level being edited does not write the group.
    NoGroup(GroupId),
    /// The level writes the group, but not as a use of a komponent.
    NotHere(GroupId),
    /// The komponent it names is not in the library.
    Unknown(KomponentId),
}

impl DetachError {
    pub fn message(&self) -> Message {
        match self {
            DetachError::NoGroup(id) => util::message!("finding.no_group", id = id),
            DetachError::NotHere(id) => util::message!("finding.detach_not_here", group = id),
            DetachError::Unknown(id) => util::message!(
                "finding.unknown_komponent",
                komponent = id,
                file = crate::library::komponent_path(id)
            ),
        }
    }
}

/// The operations that detach the komponent the group `group` of `area` uses, where the level `site` of `layout` names it: the group holds the komponent's children again, each under an id the layout does not use yet, how it arranges them and its `repeat` the komponent's, and each parameter written into what reads it as it reads on the screen `screen` — so the group draws what the use drew. Whatever a level under `site` places in the group, which the use had replaced, is named in the group's `remove`, so it stays away, and a panel a child of the use opens follows that child to its new id.
pub fn detach(
    layout: &Layout,
    library: &Library,
    (site, area, group): (&Site, &AreaId, &GroupId),
    (screen, workspace): (&str, Option<&ActiveWorkspace>),
) -> Result<Vec<LayoutOp>, DetachError> {
    let entry = areas_at(layout, site)
        .iter()
        .find(|held| held.id == *area)
        .ok_or_else(|| DetachError::NoGroup(group.clone()))?;
    let written = entry
        .groups
        .iter()
        .find(|held| held.id == *group)
        .ok_or_else(|| DetachError::NoGroup(group.clone()))?;
    let id = written
        .komponent
        .clone()
        .ok_or_else(|| DetachError::NotHere(group.clone()))?;
    let komponent = library
        .komponent(&id)
        .ok_or_else(|| DetachError::Unknown(id.clone()))?;
    let (resolved, _) = resolve(layout, library, screen, workspace);
    let reads = resolved
        .area(site.layer, area)
        .and_then(|held| held.groups.iter().find(|held| held.id == *group))
        .and_then(|held| held.komponent.clone());
    let values = parameter_values(komponent, reads.as_deref());

    let mut taken = taken_instance_ids(layout, library);
    let children: Vec<Instance> = komponent
        .children
        .iter()
        .map(|child| {
            let fresh = {
                let held: BTreeSet<&str> = taken.iter().map(String::as_str).collect();
                free_id(&held, child.id.as_str())
            };
            taken.insert(fresh.clone());
            Instance {
                id: InstanceId::new(fresh),
                bindings: child
                    .bindings
                    .iter()
                    .map(|(path, expr)| (path.clone(), inlined(expr, &values)))
                    .collect(),
                ..child.clone()
            }
        })
        .collect();
    let arranges = komponent.writes_arrangement();
    let mut detached = Group {
        id: written.id.clone(),
        kind: written.kind,
        arrange: komponent.arrange,
        cols: komponent.cols,
        rows: komponent.rows,
        gap: komponent.gap,
        repeat: komponent
            .repeat
            .as_ref()
            .map(|repeat| inlined(repeat, &values)),
        children,
        style: written.style.clone(),
        remove: written.remove.clone(),
        unset: written
            .unset
            .iter()
            .filter(|unset| match unset {
                Unset::Parameter(_) => false,
                Unset::Arrange => !arranges,
                _ => true,
            })
            .cloned()
            .collect(),
        ..Group::default()
    };
    let replaced = |detached: &Group| {
        let mut changed = entry.clone();
        if let Some(held) = changed.groups.iter_mut().find(|held| held.id == *group) {
            *held = detached.clone();
        }
        vec![LayoutOp::ReplaceArea {
            site: site.clone(),
            id: area.clone(),
            area: Box::new(changed),
        }]
    };
    let ours: BTreeSet<InstanceId> = detached
        .children
        .iter()
        .map(|child| child.id.clone())
        .collect();
    let mut after = layout.clone();
    if apply_all(&mut after, &replaced(&detached)).is_ok() {
        let (now, _) = resolve(&after, library, screen, workspace);
        let under = now
            .area(site.layer, area)
            .and_then(|held| held.groups.iter().find(|held| held.id == *group))
            .map(|held| held.children.clone())
            .unwrap_or_default();
        for child in under.into_iter().filter(|child| !ours.contains(&child.id)) {
            if !detached.remove.contains(&child.id) {
                detached.remove.push(child.id);
            }
        }
    }
    let followed = komponent
        .children
        .iter()
        .zip(&detached.children)
        .flat_map(|(child, fresh)| {
            reowned(
                layout,
                &InstanceId::in_komponent(area, group, &child.id),
                &fresh.id,
            )
        });
    Ok(replaced(&detached).into_iter().chain(followed).collect())
}

/// What each parameter of `komponent` reads at a use: the use's own value where `reads` says it sets one, else the default.
fn parameter_values(
    komponent: &Komponent,
    reads: Option<&KomponentUse>,
) -> BTreeMap<String, String> {
    komponent
        .parameters
        .iter()
        .map(|(name, declared)| {
            let set = reads
                .and_then(|used| used.parameters.iter().find(|held| held.name == *name))
                .map(|held| held.expr().0.clone());
            (
                name.clone(),
                set.unwrap_or_else(|| declared.default.0.clone()),
            )
        })
        .collect()
}

/// `expr` with every `$name` that names one of `values` replaced by that value, bracketed so it reads as one operand; everything else as written. Text that does not parse is left as it is, for validation to report where it is.
pub fn inlined(expr: &Expr, values: &BTreeMap<String, String>) -> Expr {
    let Ok(found) = telar_expression::references_in(&expr.0) else {
        return expr.clone();
    };
    let mut text = expr.0.clone();
    for (span, reference) in found.into_iter().rev() {
        if !reference.path.is_empty() {
            continue;
        }
        if let Some(value) = values.get(&reference.name) {
            text.replace_range(span.range(), &format!("({value})"));
        }
    }
    Expr(text)
}
