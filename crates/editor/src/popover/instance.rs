//! An instance's inspector, generated from what its module declares it takes (F-3.2): one row per option, its control chosen by the option's type and its help the option's doc comment.
//!
//! A toggle for a switch, the options side by side for an enum with a few of them and a dropdown for more, a scrub for a number (with − and + for a short run of whole ones), the accents for a colour, a field for text, and a list, a table of names or a table of options as rows of their own under a heading that folds them away.

use std::rc::Rc;

use telar::{
    AlignItems, Children, Container, LayoutItem, LayoutStyle, Reactive, ReactiveList, RwSignal,
    SizeDimension, box_item, signal,
};
use toml::Value;

use config::Config;
use config::fields::{Control, Number, OptionField};
use layout::Representation;
use ui::descriptor::Built;
use ui::host::WidgetSize;

use super::area::{parsed, spelled};
use super::draft::InstanceDraft;
use super::rows::{self, Range, Rows};
use super::value::{self, Path, Step};

/// Every row of `draft`'s inspector: what it is drawn as where there is a choice, then each of its options in the order its module declares them.
pub fn rows(draft: &InstanceDraft) -> Rows {
    let mut list = representation(draft)?;
    for field in ui::descriptor::option_fields(&draft.resolved.module) {
        list.push(option(draft, value::path_of(&field.key), &field)?);
    }
    Ok(list)
}

/// The options of `module` as a screen with `config` shows an instance setting `own`: its sections, then how it is presented, then what the instance sets.
pub fn shown(config: &Config, module: &str, own: &toml::Table) -> toml::Table {
    let mut shown = toml::Table::new();
    let whole = toml::Value::try_from(config).ok();
    if let Some(descriptor) = ui::descriptor::find(module) {
        for section in descriptor.options {
            if let Some(Value::Table(options)) =
                whole.as_ref().and_then(|whole| whole.get(section.section))
            {
                layout::merge::merge_table(&mut shown, options);
            }
        }
    }
    if let Ok(Value::Table(presented)) =
        toml::Value::try_from(config.presentation(module, &toml::Table::new()))
    {
        layout::merge::merge_table(&mut shown, &presented);
    }
    layout::merge::merge_table(&mut shown, own);
    shown
}

/// Chip, widget of a size, or card, where the area it is in draws more than one of them and its module has more than one.
fn representation(draft: &InstanceDraft) -> Rows {
    if !matches!(draft.area_kind, "grid" | "free") {
        return Ok(Vec::new());
    }
    let Some(descriptor) = ui::descriptor::find(&draft.resolved.module) else {
        return Ok(Vec::new());
    };
    let mut offered: Vec<Representation> = descriptor
        .representations
        .widget
        .map(|widget| {
            widget
                .sizes
                .iter()
                .map(|size| match size {
                    WidgetSize::S => Representation::WidgetS,
                    WidgetSize::M => Representation::WidgetM,
                    WidgetSize::L => Representation::WidgetL,
                })
                .collect()
        })
        .unwrap_or_default();
    if descriptor.representations.card.is_some() {
        offered.push(Representation::Card);
    }
    if offered.len() < 2 {
        return Ok(Vec::new());
    }
    let options: Rc<[&'static str]> = offered
        .iter()
        .filter_map(|representation| spelling(*representation))
        .collect();
    let seed = spelled(&draft.resolved.representation);
    let instance = draft.clone();
    let picked = signal(seed);
    let seeded = std::cell::Cell::new(false);
    telar::effect(move || {
        let text = picked.get();
        if seeded.replace(true) {
            instance.update(|held| held.representation = parsed(&text));
        }
    });
    Ok(vec![rows::choice(
        Reactive::of(|| telar::t!("editor.popover.drawn_as")),
        None,
        picked,
        options,
    )?])
}

fn spelling(representation: Representation) -> Option<&'static str> {
    Some(match representation {
        Representation::Chip => "chip",
        Representation::WidgetS => "widget_s",
        Representation::WidgetM => "widget_m",
        Representation::WidgetL => "widget_l",
        Representation::Card => "card",
    })
}

/// The row for the option `field` at `path`, with a way back to what it inherits once the instance sets it.
pub fn option(draft: &InstanceDraft, path: Path, field: &OptionField) -> Built {
    let label = {
        let field = field.clone();
        Reactive::of(move || field.label())
    };
    let help = field.doc.map(str::to_string);
    let fallback = field.default.clone();
    control(draft, path, &field.control, label, help, fallback)
}

