//! Laying one level of a layout over another.
//!
//! Merging is **by id, never by position**. Today a per-monitor file replaces an array wholesale, so a monitor that wants one extra chip has to restate the whole bar and then drifts away from the global one key at a time. Here a level names the area, group or instance it means and writes only what it changes, so "this monitor also has a clock" and "this monitor's bar is thicker" are each one line that keeps following the layout it refines.
//!
//! Three things can happen to an item at a level: it is **added** (an id that level is the first to name), **overridden** (an id an earlier level placed, field by field) or **removed** (its id in that level's `remove` list). Removals are applied before additions, so a level that removes an id and then names it again is placing a fresh item rather than editing the old one.
//!
//! Order is z-order, and an override never moves an item: added items go after the ones already there, which is what keeps a monitor rule from silently restacking a layer it only meant to adjust.

use crate::model::*;

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
    replace_if_set(&mut base.visible, &over.visible);

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
    for (path, expr) in &over.bindings {
        base.bindings.insert(path.clone(), expr.clone());
    }
    for (trigger, action) in &over.actions {
        base.actions.insert(*trigger, action.clone());
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
                width,
                output_policy,
                routes,
            },
            AreaKind::Stack {
                anchor: over_anchor,
                width: over_width,
                output_policy: over_policy,
                routes: over_routes,
            },
        ) => {
            replace_if_set(anchor, over_anchor);
            replace_if_set(width, over_width);
            replace_if_set(output_policy, over_policy);
            if !over_routes.is_empty() {
                *routes = over_routes.clone();
            }
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
        (
            AreaKind::Prompt { rect, style },
            AreaKind::Prompt {
                rect: over_rect,
                style: over_style,
            },
        ) => {
            replace_if_set(rect, over_rect);
            replace_if_set(&mut style.fill, &over_style.fill);
            replace_if_set(&mut style.radius, &over_style.radius);
            replace_if_set(&mut style.opacity, &over_style.opacity);
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

fn replace_if_set<T: Clone>(base: &mut Option<T>, over: &Option<T>) {
    if over.is_some() {
        *base = over.clone();
    }
}
