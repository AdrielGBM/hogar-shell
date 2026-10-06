//! How a layout's [`Style`] paints the box it is written on: an area's own, a group's plate and an instance's.
//!
//! Resolved once, when the box is built, into a [`Look`]; what is left for paint time is only what depends on the box's size, its corners being cut to half its short side.

use config::Config;
use config::theme::NordTheme;
use layout::{Corners, ResolvedArea, ResolvedAreaKind, ResolvedGroup, Style};
use telar::{
    Border, BorderRadius, Clip, ClippedItem, Color, LayoutItem, Paint, Rect, RectStyle, Shadow,
};
use ui::scale::{elevation, plate};

use crate::expressions::{BoundStyle, Overlay};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub fill: Color,
    pub radius: BorderRadius,
    pub border: Option<Border>,
    pub shadow: Option<Shadow>,
}

/// What an instance's kind paints under it where its style says nothing: the colour it rests on, the opacity a fill it names is drawn at, and its corner.
#[derive(Clone, Copy, Debug)]
pub struct Rest {
    pub fill: Color,
    pub opacity: f32,
    pub radius: f32,
}

impl Rest {
    /// The kind rests on the theme's surface at `opacity`.
    pub fn on_surface(theme: &NordTheme, opacity: f32, radius: f32) -> Self {
        Self {
            fill: theme.surface.with_alpha(opacity),
            opacity,
            radius,
        }
    }
}

/// The corners an area of `kind` rests at when its style writes none: a bar's shape, a panel's theme radius unless it runs `along` its owner's bar, and square for anything else.
pub fn rest_radius(kind: &ResolvedAreaKind, along: bool, config: &Config) -> Corners {
    match kind {
        ResolvedAreaKind::Bar { shape, .. } => {
            Corners::all(crate::bar::bar_shape(config, *shape).radius)
        }
        ResolvedAreaKind::Panel { .. } => Corners::all(match along {
            true => 0.0,
            false => config.shape_from(None, None, None, None).radius,
        }),
        _ => Corners::all(0.0),
    }
}

impl Look {
    /// What a box paints where nothing writes over its kind's rest.
    pub fn resting(rest: Rest) -> Self {
        Self {
            fill: rest.fill,
            radius: BorderRadius::all(rest.radius),
            border: None,
            shadow: None,
        }
    }

    /// An area's own box: its `fill` as [`Style::paint`] reads it, square unless it names a radius.
    pub fn area(style: &Style, theme: &NordTheme) -> Self {
        Self::edged(
            style,
            BoundStyle::default(),
            theme,
            style.paint(theme).unwrap_or(Color::TRANSPARENT),
            style
                .radius
                .map_or(BorderRadius::zero(), BorderRadius::from),
        )
    }

    /// A group's plate, which is the theme's overlay at [`plate::OPACITY`] and rounded off the [`plate`] scale wherever `style` says nothing.
    pub fn plate(style: &Style, theme: &NordTheme, in_bar: bool) -> Self {
        let fill = layout::color_of(style.fill.as_deref().unwrap_or(plate::FILL), theme)
            .with_alpha(style.opacity.unwrap_or(plate::OPACITY).clamp(0.0, 1.0));
        Self::edged(
            style,
            BoundStyle::default(),
            theme,
            fill,
            style
                .radius
                .map_or(BorderRadius::all(plate::radius(in_bar)), BorderRadius::from),
        )
    }

    /// An instance's own box, or `None` while neither its style nor a binding says anything and its kind draws its own.
    pub fn of_instance(
        style: &Style,
        bound: Option<&Overlay>,
        theme: &NordTheme,
        rest: Rest,
    ) -> Option<Self> {
        let bound = bound.map_or_else(BoundStyle::default, |overlay| overlay.style);
        (!style.is_empty() || bound != BoundStyle::default())
            .then(|| Self::instance(style, bound, theme, rest))
    }

    /// A panel's sheet: its style laid over the theme's surface at `[theme] opacity`, at `radius` where its style names none.
    pub fn sheet(style: &Style, config: &Config, theme: &NordTheme, radius: f32) -> Self {
        let rest = Rest::on_surface(theme, config.opacity(), radius);
        Self::of_instance(style, None, theme, rest).unwrap_or_else(|| Self::resting(rest))
    }