fn control(
    draft: &InstanceDraft,
    path: Path,
    control: &Control,
    label: Reactive<String>,
    help: Option<String>,
    fallback: Option<Value>,
) -> Built {
    match control {
        Control::List(element) => list(draft, path, element, label, help),
        Control::Map(element) => map(draft, path, element, label, help),
        Control::Table(fields) => table(draft, path, fields, label),
        _ => {
            let (building, at, control) = (draft.clone(), path.clone(), control.clone());
            resettable(draft, path, move || {
                scalar(
                    &building,
                    &at,
                    &control,
                    label.clone(),
                    help.clone(),
                    fallback.clone(),
                )
            })
        }
    }
}

/// The row for one value, seeded with what the screen shows for it.
fn scalar(
    draft: &InstanceDraft,
    path: &Path,
    control: &Control,
    label: Reactive<String>,
    help: Option<String>,
    fallback: Option<Value>,
) -> Built {
    let shown = draft.shown_at(path).or(fallback);
    match control {
        Control::Bool => {
            let seed = shown.and_then(|value| value.as_bool()).unwrap_or(false);
            let value = draft.bind_option(path.clone(), seed, |on: &bool| Value::Boolean(*on));
            rows::toggle(label, help, value)
        }
        Control::Enum(variants) => {
            let value = text_of(draft, path, shown);
            rows::choice(label, help, value, Rc::from(variants.as_slice()))
        }
        Control::Number(number) => {
            let seed = shown
                .and_then(|value| value.as_float().or(value.as_integer().map(|n| n as f64)))
                .unwrap_or(number.min.unwrap_or(0.0)) as f32;
            let integer = number.integer;
            let value = draft.bind_option(path.clone(), seed, move |now: &f32| match integer {
                true => Value::Integer(now.round() as i64),
                false => Value::Float(f64::from(*now)),
            });
            rows::number(label, help, value, range_of(number))
        }
        Control::Colour => {
            let value = text_of(draft, path, shown);
            rows::colour(label, help, value, Rc::from(config::theme::ACCENTS))
        }
        Control::Unknown(declared) => {
            let declared = *declared;
            let said = shown.map(|value| value.to_string()).unwrap_or_default();
            rows::note(move || format!("{declared}: {said}"))
        }
        Control::Text | Control::List(_) | Control::Map(_) | Control::Table(_) => {
            rows::text(label, help, text_of(draft, path, shown))
        }
    }
}

fn text_of(draft: &InstanceDraft, path: &Path, shown: Option<Value>) -> RwSignal<String> {
    let seed = shown
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    draft.bind_option(path.clone(), seed, |text: &String| {
        Value::String(text.clone())
    })
}

/// How a number is scrubbed: its declared range and step, else a whole step for a whole number and a hundredth or a tenth for any other.
fn range_of(number: &Number) -> Range {
    let min = number.min.map_or(f32::NEG_INFINITY, |min| min as f32);
    let max = number.max.map_or(f32::INFINITY, |max| max as f32);
    let step = match (number.step, number.integer) {
        (Some(step), _) => step as f32,
        (None, true) => 1.0,
        (None, false) if max - min <= 1.0 => 0.01,
        (None, false) => 0.1,
    };
    Range {
        min,
        max,
        step,
        integer: number.integer,
    }
}

/// The row `build` makes, with a button beside it that takes the option back off the instance while the instance sets it, and the row built again to show what it inherits then.
fn resettable(draft: &InstanceDraft, path: Path, build: impl Fn() -> Built + 'static) -> Built {
    let generation = signal(0u64);
    let row = ReactiveList::with_style(
        LayoutStyle::new().flex_grow(1.0),
        move || vec![generation.get()],
        |built: &u64| *built,
        move |_| build(),
    )?;
    let (showing, watched) = (draft.clone(), path.clone());
    let resetting = draft.clone();
    let reset = ReactiveList::with_style(
        LayoutStyle::new(),
        move || match showing.sets(&watched) {
            true => vec![()],
            false => Vec::new(),
        },
        |_: &()| (),
        move |()| {
            let (draft, path) = (resetting.clone(), path.clone());
            telar::button(
                telar::ButtonProps::props()
                    .label(Reactive::of(|| telar::t!("editor.popover.reset")))
                    .ghost(true)
                    .on_press(Rc::new(move || {
                        draft.unset(&path);
                        generation.update(|built| *built += 1);
                    }))
                    .build(),
                Children::default(),
            )
        },
    )?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(row), Box::new(reset)],
    )?))
}

