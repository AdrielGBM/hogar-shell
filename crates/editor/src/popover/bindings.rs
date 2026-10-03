//! An instance's bindings (TA-6): a row for each option an expression drives, and one more for each the popover adds — what it drives, the expression, and a way to take it off.
//!
//! **What a row writes.** Each row asks for its target path alone, by name ([`InstanceDraft::bind`]): the expression where it checks, nothing there where it is emptied, and where it does not check, every path it has touched back as the layout writes it ([`InstanceDraft::keep_binding`]). A row moved to another target takes its expression off the path it started on. So a row never writes what does not check, and a row put back as it was writes nothing. Taking off a row a broader level writes names its path in the instance's `unset` instead (DEC-26).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use telar::{
    AlignItems, Children, Container, LayoutError, LayoutStyle, ReactiveList, SizeDimension,
    box_item, signal,
};
use telar_expression::Type;

use automation::bindings::{ACCENT, Target};
use config::fields::OptionField;
use layout::Expr;
use ui::descriptor::Built;

use crate::expr_field::{self, Field, Wanted};

use super::Inspector;
use super::draft::InstanceDraft;
use super::rows::{self, label};

/// What a row asks of the binding at one path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Bind(String, String),
    Unbind(String),
    /// Taken back where the popover writes, the one a broader level gives it as well (DEC-26).
    TakeBack(String),
    /// As the layout writes it.
    Keep(String),
}

/// What a row that started as `original` (none for a row the popover added), has driven every path in `touched`, and now drives `target` asks for, given what its field says: the expression where it checks (empty for none), `None` where it does not.
pub(crate) fn plan(
    original: Option<&(String, String)>,
    touched: &BTreeSet<String>,
    target: &str,
    checked: Option<&str>,
) -> Vec<Step> {
    let started_at = original.map(|(path, _)| path.as_str());
    let mut paths: BTreeSet<&str> = touched.iter().map(String::as_str).collect();
    paths.insert(target);
    paths.extend(started_at);
    let unchanged = match (original, checked) {
        (_, None) | (None, Some("")) => true,
        (Some((path, text)), Some(now)) => path == target && text == now,
        (None, Some(_)) => false,
    };
    paths
        .into_iter()
        .map(|path| match checked {
            _ if unchanged => Step::Keep(path.to_string()),
            Some("") if path == target && started_at != Some(path) => Step::Keep(path.to_string()),
            Some("") if path == target => Step::Unbind(path.to_string()),
            Some(text) if path == target => Step::Bind(path.to_string(), text.to_string()),
            _ if started_at == Some(path) => Step::Unbind(path.to_string()),
            _ => Step::Keep(path.to_string()),
        })
        .collect()
}

/// What taking a row off asks for: its own binding gone, and every other path it touched as the layout writes it.
pub(crate) fn removal(
    original: Option<&(String, String)>,
    touched: &BTreeSet<String>,
) -> Vec<Step> {
    let started_at = original.map(|(path, _)| path.as_str());
    let mut paths: BTreeSet<&str> = touched.iter().map(String::as_str).collect();
    paths.extend(started_at);
    paths
        .into_iter()
        .map(|path| match started_at == Some(path) {
            true => Step::Unbind(path.to_string()),
            false => Step::Keep(path.to_string()),
        })
        .collect()
}

/// What taking off a row whose binding a broader level writes asks for: [`removal`], with its binding taken back rather than only its own expression taken off.
pub(crate) fn taking_back(
    original: Option<&(String, String)>,
    touched: &BTreeSet<String>,
) -> Vec<Step> {
    removal(original, touched)
        .into_iter()
        .map(|step| match step {
            Step::Unbind(path) => Step::TakeBack(path),
            other => other,
        })
        .collect()
}

fn apply(draft: &InstanceDraft, steps: Vec<Step>) {
    telar::batch(|| {
        for step in steps {
            match step {
                Step::Bind(path, text) => draft.bind(&path, Some(Expr(text))),
                Step::Unbind(path) => draft.bind(&path, None),
                Step::TakeBack(path) => draft.take_back(&path),
                Step::Keep(path) => draft.keep_binding(&path),
            }
        }
    });
}

/// Every path of the instance's module an expression can drive, with what the popover calls it: `accent` first, then each option of a type an expression can give.
fn targets(fields: &[OptionField]) -> Vec<(String, String)> {
    let options = fields
        .iter()
        .filter(|field| Target::of(fields, &field.key).is_ok())
        .map(|field| (field.key.clone(), field.label()));
    std::iter::once((ACCENT.to_string(), telar::t!("editor.expr.accent")))
        .chain(options)
        .collect()
}

/// Where a row starts: the binding it shows as the popover found it, or the target a row the popover added starts at.
#[derive(Clone)]
struct Seed {
    original: Option<(String, String)>,
    target: String,
}

