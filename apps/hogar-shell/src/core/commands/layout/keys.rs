use serde::Serialize;
use serde::de::DeserializeOwned;

use layout::ops::{placement_of, site_of_area};
use layout::{
    Action, AreaId, AreaKind, ChildCell, Group, GroupId, GroupKind, Instance, InstanceId,
    KomponentId, LayerKind, Layout, LayoutOp, Library, NOMINAL_OUTPUT, Style, Trigger, Unset,
};
use ui::form::swatch_row::{is_colour, parse_colour};
use util::report::Finding;

use editor::written::Written;
use surfaces::layouts;

use super::{
    area_ids, as_toml, catalogue, group_entry, group_kind, inherited_group_named, listing,
    nothing_called, put,
};

const AREA_KEYS: [&str; 3] = ["reserve", "above_fullscreen", "within"];
const GROUP_KEYS: [&str; 4] = ["arrange", "cols", "rows", "gap"];
const AREA_KIND: &str = "AreaKind";
const GROUP_PLACE: &str = "GroupKind";

pub(super) enum Aimed {
    Instance(InstanceId),
    Area(AreaId),
    Group(AreaId, GroupId),
}

fn head(key: &str) -> &str {
    key.split_once('.').map_or(key, |(head, _)| head)
}

pub(super) fn on_instance(key: &str) -> bool {
    matches!(
        key,
        "module" | "representation" | "weight" | "cell" | "rect"
    ) || matches!(
        head(key),
        "options" | "bindings" | "actions" | "style" | "cell" | "rect"
    ) && key.contains('.')
}

fn on_area(key: &str, kind: Option<&AreaKind>) -> bool {
    key.starts_with("style.")
        || key.starts_with("actions.")
        || AREA_KEYS.contains(&key)
        || kind.is_some_and(|kind| keys_of_kind(AREA_KIND, kind).contains(&head(key)))
}

fn on_group(key: &str, place: Option<&GroupKind>) -> bool {
    key.starts_with("style.")
        || GROUP_KEYS.contains(&key)
        || place.is_some_and(|place| keys_of_kind(GROUP_PLACE, place).contains(&head(key)))
}

fn known_key(key: &str) -> bool {
    on_instance(key)
        || on_area(key, None)
        || on_group(key, None)
        || [AREA_KIND, GROUP_PLACE]
            .into_iter()
            .any(|owner| keys_of_any(owner).contains(&head(key)))
}

fn keys_of_kind<K: Serialize>(owner: &str, kind: &K) -> Vec<&'static str> {
    tag_of(owner, kind)
        .map(|tag| layout::schema::keys_of(&layout::schema::variant_of(owner, &tag)))
        .unwrap_or_default()
}

fn keys_of_any(owner: &str) -> Vec<&'static str> {
    let variant = format!("{owner}::");
    layout::schema::vocabulary()
        .into_iter()
        .filter(|item| item.name.starts_with(&variant))
        .flat_map(|item| item.keys.into_iter().map(|key| key.name))
        .collect()
}

fn tag_of<K: Serialize>(owner: &str, kind: &K) -> Option<String> {
    let tag = layout::schema::tag_of(owner)?;
    match toml::Value::try_from(kind).ok()? {
        toml::Value::Table(table) => table.get(tag)?.as_str().map(str::to_string),
        _ => None,
    }
}

/// `None` for a variant with fields a level cannot leave out.
fn unwritten<K: Serialize + DeserializeOwned>(owner: &str, kind: &K) -> Option<K> {
    let tag = layout::schema::tag_of(owner)?;
    let mut table = toml::Table::new();
    table.insert(tag.to_string(), toml::Value::String(tag_of(owner, kind)?));
    toml::Value::Table(table).try_into().ok()
}

fn kind_key<K: Serialize + DeserializeOwned>(
    owner: &str,
    kind: K,
    key: &str,
    value: &str,
) -> Result<K, String> {
    let keys = keys_of_kind(owner, &kind);
    if !keys.contains(&head(key)) {
        return Err(format!(
            "`{key}` is not a key of a `{}` (it has {})",
            tag_of(owner, &kind).unwrap_or_default(),
            keys.join(", ")
        ));
    }
    field_of(kind, key, key, value)
}

