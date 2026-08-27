//! The small pieces more than one surface draws.
//!
//! A meter and a label/value row are six lines each, which is exactly why they drift: the popout grew one, the dashboard wanted the same thing a size larger, and two copies of "how a fraction reads as a bar" is one copy too many. Each takes its metrics as arguments so a glance card and a full page can share the drawing without sharing a size.

use crate::scale::space;
use std::sync::Arc;

use telar::{
    AlignItems, Canvas, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    LineCap, LineJoin, PathData, PathStyle, Point, Rect, RectStyle, RenderNode, ShapeStyle,
    SizeDimension, Stroke, StyledContainer, Text, TextStyle, Transform, box_item,
};

use util::reactive::Live;

/// How much of the tint an area fill keeps under its line. Enough to read as a filled series, light enough that two sparklines stacked in a column don't fight the text between them.
const AREA_ALPHA: f32 = 0.22;
const LINE_WIDTH: f32 = 1.5;

/// A 0..1 bar, full width, that follows its fraction.
///
/// The fill is an absolutely-positioned child scaled horizontally from the left edge rather than a box laid out narrower — the technique `progress` uses, and what makes the bar cheap enough to track a value that moves on every wheel notch. It is spelled out here rather than delegated because `progress` sizes its track in px and binds through an `RwSignal`, and a card wants a percentage-width track fed straight from a [`Live`] value.
pub fn meter(
    fraction: Live<f32>,
    tint: Live<Color>,
    track: Color,
    height: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let radius = height / 2.0;
    let empty = fraction;
    let fill = StyledContainer::new(
        LayoutStyle::new().absolute_fill(),
        move |_r| {
            // A bar with nothing in it has no fill to draw, and saying so is not only tidier. The rect stays its full laid-out size — the collapse below is in the matrix, not in the box — so a renderer handed a fill still has a rounded path to fill, and a path a transform has flattened to a line is one tiny_skia refuses, warning once a frame for as long as the reading sits at zero.
            if empty.get() <= 0.0 {
                RectStyle::default()
            } else {
                RectStyle::filled(tint.get(), radius)
            }
        },
        vec![],
    )?
    .with_transform(move |r| {
        let value = fraction.get().clamp(0.0, 1.0);
        // `box_transform` pivots scale on the rect centre; a bar has to grow rightward from its left edge.
        Some(Transform::scale_around(value, 1.0, r.x, r.y + r.height / 2.0).to_array())
    });
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(height),
        move |_r| RectStyle::filled(track, radius),
        vec![box_item(fill)],
    )?))
}

/// How much taller than its bar a [`slider`]'s hit area is, above and below. A 6px meter is a target a pointer misses; the padding is what makes the whole row's height pressable without the bar itself growing.
const SLIDER_REACH: f32 = 7.0;

/// A [`meter`] that is also a control: pressing or dragging anywhere along it reports the fraction under the pointer, as `0..1`.
///
/// `on_drag` fires on the press as well as on every move until release, so a single click is a set and a drag is a scrub — one handler covers both. The width comes from the box's own laid-out rect rather than from a constant, so the same slider works in a drawer and on a settings page without being told how wide it is.
pub fn slider(
    fraction: Live<f32>,
    tint: Live<Color>,
    track: Color,
    height: f32,
    on_set: impl Fn(f32) + 'static,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let bar = meter(fraction, tint, track, height)?;
    let area = StyledContainer::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .padding_vertical(SLIDER_REACH)
            .justify_content(JustifyContent::CENTER),
        |_r| RectStyle::filled(Color::TRANSPARENT, 0.0),
        vec![bar],
    )?;
    let rect = telar::track_layout(area.layout_node())
        .expect("a container registers its rect")
        .read_only();
    Ok(Box::new(area.on_drag(move |px, _py| {
        let width = rect.get().width;
        if width <= 0.0 {
            return;
        }
        on_set((px / width).clamp(0.0, 1.0));
    })))
}

