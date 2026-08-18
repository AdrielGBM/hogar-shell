//! The widgets surface: what the shell draws on the desktop itself — a clock face, an audio visualiser.
//!
//! One surface per monitor, over the wallpaper and under every window. It is deliberately *not* the wallpaper's
//! surface, and the reason is what a repaint costs. A layer that changes forces its whole surface to be redrawn,
//! and the visualiser changes with the music: sharing the wallpaper's surface meant rasterizing a full screen of
//! photograph sixty times a second on the CPU, which on a laptop is a core pinned and forty degrees.
//!
//! **It measures the free area, not the screen.** The wallpaper opts out of every exclusive zone; this one
//! respects them, so the compositor hands it exactly what the bars left over, and a gap off that keeps it clear
//! of them. So `position = "center"` is the centre of the space applications get, which is also the centre a
//! user looking at their desktop sees.

use telar::{
    AlignItems, App, Color, Component, Container, JustifyContent, LayoutError, LayoutItem,
    LayoutStyle, RectStyle, Shadow, SizeDimension, StyledContainer, Text, WindowConfig, box_item,
    motion::Animated, reset_layout_runtime, set_theme, signal,
};

use config::theme::FontRole;
use config::{Align, Config};
use services::{clock, visualiser};
use telar::WindowRoot;
use util::reactive::{derive, fixed};

/// Per-output widgets: a click-through surface over the free area of the screen, carrying whatever `[widgets]`
/// asks for.
pub struct WidgetsApp {
    /// Read at every build rather than held: the surface outlives the config it was first drawn from, and a
    /// reload rebuilds it in place from whatever is in here now.
    pub config: config::LiveConfig,
}

impl App for WidgetsApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        Box::new(WindowRoot::wrapping(content(&config)).expect("widgets layout failed"))
    }

    fn clear_color(&self) -> Option<Color> {
        None
    }

    fn window_config(&self) -> Option<WindowConfig> {
        // Transparent: everything under here — the wallpaper, or whatever the compositor paints — has to show.
        Some(WindowConfig {
            is_transparent: true,
            ..WindowConfig::default()
        })
    }
}

/// The desktop's widgets as this surface draws them, for [`crate::preview`]. The clock is forced on because it
/// is the widget that has something to look at with no music playing and no wallpaper set.
pub(crate) fn preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut config = config::config()
        .map(|live| (*live).clone())
        .unwrap_or_else(Config::starter);
    config.widgets.clock.enabled = true;
    Ok(content(&config))
}

/// Every widget `[widgets]` asks for, stacked over the same area.
///
/// A widget that fails to lay out costs its own layer and a log line, never the surface: a clock with an
/// impossible scale must not take the visualiser down with it.
fn content(config: &Config) -> Box<dyn LayoutItem> {
    let mut layers: Vec<Box<dyn LayoutItem>> = Vec::new();
    if config.widgets.clock.enabled {
        match clock_face(config) {
            Ok(face) => layers.push(face),
            Err(e) => tracing::warn!("desktop clock: {e}"),
        }
    }
    if config.widgets.visualiser.enabled {
        match visualiser_row(config) {
            Ok(row) => layers.push(row),
            Err(e) => tracing::warn!("desktop visualiser: {e}"),
        }
    }
    Container::new(fill(), layers)
        .map(|container| Box::new(container) as Box<dyn LayoutItem>)
        .expect("widgets root container")
}