pub(super) fn aimed(layouts: &[&Layout], target: &str, key: &str) -> Result<Aimed, String> {
    if !known_key(key) {
        return Err(format!(
            "'{key}' is not a key `layout set` takes: of a placed module module, representation, options.<key>, bindings.<key>, actions.<gesture>, weight, cell.<key>, rect.<key> or style.<key>; of an area visible, {}, actions.<gesture>, style.<key> or a key of its kind; of a group repeat, parameters.<name>, {}, a key of its place or style.<key>; or unset",
            AREA_KEYS.join(", "),
            GROUP_KEYS.join(", ")
        ));
    }
    let instance = InstanceId::new(target);
    let placed = placement_of(layouts[0], &instance).is_some();
    if placed && on_instance(key) {
        return Ok(Aimed::Instance(instance));
    }
    let area = AreaId::new(target);
    let kind = written_kind(layouts, &area);
    let is_area = layouts
        .iter()
        .any(|layout| site_of_area(layout, &area).is_some());
    if is_area && on_area(key, kind.as_ref()) {
        return Ok(Aimed::Area(area));
    }
    let group = inherited_group_named(layouts, target).ok();
    if let Some((area, group)) = group.clone()
        && on_group(key, group_kind(layouts, &area, &group).as_ref())
    {
        return Ok(Aimed::Group(area, group));
    }
    let what = match (placed, is_area, group) {
        (true, _, _) => format!("the placed module `{target}`"),
        (_, true, _) => format!("the area `{target}`"),
        (_, _, Some((area, group))) => format!("the group `{area}.{group}`"),
        _ => return Err(nothing_called(layouts[0], target)),
    };
    Err(format!("`{key}` is not a key of {what}"))
}

fn written_kind(layouts: &[&Layout], area: &AreaId) -> Option<AreaKind> {
    layouts.iter().find_map(|layout| {
        layout::ops::sites(layout)
            .flat_map(|(_, layer)| layer.areas.iter())
            .filter(|held| held.id == *area)
            .find_map(|held| held.kind.clone())
    })
}

pub(super) fn instance_key(instance: &mut Instance, key: &str, value: &str) -> Result<(), String> {
    match key.split_once('.') {
        None if key == "weight" => instance.weight = Some(read(key, value)?),
        None if key == "cell" => instance.cell = Some(read(key, value)?),
        None if key == "rect" => instance.rect = Some(read(key, value)?),
        Some(("cell", field)) => {
            let seed = instance.cell.unwrap_or(ChildCell::at(0, 0));
            instance.cell = Some(field_of(seed, key, field, value)?);
        }
        Some(("rect", field)) => {
            let seed = instance.rect.unwrap_or_default();
            instance.rect = Some(field_of(seed, key, field, value)?);
        }
        Some(("style", path)) => instance.style = restyled(&instance.style, path, value)?,
        _ => return Err(format!("'{key}' is not a property of a placed module")),
    }
    Ok(())
}

pub(super) fn bound(
    layer: LayerKind,
    kind: Option<&AreaKind>,
    trigger: &str,
    chain: &str,
) -> Result<(Trigger, Action), String> {
    layout::actions::takes_actions(layer, kind)
        .and_then(|()| {
            layout::actions::action_from(trigger, chain, crate::core::commands::resolves)
        })
        .map_err(|why| why.english())
}

pub(super) fn set_area(area: AreaId, key: String, value: String) -> Result<String, String> {
    let known = known_layouts();
    let label = format!("Set `{key}` on `{area}`");
    super::edit_layout(&label, move |layout| {
        let base = layout::reset::base_of(layout, &known);
        let written = written_area(layout, &base, &area)?;
        let kind = written_kind(&[layout, &base], &area);
        let bar = matches!(kind, Some(AreaKind::Bar { .. }));
        let mut changed = written.area.clone();
        let mut reported = key.as_str();
        match key.split_once('.') {
            Some(("style", path)) => {
                layout::restyle::restyle(&mut changed, bar, |style| {
                    *style = restyled(style, path, &value)?;
                    Ok::<(), String>(())
                })?;
                if bar {
                    reported = layout::restyle::bar_key(&key);
                }
            }
            Some(("actions", trigger)) => {
                let (trigger, action) = bound(written.site.layer, kind.as_ref(), trigger, &value)?;
                changed.actions.insert(trigger, action);
            }
            _ => match key.as_str() {
                "reserve" => changed.reserve = Some(read(&key, &value)?),
                "above_fullscreen" => changed.above_fullscreen = Some(read(&key, &value)?),
                "within" => changed.within = Some(read(&key, &value)?),
                _ => {
                    let seed = changed
                        .kind
                        .take()
                        .or_else(|| kind.as_ref().and_then(|kind| unwritten(AREA_KIND, kind)))
                        .ok_or_else(|| format!("`{area}` has no kind yet"))?;
                    changed.kind = Some(kind_key(AREA_KIND, seed, &key, &value)?);
                }
            },
        }
        let ops = written.ops(&changed);
        unreported(layout, &ops, &known, &format!("areas.{area}"), reported)?;
        Ok((ops, format!("set `{key}` on `{area}`")))
    })
}

