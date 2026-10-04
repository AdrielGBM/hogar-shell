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
            ..Group::default()
        };
    }
    replace_if_set(&mut base.stacked, &over.stacked);
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
    carry_unset(&mut base.unset, &over.unset, |unset| match unset {
        Unset::Repeat => base.repeat.is_some(),
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
                output_policy,
                routes,
                launcher,
            },
            AreaKind::Stack {
                anchor: over_anchor,
                offset: over_offset,
                width: over_width,
                output_policy: over_policy,
                routes: over_routes,
                launcher: over_launcher,
            },
        ) => {
            replace_if_set(anchor, over_anchor);
            replace_if_set(offset, over_offset);
            replace_if_set(width, over_width);
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
            },
            AreaKind::WallpaperRegion {
                rect: over_rect,
                source: over_source,
                fit: over_fit,
                transition: over_transition,
            },
        ) => {
            replace_if_set(rect, over_rect);
            replace_if_set(source, over_source);
            replace_if_set(fit, over_fit);
            replace_if_set(transition, over_transition);
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
        (AreaKind::Free { rect }, AreaKind::Free { rect: over_rect }) => {
            replace_if_set(rect, over_rect);
        }
        (AreaKind::Prompt { rect }, AreaKind::Prompt { rect: over_rect }) => {
            replace_if_set(rect, over_rect);
        }
        _ => unreachable!("is_same_kind already proved both sides are the same variant"),
    }
}

fn merge_style(base: &mut AreaStyle, over: &AreaStyle) {
    replace_if_set(&mut base.fill, &over.fill);
    replace_if_set(&mut base.radius, &over.radius);
    replace_if_set(&mut base.opacity, &over.opacity);
    replace_if_set(&mut base.padding, &over.padding);
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

/// An expression a level writes, by where it is: its layer, the ids down to what holds it, and the path a level would take it back by.
pub(crate) type At = (
    LayerKind,
    AreaId,
    Option<GroupId>,
    Option<InstanceId>,
    Unset,
);

/// Which level wrote each expression of a merge: laid level by level beside the merge itself, with the same three verbs — a removed item takes its expressions with it, an `unset` takes one back, a written key is the level's own.
#[derive(Debug, Default)]
pub(crate) struct Origins(BTreeMap<At, Level>);

impl Origins {
    pub(crate) fn of(&self, at: &At) -> Option<&Level> {
        self.0.get(at)
    }

    /// Records `layers`, laid over the levels before it by [`merge_layers`] or [`merge_session_layers`], as written by `level`.
    pub(crate) fn lay<'a>(
        &mut self,
        layers: impl IntoIterator<Item = (LayerKind, &'a Layer)>,
        level: &Level,
    ) {
        for (kind, layer) in layers {
            for id in &layer.remove {
                self.0
                    .retain(|(on, area, ..), _| !(*on == kind && area == id));
            }
            for area in &layer.areas {
                let id = &area.id;
                self.write(
                    (kind, id.clone(), None, None, Unset::Visible),
                    &area.unset,
                    area.visible.is_some(),
                    level,
                );
                for group in &area.remove {
                    self.0.retain(|(on, held, of, ..), _| {
                        !(*on == kind && held == id && of.as_ref() == Some(group))
                    });
                }
                for group in &area.groups {
                    let held = Some(group.id.clone());
                    self.write(
                        (kind, id.clone(), held.clone(), None, Unset::Repeat),
                        &group.unset,
                        group.repeat.is_some(),
                        level,
                    );
                    let parameters = group
                        .unset
                        .iter()
                        .filter_map(|unset| match unset {
                            Unset::Parameter(name) => Some(name),
                            _ => None,
                        })
                        .chain(group.parameters.keys());
                    for name in parameters {
                        self.write(
                            (kind, id.clone(), held.clone(), None, Unset::parameter(name)),
                            &group.unset,
                            group.parameters.contains_key(name),
                            level,
                        );
                    }
                    for instance in &group.remove {
                        self.0.retain(|(on, area, of, child, _), _| {
                            !(*on == kind
                                && area == id
                                && *of == held
                                && child.as_ref() == Some(instance))
                        });
                    }
                    for instance in &group.children {
                        let child = Some(instance.id.clone());
                        let paths = instance
                            .unset
                            .iter()
                            .filter_map(|unset| match unset {
                                Unset::Binding(path) => Some(path),
                                _ => None,
                            })
                            .chain(instance.bindings.keys());
                        for path in paths {
                            self.write(
                                (
                                    kind,
                                    id.clone(),
                                    held.clone(),
                                    child.clone(),
                                    Unset::binding(path),
                                ),
                                &instance.unset,
                                instance.bindings.contains_key(path),
                                level,
                            );
                        }
                    }
                }
            }
        }
    }

    /// One expression of one level: taken back where its holder's `unset` names it, then the level's own where it writes it.
    fn write(&mut self, at: At, unset: &[Unset], written: bool, level: &Level) {
        if unset.contains(&at.4) {
            self.0.remove(&at);
        }
        if written {
            self.0.insert(at, level.clone());
        }
    }
}
