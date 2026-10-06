//! The rows every area's inspector has, and the rows of the kinds no mode edits: a dock's and a free area's.
//!
//! Each kind's rows are a tool registered by kind name ([`super::add_area_tool`]): the mode that edits a kind registers its rows ([`crate::modes`]) — a bar's, a grid's, a stack's, a region's, a texture's, the prompt's — with the handles they share values with by name ([`AreaDraft::value`]). What every area has comes after them ([`common`]), first among it the switch that edits the area for one workspace alone ([`variant_rows`]), the expression that decides whether it is shown ([`visible_row`]) and what each of its groups repeats over ([`repeat_rows`]).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use telar::{LayoutStyle, Reactive, ReactiveList, SizeDimension};
use telar_expression::Type;

use config::Edge;
use layout::{
    Area, AreaKind, Backdrop, Corners, Expr, Group, GroupId, GroupKind, LayerKind, Origin, Rect,
    ResolvedArea, ResolvedAreaKind, Sides, Unset, Within,
};
use ui::descriptor::Built;

use crate::expr_field::{self, Field, Wanted};

use super::draft::{AreaDraft, kind_field};
use super::handles;
use super::origin::Provenance;
use super::rows::{self, Range, Rows, label};
use super::{Inspector, add_area_tool};

pub(crate) fn install() {
    add_area_tool("dock", dock);
    add_area_tool("free", free);
}

/// What the file says about `key` of `item`, for a row's help.
pub fn help(item: &str, key: &str) -> Option<String> {
    thread_local! {
        static VOCABULARY: Vec<layout::schema::Item> = layout::schema::vocabulary();
    }
    VOCABULARY.with(|vocabulary| {
        vocabulary
            .iter()
            .find(|found| found.name == item)?
            .keys
            .iter()
            .find(|found| found.name == key)?
            .doc
            .map(str::to_string)
    })
}

/// How a model value is spelled in the layout file.
pub fn spelled<T: Serialize>(value: &T) -> String {
    match toml::Value::try_from(value) {
        Ok(toml::Value::String(text)) => text,
        _ => String::new(),
    }
}

/// The model value the layout file spells as `text`.
pub fn parsed<T: DeserializeOwned>(text: &str) -> Option<T> {
    toml::Value::String(text.to_string()).try_into().ok()
}

/// Every spelling of the variants the layout's schema lists for `kind`.
pub(crate) fn variants(kind: &str) -> Rc<[&'static str]> {
    layout::schema::LAYOUT_VARIANTS
        .iter()
        .chain(config::schema::CONFIG_VARIANTS.iter())
        .filter(|(owner, _)| *owner == kind)
        .map(|(_, variant)| *variant)
        .collect()
}

pub(crate) fn edges() -> Rc<[&'static str]> {
    Edge::ALL
        .iter()
        .filter_map(|edge| {
            let spelled = spelled(edge);
            variants("Edge")
                .iter()
                .copied()
                .find(|known| *known == spelled)
        })
        .collect()
}

/// A picker over a model enum for the area's key `key`, the value read from the area as drawn by `read` ([`AreaDraft::setting`]), with where it comes from under it and its Reset ([`AreaDraft::marked`]).
pub(crate) fn chosen<T: Serialize + DeserializeOwned + 'static>(
    draft: &AreaDraft,
    key: &'static str,
    label: Reactive<String>,
    help: Option<String>,
    options: Rc<[&'static str]>,
    read: impl Fn(&ResolvedArea) -> T + 'static,
    write: impl Fn(&mut Area, T) + 'static,
) -> Rows {
    chosen_as(draft, (key, key), label, help, options, read, write)
}

