//! What the layout says should be on screen, and keeping the screen in step with it.
//!
//! The shell puts up two kinds of surface on its own: one fullscreen layer window per wlr layer per output ([`crate::layer_window`]), which is where everything it draws lives, and the invisible strip each reserving edge carves out of every window's idea of the screen. This module is the one pass that answers both from the same arrangement: [`plan`] resolves the active layout for every output, and [`Shell`] owns the live windows and strips and brings them in line with it.
//!
//! **A reload reuses, it does not replace.** A window's identity is its `(output, layer)` and a strip's is its `(output, edge)`, and both survive an edit — so a layout change reaches the window that is already up and is a rebuild of its tree rather than a new surface. Which is what makes editing a layout with the settings window open bearable, and what keeps a chip's state across the edit that moved the chip beside it.
//!
//! **Reservation is an output-level fact, never a workspace one.** A strip's thickness is summed across every layer of the output's own rules, so switching workspaces can add and remove areas but can never re-tile the user's windows (F-6.7).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use platform_wayland::{Anchor, Layer, LayerConfig, OutputDescriptor, SurfaceHandle};

use config::{Config, Edge};
use layout::{
    ActiveWorkspace, AreaId, LayerKind, Layout, LayoutId, NOMINAL_OUTPUT, Resolved,
    ResolvedAreaKind, Route, StackOutputPolicy, resolve,
};
use util::report::Report;

use crate::area::ShellAreas;
use crate::layer_window::{Content, LayerPlan, LayerWindows, Reconciled, Reserved};

/// One output's arrangement, owned for as long as the pass that planned it: the config it resolves module behaviour against, what the layout said about it, and how big it is.
pub struct Desktop {
    pub output: Option<String>,
    pub config: Arc<Config>,
    pub resolved: Resolved,
    pub reserved: Reserved,
    pub size: (f32, f32),
}

impl Desktop {
    pub(crate) fn plan(&self) -> LayerPlan<'_> {
        LayerPlan {
            output: self.output.as_deref(),
            config: &self.config,
            resolved: &self.resolved,
            reserved: self.reserved,
            size: self.size,
        }
    }
}

/// What a reconcile did to the screen, in the terms the log and the tests speak.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub windows: Reconciled,
    pub strips: usize,
}

/// The live windows and reservation strips, keyed so a reload finds the ones it is about.
pub struct Shell {
    windows: LayerWindows,
    strips: Vec<(StripKey, Strip)>,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    pub fn new() -> Self {
        Self {
            windows: LayerWindows::new(Rc::new(ShellAreas)),
            strips: Vec::new(),
        }
    }

    /// Brings the screen in line with `desktops`: hands every window its layer's new arrangement and every edge the strip its areas reserve.
    ///
    /// The strips go first, so an edge that stopped reserving gives its zone back before anything else is measured against the screen it was on — otherwise every surface would be configured once against the old zone and again a frame later.
    pub fn reconcile(&mut self, desktops: &[Desktop], content: Content) -> Done {
        self.reconcile_strips(desktops);
        publish_stacks(desktops);
        let plans: Vec<LayerPlan<'_>> = desktops.iter().map(Desktop::plan).collect();
        let windows = self.windows.reconcile(&plans, content);
        tracing::info!(
            windows = self.windows.len(),
            mapped = windows.mapped,
            opened = windows.opened,
            closed = windows.closed,
            rebuilt = windows.rebuilt,
            strips = self.strips.len(),
            "the screen is in step with the layout"
        );
        Done {
            windows,
            strips: self.strips.len(),
        }
    }

    /// The layer windows, for whatever needs to hold one open or ask what it is asking of the compositor.
    pub fn windows(&self) -> &LayerWindows {
        &self.windows
    }

    pub fn len(&self) -> usize {
        self.windows.len() + self.strips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty() && self.strips.is_empty()
    }