/// A label on the left, its value on the right. The label never shrinks, so a long value wraps or truncates rather than squeezing the word that says what it is.
pub fn label_value(
    label: Live<String>,
    value: Live<String>,
    size: f32,
    label_color: Color,
    value_color: Color,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let label = Text::new(
        move || label.get(),
        LayoutStyle::new().flex_shrink(0.0),
        move || TextStyle::new(size, label_color),
    )?;
    let value = Text::new(
        move || value.get(),
        LayoutStyle::new(),
        move || TextStyle::new(size, value_color),
    )?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .width(SizeDimension::Percent(1.0))
            .gap(space::lg()),
        vec![box_item(label), box_item(value)],
    )?))
}

/// A filled area chart over `values`, oldest first, scaled so `ceiling` reaches the top.
///
/// `ceiling` is a signal rather than a constant because half the series here have no natural one: CPU is a percentage and tops out at 100, but a byte rate's full scale is whatever the last minute peaked at, and a chart that rescaled only when it was rebuilt would flatten the moment traffic dropped.
pub fn sparkline(
    values: Live<Vec<f32>>,
    ceiling: Live<f32>,
    tint: Color,
    height: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let canvas = Canvas::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(height),
        move |rect| {
            let series = values.get();
            let Some(line) = series_path(&series, ceiling.get(), rect) else {
                return RenderNode::Empty;
            };
            let area = Arc::new(close_to_baseline(&line, rect));
            RenderNode::group([
                RenderNode::path(
                    area,
                    PathStyle::default().with_fill(tint.with_alpha(AREA_ALPHA)),
                ),
                RenderNode::path(
                    Arc::new(line),
                    PathStyle::default().with_stroke(
                        Stroke::new(tint, LINE_WIDTH)
                            .with_cap(LineCap::Round)
                            .with_join(LineJoin::Round),
                    ),
                ),
            ])
        },
    )?;
    Ok(Box::new(canvas))
}

/// How a spectrum's bars are drawn: the gap between them and how round their ends are, both in pixels.
///
/// A struct rather than two more positional arguments because the two consumers are drawn very differently — a desktop-wide row and a ring the size of an album cover — and a call site reading `(4.0, 2.0, 3.0)` says nothing about which number is which.
#[derive(Clone, Copy, Debug)]
pub struct SpectrumStyle {
    pub gap: f32,
    pub radius: f32,
    /// How tall a band reading zero still draws, so a silent row is a line rather than nothing at all. The caller decides whether that is wanted: a background visualiser that vanishes entirely is the point.
    pub floor: f32,
}

impl Default for SpectrumStyle {
    fn default() -> Self {
        Self {
            gap: 3.0,
            radius: 2.0,
            floor: 0.0,
        }
    }
}

/// A row of bars, one per band, growing away from `edge`.
///
/// The bars are laid out across the box's *long* axis and grow along its short one, so the same call draws a row along the bottom of a screen and a column up its left-hand side. Each is one `RenderNode::rect`: a spectrum arriving sixty times a second is the one place in this shell where the drawing has to be free, and a rounded rect is a primitive the renderer already batches.
pub fn spectrum(
    bands: Live<Arc<[f32]>>,
    tint: Live<Color>,
    edge: config::Edge,
    style: SpectrumStyle,
    layout: LayoutStyle,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let canvas = Canvas::new(layout, move |rect| {
        let values = bands.get();
        let color = tint.get();
        if values.is_empty() {
            return RenderNode::Empty;
        }
        RenderNode::group(
            bar_rects(&values, edge, style, rect)
                .map(|bar| RenderNode::rect(bar, RectStyle::filled(color, style.radius))),
        )
    })?;
    Ok(Box::new(canvas))
}

