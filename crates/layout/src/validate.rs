//! What a layout is not allowed to say, and why.
//!
//! Resolution ([`crate::resolve`]) answers fields and reports the ones nobody filled. Validation is the other half: rules that a complete, well-typed layout can still break. They are all here rather than spread through the code that draws, because a rule enforced where it is drawn is a rule that is enforced differently in each of the five places something is drawn.
//!
//! Two of them are load-bearing:
//!
//! - **A workspace rule may not touch reservation.** Reservation is what keeps windows out of a bar's strip, and the compositor re-tiles every window when it changes. If switching workspaces could change it, every workspace switch would shuffle the user's windows. So a workspace rule that adds, removes or resizes a reserving area is rejected with its path, and the output-level arrangement stands.
//! - **The lock layer holds readings, never controls.** Anything placed there is on a screen that anyone walking past can see and touch, so only representations a module declares `ReadOnly` may go there, actions are refused outright, and a command source has to opt in. A layout that breaks any of it is not silently fixed: the lock falls back to the built-in minimal lock, because a half-corrected lock screen is worse than a plain one.
//!
//! What this crate cannot know on its own — whether a module exists, whether it has a representation, whether that representation is read-only, and whether a command line resolves — is asked of a [`Catalogue`]. That keeps the layout model free of the module registry and the IPC table, and lets a test state exactly which modules it is talking about.

use std::collections::BTreeSet;

use util::report::{Finding, Report};

use crate::merge::merge_layers;
use crate::model::*;
use crate::resolve::{Resolved, ResolvedAreaKind};

/// What validation has to ask someone else.
pub trait Catalogue {
    fn knows_module(&self, module: &str) -> bool;
    /// Whether the module can be drawn at this size at all.
    fn has_representation(&self, module: &str, representation: Representation) -> bool;
    /// Whether that representation registers no press, drag, scroll or hover target.
    fn is_read_only(&self, module: &str, representation: Representation) -> bool;
    /// Whether the line names a real IPC command, checked without running it.
    fn command_resolves(&self, line: &str) -> bool;
}

/// The smallest prompt that is still usable, as a fraction of the output. Anything smaller is a lockout on a large monitor as surely as a hidden one.
const SMALLEST_PROMPT: f32 = 0.05;

/// The faintest a prompt may be drawn. Below this the field a user has to type into disappears into the wallpaper behind it.
const FAINTEST_PROMPT: f32 = 0.9;

/// Everything wrong with `layout` that does not depend on which output it is shown on.
pub fn validate(layout: &Layout, catalogue: &dyn Catalogue) -> Report {
    let mut report = Report::default();
    let file = format!("layouts/{}.toml", layout.id);

    let mut instances = BTreeSet::new();
    for rule in &layout.outputs {
        let at = format!("outputs.{}", rule.matches.0);
        check_layer_ids(&rule.layers, &at, &file, &mut instances, &mut report);
        check_lock_layer(&rule.layers.lock, &at, &file, catalogue, &mut report);
        check_modules(&rule.layers, &at, &file, catalogue, &mut report);
        check_actions(&rule.layers, &at, &file, catalogue, &mut report);
        check_gradients(&rule.layers, &at, &file, &mut report);
        check_workspace_rules(layout, rule, &at, &file, catalogue, &mut report);
    }
    report
}