/// [`chosen`] for a value the controls of the popover share under a name that is not the key it is written at: `(name, key)`.
pub(crate) fn chosen_as<T: Serialize + DeserializeOwned + 'static>(
    draft: &AreaDraft,
    (name, key): (&'static str, &'static str),
    label: Reactive<String>,
    help: Option<String>,
    options: Rc<[&'static str]>,
    read: impl Fn(&ResolvedArea) -> T + 'static,
    write: impl Fn(&mut Area, T) + 'static,
) -> Rows {
    let value = draft.setting(
        name,
        key,
        move |area| spelled(&read(area)),
        move |area, text: &String| {
            if let Some(value) = parsed::<T>(text) {
                write(area, value);
            }
        },
    );
    Ok(vec![
        draft.marked(key, rows::choice(label, help, value, options)?)?,
    ])
}

fn dock(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Dock { edge, thickness } = draft.resolved.kind.clone() else {
        return Ok(Inspector::default());
    };
    let kind = draft.kind();
    let mut list = chosen(
        draft,
        "edge",
        label!("editor.area.edge"),
        help("AreaKind::Dock", "edge"),
        edges(),
        move |area| area.kind.edge().unwrap_or(edge),
        move |area, edge: Edge| kind_field!(area, kind, Dock { edge }, edge),
    )?;
    let thickness = draft.setting(
        "thickness",
        "thickness",
        move |area| match area.kind {
            ResolvedAreaKind::Dock { thickness, .. } => thickness,
            _ => thickness,
        },
        move |area, value: &f32| kind_field!(area, kind, Dock { thickness }, *value),
    );
    list.push(draft.marked(
        "thickness",
        rows::number(
            label!("editor.area.thickness"),
            help("AreaKind::Dock", "thickness"),
            thickness,
            handles::THICKNESS,
        )?,
    )?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

/// A free area: a rectangle placed by hand, and where what it holds sits inside it.
fn free(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Free { rect, anchor } = draft.resolved.kind else {
        return Ok(Inspector::default());
    };
    let mut rows = chosen(
        draft,
        "anchor",
        label!("editor.area.anchor"),
        help("AreaKind::Free", "anchor"),
        variants("Anchor"),
        move |area| match area.kind {
            ResolvedAreaKind::Free { anchor, .. } => anchor,
            _ => anchor,
        },
        |area, anchor: layout::Anchor| kind_field!(area, "free", Free { anchor }, anchor),
    )?;
    rows.extend(rect_rows(draft, rect)?);
    Ok(Inspector {
        rows,
        handles: Vec::new(),
    })
}

/// A rectangle's four fractions of the output, for every kind placed by one — the lock's prompt kept wholly on its output and no smaller than it may be (TA-8) — under one line saying where the rectangle comes from, since a level writes it whole.
pub(crate) fn rect_rows(draft: &AreaDraft, seed: Rect) -> Rows {
    let kind = draft.kind();
    let drawn = Rc::new(Cell::new(seed));
    let base = Rc::clone(&drawn);
    let write = move |area: &mut Area, change: &dyn Fn(&mut Rect)| {
        let slot = match AreaDraft::kind_mut(area, kind) {
            Some(
                AreaKind::Grid { rect, .. }
                | AreaKind::WallpaperRegion { rect, .. }
                | AreaKind::Texture { rect, .. }
                | AreaKind::Free { rect, .. }
                | AreaKind::Prompt { rect },
            ) => rect,
            _ => return,
        };
        let rect = slot.get_or_insert(base.get());
        change(rect);
        if kind == "prompt" {
            *rect = rect.kept_on_output(layout::SMALLEST_PROMPT);
        }
    };
    let write = Rc::new(write);
    let side = |name: &'static str, read: fn(&Rect) -> f32, change: fn(&mut Rect, f32)| {
        let (drawn, write) = (Rc::clone(&drawn), Rc::clone(&write));
        draft.setting(
            name,
            "rect",
            move |area| {
                let rect = area.kind.rect().unwrap_or(seed);
                drawn.set(rect);
                read(&rect)
            },
            move |area, value: &f32| write(area, &|rect| change(rect, *value)),
        )
    };
    let x = side("rect.x", |rect| rect.x, |rect, value| rect.x = value);
    let y = side("rect.y", |rect| rect.y, |rect, value| rect.y = value);
    let w = side("rect.w", |rect| rect.w, |rect, value| rect.w = value);
    let h = side("rect.h", |rect| rect.h, |rect, value| rect.h = value);
    let fraction = Range::new(0.0, 1.0, 0.01);
    let sides = vec![
        rows::number(label!("editor.area.x"), help("Rect", "x"), x, fraction)?,
        rows::number(label!("editor.area.y"), help("Rect", "y"), y, fraction)?,
        rows::number(label!("editor.area.w"), help("Rect", "w"), w, fraction)?,
        rows::number(label!("editor.area.h"), help("Rect", "h"), h, fraction)?,
    ];
    Ok(vec![draft.marked("rect", rows::together(sides)?)?])
}

/// What every area has, after its own kind's rows: how it is painted, and what it asks of the compositor — each row saying where its value comes from, with a Reset while the popover's level writes it.
pub(crate) fn common(draft: &AreaDraft) -> Rows {
    let is_bar = draft.kind() == "bar";
    let mut list = variant_rows(&draft.node)?;
    if draft.kind() != "prompt" {
        list.push(visible_row(draft)?);
    }
    list.extend(repeat_rows(draft)?);
    list.extend(parameter_rows(draft)?);
    list.push(rows::heading(|| telar::t!("editor.area.style"))?);
    let fill = draft.setting(
        "style.fill",
        "style.fill",
        |area| area.style.fill.clone().unwrap_or_default(),
        |area, token: &String| area.style.fill = (!token.is_empty()).then(|| token.clone()),
    );
    list.push(draft.marked(
        "style.fill",
        rows::colour(
            label!("editor.area.fill"),
            help("Style", "fill"),
            fill,
            Rc::from(config::theme::PAINT_TOKENS),
            Rc::new(ui::form::swatch_row::is_colour),
        )?,
    )?);
    let faintest = match draft.kind() {
        "prompt" => layout::FAINTEST_PROMPT,
        _ => 0.0,
    };
    let opacity = draft.setting(
        "style.opacity",
        "style.opacity",
        |area| area.style.opacity.unwrap_or(1.0),
        |area, value: &f32| area.style.opacity = Some(*value),
    );
    list.push(draft.marked(
        "style.opacity",
        rows::number(
            label!("editor.area.opacity"),
            help("Style", "opacity"),
            opacity,
            Range::new(faintest, 1.0, 0.05),
        )?,
    )?);
    if draft.kind() == "prompt" {
        list.push(crate::modes::lock::contrast_row(draft)?);
    }
    let padding = draft.setting(
        "style.padding",
        "style.padding",
        |area| area.style.padding.map_or(0.0, Sides::largest),
        |area, value: &f32| area.style.padding = Some(Sides::all(*value)),
    );
    list.push(draft.marked(
        "style.padding",
        rows::number(
            label!("editor.area.padding"),
            help("Style", "padding"),
            padding,
            Range::whole(0.0, 64.0),
        )?,
    )?);
    if !is_bar {
        let radius = draft.setting(
            "style.radius",
            "style.radius",
            |area| area.style.radius.map_or(0.0, Corners::largest),
            |area, value: &f32| area.style.radius = Some(Corners::all(*value)),
        );
        list.push(draft.marked(
            "style.radius",
            rows::number(
                label!("editor.area.radius"),
                help("Style", "radius"),
                radius,
                Range::whole(0.0, handles::most_radius(draft)),
            )?,
        )?);
    }
    list.extend(style_backdrop(draft)?);

    if draft.node.layer == LayerKind::Lock {
        return Ok(list);
    }
    list.push(rows::heading(|| telar::t!("editor.area.behaviour"))?);
    if matches!(draft.kind(), "bar" | "dock") {
        let reserves = draft.setting(
            "reserve",
            "reserve",
            |area| area.reserve,
            |area, on: &bool| area.reserve = Some(*on),
        );
        list.push(draft.marked(
            "reserve",
            rows::toggle(
                label!("editor.area.reserve"),
                help("Area", "reserve"),
                reserves,
            )?,
        )?);
    }
    let lifted = draft.setting(
        "above_fullscreen",
        "above_fullscreen",
        |area| area.above_fullscreen,
        |area, on: &bool| area.above_fullscreen = Some(*on),
    );
    list.push(draft.marked(
        "above_fullscreen",
        rows::toggle(
            label!("editor.area.above_fullscreen"),
            help("Area", "above_fullscreen"),
            lifted,
        )?,
    )?);
    list.push(rows::note(|| telar::t!("editor.area.scanout"))?);
    list.extend(chosen(
        draft,
        "within",
        label!("editor.area.within"),
        help("Area", "within"),
        variants("Within"),
        |area| area.within,
        |area, within: Within| area.within = Some(within),
    )?);
    Ok(list)
}

/// The backdrop picker, and under it, while the area asks for a blur only the compositor can give and this one gives none, why it draws translucent instead (F-10.49).
fn style_backdrop(draft: &AreaDraft) -> Rows {
    let mut list = chosen(
        draft,
        "style.backdrop",
        label!("editor.area.backdrop"),
        help("Style", "backdrop"),
        variants("Backdrop"),
        |area| area.style.backdrop.unwrap_or_default(),
        |area, backdrop: Backdrop| area.style.backdrop = Some(backdrop),
    )?;
    let Some(picked) = draft.shared::<String>("style.backdrop") else {
        return Ok(list);
    };
    let mut blurred = (*draft.resolved).clone();
    blurred.style.backdrop = Some(Backdrop::Blur);
    let by_compositor = surfaces::layer_window::blur_of(draft.node.layer, &blurred)
        == Some(surfaces::layer_window::Blur::Compositor);
    list.push(rows::note(move || {
        let unblurred = by_compositor
            && picked.with(|now| parsed::<Backdrop>(now) == Some(Backdrop::Blur))
            && !platform_wayland::background_effect_supported();
        match unblurred {
            true => telar::t!("editor.texture.blur_unsupported"),
            false => String::new(),
        }
    })?);
    Ok(list)
}

/// "This workspace only", in the popover of anything on a layer with variants where the variant applies to it ([`crate::variant::applies_to`]) and the compositor has said which workspace is up ([`crate::variant::active`]): every change the popover makes is written for that workspace alone while it is on.
pub(crate) fn variant_rows(node: &surfaces::rects::Node) -> Rows {
    let offered = crate::variant::applies_to(node)
        && crate::variant::allowed(node.layer)
        && crate::variant::active().is_some();
    if !offered {
        return Ok(Vec::new());
    }
    Ok(vec![rows::toggle(
        Reactive::of(|| {
            telar::t!(
                "editor.variant.only",
                workspace = crate::variant::active()
                    .map(|workspace| workspace.0)
                    .unwrap_or_default()
            )
        }),
        help("OutputRule", "workspaces"),
        crate::variant::only(),
    )?])
}

/// What an expression row of an area writes: the text its field holds, and whether the level the popover writes takes back the expression it inherits there (DEC-26) — which an expression of its own makes moot.
#[derive(Clone, Debug, Default, PartialEq)]
struct Held {
    text: String,
    taken_back: bool,
}

impl Held {
    fn expr(&self) -> Option<Expr> {
        (!self.text.is_empty()).then(|| Expr(self.text.clone()))
    }

    /// Writes into `unset` whether this takes `path` back.
    fn write_unset(&self, unset: &mut Vec<Unset>, path: Unset) {
        unset.retain(|held| *held != path);
        if self.taken_back && self.text.is_empty() {
            unset.push(path);
        }
    }
}

/// One expression an area row edits — its `visible`, or a group's `repeat`: how it is read and written, and whether a level under the one the popover writes gives it.
struct ExprRow {
    label: Reactive<String>,
    help: Option<String>,
    layer: LayerKind,
    expected: Rc<dyn Fn() -> Wanted>,
    empty: Rc<dyn Fn() -> String>,
    /// What the row holds now, read reactively.
    held: Rc<dyn Fn() -> Held>,
    peek: Rc<dyn Fn() -> Held>,
    write: Rc<dyn Fn(Held)>,
    inherited: bool,
    /// The level that gives the expression on screen.
    writer: Option<Origin>,
    /// Whether Remove may take the inherited expression back where the popover writes, or why not.
    takes_back: Rc<dyn Fn() -> Result<(), String>>,
}

/// The row, made again whenever the level takes the inherited expression back, so its field starts empty from then on, under a line saying where the expression comes from like every other row's. An inherited expression has a Remove beside it that takes it back where the popover writes, previewed and kept or reverted with the rest of the popover.
fn expr_row(row: ExprRow) -> Built {
    let ExprRow {
        label,
        help,
        layer,
        expected,
        empty,
        held,
        peek,
        write,
        inherited,
        writer,
        takes_back,
    } = row;
    let started = peek().text;
    let editing = Rc::clone(&write);
    let taken = {
        let held = Rc::clone(&held);
        telar::memo(move || held().taken_back)
    };
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        move || vec![taken.get()],
        |taken_back: &bool| *taken_back,
        move |taken_back: bool| {
            let now = peek();
            let seed = match taken_back {
                true => String::new(),
                false => now.text.clone(),
            };
            let (restored, reading, writing) =
                (seed.clone(), Rc::clone(&peek), Rc::clone(&editing));
            expr_field::field(Field {
                seed,
                expected: Rc::clone(&expected),
                env: expr_field::environment(layer),
                empty: Rc::clone(&empty),
                checked: Rc::new(move |checked: Option<&str>| {
                    let next = checked.map_or_else(|| restored.clone(), str::to_string);
                    let now = reading();
                    if now.text != next {
                        writing(Held { text: next, ..now });
                    }
                }),
            })
        },
    )?;
    let field = rows::captioned(label, help, Box::new(list))?;
    let said = {
        let (held, takes_back, writer) = (Rc::clone(&held), Rc::clone(&takes_back), writer.clone());
        move || {
            let now = held();
            let inheriting =
                inherited && !now.taken_back && (now.text.is_empty() || now.text == started);
            match (inheriting, now.text.is_empty() || now.taken_back) {
                (true, _) => super::origin::inherited_said(writer.as_ref(), takes_back().is_err()),
                (false, true) => Provenance::Default.said(),
                (false, false) => Provenance::Here.said(),
            }
        }
    };
    let offered = move || inherited && !held().taken_back;
    let removing = write;
    super::origin::captioned(
        field,
        said,
        label!("editor.popover.remove"),
        offered,
        move || match takes_back() {
            Ok(()) => removing(Held {
                text: String::new(),
                taken_back: true,
            }),
            Err(why) => crate::mode::refuse(why),
        },
    )
}

