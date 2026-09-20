//! The widgets surface: what the shell draws on the desktop itself — a clock face, an audio visualiser; kept off the wallpaper's surface since a changing layer forces a full redraw, which would rasterize the whole wallpaper photo every frame the visualiser moved.

use std::collections::BTreeMap;
use std::sync::Arc;

use telar::{
    App, Color, Component, Container, LayoutItem, WindowConfig, WindowRoot, reset_layout_runtime,
    set_theme,
};

use config::Config;
use layout::{
    Anchor, AreaId, AreaStyle, GroupId, GroupKind, InstanceId, Rect, Representation, ResolvedArea,
    ResolvedAreaKind, ResolvedGroup, ResolvedInstance, Zone,
};
use ui::descriptor::Built;
use ui::host::{GRID_CELL, GRID_GAP};
use ui::layout::fill;

use crate::area::{self, Surround};

/// Per-output widgets: a click-through surface over the free area of the screen, carrying whatever `[widgets]` asks for.
pub struct WidgetsApp {
    /// Read at every build rather than held: the surface outlives the config it was first drawn from, and a reload rebuilds it in place from whatever is in here now.
    pub config: config::LiveConfig,
}

impl App for WidgetsApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        let content = ui::panel::or_empty("widgets", content(&config));
        Box::new(WindowRoot::wrapping(content).expect("widgets surface root"))
    }

    fn clear_color(&self) -> Option<Color> {
        None
    }

    fn window_config(&self) -> Option<WindowConfig> {
        Some(WindowConfig {
            is_transparent: true,
            ..WindowConfig::default()
        })
    }
}

/// The desktop's widgets as this surface draws them, for [`crate::preview`]. The clock is forced on because it is the widget that has something to look at with no music playing and no wallpaper set.
pub(crate) fn preview() -> Built {
    let mut config = config::config()
        .map(|live| (*live).clone())
        .unwrap_or_else(Config::starter);
    config.widgets.clock.enabled = true;
    content(&Arc::new(config))
}

/// Every widget `[widgets]` asks for, stacked over the same area.
fn content(config: &Arc<Config>) -> Built {
    let layers = layers(config)
        .into_iter()
        .map(|(_, layer)| layer)
        .collect::<Result<Vec<Box<dyn LayoutItem>>, _>>()?;
    Ok(Box::new(Container::new(fill(), layers)?))
}

/// Each area `[widgets]` switches on, named by the module in it, built as the layout kind it is.
pub fn layers(config: &Arc<Config>) -> Vec<(&'static str, Built)> {
    let surround = Surround {
        config,
        theme: config.resolve_theme(),
        output: None,
    };
    areas(config)
        .into_iter()
        .filter_map(|(module, area)| Some((module, area::build(&area, surround)?)))
        .collect()
}

/// The desktop areas today's `[widgets]` pair describes, said in the layout model's own words.
///
/// Temporary: it stands in for the desktop layer of a resolved layout until every surface is one window per layer, and it goes when `[widgets]` does. It says exactly what `hogar-shell layout import-config` writes for the same keys — a grid anchored where the clock's nine-way placement put it, a dock on the visualiser's edge, and each `margin` the area's padding — so what the shell draws from a config and what an imported layout resolves to are the same two areas.
fn areas(config: &Config) -> Vec<(&'static str, ResolvedArea)> {
    let mut areas = Vec::new();
    if config.widgets.clock.enabled {
        areas.push(("clock", clock_grid(&config.widgets.clock)));
    }
    if config.widgets.visualiser.enabled {
        areas.push(("visualiser", visualiser_dock(&config.widgets.visualiser)));
    }
    areas
}

fn clock_grid(clock: &config::DesktopClockConfig) -> ResolvedArea {
    desktop_area(
        "widgets",
        ResolvedAreaKind::Grid {
            rect: Rect::default(),
            cell: GRID_CELL,
            gap: GRID_GAP,
            anchor: anchored(clock.position),
        },
        clock.margin,
        vec![alone(
            "clock",
            GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
            },
            "clock",
            Representation::WidgetM,
        )],
    )
}

fn visualiser_dock(visualiser: &config::DesktopVisualiserConfig) -> ResolvedArea {
    desktop_area(
        "visualiser",
        ResolvedAreaKind::Dock {
            edge: visualiser.edge,
            thickness: visualiser.reach_px(),
        },
        visualiser.margin,
        vec![alone(
            "row",
            GroupKind::Zone { zone: Zone::Center },
            "visualiser",
            Representation::WidgetL,
        )],
    )
}

/// An area of the desktop layer: it never reserves, never stays up over a fullscreen window and is always drawn, which is everything the `[widgets]` keys have nothing to say about.
fn desktop_area(
    id: &str,
    kind: ResolvedAreaKind,
    margin: u32,
    groups: Vec<ResolvedGroup>,
) -> ResolvedArea {
    ResolvedArea {
        id: AreaId::new(id),
        kind,
        reserve: false,
        above_fullscreen: false,
        within: layout::Within::Usable,
        style: AreaStyle {
            padding: (margin > 0).then_some(margin as f32),
            ..AreaStyle::default()
        },
        visible: None,
        groups,
    }
}

