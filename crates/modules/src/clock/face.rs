//! The clock as a face rather than a chip: the time at display size over the date, on a plate when `[widgets.clock] background` asks for one. It shares the tick and the `strftime` patterns with the bar chip through the clock service and `[clock]`.

use telar::{
    Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, RectStyle, Shadow,
    SizeDimension, StyledContainer, Text, box_item, signal, use_theme,
};

use config::theme::{FontRole, NordTheme};
use config::{ClockConfig, WidgetsConfig};
use services::clock;
use ui::host::{Host, Size};

/// How wide a glyph of the time is against its font size, close enough for the faces the shell ships, so the face can fit a box it has not been laid out in yet.
const ADVANCE: f32 = 0.62;
const DATE_SCALE: f32 = 0.28;
const LINE_GAP: f32 = 0.08;
const PAD_ACROSS: f32 = 0.3;
const PAD_DOWN: f32 = 0.1;
/// How tall a line of text is against its font size, with room to spare for the faces whose ascenders run tall.
const LINE_HEIGHT: f32 = 1.25;

/// The face at `[widgets.clock] scale`, shrunk until it fits the host's extent; an unbounded extent leaves the configured size alone. A small one is too narrow for a date beside a time worth reading, so it shows the time alone.
pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let settings = host.options::<WidgetsConfig>().clock.clone();
    let chip = host.options::<ClockConfig>();
    let ink = if settings.invert {
        theme.base
    } else {
        host.foreground
    };
    let show_date = settings.show_date && !host.is_small();
    let extent = host.extent;

    let format = settings.time_format(chip).to_string();
    let date_format = settings.date_format(chip).to_string();
    let started = chrono::Local::now();
    let first = started.format(&format).to_string();
    let glyphs = first.chars().count();
    let now = signal(first);
    let today = signal(started.format(&date_format).to_string());
    let (tick_time, tick_date) = (now, today);
    platform_wayland::watch(clock::subscribe, move |at: clock::Now| {
        tick_time.set(at.format(&format).to_string());
        tick_date.set(at.format(&date_format).to_string());
    });

    let size = fitted(
        theme.font(FontRole::Display) * settings.resolved_scale(),
        extent,
        glyphs,
        show_date,
    );
    let shadow = settings
        .shadow
        .then(|| Shadow::new(0.0, 2.0, 12.0, Color::BLACK.with_alpha(0.55)));

    let reading = now.read_only();
    let time = Text::new(
        move || reading.get(),
        LayoutStyle::new(),
        move || {
            let style = theme
                .text_style_at(FontRole::Display, ink, size)
                .with_font_weight(600);
            match shadow {
                Some(shadow) => style.with_text_shadow(shadow),
                None => style,
            }
        },
    )?;

    let mut lines: Vec<Box<dyn LayoutItem>> = vec![box_item(time)];
    if show_date {
        let reading = today.read_only();
        let date_size = (size * DATE_SCALE).max(theme.font(FontRole::Body));
        let date = Text::new(
            move || reading.get(),
            LayoutStyle::new(),
            move || {
                let style = theme.text_style_at(FontRole::Title, ink, date_size);
                match shadow {
                    Some(shadow) => style.with_text_shadow(shadow),
                    None => style,
                }
            },
        )?;
        lines.push(box_item(date));
    }

    // A `Text` in a column takes the column's width and draws its glyphs from the left, so each line is centred inside a row of its own.
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

    let column = Container::new(LayoutStyle::new().flex_column().gap(size * LINE_GAP), rows)?;

    let plate_radius = theme.radius.max(12.0);
    let opacity = settings.plate_opacity();
    let feather = settings.background_blur.max(0.0);
    // The raised surface, not the base: a plate painted in the colour behind it is invisible on a screen with no image, which is exactly the state a user switching it on for the first time is looking at.
    let plate_fill = if settings.invert {
        host.foreground
    } else {
        theme.surface
    };
    let plate = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .justify_content(JustifyContent::CENTER)
            .padding_horizontal(size * PAD_ACROSS)
            .padding_vertical(size * PAD_DOWN),
        move |_| {
            if !settings.background {
                return RectStyle::default();
            }
            let mut style = RectStyle::filled(plate_fill.with_alpha(opacity), plate_radius);
            if feather > 0.0 {
                // A feathered plate is drawn as its own shadow, spread to the plate's size and blurred by `background_blur`: the renderer has no backdrop blur to sample the wallpaper through.
                style.shadow = Some(
                    Shadow::new(0.0, 0.0, feather, plate_fill.with_alpha(opacity))
                        .with_spread(feather * 0.5),
                );
            }
            style
        },
        vec![box_item(column)],
    )?;
    Ok(Box::new(plate))
}

/// `asked`, or the largest size at which `glyphs` of time and the date under it still fit `extent` with the plate's padding around them.
fn fitted(asked: f32, extent: Size, glyphs: usize, show_date: bool) -> f32 {
    let lines = if show_date {
        (1.0 + DATE_SCALE) * LINE_HEIGHT + LINE_GAP
    } else {
        LINE_HEIGHT
    };
    let tall = lines + PAD_DOWN * 2.0;
    let wide = glyphs.max(1) as f32 * ADVANCE + PAD_ACROSS * 2.0;
    asked.min(extent.height / tall).min(extent.width / wide)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use config::{Config, DesktopClockConfig};
    use ui::host::{Representation, WidgetSize};

    use super::*;

    fn host(config: Config, size: WidgetSize) -> Host {
        ui::preview::host_on(
            Arc::new(config),
            "clock",
            Representation::Widget(size),
            size.extent(),
        )
    }

    #[test]
    fn the_face_builds_at_every_size_bare_and_with_every_decoration() {
        for size in WidgetSize::ALL {
            for decorated in [false, true] {
                telar::reset_layout_runtime();
                let _scope = telar::owner_scope();
                let mut config = Config::starter();
                config.widgets.clock = DesktopClockConfig {
                    background: decorated,
                    background_blur: 8.0,
                    invert: decorated,
                    shadow: decorated,
                    ..DesktopClockConfig::default()
                };
                telar::set_theme(config.resolve_theme());
                assert!(
                    widget(&host(config, size)).is_ok(),
                    "{size:?} decorated={decorated}"
                );
            }
        }
    }

    #[test]
    fn an_unbounded_face_keeps_its_configured_size_and_a_bounded_one_fits_its_box() {
        let unbounded = Size {
            width: f32::INFINITY,
            height: f32::INFINITY,
        };
        assert_eq!(fitted(96.0, unbounded, 5, true), 96.0);

        let small = WidgetSize::S.extent();
        let size = fitted(96.0, small, 5, false);
        assert!(
            size < 96.0,
            "a configured face is wider than a small widget"
        );
        assert!(size * (5.0 * ADVANCE + PAD_ACROSS * 2.0) <= small.width + 0.01);
        assert!(size * (LINE_HEIGHT + PAD_DOWN * 2.0) <= small.height + 0.01);
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

        // A user who set `[clock] format` has said what a clock looks like; the face follows rather than second-guessing them.
        let explicit = config::ClockConfig {
            format: Some("%H.%M".to_string()),
            ..config::ClockConfig::default()
        };
        assert_eq!(desktop.time_format(&explicit), "%H.%M");

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
}
