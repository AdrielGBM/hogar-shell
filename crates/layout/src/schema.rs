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
    "AreaKind::Panel",
    "AreaKind::Prompt",
    "Style",
    "Border",
    "Corners",
    "Sides",
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
    "ChildCell",
    "Komponent",
    "Parameter",
];

/// One item of the layout file's vocabulary: a table a file can hold, a variant of one, or a value written in place of a table (`Corners`, `Sides`), with what it is for and the keys it has.
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
/// The shipped layout plus one area of every kind it does not use and a container on its grid, so the file shows the whole vocabulary — and still parses back as a layout, which a reference written by hand would stop doing the first time a field was renamed. The extra areas hold no instances: what a module is called is the module table's business, and a reference that named one would be a reference that goes stale when a module is renamed.
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
    if let Some(grid) = rule
        .layers
        .desktop
        .areas
        .iter_mut()
        .find(|area| area.id == AreaId::new("widgets"))
    {
        grid.groups.push(Group {
            id: GroupId::new("container"),
            kind: Some(GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 4,
                row_span: 2,
            }),
            arrange: Some(Arrange::Grid),
            cols: Some(2),
            rows: Some(1),
            gap: Some(8.0),
            ..Group::default()
        });
    }
    rule.layers.desktop.areas.extend([
        area(
            "cards",
            AreaKind::Stack {
                anchor: Some(Anchor::TopRight),
                offset: Some(Offset { x: 0.0, y: 48.0 }),
                width: Some(380.0),
                flow: Some(StackFlow::Column),
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
        Area {
            reserve: Some(false),
            ..area(
                "visualiser",
                AreaKind::Dock {
                    edge: Some(config::Edge::Bottom),
                    thickness: Some(96.0),
                },
            )
        },
        Area {
            style: Style {
                radius: Some(Corners::each(16.0, 16.0, 0.0, 16.0)),
                padding: Some(Sides::each(8.0, 12.0, 8.0, 12.0)),
                border: Some(Border {
                    width: Some(1.0),
                    color: Some(Border::COLOR.to_string()),
                }),
                shadow: Some(1),
                ..Style::default()
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
                    anchor: None,
                },
            )
        },
    ]);
    if let Some(AreaKind::Bar { shape, .. }) = rule
        .layers
        .top
        .areas
        .iter_mut()
        .find(|area| area.id == AreaId::new("bar-top"))
        .and_then(|area| area.kind.as_mut())
    {
        shape.fillet = Some(12.0);
    }
    rule.layers.top.areas.push(area(
        "panel-clock",
        AreaKind::Panel {
            owner: Some(InstanceId::new("clock")),
            along: Some(false),
            cols: Some(4),
            rows: Some(3),
            cell: Some(64.0),
            gap: Some(8.0),
        },
    ));
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

/// The variant a `kind = "bar"` or `place = "zone"` line names, as the item its flattened keys are looked up under: `key` is the tag of the table's own enum (a source), or of the enum a field of that name flattens into it (an area's `kind`, a group's `place`).
fn variant_named(table: &str, key: &str, value: &str) -> Option<String> {
    let item = item_at(table)?;
    let owner = [Some(item), type_of(item, key)]
        .into_iter()
        .flatten()
        .find(|owner| tag_of(owner) == Some(key))?;
    Some(variant_of(owner, value.trim().trim_matches('"')))
}

/// The key the internally tagged enum `owner` names its variant under.
pub fn tag_of(owner: &str) -> Option<&'static str> {
    LAYOUT_TAGS
        .iter()
        .find(|(name, _)| *name == owner)
        .map(|(_, tag)| *tag)
}

/// The item of the variant of `owner` a file names `value` (`wallpaper_region` → `AreaKind::WallpaperRegion`), its keys looked up under it.
pub fn variant_of(owner: &str, value: &str) -> String {
    let name: String = value
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            match letters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + letters.as_str(),
                None => String::new(),
            }
        })
        .collect();
    format!("{owner}::{name}")
}