    fn reconcile_strips(&mut self, desktops: &[Desktop]) {
        let wanted: Vec<(StripKey, u32)> = desktops
            .iter()
            .flat_map(|desktop| {
                let output = desktop.output.clone();
                Edge::ALL.into_iter().filter_map(move |edge| {
                    let thickness = desktop.reserved.on(edge).round() as u32;
                    (thickness > 0).then(|| {
                        (
                            StripKey {
                                output: output.clone(),
                                edge,
                            },
                            thickness,
                        )
                    })
                })
            })
            .collect();
        let keep: HashSet<&StripKey> = wanted.iter().map(|(key, _)| key).collect();
        self.strips.retain(|(key, _)| keep.contains(key));
        for (key, thickness) in wanted {
            let layer = reservation(key.edge, thickness, key.output.clone());
            match self.strips.iter_mut().find(|(live, _)| *live == key) {
                Some((_, strip)) => strip.adopt(layer),
                None => {
                    let strip = Strip::open(layer);
                    self.strips.push((key, strip));
                }
            }
        }
    }
}

/// A reservation strip's identity across a reload: which screen it is on and which edge it carves.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct StripKey {
    output: Option<String>,
    edge: Edge,
}

/// One live reservation strip. It draws nothing and takes no input, so there is no tree to rebuild — only an exclusive zone to renegotiate when the areas behind it change thickness.
struct Strip {
    handle: SurfaceHandle,
    layer: LayerConfig,
}

impl Strip {
    fn open(layer: LayerConfig) -> Self {
        let handle = platform_wayland::open_reservation(layer.clone());
        Self { handle, layer }
    }

    fn adopt(&mut self, layer: LayerConfig) {
        let change = self.layer.delta(&layer);
        if !change.is_empty() {
            self.handle.update(change);
        }
        self.layer = layer;
    }
}

/// Every output's arrangement under `layout`, with the per-monitor config each one resolves module behaviour against.
///
/// Resolution is per output and independent, which is what lets a monitor be re-planned on hotplug without touching the others. Whatever a layout could not answer is in the returned [`Report`] and missing from the arrangement, so the screen shows what the user asked for minus the parts that were not finished.
pub fn plan(
    path: &Path,
    config: &Arc<Config>,
    layout: &Layout,
    known: &BTreeMap<LayoutId, Layout>,
    outputs: &[OutputDescriptor],
    workspace: &dyn Fn(Option<&str>) -> Option<ActiveWorkspace>,
) -> (Vec<Desktop>, Report) {
    layout::set_running(Arc::new(layout.clone()));
    let mut report = Report::default();
    let mut unblurred = std::collections::BTreeSet::new();
    let desktops: Vec<Desktop> = outputs
        .iter()
        .map(|out| {
            let name = out.name.as_deref();
            // Every window on this output resolves against the same merged config, so a per-monitor override reaches the bars, the wallpaper and the strips together — a bar sized by one config and a strip sized by another would carve the wrong zone out of the screen.
            let config = output_config(path, config, name);
            if let Some(name) = name {
                config::set_output_config(name, Arc::clone(&config));
            }
            let active = workspace(name);
            let (resolved, findings) = resolve(
                layout,
                known,
                name.unwrap_or(NOMINAL_OUTPUT),
                active.as_ref(),
            );
            report.merge(findings);
            unblurred.extend(
                resolved
                    .areas()
                    .filter(|(_, area)| area.style.backdrop == Some(layout::Backdrop::Blur))
                    .map(|(layer, area)| format!("{layer}.{}", area.id)),
            );
            let reserved = Reserved::of(&resolved, &config);
            Desktop {
                output: name.map(str::to_string),
                config,
                resolved,
                reserved,
                size: logical_size(out),
            }
        })
        .collect();
    let stacked = desktops.iter().any(|desktop| {
        desktop
            .resolved
            .areas()
            .any(|(_, area)| matches!(area.kind, ResolvedAreaKind::Stack { .. }))
    });
    if !stacked && !desktops.is_empty() {
        report.warn(util::report::Finding::new(
            format!("layouts/{}.toml", layout.id),
            "outputs",
            "no output has a stack area, so notifications, toasts and OSDs are shown nowhere"
                .to_string(),
        ));
    }
    if !platform_wayland::background_effect_supported() {
        for area in unblurred {
            report.warn(util::report::Finding::new(
                format!("layouts/{}.toml", layout.id),
                area,
                "the compositor offers no background blur (ext-background-effect-v1), so this area draws translucent without it".to_string(),
            ));
        }
    }
    (desktops, report)
}