/// A full-surface style, used for every layer so they stack rather than sit side by side.
fn fill() -> LayoutStyle {
    LayoutStyle::new()
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

/// The clock face (`[widgets.clock]`).
///
/// It lives here rather than in the `clock` module because it is not that module: the bar chip is a chip in a
/// row of chips, and this is a face placed on a screen. What they do share — the tick and the `strftime`
/// patterns — they share through the clock *service* and `[clock]`, which is the part that would actually be
/// wrong to duplicate.
fn clock_face(config: &Config) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = config.resolve_theme();
    let settings = config.widgets.clock.clone();
    let ink = if settings.invert {
        theme.base
    } else {
        theme.text
    };

    let format = settings.time_format(&config.clock).to_string();
    let date_format = settings.date_format(&config.clock).to_string();
    let now = signal(chrono::Local::now().format(&format).to_string());
    let today = signal(chrono::Local::now().format(&date_format).to_string());
    let (tick_time, tick_date) = (now.clone(), today.clone());
    platform_wayland::watch(clock::subscribe, move |at: clock::Now| {
        tick_time.set(at.format(&format).to_string());
        tick_date.set(at.format(&date_format).to_string());
    });

    let size = theme.font(FontRole::Display) * settings.resolved_scale();
    let shadow = settings
        .shadow
        .then(|| Shadow::new(0.0, 2.0, 12.0, Color::BLACK.with_alpha(0.55)));

    let reading = now.read_only();
    let time = Text::auto(
        move || reading.get(),
        LayoutStyle::new(),
        move || {
            let style = theme
                .text_style_at(FontRole::Display, ink, size)
                .with_weight(600);
            match shadow {
                Some(shadow) => style.with_shadow(shadow),
                None => style,
            }
        },
    )?;

    let mut lines: Vec<Box<dyn LayoutItem>> = vec![box_item(time)];
    if settings.show_date {
        let reading = today.read_only();
        let date_size = (size * 0.28).max(theme.font(FontRole::Body));
        let date = Text::auto(
            move || reading.get(),
            LayoutStyle::new(),
            move || {
                let style = theme.text_style_at(FontRole::Title, ink, date_size);
                match shadow {
                    Some(shadow) => style.with_shadow(shadow),
                    None => style,
                }
            },
        )?;
        lines.push(box_item(date));
    }

    // Every reading is centred inside its own row, since a `Text` in a column takes the column's width and
    // draws its glyphs from the left — the same rule the lock screen's `centred` exists for.
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::new();
    for line in lines {
        rows.push(Box::new(Container::new(
            LayoutStyle::new()
                .flex_row()
                .justify_content(JustifyContent::CENTER)
                .width(SizeDimension::Percent(1.0)),
            vec![line],
        )?));
    }

    let column = Container::new(LayoutStyle::new().flex_column().gap(size * 0.08), rows)?;

    let plate_radius = theme.radius.max(12.0);
    let opacity = settings.plate_opacity();
    let feather = settings.background_blur.max(0.0);
    // The raised surface, not the base: a plate painted in the colour behind it is invisible on a screen with
    // no image, which is exactly the state a user switching it on for the first time is looking at.
    let plate_fill = if settings.invert {
        theme.text
    } else {
        theme.surface
    };
    let plate = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .padding_horizontal(size * 0.3)
            .padding_vertical(size * 0.1),
        move |_| {
            if !settings.background {
                return RectStyle::default();
            }
            let mut style = RectStyle::filled(plate_fill.with_alpha(opacity), plate_radius);
            if feather > 0.0 {
                // A feathered plate is drawn as its own shadow: same colour, spread to the plate's size, blurred
                // by `background_blur`. That is what "the plate's edge fades into the wallpaper" means with the
                // primitives the renderer has — there is no backdrop blur to sample the image through.
                style.shadow = Some(
                    Shadow::new(0.0, 0.0, feather, plate_fill.with_alpha(opacity))
                        .with_spread(feather * 0.5),
                );
            }
            style
        },
        vec![box_item(column)],
    )?;

    let (vertical, horizontal) = settings.position.alignment();
    let margin = settings.margin as f32;
    let placed = Container::new(
        fill()
            .absolute_fill()
            .flex_row()
            .padding_all(margin)
            .align_items(align_items(vertical))
            .justify_content(justify(horizontal)),
        vec![box_item(plate)],
    )?;
    Ok(Box::new(placed))
}

/// The audio visualiser (`[widgets.visualiser]`).
///
/// **The row hides itself by opacity, never by leaving the tree.** Rebuilding a surface's children on a value
/// that changes with the music is a re-layout per frame; and the spectrum service stops publishing entirely
/// once the sound does, so the last frame it sends is the all-zero one that starts the fade — the row costs
/// exactly one animation after the music stops and nothing at all thereafter.
fn visualiser_row(config: &Config) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let settings = config.widgets.visualiser;
    let theme = config.resolve_theme();
    let tint = if settings.accent {
        theme.accent
    } else {
        theme.text
    };

    let start = visualiser::Spectrum::quiet(config.visualiser.band_count());
    let bands = signal(start.bars.clone());
    let silent = signal(start.silent);
    let (next_bands, next_silent) = (bands.clone(), silent.clone());
    platform_wayland::watch(
        visualiser::subscribe,
        move |spectrum: visualiser::Spectrum| {
            next_bands.set(spectrum.bars);
            next_silent.set(spectrum.silent);
        },
    );

    let row = ui::widget::spectrum(
        derive(bands.read_only(), |bars| bars),
        fixed(tint.with_alpha(settings.alpha())),
        settings.edge,
        ui::widget::SpectrumStyle {
            gap: settings.gap_px(),
            radius: settings.radius_px(),
            floor: 0.0,
        },
        thickness(settings.edge, settings.reach_px()),
    )?;

    let fade = visualiser_fade(config, silent.read_only());
    let layer = StyledContainer::new(
        fill()
            .absolute_fill()
            .flex_row()
            .padding_all(settings.margin as f32)
            .align_items(align_items(match settings.edge {
                config::Edge::Top => Align::Start,
                config::Edge::Bottom => Align::End,
                _ => Align::Center,
            }))
            .justify_content(justify(match settings.edge {
                config::Edge::Left => Align::Start,
                config::Edge::Right => Align::End,
                _ => Align::Center,
            })),
        |_| RectStyle::default(),
        vec![row],
    )?
    .with_opacity(fade);
    Ok(Box::new(layer))
}