/// The bindings section of an instance's popover.
pub(super) fn tool(draft: &InstanceDraft) -> Result<Inspector, LayoutError> {
    let fields: Rc<[OptionField]> = ui::descriptor::find(&draft.resolved.module)
        .map(|module| module.option_fields())
        .unwrap_or_default()
        .into();
    let targets: Rc<[(String, String)]> = targets(&fields).into();
    let seeds: Rc<RefCell<Vec<Seed>>> = Rc::new(RefCell::new(
        draft
            .bindings()
            .into_iter()
            .map(|(path, expr)| Seed {
                target: path.clone(),
                original: Some((path, expr.0)),
            })
            .collect(),
    ));
    let shown = signal((0..seeds.borrow().len()).collect::<Vec<usize>>());

    let building = draft.clone();
    let (rows_fields, rows_targets, rows_seeds) =
        (Rc::clone(&fields), Rc::clone(&targets), Rc::clone(&seeds));
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::md())
            .width(SizeDimension::Percent(1.0)),
        move || shown.get(),
        |row: &usize| *row,
        move |row: usize| {
            let seed = rows_seeds.borrow()[row].clone();
            binding_row(
                &building,
                seed,
                Rc::clone(&rows_fields),
                Rc::clone(&rows_targets),
                Rc::new(move || shown.update(|rows| rows.retain(|held| *held != row))),
            )
        },
    )?;

    let adding = draft.clone();
    let add = telar::button(
        telar::ButtonProps::props()
            .label(label!("editor.expr.add"))
            .ghost(true)
            .on_press(Rc::new(move || {
                let bound = adding.bindings();
                let target = targets
                    .iter()
                    .find(|(path, _)| !bound.contains_key(path))
                    .or(targets.first())
                    .map_or_else(|| ACCENT.to_string(), |(path, _)| path.clone());
                let row = {
                    let mut seeds = seeds.borrow_mut();
                    seeds.push(Seed {
                        original: None,
                        target,
                    });
                    seeds.len() - 1
                };
                shown.update(|rows| rows.push(row));
            }))
            .build(),
        Children::default(),
    )?;
    Ok(Inspector {
        rows: vec![
            rows::heading(|| telar::t!("editor.expr.bindings"))?,
            Box::new(list),
            add,
        ],
        handles: Vec::new(),
    })
}

/// One binding: what it drives, its expression, and a way to take it off — its own expression deleted, or one a broader level writes taken back where the popover writes ([`InstanceDraft::take_back`]).
fn binding_row(
    draft: &InstanceDraft,
    seed: Seed,
    fields: Rc<[OptionField]>,
    targets: Rc<[(String, String)]>,
    gone: Rc<dyn Fn()>,
) -> Built {
    let Seed { original, target } = seed;
    let touched = Rc::new(RefCell::new(BTreeSet::from([target.clone()])));
    let target = signal(target);
    let original = Rc::new(original);

    let expected = {
        let fields = Rc::clone(&fields);
        Rc::new(move || {
            target.with(|path| {
                Wanted::Exactly(
                    Target::of(&fields, path)
                        .map(|target| target.ty())
                        .unwrap_or(Type::Never),
                )
            })
        })
    };
    let checked = {
        let (draft, touched, original) = (draft.clone(), Rc::clone(&touched), Rc::clone(&original));
        Rc::new(move |checked: Option<&str>| {
            let now = target.peek();
            touched.borrow_mut().insert(now.clone());
            let steps = plan((*original).as_ref(), &touched.borrow(), &now, checked);
            apply(&draft, steps);
        })
    };
    let field = expr_field::field(Field {
        seed: (*original)
            .as_ref()
            .map(|(_, text)| text.clone())
            .unwrap_or_default(),
        expected,
        env: expr_field::instance_environment(&draft.node, &draft.locals()),
        empty: || telar::t!("editor.expr.unbound"),
        checked,
    })?;

    let picker = rows::listed(label!("editor.expr.target"), None, target, targets)?;
    let inherited = (*original)
        .as_ref()
        .is_some_and(|(path, _)| draft.inherits_binding(path));
    let head = vec![
        box_item(Container::new(
            LayoutStyle::new().flex_grow(1.0),
            vec![picker],
        )?),
        {
            let (draft, touched, original) =
                (draft.clone(), Rc::clone(&touched), Rc::clone(&original));
            telar::button(
                telar::ButtonProps::props()
                    .label(label!("editor.popover.remove"))
                    .ghost(true)
                    .on_press(Rc::new(move || {
                        let original = (*original).as_ref();
                        let writer = original
                            .filter(|_| inherited)
                            .and_then(|(path, _)| draft.resolved.bindings.get(path));
                        if let Some(bound) = writer
                            && !draft.lays_over(&bound.origin)
                        {
                            crate::mode::refuse(super::beyond(&bound.origin));
                            return;
                        }
                        let steps = match inherited {
                            true => taking_back(original, &touched.borrow()),
                            false => removal(original, &touched.borrow()),
                        };
                        apply(&draft, steps);
                        gone();
                    }))
                    .build(),
                Children::default(),
            )?
        },
    ];
    let mut column = vec![
        box_item(Container::new(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .width(SizeDimension::Percent(1.0)),
            head,
        )?),
        field,
    ];
    if inherited {
        column.push(rows::note(|| telar::t!("editor.expr.inherited"))?);
    }
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        column,
    )?))
}