/// The monitor's logical size, and a sane stand-in where the compositor has not reported one yet — a window built against zero would lay every area out at nothing and have to be rebuilt the moment the size arrived.
fn logical_size(out: &OutputDescriptor) -> (f32, f32) {
    match out.logical_size {
        Some((width, height)) if width > 0 && height > 0 => (width as f32, height as f32),
        _ => (1920.0, 1080.0),
    }
}

/// The config `output` runs under: its `monitors/<output>/config.toml` merged over the global one, falling back to the global config when it has no override or that override will not parse. A broken per-monitor file costs that one screen its overrides and a log line, never the whole shell's layout.
fn output_config(path: &Path, global: &Arc<Config>, output: Option<&str>) -> Arc<Config> {
    let Some(output) = output else {
        return Arc::clone(global);
    };
    match Config::for_output(path, Some(output)) {
        Ok(config) => Arc::new(config),
        Err(e) => {
            tracing::warn!("monitor '{output}': {e}; using the global config");
            Arc::clone(global)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StackSite {
    pub output: Option<String>,
    pub layer: LayerKind,
    pub area: AreaId,
    pub policy: StackOutputPolicy,
    pub routes: Vec<Route>,
}

thread_local! {
    static STACKS: std::cell::RefCell<Vec<StackSite>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Every stack area of every output, in the order a card is offered to them: output by output, bottom layer first, and in each layer in z-order.
pub fn stacks() -> Vec<StackSite> {
    STACKS.with(|stacks| stacks.borrow().clone())
}

pub fn publish_stacks(desktops: &[Desktop]) {
    let sites = desktops
        .iter()
        .flat_map(|desktop| {
            desktop
                .resolved
                .areas()
                .filter_map(|(layer, area)| match &area.kind {
                    ResolvedAreaKind::Stack {
                        output_policy,
                        routes,
                        ..
                    } => Some(StackSite {
                        output: desktop.output.clone(),
                        layer,
                        area: area.id.clone(),
                        policy: output_policy.clone(),
                        routes: routes.clone(),
                    }),
                    _ => None,
                })
        })
        .collect();
    STACKS.with(|stacks| *stacks.borrow_mut() = sites);
}

/// The strip that carves `thickness` off `edge` of `output` out of every window's idea of the screen. It draws nothing and takes no input: it exists to hold an exclusive zone.
fn reservation(edge: Edge, thickness: u32, output: Option<String>) -> LayerConfig {
    let (anchor, size) = match edge {
        Edge::Top => (Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, thickness)),
        Edge::Bottom => (
            Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            (0, thickness),
        ),
        Edge::Left => (Anchor::LEFT | Anchor::TOP | Anchor::BOTTOM, (thickness, 0)),
        Edge::Right => (Anchor::RIGHT | Anchor::TOP | Anchor::BOTTOM, (thickness, 0)),
    };
    LayerConfig {
        output,
        layer: Layer::Bottom,
        anchor,
        exclusive_zone: thickness as i32,
        size,
        namespace: RESERVE_NAMESPACE.to_string(),
        reserve_only: true,
        ..LayerConfig::default()
    }
}

pub const RESERVE_NAMESPACE: &str = "hogar-shell-reserve";