/// When the area is drawn: an expression giving true or false, checked and evaluated as it is typed, on the lock layer over what the lock screen may show. The lock's prompt has none, since it can never be hidden (TA-8).
pub(crate) fn visible_row(draft: &AreaDraft) -> Built {
    let written = draft.area().peek();
    let seed = Held {
        text: draft
            .resolved
            .visible
            .as_ref()
            .map(|visible| visible.expr.0.clone())
            .unwrap_or_default(),
        taken_back: written.unset.contains(&Unset::Visible),
    };
    let inherited = draft.resolved.visible.is_some() && written.visible.is_none();
    let writer = draft
        .resolved
        .visible
        .as_ref()
        .map(|visible| visible.origin.clone());
    let takes_back = taking_back(draft, writer.as_ref());
    let value = draft.value(
        "visible",
        || seed,
        |area, now: &Held| {
            area.visible = now.expr();
            now.write_unset(&mut area.unset, Unset::Visible);
        },
    );
    expr_row(ExprRow {
        label: label!("editor.expr.visible"),
        help: help("Area", "visible"),
        layer: draft.node.layer,
        expected: Rc::new(|| Wanted::Exactly(Type::Bool)),
        empty: Rc::new(|| telar::t!("editor.expr.always")),
        held: Rc::new(move || value.get()),
        peek: Rc::new(move || value.peek()),
        write: Rc::new(move |next| value.set(next)),
        inherited,
        writer,
        takes_back,
    })
}