pub(super) fn set_group(
    (area, group): (AreaId, GroupId),
    key: String,
    value: String,
) -> Result<String, String> {
    let known = known_layouts();
    let name = format!("{area}.{group}");
    let label = format!("Set `{key}` on `{name}`");
    super::edit_layout(&label, move |layout| {
        let base = layout::reset::base_of(layout, &known);
        let written = written_area(layout, &base, &area)?;
        let mut changed = written.area.clone();
        let komponent = komponent_of(&[layout, &base], &area, &group);
        let place = group_kind(&[layout, &base], &area, &group);
        let held = group_entry(&mut changed, &group);
        match key.split_once('.') {
            Some(("style", path)) => held.style = restyled(&held.style, path, &value)?,
            _ if GROUP_KEYS.contains(&key.as_str()) => {
                if let Some(komponent) = komponent {
                    return Err(format!(
                        "`{name}` draws the komponent `{komponent}`, which arranges its children: edit `{}`, or detach it first",
                        layout::komponent_path(&komponent)
                    ));
                }
                arrange_key(held, &key, &value)?;
            }
            _ => {
                let seed = held
                    .kind
                    .take()
                    .or(place)
                    .ok_or_else(|| format!("`{name}` has no place yet"))?;
                held.kind = Some(kind_key(GROUP_PLACE, seed, &key, &value)?);
            }
        }
        let ops = written.ops(&changed);
        unreported(
            layout,
            &ops,
            &known,
            &format!("areas.{area}.groups.{group}"),
            &key,
        )?;
        Ok((ops, format!("set `{key}` on `{name}`")))
    })
}

pub(super) fn unreported_instance(
    layout: &Layout,
    ops: &[LayoutOp],
    instance: &InstanceId,
    key: &str,
) -> Result<(), String> {
    match head(key) {
        "weight" | "cell" | "rect" | "style" => unreported(
            layout,
            ops,
            &known_layouts(),
            &format!("children.{instance}"),
            key,
        ),
        _ => Ok(()),
    }
}

fn komponent_of(layouts: &[&Layout], area: &AreaId, group: &GroupId) -> Option<KomponentId> {
    layouts.iter().find_map(|layout| {
        layout::ops::sites(layout)
            .flat_map(|(_, layer)| layer.areas.iter())
            .filter(|held| held.id == *area)
            .flat_map(|held| held.groups.iter())
            .find(|held| held.id == *group)
            .and_then(|held| held.komponent.clone())
    })
}

fn known_layouts() -> Library {
    layouts::read(|store| store.all().clone()).unwrap_or_default()
}

fn written_area(layout: &Layout, base: &Layout, area: &AreaId) -> Result<Written, String> {
    let site = site_of_area(layout, area)
        .or_else(|| site_of_area(base, area))
        .ok_or_else(|| {
            format!(
                "there is no area called `{area}`{}",
                listing("areas", area_ids(layout))
            )
        })?;
    Written::area(
        layout,
        Some(&site.output.0),
        site.layer,
        area,
        site.workspace.as_ref(),
    )
    .map_err(|why| why.english())
}

fn arrange_key(group: &mut Group, key: &str, value: &str) -> Result<(), String> {
    match key {
        "arrange" => group.arrange = Some(read(key, value)?),
        "cols" => group.cols = Some(read(key, value)?),
        "rows" => group.rows = Some(read(key, value)?),
        "gap" => group.gap = Some(read(key, value)?),
        _ => return Err(format!("'{key}' is not a property of a group")),
    }
    group.unset.retain(|held| *held != Unset::Arrange);
    Ok(())
}