    /// An instance's own box: `style`, with what its bindings say over it, laid over what its kind rests on.
    fn instance(style: &Style, bound: BoundStyle, theme: &NordTheme, rest: Rest) -> Self {
        Self::edged(
            style,
            bound,
            theme,
            own_fill(style, bound, theme, rest.opacity).unwrap_or(rest.fill),
            style
                .radius
                .map_or(BorderRadius::all(rest.radius), BorderRadius::from),
        )
    }

    fn edged(
        style: &Style,
        bound: BoundStyle,
        theme: &NordTheme,
        fill: Color,
        radius: BorderRadius,
    ) -> Self {
        let border = match (&style.border, bound.border_color) {
            (None, None) => None,
            (written, color) => {
                let written = written.clone().unwrap_or_default();
                let color = color.unwrap_or_else(|| written.color(theme));
                written
                    .draws()
                    .then(|| Border::uniform(color, written.width()))
            }
        };
        Self {
            fill,
            radius,
            border,
            shadow: style.shadow.and_then(elevation::shadow),
        }
    }

    /// The paint of a box laid out at `rect`.
    pub fn paint(&self, rect: Rect) -> RectStyle {
        RectStyle {
            fill: (self.fill.a > 0.0).then_some(Paint::Solid(self.fill)),
            border: self.border,
            shadow: self.shadow,
            radius: within(self.radius, rect),
        }
    }
}

/// What an instance's box fills with where its style or a binding names a fill or an opacity: that fill, else the theme's surface, at that opacity, else at `resting`.
fn own_fill(style: &Style, bound: BoundStyle, theme: &NordTheme, resting: f32) -> Option<Color> {
    let names_fill = style.fill.is_some()
        || style.opacity.is_some()
        || bound.fill.is_some()
        || bound.opacity.is_some();
    names_fill.then(|| {
        bound
            .fill
            .unwrap_or_else(|| {
                style
                    .fill
                    .as_deref()
                    .map_or(theme.surface, |fill| layout::color_of(fill, theme))
            })
            .with_alpha(
                bound
                    .opacity
                    .or(style.opacity)
                    .unwrap_or(resting)
                    .clamp(0.0, 1.0),
            )
    })
}