/// Where each band's bar sits inside `rect`, in the canvas's own local coordinates.
fn bar_rects(
    values: &[f32],
    edge: config::Edge,
    style: SpectrumStyle,
    rect: Rect,
) -> impl Iterator<Item = Rect> + '_ {
    let along = if edge.is_horizontal() {
        rect.width
    } else {
        rect.height
    };
    let across = if edge.is_horizontal() {
        rect.height
    } else {
        rect.width
    };
    let slot = along / values.len() as f32;
    // A gap wider than the slot would give every bar a negative width, which reads as bars that vanish as the row gets denser rather than as a gap the user set too large.
    let thickness = (slot - style.gap).max(1.0);
    let inset = (slot - thickness) / 2.0;

    values.iter().enumerate().map(move |(index, value)| {
        let length = (value.clamp(0.0, 1.0) * across)
            .max(style.floor)
            .min(across);
        let start = index as f32 * slot + inset;
        match edge {
            config::Edge::Bottom => Rect {
                x: start,
                y: rect.height - length,
                width: thickness,
                height: length,
            },
            config::Edge::Top => Rect {
                x: start,
                y: 0.0,
                width: thickness,
                height: length,
            },
            config::Edge::Left => Rect {
                x: 0.0,
                y: start,
                width: length,
                height: thickness,
            },
            config::Edge::Right => Rect {
                x: rect.width - length,
                y: start,
                width: length,
                height: thickness,
            },
        }
    })
}

/// The same bands radiating outward from a circle of `inner` radius, centred in the box.
///
/// A ring rather than a second row because what it wraps is a square picture: bars along one of its sides would read as belonging to the layout, and the thing a cover-art visualiser is *for* is looking like it belongs to the record. Each bar is an upright rect rotated about the centre — the renderer's rotation, so a band that changes costs a matrix and not a re-tessellated path.
pub fn spectrum_ring(
    bands: Live<Arc<[f32]>>,
    tint: Live<Color>,
    inner: f32,
    reach: f32,
    style: SpectrumStyle,
    layout: LayoutStyle,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let canvas = Canvas::new(layout, move |rect| {
        let values = bands.get();
        let color = tint.get();
        if values.is_empty() {
            return RenderNode::Empty;
        }
        let (cx, cy) = (rect.width / 2.0, rect.height / 2.0);
        // The spectrum is mirrored around the top of the circle rather than wrapped once, so the two halves answer each other. A single sweep puts the bass beside the treble, which reads as a seam.
        let spokes = values.len() * 2;
        let thickness = ((std::f32::consts::TAU * inner / spokes as f32) - style.gap).max(1.0);

        RenderNode::group((0..spokes).map(|spoke| {
            let band = if spoke < values.len() {
                spoke
            } else {
                spokes - 1 - spoke
            };
            let length = (values[band].clamp(0.0, 1.0) * reach).max(style.floor);
            let bar = Rect {
                x: cx - thickness / 2.0,
                y: cy - inner - length,
                width: thickness,
                height: length.max(f32::EPSILON),
            };
            RenderNode::transform_with(
                Transform::rotate_around(spoke as f32 * 360.0 / spokes as f32, cx, cy).to_array(),
                [RenderNode::rect(
                    bar,
                    RectStyle::filled(color, style.radius),
                )],
            )
        }))
    })?;
    Ok(Box::new(canvas))
}

/// The polyline through `values`. `None` for a series too short to be a line — one reading is a dot, and a chart drawn from it would claim a trend the shell has not measured yet.
fn series_path(values: &[f32], ceiling: f32, rect: Rect) -> Option<PathData> {
    if values.len() < 2 || rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    // The stroke is centred on the path, so a reading at either extreme would lose half its width off the edge.
    let inset = LINE_WIDTH / 2.0;
    let usable = (rect.height - LINE_WIDTH).max(1.0);
    let ceiling = ceiling.max(f32::EPSILON);
    let step = rect.width / (values.len() - 1) as f32;
    let point = |(i, value): (usize, &f32)| {
        let fraction = (value / ceiling).clamp(0.0, 1.0);
        Point::new(i as f32 * step, inset + usable * (1.0 - fraction))
    };
    let mut points = values.iter().enumerate();
    let mut path = PathData::new().move_to(point(points.next()?));
    for reading in points {
        path = path.line_to(point(reading));
    }
    Some(path)
}