/// A group of the one instance `[widgets]` can put in it, addressed by its module's own name so the state a widget keeps survives the move onto the model.
fn alone(
    group: &str,
    kind: GroupKind,
    module: &str,
    representation: Representation,
) -> ResolvedGroup {
    ResolvedGroup {
        id: GroupId::new(group),
        kind,
        children: vec![ResolvedInstance {
            id: InstanceId::new(module),
            module: module.to_string(),
            representation,
            options: toml::Table::new(),
            bindings: BTreeMap::new(),
            actions: BTreeMap::new(),
        }],
    }
}

/// The anchor a nine-way clock placement names. The two vocabularies have the same nine positions, so this is a rename and nothing else.
fn anchored(placement: config::ClockPlacement) -> Anchor {
    use config::ClockPlacement as Placed;
    match placement {
        Placed::TopLeft => Anchor::TopLeft,
        Placed::TopCenter => Anchor::Top,
        Placed::TopRight => Anchor::TopRight,
        Placed::CenterLeft => Anchor::Left,
        Placed::Center => Anchor::Center,
        Placed::CenterRight => Anchor::Right,
        Placed::BottomLeft => Anchor::BottomLeft,
        Placed::BottomCenter => Anchor::Bottom,
        Placed::BottomRight => Anchor::BottomRight,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Align, ClockPlacement};

    /// With no module table installed there is nothing to place, and each layer is a placeholder rather than a surface that fails to exist.
    #[test]
    fn a_widget_with_no_installed_module_is_a_placeholder_layer() {
        telar::reset_layout_runtime();
        let mut config = Config::starter();
        telar::set_theme(config.resolve_theme());
        config.widgets.clock.enabled = true;
        config.widgets.visualiser.enabled = true;
        let layers = layers(&Arc::new(config.clone()));
        assert_eq!(layers.len(), 2);
        assert!(layers.iter().all(|(_, layer)| layer.is_ok()));
        let _ = WidgetsApp {
            config: Arc::new(config).into(),
        }
        .root();
    }

    #[test]
    fn the_nine_positions_map_onto_distinct_corners() {
        let corners: Vec<(Align, Align)> = ClockPlacement::ALL
            .into_iter()
            .map(|placement| placement.alignment())
            .collect();
        for (index, corner) in corners.iter().enumerate() {
            for other in &corners[index + 1..] {
                assert_ne!(corner, other, "two placements land in the same spot");
            }
        }
        assert_eq!(
            ClockPlacement::TopLeft.alignment(),
            (Align::Start, Align::Start),
            "the row is the vertical axis and the column the horizontal one"
        );
        assert_eq!(
            ClockPlacement::from_id("bottom-right"),
            Some(ClockPlacement::BottomRight)
        );
        assert_eq!(ClockPlacement::from_id("nowhere"), None);
    }

    /// The clock's placement is where the grid's cells sit in the region, and its margin is how far the area holds them off its edges — the two keys the area answers rather than the widget.
    #[test]
    fn the_clock_becomes_one_cell_of_a_grid_anchored_where_it_was_placed() {
        let mut config = Config::starter();
        config.widgets.clock.enabled = true;
        config.widgets.clock.position = ClockPlacement::BottomRight;
        config.widgets.clock.margin = 64;
        let areas = areas(&config);
        assert_eq!(
            areas.iter().map(|(module, _)| *module).collect::<Vec<_>>(),
            vec!["clock"]
        );

        let clock = &areas[0].1;
        let ResolvedAreaKind::Grid {
            cell, gap, anchor, ..
        } = clock.kind
        else {
            panic!("the clock sits on a grid");
        };
        assert_eq!(anchor, Anchor::BottomRight);
        assert_eq!((cell, gap), (GRID_CELL, GRID_GAP));
        assert_eq!(clock.style.padding, Some(64.0));
        assert_eq!(
            clock.groups[0].kind,
            GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1
            }
        );
        assert_eq!(clock.groups[0].children[0].id.as_str(), "clock");
    }

    /// The visualiser's reach is how deep its strip is and its edge is the one the strip hugs, which is a dock and nothing else.
    #[test]
    fn the_visualiser_becomes_a_dock_as_deep_as_its_reach() {
        let mut config = Config::starter();
        config.widgets.visualiser.enabled = true;
        config.widgets.visualiser.edge = config::Edge::Left;
        let reach = config.widgets.visualiser.reach_px();
        let areas = areas(&config);
        let visualiser = &areas[0].1;
        assert_eq!(
            visualiser.kind,
            ResolvedAreaKind::Dock {
                edge: config::Edge::Left,
                thickness: reach,
            }
        );
        assert_eq!(
            visualiser.groups[0].kind,
            GroupKind::Zone { zone: Zone::Center }
        );
    }
}
