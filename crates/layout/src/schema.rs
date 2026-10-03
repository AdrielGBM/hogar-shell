//! The annotated layout file, generated rather than written.
//!
//! `hogar-shell config schema layout` prints a layout that parses back as one, with every key explained by the doc comment on the field that backs it — so the reference for the file everything the shell draws is written in is a view of [`crate::model`] rather than a second copy of it. Same pipeline the config's own schema goes through, and the same scanner behind it (`hogar-shell-doc-scanner`).
//!
//! **Its failure mode is silence.** A comment the scanner does not recognise is simply not printed, and nothing about a missing one looks wrong — which is how the whole of this crate's prose stayed invisible to the config's generator for a sprint (F-10.8). So two tests hold the pipeline down: one takes a comment from the table and looks for it in the printed file, and one asserts that every item with keys is named in [`vocabulary`], which is what the manual and `docs/reference/layout.md` walk.

use std::fmt::Write as _;

use crate::model::*;

include!(concat!(env!("OUT_DIR"), "/layout_docs.rs"));

/// The items the reference walks, in the order a reader meets them: a layout, then what it holds, then what an area can be, then what goes inside one.
///
/// Written out rather than taken from the table, because the table is in declaration order and a reference is not. [`every_documented_item_is_in_the_vocabulary`](tests::every_documented_item_is_in_the_vocabulary) is what keeps the list honest when the model grows.
const ORDER: &[&str] = &[
    "Layout",
    "Source::Poll",
    "Source::Listen",
    "Source::Http",
    "OutputRule",
    "WorkspaceRule",
    "Layers",
    "SessionLayers",
    "Layer",
    "Area",
    "AreaKind::Bar",
    "AreaKind::Grid",
    "AreaKind::Stack",
    "AreaKind::WallpaperRegion",
    "AreaKind::Texture",
    "AreaKind::Dock",
    "AreaKind::Free",
    "AreaKind::Prompt",
    "AreaStyle",
    "BarShape",
    "AutoHide",
    "Rect",
    "Gradient",
    "GradientStop",
    "Tile::NineSlice",
    "Offset",
    "Route",
    "Group",
    "GroupKind::Zone",
    "GroupKind::Cell",
    "Instance",
];

/// One item of the layout file's vocabulary: a table a file can hold, or a variant of one, with what it is for and the keys it has.
pub struct Item {
    pub name: &'static str,
    pub doc: Option<&'static str>,
    pub keys: Vec<Key>,
}

pub struct Key {
    pub name: &'static str,
    pub doc: Option<&'static str>,
}

/// The layout file's vocabulary as data, for the manual and the docs reference to walk. [`render`] is the other reading of the same tables: the same explanations, printed as a file a user can edit.
pub fn vocabulary() -> Vec<Item> {
    ORDER
        .iter()
        .map(|name| Item {
            name,
            doc: doc_for(name, ""),
            keys: LAYOUT_FIELDS
                .iter()
                .filter(|(item, _)| item == name)
                .map(|(_, field)| Key {
                    name: field,
                    doc: explains(name, field),
                })
                .collect(),
        })
        .collect()
}

/// The layout the reference prints.
///
/// The shipped layout plus one area of every kind it does not use, so the file shows the whole vocabulary — and still parses back as a layout, which a reference written by hand would stop doing the first time a field was renamed. The extra areas hold no instances: what a module is called is the module table's business, and a reference that named one would be a reference that goes stale when a module is renamed.
pub fn reference() -> Layout {
    let mut layout = crate::built_in::layout();
    layout.sources.insert(
        "load".to_string(),
        Source::Poll {
            cmd: Some("cut -d ' ' -f 1 /proc/loadavg".to_string()),
            every: Some("5s".to_string()),
            initial: Some(toml::Value::Float(0.0)),
            parse: Some("text".to_string()),
            while_: Some(While::Visible),
            lock_safe: Some(false),
        },
    );
    let Some(rule) = layout.outputs.first_mut() else {
        return layout;
    };
    rule.layers.desktop.areas.extend([
        area(
            "cards",
            AreaKind::Stack {
                anchor: Some(Anchor::TopRight),
                offset: Some(Offset { x: 0.0, y: 48.0 }),
                width: Some(380.0),
                output_policy: Some(StackOutputPolicy::Focused),
                routes: vec![Route {
                    kind: Some(CardKind::Notification),
                    app: None,
                    urgency: None,
                }],
                launcher: Some(false),
            },
        ),
        area(
            "overlay",
            AreaKind::Texture {
                rect: Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 0.25,
                }),
                image: None,
                gradient: Some(Gradient {
                    angle: 90.0,
                    stops: vec![
                        GradientStop {
                            at: 0.0,
                            color: "base".into(),
                        },
                        GradientStop {
                            at: 1.0,
                            color: "#00000000".into(),
                        },
                    ],
                }),
                tile: Some(Tile::None),
                blend: Some(Blend::Normal),
                opacity: Some(0.6),
            },
        ),
        area(
            "visualiser",
            AreaKind::Dock {
                edge: Some(config::Edge::Bottom),
                thickness: Some(96.0),
            },
        ),
        Area {
            style: AreaStyle {
                radius: Some(Corners::each(16.0, 16.0, 0.0, 16.0)),
                ..AreaStyle::default()
            },
            ..area(
                "note",
                AreaKind::Free {
                    rect: Some(Rect {
                        x: 0.7,
                        y: 0.1,
                        w: 0.25,
                        h: 0.2,
                    }),
                },
            )
        },
    ]);
    layout
}

