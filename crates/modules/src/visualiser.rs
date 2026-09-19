//! The audio visualiser: a ring in a small widget, a row on `[widgets.visualiser] edge` in a wider one. The bars hide by opacity, never by leaving the tree: rebuilding children on a value that changes with the music is a re-layout per frame.

use telar::{
    LayoutError, LayoutItem, LayoutStyle, RectStyle, SizeDimension, StyledContainer,
    motion::Animated, signal,
};

use config::{
    Align, AnimationConfig, DesktopVisualiserConfig, Edge, VisualiserConfig, WidgetsConfig,
};
use services::visualiser;
use ui::host::Host;
use ui::layout::{align_items, fill, justify};
use ui::widget::{self, SpectrumStyle};
use util::reactive::{derive, fixed};

/// How much of the ring's radius is the hole in the middle, the rest being the bars' reach.
const RING_HOLE: f32 = 0.45;

pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let settings = host.options::<WidgetsConfig>().visualiser;
    let tint = if settings.accent {
        host.accent
    } else {
        host.foreground
    };

    let start = visualiser::Spectrum::quiet(host.options::<VisualiserConfig>().band_count());
    let bands = signal(start.bars.clone());
    let silent = signal(start.silent);
    let (next_bands, next_silent) = (bands, silent);
    platform_wayland::watch(
        visualiser::subscribe,
        move |spectrum: visualiser::Spectrum| {
            next_bands.set(spectrum.bars);
            next_silent.set(spectrum.silent);
        },
    );
    let bands = derive(bands.read_only(), |bars| bars);
    let tint = fixed(tint.with_alpha(settings.alpha()));
    let style = SpectrumStyle {
        gap: settings.gap_px(),
        radius: settings.radius_px(),
        floor: 0.0,
    };

    let edge = settings.edge;
    let bars = if host.is_small() {
        let radius = host.extent.width.min(host.extent.height) / 2.0;
        widget::spectrum_ring(
            bands,
            tint,
            radius * RING_HOLE,
            radius * (1.0 - RING_HOLE),
            style,
            fill(),
        )?
    } else {
        let across = if edge.is_horizontal() {
            host.extent.height
        } else {
            host.extent.width
        };
        widget::spectrum(
            bands,
            tint,
            edge,
            style,
            standing_on(edge, settings.reach_px().min(across)),
        )?
    };

    let fade = fade(&settings, &host.config().animation, silent.read_only());
    let layer = StyledContainer::new(
        fill()
            .flex_row()
            .align_items(align_items(match edge {
                Edge::Top => Align::Start,
                Edge::Bottom => Align::End,
                _ => Align::Center,
            }))
            .justify_content(justify(match edge {
                Edge::Left => Align::Start,
                Edge::Right => Align::End,
                _ => Align::Center,
            })),
        |_| RectStyle::default(),
        vec![bars],
    )?
    .with_opacity(fade);
    Ok(Box::new(layer))
}

/// The row's own box: as long as the edge it stands on, as deep as its reach.
fn standing_on(edge: Edge, reach: f32) -> LayoutStyle {
    if edge.is_horizontal() {
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(reach)
    } else {
        LayoutStyle::new()
            .width(reach)
            .height(SizeDimension::Percent(1.0))
    }
}

/// How opaque the bars are: one with sound, zero when `hide_when_silent` and there is none — switched rather than tweened with animation off, where a tween would have no duration to divide by.
fn fade(
    settings: &DesktopVisualiserConfig,
    animation: &AnimationConfig,
    silent: telar::ReadSignal<bool>,
) -> Box<dyn Fn() -> f32> {
    if !settings.hide_when_silent {
        return Box::new(|| 1.0);
    }
    if !animation.enabled {
        return Box::new(move || if silent.get() { 0.0 } else { 1.0 });
    }
    let fade = Animated::new(0.0f32, animation.tween_ms(400, 5_000));
    let target = fade;
    Box::new(move || {
        // Read first: the retarget is what makes the bars appear, and it has to be registered as a dependency on the frame that draws nothing too.
        let quiet = silent.get();
        target.retarget(if quiet { 0.0 } else { 1.0 });
        fade.get()
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use config::Config;
    use ui::host::{Representation, WidgetSize};

    use super::*;

    #[test]
    fn the_bars_build_at_every_size_on_every_edge_and_with_the_fade_both_ways() {
        for size in WidgetSize::ALL {
            for edge in Edge::ALL {
                for hide in [true, false] {
                    for animated in [true, false] {
                        telar::reset_layout_runtime();
                        let _scope = telar::owner_scope();
                        let mut config = Config::starter();
                        config.widgets.visualiser = config::DesktopVisualiserConfig {
                            enabled: true,
                            edge,
                            hide_when_silent: hide,
                            ..config::DesktopVisualiserConfig::default()
                        };
                        config.animation.enabled = animated;
                        telar::set_theme(config.resolve_theme());
                        let host = ui::preview::host_on(
                            Arc::new(config),
                            "visualiser",
                            Representation::Widget(size),
                            size.extent(),
                        );
                        assert!(
                            host.build(widget).is_ok(),
                            "{size:?} on {edge:?}, hide={hide}, animated={animated}"
                        );
                    }
                }
            }
        }
    }
}