/// A colour is taken as the editor's colour rows take it: lowercased, and nothing for an empty one.
fn restyled(style: &Style, path: &str, value: &str) -> Result<Style, String> {
    let mut changed = field_of(style.clone(), &format!("style.{path}"), path, value)?;
    if changed.fill != style.fill {
        changed.fill = painted(changed.fill.as_deref())?;
    }
    let colour = |style: &Style| style.border.as_ref().and_then(|it| it.color.clone());
    if colour(&changed) != colour(style)
        && let Some(border) = changed.border.as_mut()
    {
        border.color = painted(border.color.as_deref())?;
        if border.is_empty() {
            changed.border = None;
        }
    }
    Ok(changed)
}

fn painted(paint: Option<&str>) -> Result<Option<String>, String> {
    let Some(text) = paint else {
        return Ok(None);
    };
    match parse_colour(text, &is_colour) {
        Some(colour) => Ok((!colour.is_empty()).then_some(colour)),
        None => Err(format!(
            "'{text}' is not a colour: write #rrggbb, #rrggbbaa or a theme token such as `surface`"
        )),
    }
}

fn read<T: DeserializeOwned>(key: &str, value: &str) -> Result<T, String> {
    as_toml(value).try_into().map_err(|why| refused(key, why))
}

fn field_of<T: Serialize + DeserializeOwned>(
    seed: T,
    key: &str,
    field: &str,
    value: &str,
) -> Result<T, String> {
    let mut table = match toml::Value::try_from(seed) {
        Ok(toml::Value::Table(table)) => table,
        _ => toml::Table::new(),
    };
    put(&mut table, field, as_toml(value))?;
    toml::Value::Table(table)
        .try_into()
        .map_err(|why| refused(key, why))
}

fn refused(key: &str, why: toml::de::Error) -> String {
    format!("`{key}`: {}", why.message().trim())
}

fn unreported(
    layout: &Layout,
    ops: &[LayoutOp],
    known: &Library,
    holder: &str,
    key: &str,
) -> Result<(), String> {
    let after = super::landed(layout, ops)?;
    let was = findings(layout, known);
    let new: Vec<String> = findings(&after, known)
        .into_iter()
        .filter(|found| {
            !was.iter().any(|known| known.is_same_fault(found)) && about(&found.key, holder, key)
        })
        .map(|found| found.message.english())
        .collect();
    match new.is_empty() {
        true => Ok(()),
        false => Err(new.join("\n")),
    }
}

fn findings(layout: &Layout, known: &Library) -> Vec<Finding> {
    let mut report = layout::validate(layout, &catalogue());
    report.merge(layout::resolve(layout, known, NOMINAL_OUTPUT, None).1);
    report.findings().cloned().collect()
}

fn about(found: &str, holder: &str, key: &str) -> bool {
    let Some((_, tail)) = found.split_once(&format!("{holder}.")) else {
        return false;
    };
    tail == key || key.starts_with(&format!("{tail}.")) || tail.starts_with(&format!("{key}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widgets_styled(opacity: Option<f32>, shadow: Option<u8>) -> Layout {
        let mut layout = layout::built_in();
        let widgets = layout
            .outputs
            .iter_mut()
            .flat_map(|output| output.layers.desktop.areas.iter_mut())
            .find(|area| area.id.as_str() == "widgets")
            .expect("the built-in desktop has its widgets");
        widgets.style.opacity = opacity;
        widgets.style.shadow = shadow;
        layout
    }

    fn style_set(layout: &Layout, opacity: Option<f32>, shadow: Option<u8>) -> Result<(), String> {
        let widgets = AreaId::new("widgets");
        let written = written_area(layout, layout, &widgets).expect("the widgets area");
        let mut changed = written.area.clone();
        changed.style.opacity = opacity;
        changed.style.shadow = shadow;
        let ops = written.ops(&changed);
        unreported(layout, &ops, &Library::default(), "areas.widgets", "style")
    }

    #[test]
    fn a_set_that_leaves_a_fault_in_place_is_not_refused_for_the_number_it_quotes() {
        let faulty = widgets_styled(Some(2.0), Some(9));
        style_set(&faulty, Some(0.5), Some(8))
            .expect("the opacity is fixed, the shadow is no new fault");
        assert!(
            findings(&faulty, &Library::default())
                .iter()
                .any(|found| found.key.ends_with("style.shadow")),
            "the shadow is a fault to begin with"
        );
    }

    #[test]
    fn a_set_that_makes_a_new_fault_is_refused() {
        let clean = widgets_styled(None, None);
        let refused =
            style_set(&clean, Some(2.0), None).expect_err("an opacity of 2 is no opacity");
        assert!(refused.contains("opacity"), "{refused}");
    }
}
