//! Laying one level of a layout over another.
//!
//! Merging is **by id, never by position**. Today a per-monitor file replaces an array wholesale, so a monitor that wants one extra chip has to restate the whole bar and then drifts away from the global one key at a time. Here a level names the area, group or instance it means and writes only what it changes, so "this monitor also has a clock" and "this monitor's bar is thicker" are each one line that keeps following the layout it refines.
//!
//! Three things can happen to an item at a level: it is **added** (an id that level is the first to name), **overridden** (an id an earlier level placed, field by field) or **removed** (its id in that level's `remove` list). Removals are applied before additions, so a level that removes an id and then names it again is placing a fresh item rather than editing the old one. An override can also take back an expression the item inherits (`unset`), which happens before the level's own keys are laid over it.
//!
//! Order is z-order, and an override never moves an item: added items go after the ones already there, which is what keeps a monitor rule from silently restacking a layer it only meant to adjust.

use std::collections::BTreeMap;

use crate::model::*;
use crate::resolve::Level;

/// Lays `over` on top of `base`, in place.
pub fn merge_layers(base: &mut Layers, over: &Layers) {
    merge_layer(&mut base.background, &over.background);
    merge_layer(&mut base.desktop, &over.desktop);
    merge_layer(&mut base.top, &over.top);
    merge_layer(&mut base.overlay, &over.overlay);
    merge_layer(&mut base.lock, &over.lock);
}

/// Lays a workspace rule's four layers over the output's. The lock layer is untouched, because no workspace is visible while the session is locked.
pub fn merge_session_layers(base: &mut Layers, over: &SessionLayers) {
    merge_layer(&mut base.background, &over.background);
    merge_layer(&mut base.desktop, &over.desktop);
    merge_layer(&mut base.top, &over.top);
    merge_layer(&mut base.overlay, &over.overlay);
}

pub fn merge_layer(base: &mut Layer, over: &Layer) {
    for id in &over.remove {
        base.areas.retain(|area| &area.id != id);
    }
    for area in &over.areas {
        match base.areas.iter_mut().find(|held| held.id == area.id) {
            Some(held) => merge_area(held, area),
            None => base.areas.push(area.clone()),
        }
    }
}

fn merge_area(base: &mut Area, over: &Area) {
    merge_kind(&mut base.kind, &over.kind);
    replace_if_set(&mut base.reserve, &over.reserve);
    replace_if_set(&mut base.above_fullscreen, &over.above_fullscreen);
    replace_if_set(&mut base.within, &over.within);
    merge_style(&mut base.style, &over.style);
    if over.unset.contains(&Unset::Visible) {
        base.visible = None;
    }
    replace_if_set(&mut base.visible, &over.visible);
    carry_unset(&mut base.unset, &over.unset, |unset| {
        *unset == Unset::Visible && base.visible.is_some()
    });
    merge_actions(&mut base.actions, &over.actions);

    for id in &over.remove {
        base.groups.retain(|group| &group.id != id);
    }
    for group in &over.groups {
        match base.groups.iter_mut().find(|held| held.id == group.id) {
            Some(held) => merge_group(held, group),
            None => base.groups.push(group.clone()),
        }
    }
}

fn merge_group(base: &mut Group, over: &Group) {
    if over.kind.is_some() {
        base.kind = over.kind;
    }
    if over.komponent.is_some() && over.komponent != base.komponent {
        *base = Group {
            id: base.id.clone(),
            kind: base.kind,
            komponent: over.komponent.clone(),
            style: std::mem::take(&mut base.style),
            ..Group::default()
        };
    }
    if over.unset.contains(&Unset::Arrange) {
        base.clear_arrangement();
    }
    replace_if_set(&mut base.arrange, &over.arrange);
    replace_if_set(&mut base.cols, &over.cols);
    replace_if_set(&mut base.rows, &over.rows);
    replace_if_set(&mut base.gap, &over.gap);
    merge_style(&mut base.style, &over.style);
    if over.unset.contains(&Unset::Repeat) {
        base.repeat = None;
    }
    replace_if_set(&mut base.repeat, &over.repeat);
    for unset in &over.unset {
        if let Unset::Parameter(name) = unset {
            base.parameters.remove(name);
        }
    }
    for (name, expr) in &over.parameters {
        base.parameters.insert(name.clone(), expr.clone());
    }
    let arranged = base.writes_arrangement();
    carry_unset(&mut base.unset, &over.unset, |unset| match unset {
        Unset::Repeat => base.repeat.is_some(),
        Unset::Arrange => arranged,
        Unset::Parameter(name) => base.parameters.contains_key(name),
        _ => false,
    });
    for id in &over.remove {
        base.children.retain(|instance| &instance.id != id);
    }
    for instance in &over.children {
        match base.children.iter_mut().find(|held| held.id == instance.id) {
            Some(held) => merge_instance(held, instance),
            None => base.children.push(instance.clone()),
        }
    }
}