/// A list: a heading saying how many there are, folded away until opened, and when open a row per element with a way to take it out, and one to add another.
fn list(
    draft: &InstanceDraft,
    path: Path,
    element: &Control,
    label: Reactive<String>,
    help: Option<String>,
) -> Built {
    let element = element.clone();
    let count = {
        let draft = draft.clone();
        let path = path.clone();
        move || {
            draft.instance().with(|_| ());
            match draft.shown_at(&path) {
                Some(Value::Array(items)) => items.len(),
                _ => 0,
            }
        }
    };
    let body = {
        let draft = draft.clone();
        let path = path.clone();
        move |count: usize| -> Rows {
            let mut list = Vec::with_capacity(count + 1);
            for index in 0..count {
                let mut at = path.clone();
                at.push(Step::Index(index));
                let row = element_row(
                    &draft,
                    at,
                    &element,
                    Reactive::of(move || format!("{}", index + 1)),
                    None,
                    None,
                )?;
                list.push(removable(&draft, &path, Step::Index(index), row)?);
            }
            let adding = draft.clone();
            let added = path.clone();
            let blank = blank(&element);
            list.push(telar::button(
                telar::ButtonProps::props()
                    .label(Reactive::of(|| telar::t!("editor.popover.add")))
                    .ghost(true)
                    .on_press(Rc::new(move || {
                        let mut items = match adding.shown_at(&added) {
                            Some(Value::Array(items)) => items,
                            _ => Vec::new(),
                        };
                        items.push(blank.clone());
                        replace(&adding, &added, Value::Array(items));
                    }))
                    .build(),
                Children::default(),
            )?);
            Ok(list)
        }
    };
    folded(label, help, count, body)
}

/// A table of names the user chooses, each holding a value: a row per name with a way to take it out, and a field to name a new one.
fn map(
    draft: &InstanceDraft,
    path: Path,
    element: &Control,
    label: Reactive<String>,
    help: Option<String>,
) -> Built {
    let element = element.clone();
    let names = {
        let draft = draft.clone();
        let path = path.clone();
        move || {
            draft.instance().with(|_| ());
            match draft.shown_at(&path) {
                Some(Value::Table(entries)) => entries.keys().cloned().collect::<Vec<_>>(),
                _ => Vec::new(),
            }
        }
    };
    let count = {
        let names = names.clone();
        move || names().len()
    };
    let body = {
        let draft = draft.clone();
        let path = path.clone();
        move |_: usize| -> Rows {
            let mut list = Vec::new();
            for name in names() {
                let mut at = path.clone();
                at.push(Step::Key(name.clone()));
                let shown_name = name.clone();
                let row = element_row(
                    &draft,
                    at,
                    &element,
                    Reactive::of(move || shown_name.clone()),
                    None,
                    None,
                )?;
                list.push(removable(&draft, &path, Step::Key(name), row)?);
            }
            let named = signal(String::new());
            list.push(rows::text(
                Reactive::of(|| telar::t!("editor.popover.new_name")),
                None,
                named,
            )?);
            let adding = draft.clone();
            let added = path.clone();
            let blank = blank(&element);
            list.push(telar::button(
                telar::ButtonProps::props()
                    .label(Reactive::of(|| telar::t!("editor.popover.add")))
                    .ghost(true)
                    .on_press(Rc::new(move || {
                        let name = named.peek().trim().to_string();
                        let mut entries = match adding.shown_at(&added) {
                            Some(Value::Table(entries)) => entries,
                            _ => toml::Table::new(),
                        };
                        if name.is_empty() || entries.contains_key(&name) {
                            return;
                        }
                        entries.insert(name, blank.clone());
                        replace(&adding, &added, Value::Table(entries));
                    }))
                    .build(),
                Children::default(),
            )?);
            Ok(list)
        }
    };
    folded(label, help, count, body)
}

/// One element of a list of tables: its own options, under the heading its list gave it.
fn table(
    draft: &InstanceDraft,
    path: Path,
    fields: &[OptionField],
    label: Reactive<String>,
) -> Built {
    let mut list = vec![rows::heading(move || label.get())?];
    for field in fields {
        let mut at = path.clone();
        at.extend(value::path_of(&field.key));
        list.push(option(draft, at, field)?);
    }
    column(list)
}