/// Everything wrong with one output's resolved arrangement. The lock layer's prompt lives here rather than in [`validate`] because "exactly one per output" is only answerable once an output is known.
pub fn validate_resolved(resolved: &Resolved, file: &str) -> Report {
    let mut report = Report::default();
    let Some(lock) = resolved.layer(LayerKind::Lock) else {
        return report;
    };

    let prompts: Vec<usize> = lock
        .areas
        .iter()
        .enumerate()
        .filter(|(_, area)| matches!(area.kind, ResolvedAreaKind::Prompt { .. }))
        .map(|(at, _)| at)
        .collect();

    let at = format!("layers.lock on {}", resolved.output);
    match prompts.len() {
        1 => {}
        0 => {
            report.error(Finding::new(
                file,
                at.clone(),
                "the lock layer has no prompt, so there would be nothing to type a password into"
                    .to_string(),
            ));
            return report;
        }
        several => {
            report.error(Finding::new(
                file,
                at.clone(),
                format!("the lock layer has {several} prompts, and only the one holding keyboard focus would work"),
            ));
            return report;
        }
    }

    let prompt_at = prompts[0];
    let prompt = &lock.areas[prompt_at];
    let ResolvedAreaKind::Prompt { rect, style } = &prompt.kind else {
        return report;
    };

    if prompt.visible.is_some() {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}.visible", prompt.id),
            "the prompt cannot be given a visibility expression: an expression that turns false locks the user out".to_string(),
        ));
    }
    if !rect.is_on_output() {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}.rect", prompt.id),
            "the prompt does not lie on the output, so it could not be reached".to_string(),
        ));
    }
    if rect.w < SMALLEST_PROMPT || rect.h < SMALLEST_PROMPT {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}.rect", prompt.id),
            "the prompt is too small to type into".to_string(),
        ));
    }
    if let Some(opacity) = style.opacity.or(prompt.style.opacity)
        && opacity < FAINTEST_PROMPT
    {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}.style.opacity", prompt.id),
            format!("the prompt is drawn at {opacity}, too faint to find; it stays at {FAINTEST_PROMPT} or above"),
        ));
    }
    for covering in &lock.areas[prompt_at + 1..] {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}", covering.id),
            format!(
                "`{}` is stacked over the prompt, which would cover it",
                covering.id
            ),
        ));
    }
    report
}

/// The keys every area has, whatever kind it is.
const AREA_KEYS: &[&str] = &[
    "id",
    "kind",
    "reserve",
    "above_fullscreen",
    "within",
    "style",
    "visible",
    "groups",
    "remove",
];

/// The keys every group has, wherever it sits.
const GROUP_KEYS: &[&str] = &["id", "place", "children", "remove"];

fn keys_of_kind(kind: &str) -> &'static [&'static str] {
    match kind {
        "bar" => &["edge", "thickness", "length", "offset", "shape", "autohide"],
        "grid" => &["rect", "cell", "gap", "anchor"],
        "stack" => &["anchor", "width", "output_policy", "routes"],
        "wallpaper_region" => &["rect", "source", "fit", "transition"],
        "texture" => &["rect", "image", "gradient", "tile", "blend", "opacity"],
        "dock" => &["edge", "thickness"],
        "free" => &["rect"],
        "prompt" => &["rect", "style"],
        _ => &[],
    }
}

fn keys_of_place(place: &str) -> &'static [&'static str] {
    match place {
        "zone" => &["zone"],
        "cell" => &["col", "row", "col_span", "row_span"],
        _ => &[],
    }
}

/// Reports keys an area or a group does not have.
///
/// This reads the file's own text rather than the parsed model, because an area's geometry is flattened into its table and serde cannot both flatten and refuse unknown keys. Reading the text also means each finding carries a real line and column, so a mistyped `thikness` points at itself instead of at the area it belongs to.
pub fn check_unknown_keys(text: &str, id: &LayoutId) -> Report {
    let mut report = Report::default();
    let file = format!("layouts/{id}.toml");
    let Ok(parsed) = toml_edit::Document::parse(text) else {
        return report;
    };

    let Some(outputs) = parsed.get("outputs").and_then(|it| it.as_array_of_tables()) else {
        return report;
    };

    for rule in outputs {
        let at = rule
            .get("match")
            .and_then(|it| it.as_str())
            .unwrap_or("*")
            .to_string();
        check_rule_keys(rule, &at, text, &file, &mut report);
        if let Some(workspaces) = rule
            .get("workspaces")
            .and_then(|it| it.as_array_of_tables())
        {
            for workspace in workspaces {
                let at = format!(
                    "{at}.workspaces.{}",
                    workspace
                        .get("match")
                        .and_then(|it| it.as_str())
                        .unwrap_or("")
                );
                check_rule_keys(workspace, &at, text, &file, &mut report);
            }
        }
    }
    report
}