/// The row's own box: as long as the edge it stands on, as deep as its reach.
fn thickness(edge: config::Edge, reach: f32) -> LayoutStyle {
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

/// How opaque the row is: one when there is sound, zero when `hide_when_silent` and there is not.
///
/// Two shapes rather than one, because with animation off an `Animated` would be a tween with no duration to
/// divide by.
fn visualiser_fade(config: &Config, silent: telar::ReadSignal<bool>) -> Box<dyn Fn() -> f32> {
    if !config.widgets.visualiser.hide_when_silent {
        return Box::new(|| 1.0);
    }
    if !config.animation.enabled {
        return Box::new(move || if silent.get() { 0.0 } else { 1.0 });
    }
    let fade = Animated::new(0.0f32, config.animation.tween_ms(400, 5_000));
    let target = fade.clone();
    Box::new(move || {
        // Read out first: the retarget is what makes the row appear, and it has to be registered as a
        // dependency on the frame that draws nothing too.
        let quiet = silent.get();
        target.retarget(if quiet { 0.0 } else { 1.0 });
        fade.get()
    })
}

fn align_items(align: Align) -> AlignItems {
    match align {
        Align::Start => AlignItems::FLEX_START,
        Align::Center => AlignItems::CENTER,
        Align::End => AlignItems::FLEX_END,
    }
}

fn justify(align: Align) -> JustifyContent {
    match align {
        Align::Start => JustifyContent::FLEX_START,
        Align::Center => JustifyContent::CENTER,
        Align::End => JustifyContent::FLEX_END,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use config::{ClockPlacement, DesktopClockConfig};

    fn built(config: Config) -> Box<dyn Component> {
        WidgetsApp {
            config: Arc::new(config).into(),
        }
        .root()
    }

    #[test]
    fn the_surface_builds_empty_and_with_every_widget_on() {
        // The build is where a layout error would surface, and nothing else runs these closures.
        let _ = built(Config::starter());

        let mut everything = Config::starter();
        everything.widgets.clock.enabled = true;
        everything.widgets.visualiser.enabled = true;
        let _ = built(everything);
    }

    #[test]
    fn the_clock_builds_in_every_position_and_with_every_decoration() {
        for position in ClockPlacement::ALL {
            let mut config = Config::starter();
            config.widgets.clock = DesktopClockConfig {
                enabled: true,
                position,
                background: true,
                background_blur: 8.0,
                invert: true,
                shadow: true,
                ..DesktopClockConfig::default()
            };
            let _ = built(config);
        }
    }

    #[test]
    fn the_visualiser_builds_on_every_edge_and_with_the_fade_both_ways() {
        // The build is what runs the closures: the opacity closure retargets an animation, and the row's own
        // box swaps its axis per edge, neither of which any other test reaches.
        for edge in config::Edge::ALL {
            for hide in [true, false] {
                for animated in [true, false] {
                    let mut config = Config::starter();
                    config.widgets.visualiser = config::DesktopVisualiserConfig {
                        enabled: true,
                        edge,
                        hide_when_silent: hide,
                        ..config::DesktopVisualiserConfig::default()
                    };
                    config.animation.enabled = animated;
                    let _ = built(config);
                }
            }
        }
    }

    #[test]
    fn the_desktop_face_drops_the_seconds_the_bar_chip_keeps() {
        let clock = config::ClockConfig::default();
        let desktop = DesktopClockConfig::default();
        assert_eq!(clock.time_format(), "%H:%M:%S");
        assert_eq!(
            desktop.time_format(&clock),
            "%H:%M",
            "a face that repainted every second would be a surface animating"
        );

        // A user who set `[clock] format` has said what a clock looks like; the face follows rather than
        // second-guessing them.
        let explicit = config::ClockConfig {
            format: Some("%H.%M".to_string()),
            ..config::ClockConfig::default()
        };
        assert_eq!(desktop.time_format(&explicit), "%H.%M");

        // And its own override wins over both.
        let own = DesktopClockConfig {
            format: Some("%I%p".to_string()),
            ..DesktopClockConfig::default()
        };
        assert_eq!(own.time_format(&explicit), "%I%p");
    }

    #[test]
    fn the_plate_opacity_can_never_resolve_to_invisible_or_opaque_by_accident() {
        let bounded = |value: f32| {
            DesktopClockConfig {
                background_opacity: value,
                ..DesktopClockConfig::default()
            }
            .plate_opacity()
        };
        assert_eq!(bounded(0.5), 0.5);
        assert_eq!(
            bounded(0.0),
            0.05,
            "a plate asked for is a plate you can see"
        );
        assert_eq!(bounded(4.0), 1.0);
        assert_eq!(bounded(f32::NAN), 0.35);
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
}