/// The keys a file may write in a table of `item`, as the model spells them: what `layout check` holds that table's keys to, so the reference and the check cannot list different keys.
pub fn keys_of(item: &str) -> Vec<&'static str> {
    LAYOUT_FIELDS
        .iter()
        .filter(|(owner, _)| *owner == item)
        .map(|(_, field)| *field)
        .collect()
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
            crate::resolve::resolve(&again, &crate::Library::default(), "DP-1", None);
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
            Some(6),
            "the built-in desktop's grid and clock, and one area of every kind the built-in layout does not already show"
        );
    }

    /// Every key the printed reference writes is a key the vocabulary lists for the table it is in, so the manual and `docs/reference/layout.md` name keys a file can hold — and since the printed file parses back, every one of them parses. A field serde writes under another name is where the two part: a group's placement is a flattened enum written as `place`, and was listed as `kind`, which no file holds.
    #[test]
    fn every_key_the_reference_writes_is_one_the_vocabulary_lists() {
        let printed = render().expect("the reference prints");
        let mut table = String::new();
        let mut variant: Option<(String, String)> = None;
        let mut checked = Vec::new();
        for line in printed.lines().filter(|line| !line.starts_with('#')) {
            if let Some(path) = header_of(line) {
                table = path;
                variant = None;
                continue;
            }
            let Some((key, value)) = line.split_once(" = ") else {
                continue;
            };
            let key = key.trim();
            let Some(item) = item_at(&table) else {
                continue;
            };
            if let Some(named) = variant_named(&table, key, value) {
                variant = Some((named, key.to_string()));
            }
            let listed = keys_of(item).contains(&key)
                || variant
                    .as_ref()
                    .is_some_and(|(named, tag)| keys_of(named).contains(&key) || *tag == key);
            assert!(
                listed,
                "`{key}` is written in a `{item}` table ({table}) but the vocabulary does not list it"
            );
            checked.push(format!("{item}.{key}"));
        }
        for renamed in ["Group.place", "OutputRule.match", "Source.while"] {
            assert!(
                checked.contains(&renamed.to_string()),
                "the reference writes `{renamed}`, so the parse-back test covers it: {checked:?}"
            );
        }
        let group: Vec<&str> = keys_of("Group");
        assert!(
            group.contains(&"place") && !group.contains(&"kind"),
            "{group:?}"
        );
        assert!(keys_of("Source::Poll").contains(&"while"));
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

    /// And the vocabulary is what the manual and the docs reference walk, so an item with no keys at all in it would be a heading with nothing under it — unless it is a value written in place of a table, which its explanation alone describes.
    #[test]
    fn every_item_in_the_vocabulary_has_keys_and_an_explanation() {
        for item in vocabulary() {
            assert!(
                !item.keys.is_empty() || ["Corners", "Sides"].contains(&item.name),
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

    /// One style is written on areas, groups and instances, so the reference names it once, with the border, the shadow steps and the four sides of its padding.
    #[test]
    fn the_reference_lists_one_style_with_its_border_and_sides() {
        let items = vocabulary();
        let item = |name: &str| {
            items
                .iter()
                .find(|item| item.name == name)
                .unwrap_or_else(|| panic!("`{name}` is in the reference"))
        };
        let style: Vec<&str> = item("Style").keys.iter().map(|key| key.name).collect();
        for key in [
            "fill", "radius", "opacity", "padding", "border", "shadow", "backdrop",
        ] {
            assert!(style.contains(&key), "`Style.{key}`: {style:?}");
        }
        let border: Vec<&str> = item("Border").keys.iter().map(|key| key.name).collect();
        assert_eq!(border, ["width", "color"]);
        assert!(
            item("Sides")
                .doc
                .is_some_and(|doc| doc.contains("[top, right, bottom, left]"))
        );
        for holder in ["Area", "Group", "Instance"] {
            assert_eq!(type_of(holder, "style"), Some("Style"), "{holder}");
        }
        assert!(items.iter().all(|item| item.name != "AreaStyle"));
    }
}