fn area(id: &str, kind: AreaKind) -> Area {
    Area {
        id: AreaId::new(id),
        kind: Some(kind),
        ..Area::default()
    }
}

/// The reference layout as a file, with each key explained above the first place it appears.
///
/// **Once, not everywhere.** A layout repeats its shapes — every area has an `id`, every bar a `thickness` — and a comment above each of them would bury the file it is explaining. So the first bar in the printed reference carries the whole explanation and the ones after it are bare, which is also the order somebody reads it in.
pub fn render() -> Result<String, String> {
    let printed =
        toml::to_string_pretty(&reference()).map_err(|why| format!("printing a layout: {why}"))?;
    let mut out = String::from(
        "# hogar-shell layout reference\n\
         #\n\
         # Generated by `hogar-shell config schema layout` from this build's own model.\n\
         # A layout lives in ~/.config/hogar-shell/layouts/<name>.toml, one file per layout, and\n\
         # `hogar-shell layout use <name>` chooses which one is drawn. Each key is explained above\n\
         # the first place it appears.\n\n",
    );

    let mut said: Vec<String> = Vec::new();
    let mut table = String::new();
    let mut flattened_variant: Option<String> = None;

    for line in printed.lines() {
        if let Some(path) = header_of(line) {
            table = path;
            flattened_variant = None;
            if let Some(item) = item_at(&table) {
                say(&mut out, &mut said, item, "");
            }
            let _ = writeln!(out, "{line}");
            continue;
        }
        let Some((key, value)) = line.split_once(" = ") else {
            let _ = writeln!(out, "{line}");
            continue;
        };
        let key = key.trim();
        // A `kind` or a `place` decides what the keys after it in this table are, since a variant's fields are written flattened beside them.
        if let Some(variant) = variant_named(&table, key, value) {
            flattened_variant = Some(variant);
        }
        let item = match item_at(&table) {
            Some(item) if doc_for(item, key).is_some() || flattened_variant.is_none() => Some(item),
            _ => flattened_variant.as_deref(),
        };
        if let Some(item) = item {
            say(&mut out, &mut said, item, key);
        }
        let _ = writeln!(out, "{line}");
    }
    Ok(out)
}

/// What to say about a key: its own comment, or — where it has none and its value is a table of its own — what that table is.
///
/// The rule the config's reference already follows for a list of tables: what an `[[idle.stages]]` table means is what an `IdleStage` is. So `areas` borrows what an `Area` is rather than carrying a sentence that would say it twice, while a key whose value is a number has no type to borrow from and stays bare rather than restating its own name.
fn explains(item: &str, key: &str) -> Option<&'static str> {
    doc_for(item, key).or_else(|| type_of(item, key).and_then(|kind| doc_for(kind, "")))
}

/// Writes an item's or a key's explanation, the first time it is asked for.
fn say(out: &mut String, said: &mut Vec<String>, item: &str, key: &str) {
    let Some(doc) = explains(item, key) else {
        return;
    };
    let at = format!("{item}.{key}");
    if said.contains(&at) {
        return;
    }
    said.push(at);
    for line in doc.lines() {
        match line.is_empty() {
            true => out.push_str("#\n"),
            false => {
                let _ = writeln!(out, "# {line}");
            }
        }
    }
}

/// `[a.b]` or `[[a.b]]` → `a.b`.
fn header_of(line: &str) -> Option<String> {
    let inner = line
        .trim()
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .or_else(|| {
            line.trim()
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
        })?;
    Some(inner.to_string())
}

/// Which item the table at `path` is, by walking the field types from [`Layout`] down.
///
/// The path a file writes is the path of *fields*, so `outputs.layers.top.areas.groups` walks `Layout.outputs` → `OutputRule.layers` → `Layers.top` → `Layer.areas` → `Area.groups`. A step through a field with no plain type behind it — an instance's `options`, which is a table of whatever the module reads — ends the walk, and the keys under it are the module's rather than the model's. A table keyed by name, such as `sources`, takes the name as its next step and lands on what each entry is.
fn item_at(path: &str) -> Option<&'static str> {
    let mut item = "Layout";
    if path.is_empty() {
        return Some(item);
    }
    let mut steps = path.split('.');
    while let Some(step) = steps.next() {
        item = match type_of(item, step) {
            Some(next) => next,
            None => {
                let held = keyed_by_name(item, step)?;
                steps.next()?;
                held
            }
        };
    }
    Some(item)
}