/// What holds a style, as far as the colour its box fills with goes.
#[derive(Clone, Copy, Debug)]
pub enum Holder<'a> {
    /// The area's own box: a bar's strip, a panel's sheet or any other area's box.
    Area(&'a ResolvedArea),
    /// A group's plate.
    Plate,
    /// An instance's box in the area.
    Instance(&'a ResolvedArea),
}

/// The colour the box of `holder` fills with when its style is `style`, through the paint its surface draws it with: `None` for an instance whose style names neither a fill nor an opacity, which shows what its kind rests on instead.
pub fn fill_of(
    holder: Holder<'_>,
    style: &Style,
    config: &Config,
    theme: &NordTheme,
) -> Option<Color> {
    match holder {
        Holder::Area(area) => Some(match &area.kind {
            ResolvedAreaKind::Bar { shape, .. } => {
                crate::bar::strip_colour(config, *shape, style, theme)
            }
            ResolvedAreaKind::Panel { .. } => Look::sheet(style, config, theme, 0.0).fill,
            _ => Look::area(style, theme).fill,
        }),
        Holder::Plate => Some(Look::plate(style, theme, false).fill),
        Holder::Instance(area) => {
            let resting = match &area.kind {
                ResolvedAreaKind::Bar { .. } => crate::bar::strip_opacity(config, &area.style),
                _ => config.opacity(),
            };
            own_fill(style, BoundStyle::default(), theme, resting)
        }
    }
}

/// The opacity the box of `holder` draws `fill` at while its style names no opacity, the theme's surface standing in where it names no fill.
pub fn resting_opacity(
    holder: Holder<'_>,
    fill: Option<&str>,
    config: &Config,
    theme: &NordTheme,
) -> f32 {
    let named = Style {
        fill: Some(fill.unwrap_or("surface").to_string()),
        ..Style::default()
    };
    fill_of(holder, &named, config, theme).map_or(1.0, |paint| paint.a)
}

/// Whether `group` paints a plate behind its children: any group whose style names something, and off a bar a container too.
pub fn draws_plate(group: &ResolvedGroup, in_bar: bool) -> bool {
    !group.style.is_empty() || (!in_bar && crate::container::arranges(group))
}

/// `radius` with no corner rounder than half the short side of `rect`, past which two corners would overlap.
pub fn within(radius: BorderRadius, rect: Rect) -> BorderRadius {
    let most = (rect.width.min(rect.height) / 2.0).max(0.0);
    BorderRadius {
        top_left: radius.top_left.min(most),
        top_right: radius.top_right.min(most),
        bottom_right: radius.bottom_right.min(most),
        bottom_left: radius.bottom_left.min(most),
    }
}

/// `inner` cut to `radius`, held to half the short side of the box it lies in as the paint of that box is ([`within`]), and read again whenever that box is laid out afresh.
pub fn cut(inner: Box<dyn LayoutItem>, radius: BorderRadius, paint_only: bool) -> ClippedItem {
    let shaped = move |rect: Rect| {
        let clip = Clip::both().rounded(within(radius, rect));
        match paint_only {
            true => clip.paint_only(),
            false => clip,
        }
    };
    let rect = telar::track_layout(inner.layout_node()).expect("a cut item's node is registered");
    ClippedItem::following(inner, move || shaped(rect.get()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use layout::{Corners, Style};

    fn theme() -> NordTheme {
        NordTheme::new()
    }

    #[test]
    fn a_corner_never_rounds_past_half_the_short_side() {
        let look = Look::area(
            &Style {
                radius: Some(Corners::each(40.0, 4.0, 40.0, 0.0)),
                ..Style::default()
            },
            &theme(),
        );
        let radius = look.paint(Rect::new(0.0, 0.0, 200.0, 30.0)).radius;
        assert_eq!(
            radius,
            BorderRadius {
                top_left: 15.0,
                top_right: 4.0,
                bottom_right: 15.0,
                bottom_left: 0.0,
            }
        );
    }

    #[test]
    fn a_border_is_its_written_width_in_its_written_colour_and_the_defaults_where_it_names_neither()
    {
        let theme = theme();
        let bordered = |border: layout::Border| {
            Look::area(
                &Style {
                    border: Some(border),
                    ..Style::default()
                },
                &theme,
            )
            .border
        };
        assert_eq!(
            bordered(layout::Border::default()),
            Some(Border::uniform(
                layout::color_of(layout::Border::COLOR, &theme),
                layout::Border::WIDTH
            ))
        );
        assert_eq!(
            bordered(layout::Border {
                width: Some(3.0),
                color: Some("#ff0000".into()),
            }),
            Some(Border::uniform(Color::from_rgb_u8(255, 0, 0), 3.0))
        );
        assert_eq!(
            bordered(layout::Border {
                width: Some(0.0),
                color: None,
            }),
            None,
            "a width of 0 is no line"
        );
    }

    #[test]
    fn an_instance_rests_on_its_kind_until_its_style_or_a_binding_names_a_fill() {
        let theme = theme();
        let rest = Rest {
            fill: Color::TRANSPARENT,
            opacity: 0.8,
            radius: 7.0,
        };
        let bare = Look::instance(&Style::default(), BoundStyle::default(), &theme, rest);
        assert_eq!(
            (bare.fill, bare.radius),
            (rest.fill, BorderRadius::all(7.0))
        );

        let faded = Style {
            opacity: Some(0.5),
            ..Style::default()
        };
        assert_eq!(
            Look::instance(&faded, BoundStyle::default(), &theme, rest).fill,
            theme.surface.with_alpha(0.5),
            "an opacity alone fills with the surface"
        );

        let red = Color::from_rgb_u8(255, 0, 0);
        let bound = BoundStyle {
            fill: Some(red),
            opacity: None,
            border_color: Some(red),
        };
        let look = Look::instance(&Style::default(), bound, &theme, rest);
        assert_eq!(look.fill, red.with_alpha(0.8), "at the kind's opacity");
        assert_eq!(
            look.border,
            Some(Border::uniform(red, layout::Border::WIDTH)),
            "a bound border colour draws a line"
        );
    }

    #[test]
    fn a_plate_is_the_overlay_unless_its_style_says_otherwise() {
        let theme = theme();
        let look = Look::plate(&Style::default(), &theme, false);
        assert_eq!(look.fill, theme.overlay.with_alpha(plate::OPACITY));
        let shadowed = Look::plate(
            &Style {
                shadow: Some(2),
                ..Style::default()
            },
            &theme,
            true,
        );
        assert_eq!(shadowed.shadow, elevation::shadow(2));
    }

    #[test]
    fn rest_radius_panels_along_a_bar_have_zero_corners() {
        let config = Config::default();
        let panel = ResolvedAreaKind::Panel {
            owner: layout::InstanceId::new("test"),
            along: true,
            cols: 1,
            rows: 1,
            cell: 100.0,
            gap: 8.0,
        };
        let radius = rest_radius(&panel, true, &config);
        assert!(radius.is_uniform() && radius.top_left() == 0.0);
    }

    #[test]
    fn rest_radius_panels_beside_a_bar_have_config_radius() {
        let config = Config::default();
        let panel = ResolvedAreaKind::Panel {
            owner: layout::InstanceId::new("test"),
            along: false,
            cols: 1,
            rows: 1,
            cell: 100.0,
            gap: 8.0,
        };
        let radius = rest_radius(&panel, false, &config);
        let expected = config.shape_from(None, None, None, None).radius;
        assert_eq!(radius.top_left(), expected);
    }

    #[test]
    fn rest_radius_for_non_panel_non_bar_kinds_is_zero() {
        let config = Config::default();
        let dock = ResolvedAreaKind::Dock {
            edge: config::Edge::Top,
            thickness: 34.0,
        };
        let radius = rest_radius(&dock, false, &config);
        assert!(radius.is_uniform() && radius.top_left() == 0.0);
    }

    fn area_of(kind: ResolvedAreaKind, style: Style) -> ResolvedArea {
        ResolvedArea {
            id: layout::AreaId::new("probe"),
            kind,
            reserve: false,
            above_fullscreen: false,
            within: layout::Within::Output,
            style,
            visible: None,
            groups: Vec::new(),
            actions: Default::default(),
        }
    }

    #[test]
    fn a_fill_is_what_each_holder_paints_with_its_opacity_in_place_of_the_colour_s_alpha() {
        let (config, theme) = (Config::default(), theme());
        let half_red = Style {
            fill: Some("#ff000080".into()),
            ..Style::default()
        };
        let opaque_red = Style {
            opacity: Some(1.0),
            ..half_red.clone()
        };
        let red = Color::from_rgb_u8(255, 0, 0);
        let dock = area_of(
            ResolvedAreaKind::Dock {
                edge: config::Edge::Top,
                thickness: 34.0,
            },
            Style::default(),
        );
        assert_eq!(
            fill_of(Holder::Area(&dock), &opaque_red, &config, &theme),
            Some(red)
        );
        assert_eq!(
            fill_of(Holder::Area(&dock), &half_red, &config, &theme).map(|fill| fill.to_rgba8()[3]),
            Some(0x80)
        );
        assert_eq!(
            fill_of(Holder::Plate, &half_red, &config, &theme),
            Some(red.with_alpha(plate::OPACITY))
        );
        assert_eq!(
            fill_of(Holder::Instance(&dock), &half_red, &config, &theme),
            Some(red.with_alpha(config.opacity()))
        );
        assert_eq!(
            fill_of(Holder::Instance(&dock), &Style::default(), &config, &theme),
            None,
            "an instance naming no fill shows what its kind rests on"
        );
        let bar = area_of(
            ResolvedAreaKind::Bar {
                edge: config::Edge::Top,
                thickness: 34.0,
                length: Default::default(),
                offset: 0.0,
                shape: Default::default(),
                autohide: None,
            },
            Style {
                opacity: Some(0.3),
                ..Style::default()
            },
        );
        assert_eq!(
            fill_of(Holder::Area(&bar), &opaque_red, &config, &theme),
            Some(red)
        );
        assert_eq!(
            resting_opacity(Holder::Instance(&bar), None, &config, &theme),
            0.3,
            "a chip rests at its bar's opacity"
        );
        assert_eq!(
            resting_opacity(Holder::Plate, None, &config, &theme),
            plate::OPACITY
        );
    }

    #[test]
    fn a_group_draws_a_plate_where_its_style_names_something_and_off_a_bar_where_it_arranges() {
        let group = |arrange, style| ResolvedGroup {
            id: layout::GroupId::new("g"),
            kind: layout::GroupKind::Zone {
                zone: layout::Zone::Start,
            },
            arrange,
            cols: 1,
            rows: 1,
            gap: None,
            repeat: None,
            komponent: None,
            style,
            children: Vec::new(),
        };
        let styled = Style {
            shadow: Some(1),
            ..Style::default()
        };
        assert!(!draws_plate(&group(None, Style::default()), false));
        assert!(draws_plate(&group(None, styled.clone()), true));
        assert!(draws_plate(
            &group(Some(layout::Arrange::Row), Style::default()),
            false
        ));
        assert!(!draws_plate(
            &group(Some(layout::Arrange::Row), Style::default()),
            true
        ));
    }
}