/// The same line taken down to the bottom of the box and closed, which is what gives the chart its fill.
fn close_to_baseline(line: &PathData, rect: Rect) -> PathData {
    line.clone()
        .line_to(Point::new(rect.width, rect.height))
        .line_to(Point::new(0.0, rect.height))
        .close()
}

/// Both spectrum forms over a synthetic sweep, for [`crate::preview`]. A hump rather than a ramp, so a band drawn in the wrong slot is obvious rather than plausible — which is the only way to see that the ring's spokes mirror instead of wrapping, and that the row's caps are the radius asked for.
pub(crate) fn spectrum_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = telar::use_theme::<config::theme::NordTheme>();
    let bands: Arc<[f32]> = (0..48)
        .map(|i| {
            let x = (i as f32 - 16.0) / 12.0;
            (-x * x).exp() * 0.9 + 0.08
        })
        .collect();
    let row = spectrum(
        util::reactive::fixed(bands.clone()),
        util::reactive::fixed(theme.accent),
        config::Edge::Bottom,
        SpectrumStyle {
            gap: 4.0,
            radius: 3.0,
            floor: 0.0,
        },
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(160.0),
    )?;
    let ring = spectrum_ring(
        util::reactive::fixed(bands),
        util::reactive::fixed(theme.text),
        60.0,
        50.0,
        SpectrumStyle {
            gap: 2.0,
            radius: 2.0,
            floor: 2.0,
        },
        LayoutStyle::new().width(240.0).height(240.0),
    )?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .gap(space::xxl())
            .width(SizeDimension::Percent(1.0)),
        vec![ring, row],
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Edge;
    use telar::PathVerb;

    fn rect() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 20.0,
        }
    }

    #[test]
    fn a_series_spans_the_box_and_reads_bottom_up() {
        let path =
            series_path(&[0.0, 50.0, 100.0], 100.0, rect()).expect("three readings is a line");
        let points: Vec<Point> = path
            .verbs()
            .iter()
            .map(|verb| match verb {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) => *p,
                other => panic!("a sparkline is only moves and lines, got {other:?}"),
            })
            .collect();
        assert_eq!(points.len(), 3);
        assert_eq!(
            points[0].x, 0.0,
            "the oldest reading starts at the left edge"
        );
        assert_eq!(points[2].x, 100.0, "the newest ends at the right");
        assert!(
            points[0].y > points[1].y && points[1].y > points[2].y,
            "a rising series climbs, so y falls: {points:?}"
        );
    }

    #[test]
    fn a_reading_over_the_ceiling_clamps_instead_of_leaving_the_box() {
        let path = series_path(&[0.0, 500.0], 100.0, rect()).expect("two readings is a line");
        for verb in path.verbs() {
            let (PathVerb::MoveTo(p) | PathVerb::LineTo(p)) = verb else {
                continue;
            };
            assert!(
                p.y >= 0.0 && p.y <= rect().height,
                "every point stays inside the box: {p:?}"
            );
        }
    }

    #[test]
    fn too_short_a_series_draws_nothing() {
        assert!(series_path(&[], 100.0, rect()).is_none());
        assert!(
            series_path(&[42.0], 100.0, rect()).is_none(),
            "one reading is not a trend"
        );
        // A collapsed box would divide by a zero width.
        assert!(
            series_path(
                &[1.0, 2.0],
                100.0,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 20.0
                }
            )
            .is_none()
        );
    }

    #[test]
    fn a_zero_ceiling_does_not_blow_the_scale_up() {
        // An idle network has peaked at nothing, and dividing by that would put every point at infinity.
        let path = series_path(&[0.0, 0.0, 0.0], 0.0, rect()).expect("still a line");
        for verb in path.verbs() {
            let (PathVerb::MoveTo(p) | PathVerb::LineTo(p)) = verb else {
                continue;
            };
            assert!(
                p.y.is_finite() && p.y <= rect().height,
                "finite and in the box: {p:?}"
            );
        }
    }

    #[test]
    fn a_bar_grows_away_from_its_own_edge() {
        // The one thing that makes this widget work on all four edges. Getting it wrong is not a crash: the bars simply grow *into* the screen from the far side, which on a bottom row means a fringe hanging off the top of the surface.
        let box_ = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let values = [0.5, 1.0, 0.0];
        let style = SpectrumStyle::default();
        let at = |edge| bar_rects(&values, edge, style, box_).collect::<Vec<_>>();

        for bar in at(Edge::Bottom) {
            assert_eq!(
                bar.y + bar.height,
                box_.height,
                "a bottom bar is rooted on the bottom"
            );
        }
        for bar in at(Edge::Top) {
            assert_eq!(bar.y, 0.0, "a top bar hangs from the top");
        }
        for bar in at(Edge::Left) {
            assert_eq!(bar.x, 0.0, "a left bar grows rightward off the left edge");
        }
        for bar in at(Edge::Right) {
            assert_eq!(
                bar.x + bar.width,
                box_.width,
                "a right bar is rooted on the right"
            );
        }
    }

    #[test]
    fn every_band_gets_one_bar_inside_the_box() {
        let box_ = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let values: Vec<f32> = (0..24).map(|i| i as f32 / 23.0).collect();
        for edge in Edge::ALL {
            let bars: Vec<Rect> =
                bar_rects(&values, edge, SpectrumStyle::default(), box_).collect();
            assert_eq!(bars.len(), values.len(), "one bar per band on {edge:?}");
            for bar in bars {
                // A band reading zero is a bar with no *length*, which is right; a bar with no thickness is a slot that swallowed its own width, which is a row that draws nothing.
                let thickness = if edge.is_horizontal() {
                    bar.width
                } else {
                    bar.height
                };
                assert!(thickness > 0.0, "{edge:?}: {bar:?} has no thickness");
                assert!(
                    bar.x >= 0.0
                        && bar.y >= 0.0
                        && bar.x + bar.width <= box_.width + f32::EPSILON
                        && bar.y + bar.height <= box_.height + f32::EPSILON,
                    "{edge:?}: {bar:?} leaves the box"
                );
            }
        }
    }

    #[test]
    fn a_gap_wider_than_the_slot_still_draws_bars() {
        // A dense row with a generous gap is a config a user reaches by turning one number up, and the arithmetic answer — a negative width — is a row that silently empties as it gets more detailed.
        let box_ = Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let values = vec![1.0f32; 64];
        let style = SpectrumStyle {
            gap: 20.0,
            ..SpectrumStyle::default()
        };
        for bar in bar_rects(&values, Edge::Bottom, style, box_) {
            assert!(bar.width >= 1.0, "{bar:?}");
        }
    }

    #[test]
    fn the_area_returns_to_the_baseline_and_closes() {
        let line = series_path(&[10.0, 90.0], 100.0, rect()).expect("two readings is a line");
        let area = close_to_baseline(&line, rect());
        assert!(
            matches!(area.verbs().last(), Some(PathVerb::Close)),
            "an unclosed area fills to wherever the rasteriser decides"
        );
        let corners: Vec<&PathVerb> = area.verbs().iter().rev().skip(1).take(2).collect();
        assert!(
            corners
                .iter()
                .all(|verb| matches!(verb, PathVerb::LineTo(p) if p.y == rect().height)),
            "both closing corners sit on the bottom edge: {corners:?}"
        );
    }
}