fn merge_instance(base: &mut Instance, over: &Instance) {
    replace_if_set(&mut base.module, &over.module);
    replace_if_set(&mut base.representation, &over.representation);
    merge_table(&mut base.options, &over.options);
    merge_style(&mut base.style, &over.style);
    replace_if_set(&mut base.weight, &over.weight);
    replace_if_set(&mut base.cell, &over.cell);
    replace_if_set(&mut base.rect, &over.rect);
    for unset in &over.unset {
        if let Unset::Binding(path) = unset {
            base.bindings.remove(path);
        }
    }
    for (path, expr) in &over.bindings {
        base.bindings.insert(path.clone(), expr.clone());
    }
    carry_unset(
        &mut base.unset,
        &over.unset,
        |unset| matches!(unset, Unset::Binding(path) if base.bindings.contains_key(path)),
    );
    merge_actions(&mut base.actions, &over.actions);
}

/// Keeps what `over` takes back in the merged entry until a level writes it again, so the merged entry laid over another level takes back the same as the levels it merged — which is what flattening an `extends` chain into one level needs ([`crate::reset::base_of`]).
fn carry_unset(held: &mut Vec<Unset>, over: &[Unset], written: impl Fn(&Unset) -> bool) {
    for unset in over {
        if !held.contains(unset) {
            held.push(unset.clone());
        }
    }
    held.retain(|unset| !written(unset));
}

/// Lays one level's declared sources over another's, by name. A source keeps the keys the later level leaves out while both mean the same kind; a level that turns a `poll` into an `http` replaces it outright, since the two share no command to carry across. A level that changes what a source runs says `lock_safe` again or the source is not lock-safe: what an earlier level vouched for was the command it replaced.
pub fn merge_sources(base: &mut BTreeMap<String, Source>, over: &BTreeMap<String, Source>) {
    for (name, source) in over {
        match base.get_mut(name) {
            Some(held) if held.is_same_kind(source) => merge_source(held, source),
            _ => {
                base.insert(name.clone(), source.clone());
            }
        }
    }
}

fn merge_source(base: &mut Source, over: &Source) {
    match (base, over) {
        (
            Source::Poll {
                cmd,
                every,
                initial,
                parse,
                while_,
                lock_safe,
            },
            Source::Poll {
                cmd: over_cmd,
                every: over_every,
                initial: over_initial,
                parse: over_parse,
                while_: over_while,
                lock_safe: over_lock_safe,
            },
        ) => {
            replace_command(cmd, lock_safe, over_cmd);
            replace_if_set(every, over_every);
            replace_if_set(initial, over_initial);
            replace_if_set(parse, over_parse);
            replace_if_set(while_, over_while);
            replace_if_set(lock_safe, over_lock_safe);
        }
        (
            Source::Listen {
                cmd,
                initial,
                parse,
                while_,
                lock_safe,
            },
            Source::Listen {
                cmd: over_cmd,
                initial: over_initial,
                parse: over_parse,
                while_: over_while,
                lock_safe: over_lock_safe,
            },
        ) => {
            replace_command(cmd, lock_safe, over_cmd);
            replace_if_set(initial, over_initial);
            replace_if_set(parse, over_parse);
            replace_if_set(while_, over_while);
            replace_if_set(lock_safe, over_lock_safe);
        }
        (
            Source::Http {
                url,
                every,
                initial,
                parse,
                while_,
                lock_safe,
            },
            Source::Http {
                url: over_url,
                every: over_every,
                initial: over_initial,
                parse: over_parse,
                while_: over_while,
                lock_safe: over_lock_safe,
            },
        ) => {
            replace_command(url, lock_safe, over_url);
            replace_if_set(every, over_every);
            replace_if_set(initial, over_initial);
            replace_if_set(parse, over_parse);
            replace_if_set(while_, over_while);
            replace_if_set(lock_safe, over_lock_safe);
        }
        (base, over) => *base = over.clone(),
    }
}