/// What each group the area places repeats its children over: a list expression per group, one copy of the children per item — but on a grid cell, whose footprint is fixed (DEC-23). A group the area only inherits gets a partial entry naming its `repeat` alone, and only once its expression changes.
pub(crate) fn repeat_rows(draft: &AreaDraft) -> Rows {
    let written = draft.area().peek();
    let groups: Vec<(GroupId, Held, bool, Option<Origin>)> = draft
        .resolved
        .groups
        .iter()
        .filter(|group| !matches!(group.kind, GroupKind::Cell { .. }) && group.komponent.is_none())
        .map(|group| {
            let own = written.groups.iter().find(|held| held.id == group.id);
            let held = Held {
                text: group
                    .repeat
                    .as_ref()
                    .map(|repeat| repeat.expr.0.clone())
                    .unwrap_or_default(),
                taken_back: own.is_some_and(|own| own.unset.contains(&Unset::Repeat)),
            };
            let inherited = group.repeat.is_some() && own.is_none_or(|own| own.repeat.is_none());
            let writer = group.repeat.as_ref().map(|repeat| repeat.origin.clone());
            (group.id.clone(), held, inherited, writer)
        })
        .collect();
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let seeds: BTreeMap<GroupId, Held> = groups
        .iter()
        .map(|(id, held, ..)| (id.clone(), held.clone()))
        .collect();
    let started = seeds.clone();
    let repeats = draft.value(
        "repeat",
        move || seeds.clone(),
        move |area, now: &BTreeMap<GroupId, Held>| {
            for (id, held) in now {
                if started.get(id) == Some(held) {
                    continue;
                }
                let group = match area.groups.iter().position(|group| group.id == *id) {
                    Some(at) => &mut area.groups[at],
                    None => {
                        area.groups.push(Group {
                            id: id.clone(),
                            ..Group::default()
                        });
                        area.groups.last_mut().expect("a group was just pushed")
                    }
                };
                group.repeat = held.expr();
                held.write_unset(&mut group.unset, Unset::Repeat);
            }
        },
    );
    let mut list = vec![rows::heading(|| telar::t!("editor.expr.repeats"))?];
    for (id, _, inherited, writer) in groups {
        let (watched, reading, writing) = (id.clone(), id.clone(), id.clone());
        let group = id.to_string();
        list.push(expr_row(ExprRow {
            label: Reactive::of(move || telar::t!("editor.expr.repeat", group = group.clone())),
            help: help("Group", "repeat"),
            layer: draft.node.layer,
            expected: Rc::new(|| Wanted::AnyList),
            empty: Rc::new(|| telar::t!("editor.expr.once")),
            held: Rc::new(move || {
                repeats.with(|now| now.get(&watched).cloned().unwrap_or_default())
            }),
            peek: Rc::new(move || {
                repeats.peek_with(|now| {
                    now.get(&reading).cloned().unwrap_or(Held {
                        text: String::new(),
                        taken_back: false,
                    })
                })
            }),
            write: Rc::new(move |next| {
                repeats.update(|now| {
                    now.insert(writing.clone(), next);
                });
            }),
            inherited,
            takes_back: taking_back(draft, writer.as_ref()),
            writer,
        })?);
    }
    Ok(list)
}