fn check_rule_keys(rule: &toml_edit::Table, at: &str, text: &str, file: &str, report: &mut Report) {
    let Some(layers) = rule.get("layers").and_then(|it| it.as_table()) else {
        return;
    };
    for (layer, value) in layers.iter() {
        let Some(areas) = value
            .as_table()
            .and_then(|it| it.get("areas"))
            .and_then(|it| it.as_array_of_tables())
        else {
            continue;
        };
        for area in areas {
            let area_id = area.get("id").and_then(|it| it.as_str()).unwrap_or("");
            let kind = area.get("kind").and_then(|it| it.as_str()).unwrap_or("");
            let allowed = keys_of_kind(kind);
            let at = format!("outputs.{at}.layers.{layer}.areas.{area_id}");
            report_strays(
                area, AREA_KEYS, allowed, &at, "area", kind, text, file, report,
            );

            let Some(groups) = area.get("groups").and_then(|it| it.as_array_of_tables()) else {
                continue;
            };
            for group in groups {
                let group_id = group.get("id").and_then(|it| it.as_str()).unwrap_or("");
                let place = group.get("place").and_then(|it| it.as_str()).unwrap_or("");
                let at = format!("{at}.groups.{group_id}");
                report_strays(
                    group,
                    GROUP_KEYS,
                    keys_of_place(place),
                    &at,
                    "group",
                    place,
                    text,
                    file,
                    report,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn report_strays(
    table: &toml_edit::Table,
    common: &[&str],
    allowed: &[&str],
    at: &str,
    what: &str,
    kind: &str,
    text: &str,
    file: &str,
    report: &mut Report,
) {
    for (key, value) in table.iter() {
        if common.contains(&key) || allowed.contains(&key) {
            continue;
        }
        let mut finding = Finding::new(
            file,
            format!("{at}.{key}"),
            match kind.is_empty() {
                true => format!("`{key}` is not a key of an {what}"),
                false => format!("`{key}` is not a key of a `{kind}` {what}"),
            },
        );
        finding.span = value
            .span()
            .map(|bytes| util::report::Span::locate(text, bytes));
        report.error(finding);
    }
}

/// How many colour stops a gradient may have. The renderer holds them in a fixed array of this length, so a ninth is not a subtlety that gets lost — it is a stop the user wrote and will never see.
const MOST_STOPS: usize = 8;

fn check_gradients(layers: &Layers, at: &str, file: &str, report: &mut Report) {
    for (kind, layer) in each_layer(layers) {
        for area in &layer.areas {
            let Some(AreaKind::Texture {
                gradient: Some(gradient),
                ..
            }) = &area.kind
            else {
                continue;
            };
            if gradient.stops.len() > MOST_STOPS {
                report.error(Finding::new(
                    file,
                    format!("{at}.layers.{kind}.areas.{}.gradient.stops", area.id),
                    format!(
                        "a gradient is drawn from at most {MOST_STOPS} stops and this one has {}, so the last {} would never appear",
                        gradient.stops.len(),
                        gradient.stops.len() - MOST_STOPS
                    ),
                ));
            }
        }
    }
}

fn check_layer_ids(
    layers: &Layers,
    at: &str,
    file: &str,
    instances: &mut BTreeSet<InstanceId>,
    report: &mut Report,
) {
    for (kind, layer) in each_layer(layers) {
        let mut areas = BTreeSet::new();
        for area in &layer.areas {
            if !areas.insert(area.id.clone()) {
                report.error(Finding::new(
                    file,
                    format!("{at}.layers.{kind}.areas.{}", area.id),
                    "two areas on this layer share an id, so a rule cannot say which it means"
                        .to_string(),
                ));
            }
            let mut groups = BTreeSet::new();
            for group in &area.groups {
                if !groups.insert(group.id.clone()) {
                    report.error(Finding::new(
                        file,
                        format!("{at}.layers.{kind}.areas.{}.groups.{}", area.id, group.id),
                        "two groups in this area share an id".to_string(),
                    ));
                }
                for instance in &group.children {
                    if !instances.insert(instance.id.clone()) {
                        report.error(Finding::new(
                            file,
                            format!("{at}.layers.{kind}.areas.{}.groups.{}.children.{}", area.id, group.id, instance.id),
                            "this instance id is already used elsewhere in the layout, and IPC addresses instances by id".to_string(),
                        ));
                    }
                }
            }
        }
    }
}

fn check_modules(
    layers: &Layers,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    for (kind, layer) in each_layer(layers) {
        for (area, group, instance) in instances_of(layer) {
            let Some(module) = &instance.module else {
                continue;
            };
            let path = format!(
                "{at}.layers.{kind}.areas.{}.groups.{}.children.{}",
                area.id, group.id, instance.id
            );
            if !catalogue.knows_module(module) {
                report.error(Finding::new(
                    file,
                    format!("{path}.module"),
                    format!("there is no module called `{module}`"),
                ));
                continue;
            }
            let representation = instance.representation.unwrap_or(Representation::Chip);
            if !catalogue.has_representation(module, representation) {
                report.error(Finding::new(
                    file,
                    format!("{path}.representation"),
                    format!(
                        "`{module}` cannot be drawn as `{}`",
                        representation.as_str()
                    ),
                ));
            }
        }
    }
}

fn check_actions(
    layers: &Layers,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    for (kind, layer) in each_layer(layers) {
        for (area, group, instance) in instances_of(layer) {
            for (trigger, action) in &instance.actions {
                let path = format!(
                    "{at}.layers.{kind}.areas.{}.groups.{}.children.{}.actions.{:?}",
                    area.id, group.id, instance.id, trigger
                );
                for line in &action.0 {
                    if !catalogue.command_resolves(line) {
                        report.error(Finding::new(
                            file,
                            path.clone(),
                            format!("`{line}` is not a command this shell has"),
                        ));
                    }
                }
            }
        }
    }
}

fn check_lock_layer(
    lock: &Layer,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    for (area, group, instance) in instances_of(lock) {
        let path = format!(
            "{at}.layers.lock.areas.{}.groups.{}.children.{}",
            area.id, group.id, instance.id
        );
        if !instance.actions.is_empty() {
            report.error(Finding::new(
                file,
                format!("{path}.actions"),
                "the lock layer holds readings, never controls, so an action here is refused"
                    .to_string(),
            ));
        }
        let Some(module) = &instance.module else {
            continue;
        };
        let representation = instance.representation.unwrap_or(Representation::Chip);
        if catalogue.knows_module(module) && !catalogue.is_read_only(module, representation) {
            report.error(Finding::new(
                file,
                format!("{path}.module"),
                format!(
                    "`{module}` as `{}` can be interacted with, and the lock layer takes readings only",
                    representation.as_str()
                ),
            ));
        }
    }
}

fn check_workspace_rules(
    layout: &Layout,
    rule: &OutputRule,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    let mut output_level = Layers::default();
    for other in &layout.outputs {
        if other.matches.specificity() <= rule.matches.specificity() {
            merge_layers(&mut output_level, &other.layers);
        }
    }
    let reserving = reserving_areas(&output_level);

    for workspace in &rule.workspaces {
        let at = format!("{at}.workspaces.{}", workspace.matches.0);
        let layers = Layers {
            background: workspace.layers.background.clone(),
            desktop: workspace.layers.desktop.clone(),
            top: workspace.layers.top.clone(),
            overlay: workspace.layers.overlay.clone(),
            lock: Layer::default(),
        };

        for (kind, layer) in each_layer(&layers) {
            for id in &layer.remove {
                if reserving.contains(id) {
                    report.error(Finding::new(
                        file,
                        format!("{at}.layers.{kind}.remove"),
                        format!("`{id}` reserves space, and a workspace rule that removed it would re-tile every window on each workspace switch"),
                    ));
                }
            }
            for area in &layer.areas {
                let path = format!("{at}.layers.{kind}.areas.{}", area.id);
                if area.reserve.is_some() {
                    report.error(Finding::new(
                        file,
                        format!("{path}.reserve"),
                        "a workspace rule cannot change what an area reserves: reservation is decided per output, so that switching workspaces never re-tiles windows".to_string(),
                    ));
                }
                if reserving.contains(&area.id) && area.kind.is_some() {
                    report.error(Finding::new(
                        file,
                        format!("{path}.kind"),
                        format!("`{}` reserves space, so a workspace rule may change what is inside it but not its geometry", area.id),
                    ));
                }
            }
        }

        check_actions(&layers, &at, file, catalogue, report);
        check_modules(&layers, &at, file, catalogue, report);
    }
}

fn reserving_areas(layers: &Layers) -> BTreeSet<AreaId> {
    each_layer(layers)
        .into_iter()
        .flat_map(|(_, layer)| layer.areas.iter())
        .filter(|area| area.reserve == Some(true))
        .map(|area| area.id.clone())
        .collect()
}

fn each_layer(layers: &Layers) -> Vec<(LayerKind, &Layer)> {
    vec![
        (LayerKind::Background, &layers.background),
        (LayerKind::Desktop, &layers.desktop),
        (LayerKind::Top, &layers.top),
        (LayerKind::Overlay, &layers.overlay),
        (LayerKind::Lock, &layers.lock),
    ]
}

fn instances_of(layer: &Layer) -> impl Iterator<Item = (&Area, &Group, &Instance)> {
    layer.areas.iter().flat_map(|area| {
        area.groups.iter().flat_map(move |group| {
            group
                .children
                .iter()
                .map(move |instance| (area, group, instance))
        })
    })
}