/// What a field holding a table keyed by name holds in each entry, so the step after `sources` — a source's name — lands on a `Source`.
fn keyed_by_name(item: &str, field: &str) -> Option<&'static str> {
    LAYOUT_FIELD_RUST
        .iter()
        .find(|(owner, name, _)| *owner == item && *name == field)?
        .2
        .strip_prefix("BTreeMap<String, ")?
        .strip_suffix('>')
}

/// The variant a `kind = "bar"` or `place = "zone"` line names, as the item its flattened keys are looked up under.
fn variant_named(table: &str, key: &str, value: &str) -> Option<String> {
    let owner = match (item_at(table)?, key) {
        ("Area", "kind") => "AreaKind",
        ("Source", "kind") => "Source",
        ("Group", "place") => "GroupKind",
        _ => return None,
    };
    let name: String = value
        .trim()
        .trim_matches('"')
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            match letters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + letters.as_str(),
                None => String::new(),
            }
        })
        .collect();
    Some(format!("{owner}::{name}"))
}

fn doc_for(item: &str, field: &str) -> Option<&'static str> {
    LAYOUT_DOCS
        .iter()
        .find(|(i, f, _)| *i == item && *f == field)
        .map(|(_, _, doc)| *doc)
}

fn type_of(item: &str, field: &str) -> Option<&'static str> {
    LAYOUT_FIELD_TYPES
        .iter()
        .find(|(owner, name, _)| *owner == item && *name == field)
        .map(|(_, _, kind)| *kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pipeline, end to end: a comment written on a field of the model has to come out in the printed file.
    ///
    /// Taken from the table rather than typed here, so the assertion is about the *pipeline* — scanner, generated table, renderer — and not about one sentence somebody may reword. Both halves are checked, because they fail separately: a scanner that found nothing leaves the table empty, and a renderer that looks an item up by the wrong name prints a file with no comments in it.
    #[test]
    fn a_doc_comment_on_a_layout_type_reaches_the_printed_schema() {
        let printed = render().expect("the reference prints");
        for (item, field) in [
            ("Layout", "name"),
            ("Area", "reserve"),
            ("AreaKind::Bar", "thickness"),
            ("AreaKind::Grid", "anchor"),
            ("Instance", "module"),
            ("GroupKind::Cell", "col"),
            ("Source::Poll", "every"),
        ] {
            let doc = doc_for(item, field)
                .unwrap_or_else(|| panic!("the scanner found no comment on `{item}.{field}`"));
            let first = doc.lines().next().expect("a comment has a first line");
            assert!(
                printed.contains(first),
                "`{item}.{field}`'s explanation never reached the printed schema:\n{printed}"
            );
        }
    }

    /// What the reference prints has to be a layout, or it is a reference to something that does not exist. Every field of every kind is written in it, so this also catches a rename the scanner would have been quiet about.
    #[test]
    fn the_printed_schema_parses_back_and_resolves() {
        let printed = render().expect("the reference prints");
        let again: Layout = toml::from_str(&printed).expect("the reference is a layout");
        let (resolved, report) =
            crate::resolve::resolve(&again, &std::collections::BTreeMap::new(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert!(
            crate::validate::validate_resolved(
                &resolved,
                "reference",
                &config::theme::NordTheme::default()
            )
            .is_clean(),
            "the reference has to be a layout the shell would draw"
        );
        assert_eq!(
            resolved
                .layer(LayerKind::Desktop)
                .map(|layer| layer.areas.len()),
            Some(5),
            "the built-in desktop's grid, and one area of every kind the built-in layout does not already show"
        );
    }

    /// The order the reference reads in is written out, so a type added to the model is a type the reference forgets. This is what says so.
    #[test]
    fn every_documented_item_is_in_the_vocabulary() {
        let mut missing: Vec<&str> = LAYOUT_FIELDS
            .iter()
            .map(|(item, _)| *item)
            .filter(|item| !ORDER.contains(item))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "these hold keys a layout file writes and are missing from ORDER: {missing:?}"
        );
    }

    /// And the vocabulary is what the manual and the docs reference walk, so an item with no keys at all in it would be a heading with nothing under it.
    #[test]
    fn every_item_in_the_vocabulary_has_keys_and_an_explanation() {
        for item in vocabulary() {
            assert!(
                !item.keys.is_empty(),
                "`{}` has no keys, so it is not a table a file writes",
                item.name
            );
            assert!(
                item.doc.is_some(),
                "`{}` has no explanation of its own",
                item.name
            );
        }
    }
}