/// What each group drawing a komponent sets its parameters to: an expression per parameter, of the type the komponent declares, read where the group is drawn — empty for the komponent's default. A value only a level under the popover's sets has a Remove that takes it back there, as an inherited `repeat` has.
pub(crate) fn parameter_rows(draft: &AreaDraft) -> Rows {
    type Key = (GroupId, String);
    let written = draft.area().peek();
    let mut parameters: Vec<(Key, String, layout::ResolvedParameter, Held, bool)> = Vec::new();
    for group in &draft.resolved.groups {
        let Some(used) = &group.komponent else {
            continue;
        };
        let own = written.groups.iter().find(|held| held.id == group.id);
        for parameter in &used.parameters {
            let name = parameter.name.clone();
            let unset = Unset::parameter(&name);
            let held = Held {
                text: parameter
                    .value
                    .as_ref()
                    .map(|value| value.expr.0.clone())
                    .unwrap_or_default(),
                taken_back: own.is_some_and(|own| own.unset.contains(&unset)),
            };
            let inherited = parameter.value.is_some()
                && own.is_none_or(|own| !own.parameters.contains_key(&name));
            let key = (group.id.clone(), name);
            parameters.push((key, used.id.to_string(), parameter.clone(), held, inherited));
        }
    }
    if parameters.is_empty() {
        return Ok(Vec::new());
    }
    let seeds: BTreeMap<Key, Held> = parameters
        .iter()
        .map(|(key, _, _, held, _)| (key.clone(), held.clone()))
        .collect();
    let started = seeds.clone();
    let values = draft.value(
        "parameters",
        move || seeds.clone(),
        move |area, now: &BTreeMap<Key, Held>| {
            for ((id, name), held) in now {
                if started.get(&(id.clone(), name.clone())) == Some(held) {
                    continue;
                }
                let group = match area.groups.iter().position(|group| group.id == *id) {
                    Some(at) => &mut area.groups[at],
                    None => {
                        area.groups.push(Group {
                            id: id.clone(),
                            ..Group::default()
                        });
                        area.groups.last_mut().expect("a group was just pushed")
                    }
                };
                match held.expr() {
                    Some(expr) => group.parameters.insert(name.clone(), expr),
                    None => group.parameters.remove(name),
                };
                held.write_unset(&mut group.unset, Unset::parameter(name));
            }
        },
    );
    let mut list = Vec::new();
    let mut headed: Option<GroupId> = None;
    for ((group, name), komponent, parameter, _, inherited) in parameters {
        if headed.as_ref() != Some(&group) {
            let (shown, used) = (group.to_string(), komponent.clone());
            list.push(rows::heading(move || {
                telar::t!(
                    "editor.komponent.heading",
                    group = shown.clone(),
                    komponent = used.clone()
                )
            })?);
            headed = Some(group.clone());
        }
        let key: Key = (group, name.clone());
        let (watched, reading, writing) = (key.clone(), key.clone(), key);
        let ty = parameter.ty.clone();
        let default = parameter.default.0.clone();
        let writer = parameter.value.as_ref().map(|value| value.origin.clone());
        list.push(expr_row(ExprRow {
            label: Reactive::of(move || name.clone()),
            help: help("Group", "parameters"),
            layer: draft.node.layer,
            expected: Rc::new(move || Wanted::Exactly(ty.clone())),
            empty: Rc::new(move || {
                telar::t!("editor.komponent.default", default = default.clone())
            }),
            held: Rc::new(move || {
                values.with(|now| now.get(&watched).cloned().unwrap_or_default())
            }),
            peek: Rc::new(move || {
                values.peek_with(|now| {
                    now.get(&reading).cloned().unwrap_or(Held {
                        text: String::new(),
                        taken_back: false,
                    })
                })
            }),
            write: Rc::new(move |next| {
                values.update(|now| {
                    now.insert(writing.clone(), next);
                });
            }),
            inherited,
            takes_back: taking_back(draft, writer.as_ref()),
            writer,
        })?);
    }
    Ok(list)
}

/// Whether Remove may take back an inherited expression `writer` wrote where `draft` writes: only where that is laid over `writer`, else it would change nothing on screen, and the refusal says where it can be changed instead.
fn taking_back(draft: &AreaDraft, writer: Option<&Origin>) -> Rc<dyn Fn() -> Result<(), String>> {
    let (draft, writer) = (draft.clone(), writer.cloned());
    Rc::new(move || match &writer {
        Some(writer) if !draft.lays_over(writer) => Err(super::beyond(writer)),
        _ => Ok(()),
    })
}
