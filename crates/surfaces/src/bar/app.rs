use std::collections::BTreeMap;
use std::sync::Arc;

use telar::{App, Color, Component, LayoutItem, WindowConfig, reset_layout_runtime, set_theme};

use config::{Config, Edge, ModuleEntry, SurfaceEnv, set_surface_env};
use layout::{
    AreaId, BarShape, Extent, GroupId, GroupKind, InstanceId, Representation, ResolvedArea,
    ResolvedAreaKind, ResolvedGroup, ResolvedInstance, Zone,
};
use telar::WindowRoot;
use toml::{Table, Value};

use super::{AutoHide, build_bar};

/// The area one edge's `[bars.<edge>]` section describes.
///
/// Temporary, and deliberately the only thing left that turns bar config into geometry. [`build_bar`] reads an area, so every caller that still has an edge in hand — this surface, the preview, the tests — comes through here instead of the builder reading `[bars.<edge>]` itself. It goes when a layer opens one window and hands the builder the areas the layout resolved.
///
/// A config bar always runs its whole edge from its start, which is [`Extent::Fill`] at offset zero; a length and an offset of its own are things only a layout file can say.
pub(crate) fn bar_area(config: &Config, edge: Edge) -> ResolvedArea {
    let bar = config.bars.get(edge);
    let (lead, trail) = config.corner_modules_for(edge);
    let mut start: Vec<ModuleEntry> = Vec::new();
    start.extend(lead.map(ModuleEntry::bare));
    start.extend(bar.start.iter().cloned());
    let mut end: Vec<ModuleEntry> = bar.end.clone();
    end.extend(trail.map(ModuleEntry::bare));
    ResolvedArea {
        id: AreaId::new(format!("bar-{}", edge.as_str())),
        kind: ResolvedAreaKind::Bar {
            edge,
            thickness: bar.size as f32,
            length: Extent::Fill,
            offset: 0.0,
            shape: BarShape {
                mode: bar.shape.mode,
                gap: bar.shape.gap.map(|px| px as f32),
                spacing: bar.shape.spacing.map(|px| px as f32),
                radius: bar.shape.radius.map(|px| px as f32),
            },
            autohide: (!bar.persistent).then(|| layout::AutoHide {
                peek: config.bar_peek(edge) as f32,
                on_hover: bar.show_on_hover,
            }),
        },
        reserve: config.bar_is_persistent(edge),
        above_fullscreen: false,
        within: layout::Within::Output,
        style: layout::AreaStyle::default(),
        visible: None,
        groups: vec![
            zone_group("start", Zone::Start, &start),
            zone_group("center", Zone::Center, &bar.center),
            zone_group("end", Zone::End, &end),
        ],
    }
}

/// `[corners]` sugar lands here rather than on a surface of its own: a corner module is routed into the start or end zone of the bar that owns its corner, which is what [`Config::corner_modules_for`] answers.
fn zone_group(id: &str, zone: Zone, entries: &[ModuleEntry]) -> ResolvedGroup {
    ResolvedGroup {
        id: GroupId::new(id),
        kind: GroupKind::Zone { zone },
        children: entries.iter().map(placed).collect(),
    }
}

/// One `[bars.<edge>]` entry as a placed instance: its `variant` and `accent` become instance options, which is where a chip's own look is read from now that an entry is not what the builder sees.
fn placed(entry: &ModuleEntry) -> ResolvedInstance {
    let mut options = Table::new();
    if let Some(variant) = entry
        .variant
        .and_then(|variant| Value::try_from(variant).ok())
    {
        options.insert("variant".into(), variant);
    }
    if let Some(accent) = &entry.accent {
        options.insert("accent".into(), Value::String(accent.clone()));
    }
    ResolvedInstance {
        id: InstanceId::new(&entry.id),
        module: entry.id.clone(),
        representation: Representation::Chip,
        options,
        bindings: BTreeMap::new(),
        actions: BTreeMap::new(),
    }
}

pub struct BarApp {
    /// Read at every build rather than held: this surface outlives the config it was first drawn from, and a reload rebuilds it in place from whatever is in here now.
    pub config: config::LiveConfig,
    pub edge: Edge,
    /// The monitor this bar surface lives on; threaded into `SurfaceEnv` so its panels open on the same screen.
    pub output: Option<String>,
}

impl App for BarApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        let theme = config.resolve_theme();
        set_theme(theme);
        services::locale::attach(config.language());
        set_surface_env(SurfaceEnv::for_edge(
            Arc::clone(&config),
            self.edge,
            self.output.clone(),
        ));
        let bar = ui::panel::or_empty(
            "bar",
            build_bar(
                &config,
                &bar_area(&config, self.edge),
                self.output.as_deref(),
                ui::descriptor::installed(),
                theme,
            ),
        );
        // `persistent = false` moves the surface itself, so the wrapper goes here — around the whole bar, inside the surface root that drives it — rather than around any one zone.
        let bar: Box<dyn LayoutItem> = if config.bar_is_persistent(self.edge) {
            bar
        } else {
            Box::new(AutoHide::new(
                bar,
                &config,
                self.edge,
                config::bar_margin_for(&config, self.edge),
            ))
        };
        Box::new(WindowRoot::wrapping(bar).expect("bar layout failed"))
    }

    fn clear_color(&self) -> Option<Color> {
        // Opaque bar fills entire surface; floating/sections/chips bar has gaps so surface must be transparent.
        let config = self.config.get();
        if config.bar_surface_opaque(self.edge) {
            Some(config.resolve_theme().base)
        } else {
            None
        }
    }

    fn window_config(&self) -> Option<WindowConfig> {
        if self.config.get().bar_surface_opaque(self.edge) {
            None
        } else {
            Some(WindowConfig {
                is_transparent: true,
                ..WindowConfig::default()
            })
        }
    }
}