/// A level rebinds one gesture without restating the others.
fn merge_actions(base: &mut BTreeMap<Trigger, Action>, over: &BTreeMap<Trigger, Action>) {
    for (trigger, action) in over {
        base.insert(*trigger, action.clone());
    }
}

/// Merges two levels' geometry. Fields merge only while both levels mean the same kind of region: a level that turns a bar into a grid replaces the geometry outright, since the two share no field worth carrying across.
fn merge_kind(base: &mut Option<AreaKind>, over: &Option<AreaKind>) {
    let Some(over) = over else { return };
    let Some(held) = base else {
        *base = Some(over.clone());
        return;
    };
    if !held.is_same_kind(over) {
        *base = Some(over.clone());
        return;
    }
    match (held, over) {
        (
            AreaKind::Bar {
                edge,
                thickness,
                length,
                offset,
                shape,
                autohide,
            },
            AreaKind::Bar {
                edge: over_edge,
                thickness: over_thickness,
                length: over_length,
                offset: over_offset,
                shape: over_shape,
                autohide: over_autohide,
            },
        ) => {
            replace_if_set(edge, over_edge);
            replace_if_set(thickness, over_thickness);
            replace_if_set(length, over_length);
            replace_if_set(offset, over_offset);
            replace_if_set(&mut shape.mode, &over_shape.mode);
            replace_if_set(&mut shape.gap, &over_shape.gap);
            replace_if_set(&mut shape.spacing, &over_shape.spacing);
            replace_if_set(&mut shape.radius, &over_shape.radius);
            replace_if_set(&mut shape.fillet, &over_shape.fillet);
            replace_if_set(autohide, over_autohide);
        }
        (
            AreaKind::Grid {
                rect,
                cell,
                gap,
                anchor,
            },
            AreaKind::Grid {
                rect: over_rect,
                cell: over_cell,
                gap: over_gap,
                anchor: over_anchor,
            },
        ) => {
            replace_if_set(rect, over_rect);
            replace_if_set(cell, over_cell);
            replace_if_set(gap, over_gap);
            replace_if_set(anchor, over_anchor);
        }
        (
            AreaKind::Stack {
                anchor,
                offset,
                width,
                flow,
                output_policy,
                routes,
                launcher,
            },
            AreaKind::Stack {
                anchor: over_anchor,
                offset: over_offset,
                width: over_width,
                flow: over_flow,
                output_policy: over_policy,
                routes: over_routes,
                launcher: over_launcher,
            },
        ) => {
            replace_if_set(anchor, over_anchor);
            replace_if_set(offset, over_offset);
            replace_if_set(width, over_width);
            replace_if_set(flow, over_flow);
            replace_if_set(output_policy, over_policy);
            if !over_routes.is_empty() {
                *routes = over_routes.clone();
            }
            replace_if_set(launcher, over_launcher);
        }
        (
            AreaKind::WallpaperRegion {
                rect,
                source,
                fit,
                transition,
                focus,
                dim,
                blur,
                parallax,
            },
            AreaKind::WallpaperRegion {
                rect: over_rect,
                source: over_source,
                fit: over_fit,
                transition: over_transition,
                focus: over_focus,
                dim: over_dim,
                blur: over_blur,
                parallax: over_parallax,
            },
        ) => {
            replace_if_set(rect, over_rect);
            replace_if_set(source, over_source);
            replace_if_set(fit, over_fit);
            replace_if_set(transition, over_transition);
            replace_if_set(focus, over_focus);
            replace_if_set(dim, over_dim);
            replace_if_set(blur, over_blur);
            replace_if_set(parallax, over_parallax);
        }
        (
            AreaKind::Texture {
                rect,
                image,
                gradient,
                tile,
                blend,
                opacity,
            },
            AreaKind::Texture {
                rect: over_rect,
                image: over_image,
                gradient: over_gradient,
                tile: over_tile,
                blend: over_blend,
                opacity: over_opacity,
            },
        ) => {
            replace_if_set(rect, over_rect);
            replace_if_set(tile, over_tile);
            replace_if_set(blend, over_blend);
            replace_if_set(opacity, over_opacity);
            if over_image.is_some() {
                *image = over_image.clone();
                *gradient = None;
            } else if over_gradient.is_some() {
                *gradient = over_gradient.clone();
                *image = None;
            }
        }
        (
            AreaKind::Dock { edge, thickness },
            AreaKind::Dock {
                edge: over_edge,
                thickness: over_thickness,
            },
        ) => {
            replace_if_set(edge, over_edge);
            replace_if_set(thickness, over_thickness);
        }
        (
            AreaKind::Free { rect, anchor },
            AreaKind::Free {
                rect: over_rect,
                anchor: over_anchor,
            },
        ) => {
            replace_if_set(rect, over_rect);
            replace_if_set(anchor, over_anchor);
        }
        (
            AreaKind::Panel {
                owner,
                along,
                cols,
                rows,
                cell,
                gap,
            },
            AreaKind::Panel {
                owner: over_owner,
                along: over_along,
                cols: over_cols,
                rows: over_rows,
                cell: over_cell,
                gap: over_gap,
            },
        ) => {
            replace_if_set(owner, over_owner);
            replace_if_set(along, over_along);
            replace_if_set(cols, over_cols);
            replace_if_set(rows, over_rows);
            replace_if_set(cell, over_cell);
            replace_if_set(gap, over_gap);
        }
        (AreaKind::Prompt { rect }, AreaKind::Prompt { rect: over_rect }) => {
            replace_if_set(rect, over_rect);
        }
        _ => unreachable!("is_same_kind already proved both sides are the same variant"),
    }
}