fn removable(draft: &InstanceDraft, path: &Path, step: Step, row: Box<dyn LayoutItem>) -> Built {
    let draft = draft.clone();
    let path = path.clone();
    let remove = telar::button(
        telar::ButtonProps::props()
            .label(Reactive::of(|| telar::t!("editor.popover.remove")))
            .ghost(true)
            .on_press(Rc::new(move || {
                let Some(held) = draft.shown_at(&path) else {
                    return;
                };
                let without = match (held, &step) {
                    (Value::Array(mut items), Step::Index(index)) if *index < items.len() => {
                        items.remove(*index);
                        Value::Array(items)
                    }
                    (Value::Table(mut entries), Step::Key(name)) => {
                        entries.remove(name);
                        Value::Table(entries)
                    }
                    _ => return,
                };
                replace(&draft, &path, without);
            }))
            .build(),
        Children::default(),
    )?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .width(SizeDimension::Percent(1.0)),
        vec![
            box_item(Container::new(
                LayoutStyle::new().flex_grow(1.0),
                vec![row],
            )?),
            remove,
        ],
    )?))
}

/// Sets the whole value at `path` on the instance, a list or a table of names being one value however many elements it has.
fn replace(draft: &InstanceDraft, path: &Path, whole: Value) {
    let shown = Rc::clone(&draft.shown);
    draft.update(|held| value::set(&mut held.options, &shown, path, whole));
}

/// What a new element of `control` holds until it is edited.
fn blank(control: &Control) -> Value {
    match control {
        Control::Bool => Value::Boolean(false),
        Control::Enum(variants) => {
            Value::String(variants.first().copied().unwrap_or_default().into())
        }
        Control::Number(number) if number.integer => {
            Value::Integer(number.min.unwrap_or(0.0) as i64)
        }
        Control::Number(number) => Value::Float(number.min.unwrap_or(0.0)),
        Control::Colour | Control::Text | Control::Unknown(_) => Value::String(String::new()),
        Control::List(_) => Value::Array(Vec::new()),
        Control::Map(_) => Value::Table(toml::Table::new()),
        Control::Table(fields) => {
            let mut table = toml::Table::new();
            for field in fields {
                let value = field
                    .default
                    .clone()
                    .unwrap_or_else(|| blank(&field.control));
                value::set(
                    &mut table,
                    &toml::Table::new(),
                    &value::path_of(&field.key),
                    value,
                );
            }
            Value::Table(table)
        }
    }
}

/// A heading that says how many there are and opens to `body`, built again whenever how many there are changes.
fn folded(
    label: Reactive<String>,
    help: Option<String>,
    count: impl Fn() -> usize + 'static,
    body: impl Fn(usize) -> Rows + 'static,
) -> Built {
    let open = signal(false);
    let count = Rc::new(count);
    let heading = {
        let count = Rc::clone(&count);
        let label = label.clone();
        telar::button(
            telar::ButtonProps::props()
                .label(Reactive::of(move || {
                    let marker = match open.get() {
                        true => "▾",
                        false => "▸",
                    };
                    format!("{marker} {} ({})", label.get(), count())
                }))
                .ghost(true)
                .on_press(Rc::new(move || open.set(!open.peek())))
                .build(),
            Children::default(),
        )?
    };
    let heading = rows::explained(label, help, heading)?;
    let contents = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::sm())
            .padding_left(ui::scale::space::lg())
            .width(SizeDimension::Percent(1.0)),
        move || match open.get() {
            true => vec![count()],
            false => Vec::new(),
        },
        |count: &usize| *count,
        move |count: usize| column(body(count)?),
    )?;
    column(vec![heading, Box::new(contents)])
}

fn column(children: Vec<Box<dyn LayoutItem>>) -> Built {
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::sm())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

/// One element of a list or a table of names: taken out rather than reset, since it has nothing of its own to inherit.
fn element_row(
    draft: &InstanceDraft,
    path: Path,
    control: &Control,
    label: Reactive<String>,
    help: Option<String>,
    fallback: Option<Value>,
) -> Built {
    match control {
        Control::List(_) | Control::Map(_) | Control::Table(_) => {
            self::control(draft, path, control, label, help, fallback)
        }
        _ => scalar(draft, &path, control, label, help, fallback),
    }
}