fn merge_style(base: &mut Style, over: &Style) {
    replace_if_set(&mut base.fill, &over.fill);
    replace_if_set(&mut base.radius, &over.radius);
    replace_if_set(&mut base.opacity, &over.opacity);
    replace_if_set(&mut base.padding, &over.padding);
    if let Some(over) = &over.border {
        let border = base.border.get_or_insert_with(Border::default);
        replace_if_set(&mut border.width, &over.width);
        replace_if_set(&mut border.color, &over.color);
    }
    replace_if_set(&mut base.shadow, &over.shadow);
    replace_if_set(&mut base.backdrop, &over.backdrop);
}

/// Deep-merges option tables so a level can set one key of a module's options without restating the rest. A table recurses; anything else, an array included, replaces, because an array here is one value the user chose rather than a list to accumulate.
pub fn merge_table(base: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(held)), toml::Value::Table(incoming)) => {
                merge_table(held, incoming);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Replaces what a source runs, forgetting the `lock_safe` an earlier level gave the command it replaces. The level's own `lock_safe`, laid over afterwards, is the only one that can vouch for the new one.
fn replace_command(base: &mut Option<String>, lock_safe: &mut Option<bool>, over: &Option<String>) {
    if over.is_some() {
        *base = over.clone();
        *lock_safe = None;
    }
}

fn replace_if_set<T: Clone>(base: &mut Option<T>, over: &Option<T>) {
    if over.is_some() {
        *base = over.clone();
    }
}

/// A key a level writes, by where it is: its layer, the ids down to what holds it, and its path there as the file spells it — `thickness`, `shape.radius`, `style.border.width`, `actions.press`, `options.face.scale`, `weight`, or an expression's `visible`, `repeat`, `parameters.<name>` or `bindings.<path>`, which is also the path a level takes it back by.
pub(crate) type KeyAt = (
    LayerKind,
    AreaId,
    Option<GroupId>,
    Option<InstanceId>,
    String,
);

/// Where an instance's options are written: the one place a key is merged table by table rather than replaced whole.
const OPTIONS: &str = "options.";

/// Whether the dotted key `inner` is inside the table `outer` names.
fn is_inside(inner: &str, outer: &str) -> bool {
    inner
        .strip_prefix(outer)
        .is_some_and(|rest| rest.starts_with('.'))
}

/// Which level wrote each key of a merge: laid level by level beside the merge itself, with the same verbs — a removed item takes its keys with it, an `unset` takes one back, a key written over a value of another kind or a komponent drawn in place of another takes what it replaced with it, and a written key is the level's own.
#[derive(Debug, Default)]
pub(crate) struct Origins(BTreeMap<KeyAt, Level>);

impl Origins {
    pub(crate) fn of(&self, at: &KeyAt) -> Option<&Level> {
        self.0.get(at)
    }

    /// [`Origins::of`], or for an option inside a table or a list a level wrote whole, the level that wrote that.
    pub(crate) fn nearest(&self, at: &KeyAt) -> Option<&Level> {
        if let Some(level) = self.0.get(at) {
            return Some(level);
        }
        let key = &at.4;
        key.strip_prefix(OPTIONS)?;
        let mut held = at.clone();
        key.match_indices('.')
            .map(|(end, _)| &key[..end])
            .filter(|parent| parent.len() >= OPTIONS.len())
            .rev()
            .find_map(|parent| {
                held.4 = parent.to_string();
                self.0.get(&held)
            })
    }

    /// Records the keys `layers` writes, as written by `level`, before [`merge_layers`] or [`merge_session_layers`] lays them over `under`.
    pub(crate) fn lay<'a>(
        &mut self,
        under: &Layers,
        layers: impl IntoIterator<Item = (LayerKind, &'a Layer)>,
        level: &Level,
    ) {
        for (kind, layer) in layers {
            for id in &layer.remove {
                self.0
                    .retain(|(on, area, ..), _| !(*on == kind && area == id));
            }
            for area in &layer.areas {
                let held = under
                    .get(kind)
                    .areas
                    .iter()
                    .find(|held| held.id == area.id)
                    .filter(|_| !layer.remove.contains(&area.id));
                let holder = Holding {
                    layer: kind,
                    area: area.id.clone(),
                    group: None,
                    instance: None,
                };
                self.lay_area(&holder, held, area, level);
            }
        }
    }

    fn lay_area(&mut self, holder: &Holding, held: Option<&Area>, area: &Area, level: &Level) {
        if let (Some(before), Some(after)) = (held.and_then(|held| held.kind.as_ref()), &area.kind)
            && !before.is_same_kind(after)
        {
            for key in kind_keys(before) {
                self.0.remove(&holder.at(key));
            }
        }
        if let Some(AreaKind::Texture {
            image, gradient, ..
        }) = &area.kind
        {
            match (image, gradient) {
                (Some(_), _) => self.0.remove(&holder.at("gradient")),
                (None, Some(_)) => self.0.remove(&holder.at("image")),
                (None, None) => None,
            };
        }
        self.take_back(holder, &area.unset);
        let mut keys = area.kind.as_ref().map(kind_keys).unwrap_or_default();
        let flags = [
            ("reserve", area.reserve.is_some()),
            ("above_fullscreen", area.above_fullscreen.is_some()),
            ("within", area.within.is_some()),
            ("visible", area.visible.is_some()),
        ];
        keys.extend(
            flags
                .into_iter()
                .filter(|(_, set)| *set)
                .map(|(key, _)| key.to_string()),
        );
        keys.extend(style_keys(&area.style));
        keys.extend(action_keys(&area.actions));
        for key in keys {
            self.write(holder.at(key), level);
        }

        for id in &area.remove {
            self.0.retain(|at, _| !holder.of_group(id).contains(at));
        }
        for group in &area.groups {
            let before = held
                .filter(|_| !area.remove.contains(&group.id))
                .and_then(|held| held.groups.iter().find(|before| before.id == group.id));
            self.lay_group(&holder.of_group(&group.id), before, group, level);
        }
    }

    fn lay_group(&mut self, holder: &Holding, held: Option<&Group>, group: &Group, level: &Level) {
        if group.komponent.is_some() && held.is_some_and(|held| held.komponent != group.komponent) {
            self.0.retain(|at, _| {
                !holder.contains(at)
                    || (holder.holds(at) && (at.4 == "place" || at.4.starts_with("style.")))
            });
        }
        self.take_back(holder, &group.unset);
        let written = [
            ("place", group.kind.is_some()),
            ("komponent", group.komponent.is_some()),
            ("arrange", group.writes_arrangement()),
            ("cols", group.cols.is_some()),
            ("rows", group.rows.is_some()),
            ("gap", group.gap.is_some()),
            ("repeat", group.repeat.is_some()),
        ];
        let mut keys: Vec<String> = written
            .into_iter()
            .filter(|(_, set)| *set)
            .map(|(key, _)| key.to_string())
            .collect();
        keys.extend(style_keys(&group.style));
        keys.extend(
            group
                .parameters
                .keys()
                .map(|name| Unset::parameter(name).to_string()),
        );
        for key in keys {
            self.write(holder.at(key), level);
        }

        for id in &group.remove {
            self.0.retain(|at, _| !holder.of_instance(id).contains(at));
        }
        for instance in &group.children {
            self.lay_instance(&holder.of_instance(&instance.id), instance, level);
        }
    }

    fn lay_instance(&mut self, holder: &Holding, instance: &Instance, level: &Level) {
        self.take_back(holder, &instance.unset);
        let written = [
            ("module", instance.module.is_some()),
            ("representation", instance.representation.is_some()),
            ("weight", instance.weight.is_some()),
            ("cell", instance.cell.is_some()),
            ("rect", instance.rect.is_some()),
        ];
        let mut keys: Vec<String> = written
            .into_iter()
            .filter(|(_, set)| *set)
            .map(|(key, _)| key.to_string())
            .collect();
        leaves(&instance.options, "options", &|_| true, &mut keys);
        keys.extend(style_keys(&instance.style));
        keys.extend(
            instance
                .bindings
                .keys()
                .map(|path| Unset::binding(path).to_string()),
        );
        keys.extend(action_keys(&instance.actions));
        for key in keys {
            self.write(holder.at(key), level);
        }
    }

    /// What a holder's `unset` takes back, before its level's own keys are laid: `arrange` with every key an arrangement is made of.
    fn take_back(&mut self, holder: &Holding, unset: &[Unset]) {
        for taken in unset {
            match taken {
                Unset::Unknown(_) => {}
                Unset::Arrange => {
                    for key in ARRANGEMENT.iter().copied() {
                        self.0.remove(&holder.at(key));
                    }
                }
                taken => {
                    self.0.remove(&holder.at(taken.to_string()));
                }
            }
        }
    }

    /// One key of one level, the level's own from now on. An option written over a table or over a value inside it replaces what it was written over, as [`merge_table`] does.
    fn write(&mut self, at: KeyAt, level: &Level) {
        if at.4.starts_with(OPTIONS) {
            let key = at.4.as_str();
            self.0.retain(|other, _| {
                let same_holder =
                    (&other.0, &other.1, &other.2, &other.3) == (&at.0, &at.1, &at.2, &at.3);
                !same_holder || !(is_inside(&other.4, key) || is_inside(key, &other.4))
            });
        }
        self.0.insert(at, level.clone());
    }
}

/// What holds a key: an area, a group in it or an instance in that.
#[derive(Clone)]
struct Holding {
    layer: LayerKind,
    area: AreaId,
    group: Option<GroupId>,
    instance: Option<InstanceId>,
}

impl Holding {
    fn at(&self, key: impl Into<String>) -> KeyAt {
        (
            self.layer,
            self.area.clone(),
            self.group.clone(),
            self.instance.clone(),
            key.into(),
        )
    }

    fn of_group(&self, id: &GroupId) -> Holding {
        Holding {
            group: Some(id.clone()),
            instance: None,
            ..self.clone()
        }
    }

    fn of_instance(&self, id: &InstanceId) -> Holding {
        Holding {
            instance: Some(id.clone()),
            ..self.clone()
        }
    }

    /// Whether `at` is a key of this holder itself.
    fn holds(&self, at: &KeyAt) -> bool {
        at.0 == self.layer && at.1 == self.area && at.2 == self.group && at.3 == self.instance
    }

    /// Whether `at` is a key of this holder or of anything it holds.
    fn contains(&self, at: &KeyAt) -> bool {
        at.0 == self.layer
            && at.1 == self.area
            && (self.group.is_none() || at.2 == self.group)
            && (self.instance.is_none() || at.3 == self.instance)
    }
}

/// The keys an area's geometry writes, beside its `kind`: a bar's `shape` key by key, since a level that writes one keeps the others, and every other value whole.
fn kind_keys(kind: &AreaKind) -> Vec<String> {
    let mut keys = Vec::new();
    if let Ok(toml::Value::Table(table)) = toml::Value::try_from(kind) {
        leaves(&table, "", &|key| key == "shape", &mut keys);
    }
    keys
}

/// The `style.*` keys `style` writes, its `border` key by key.
fn style_keys(style: &Style) -> Vec<String> {
    let mut keys = Vec::new();
    if style.is_empty() {
        return keys;
    }
    if let Ok(toml::Value::Table(table)) = toml::Value::try_from(style) {
        leaves(&table, "style", &|key| key == "style.border", &mut keys);
    }
    keys
}

fn action_keys(actions: &BTreeMap<Trigger, Action>) -> impl Iterator<Item = String> + '_ {
    actions
        .keys()
        .map(|trigger| format!("actions.{}", trigger.as_str()))
}

/// The dotted path of every value in `table` under `prefix`, going into a table only where `deep` says a level merges it key by key.
fn leaves(table: &toml::Table, prefix: &str, deep: &dyn Fn(&str) -> bool, keys: &mut Vec<String>) {
    for (key, value) in table {
        let path = match prefix.is_empty() {
            true => key.clone(),
            false => format!("{prefix}.{key}"),
        };
        match value {
            toml::Value::Table(inner) if deep(&path) => leaves(inner, &path, deep, keys),
            _ => keys.push(path),
        }
    }
}
