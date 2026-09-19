//! The region picker: a full-screen overlay the user draws a rectangle on.
//!
//! Three decisions are worth knowing about.
//!
//! **The still is the capture.** With `[screenshot] freeze` on, the overlay is drawn over a picture of the screen taken the instant before it mapped, and the selection is *cropped out of that picture*. Asking the compositor again afterwards would photograph the overlay, and waiting for the overlay to go away first would lose the menu or hover state the user was trying to capture — which is the whole reason to freeze.
//!
//! **A click is a selection too.** Dragging a box is the general case, but "this window" is the common one, so a press with no travel selects the window under the pointer, and one on empty desktop selects the whole output. Edges within a few pixels of a window snap to it, so a rough drag still comes out flush.
//!
//! **It covers the focused screen.** A selection is made on one monitor; the overlay opens on the focused one and reports its rectangle in the compositor's global logical coordinates, which is what every consumer wants.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use ui::scale::{paint, space};

use platform_wayland::{SurfaceHandle, request_close};
use telar::{
    AlignItems, Canvas, Color, Container, Image, ImageData, JustifyContent, Key, LayoutError,
    LayoutItem, LayoutStyle, NamedKey, ObjectFit, PathData, PathStyle, Point, Raster, Rect,
    RectStyle, RenderNode, RwSignal, ShapeStyle, SizeDimension, Stroke, StyledContainer, Text,
    box_item, signal,
};

use config::theme::{FontRole, NordTheme};
use services::hyprland::{self, Client};
use services::screenshot::{self, Area};
use ui::panel::PanelSurface;
use ui::placement::Placement;

/// How close an edge has to come to a window's own before it snaps to it, in logical pixels. Generous enough that a hand-drawn box lands flush, small enough that it never pulls a deliberate selection off target.
const SNAP: f32 = 12.0;

/// Below this, a drag was a click: nobody selects an 8-pixel box on purpose, and treating it as one would answer a mis-click with an unusable capture.
const CLICK_SLOP: f32 = 8.0;

const DIM: f32 = 0.45;
const BORDER: f32 = 2.0;

/// What the picker hands back: where the selection is, in global logical coordinates, and the pixels for it when the overlay was drawn over a frozen screen.
pub struct Picked {
    pub area: Area,
    pub frozen: Option<screenshot::Image>,
}

thread_local! {
    /// The open picker. Single-slot: a second one would leave two overlays fighting for the pointer, and the first one's selection would land after the second had already covered the screen.
    static OPEN: RefCell<Option<SurfaceHandle>> = const { RefCell::new(None) };
}

pub fn is_open() -> bool {
    OPEN.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|handle| !handle.is_closing())
    })
}

/// Opens the picker on the focused screen and calls `then` with the selection. Cancelling — Escape, or a right-click — closes the overlay and calls nothing.
pub fn pick(then: impl Fn(Picked) + 'static) {
    if is_open() {
        return;
    }
    let output = surfaces::shell::focused_output();
    let config = config::config_for(output.as_deref());
    let screen = output_box(output.as_deref());
    // Taken before the surface exists, which is the only moment that answers "what was on screen when the user asked". Held on the app rather than in the tree, so it also survives a rebuild — a config change while a selection is being drawn must not throw away the picture the selection is being drawn on.
    let frozen = config
        .screenshot
        .freeze
        .then(|| output.as_deref().and_then(frozen_output))
        .flatten();

    // Held on the surface rather than in the tree, so both survive a rebuild: a config change while a selection is being drawn must not throw away the picture it is being drawn on, nor who is waiting for it.
    let frozen = Rc::new(RefCell::new(frozen));
    let then: Rc<dyn Fn(Picked)> = Rc::new(then);
    let handle = PanelSurface::new(placement(output), move |env| {
        overlay(
            env.config.resolve_theme(),
            screen,
            Rc::clone(&frozen),
            Rc::clone(&then),
        )
    })
    .open_handle();
    OPEN.with(|slot| *slot.borrow_mut() = Some(handle));
}

/// Closes whatever picker is up (`hogar-shell screenshot cancel`, or a second request replacing the first).
pub fn close() {
    OPEN.with(|slot| *slot.borrow_mut() = None);
}

/// The whole screen, over everything — including a fullscreen window, because the user asked to select a region of what they can *see* — and holding the keyboard, so Escape arrives without the overlay having to be clicked into first. Both are what [`Placement::screen`] means.
fn placement(output: Option<String>) -> Placement {
    Placement::screen("hogar-shell-picker").output(output)
}

/// Where the picker's screen is and how big it is, in the compositor's logical coordinates. The origin is what turns a surface-local selection into the global rectangle every consumer takes; the size is what a click on empty desktop selects, and what says how many image pixels one logical pixel is.
#[derive(Clone, Copy)]
struct Screen {
    origin: (i32, i32),
    logical: (i32, i32),
}

fn output_box(output: Option<&str>) -> Screen {
    let outputs = platform_wayland::outputs();
    let found =
        output.and_then(|name| outputs.iter().find(|out| out.name.as_deref() == Some(name)));
    match found.or_else(|| outputs.first()) {
        Some(out) => Screen {
            origin: out.position,
            logical: out.logical_size.unwrap_or((1920, 1080)),
        },
        None => Screen {
            origin: (0, 0),
            logical: (1920, 1080),
        },
    }
}

fn frozen_output(name: &str) -> Option<screenshot::Image> {
    screenshot::freeze_outputs()
        .into_iter()
        .find(|(output, _)| output == name)
        .map(|(_, image)| image)
}

/// The live selection, in surface-local logical pixels. `None` before the first press, which is what draws the screen evenly dimmed rather than with an empty box in the corner.
type Live = RwSignal<Option<Area>>;

fn overlay(
    theme: NordTheme,
    screen: Screen,
    frozen: Rc<RefCell<Option<screenshot::Image>>>,
    then: Rc<dyn Fn(Picked)>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let selection: Live = signal(None);
    // Where the drag began. Set on the first `on_drag` of a gesture and cleared when it ends, exactly like the chip's drag-to-open — the platform reports positions, not gestures.
    let anchor: Rc<RefCell<Option<(f32, f32)>>> = Rc::new(RefCell::new(None));
    let windows = window_rects(screen);

    let mut children: Vec<Box<dyn LayoutItem>> = Vec::new();
    if let Some(still) = still_image(&frozen)? {
        children.push(still);
    }
    children.push(wash(selection.read_only(), theme)?);
    children.push(readout(selection.read_only(), theme)?);

    let drag_anchor = Rc::clone(&anchor);
    let drag_selection = selection;
    let drag_windows = windows.clone();
    let end_selection = selection;
    let end_windows = windows.clone();
    let click_selection = selection;
    let click_windows = windows;
    let commit_frozen = Rc::clone(&frozen);
    let commit = move |area: Area| finish(area, screen, &commit_frozen, &then);
    let commit_on_click = commit.clone();

    let root = StyledContainer::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0)),
        |_| RectStyle::default(),
        children,
    )?
    .on_drag(move |x, y| {
        let mut anchor = drag_anchor.borrow_mut();
        let from = *anchor.get_or_insert((x, y));
        drag_selection.set(Some(snapped(
            Area::from_corners(from, (x, y)),
            &drag_windows,
        )));
    })
    .on_drag_end(move |x, y| {
        let from = anchor.borrow_mut().take().unwrap_or((x, y));
        let drawn = Area::from_corners(from, (x, y));
        let area = if drawn.width < CLICK_SLOP as i32 || drawn.height < CLICK_SLOP as i32 {
            under_pointer(x, y, &end_windows)
        } else {
            snapped(drawn, &end_windows)
        };
        end_selection.set(Some(area));
        commit(area);
    })
    .on_press(move || {
        // Reached only when the pointer never moved enough to become a drag; `on_drag_end` handles the rest.
        let area = click_selection
            .peek()
            .unwrap_or_else(|| whole_output(&click_windows));
        commit_on_click(area);
    })
    .on_alt_press(|_button| request_close())
    .on_key(|key: &Key| {
        if matches!(key, Key::Named(NamedKey::Escape)) {
            request_close();
        }
    });
    Ok(Box::new(root))
}

/// Crops the still (when there is one), closes the overlay, and hands the selection on.
///
/// The order matters even with a still: the consumer may be the recorder, which starts capturing the screen for real, and it must not start while the overlay is still mapped. `request_close` only asks — the driver tears the surface down on its next turn — so the callback is deferred past that turn.
fn finish(
    area: Area,
    screen: Screen,
    frozen: &Rc<RefCell<Option<screenshot::Image>>>,
    then: &Rc<dyn Fn(Picked)>,
) {
    if area.is_empty() {
        request_close();
        return;
    }
    let global = Area {
        x: area.x + screen.origin.0,
        y: area.y + screen.origin.1,
        ..area
    };
    let cropped = frozen.borrow_mut().take().and_then(|still| {
        // The still is in the output's physical pixels; the selection is logical, so it scales by whatever ratio the two have — derived from the image rather than from the output's reported scale, which is rounded to an integer and so would be wrong on a fractionally-scaled screen.
        let scale = scale_of(&still, screen);
        screenshot::crop(
            &still,
            Area {
                x: (area.x as f32 * scale).round() as i32,
                y: (area.y as f32 * scale).round() as i32,
                width: (area.width as f32 * scale).round() as i32,
                height: (area.height as f32 * scale).round() as i32,
            },
        )
        .ok()
    });
    request_close();
    let then = Rc::clone(then);
    platform_wayland::timeout(std::time::Duration::from_millis(80), move || {
        then(Picked {
            area: global,
            frozen: cropped,
        })
    });
}

/// How many image pixels one logical pixel is, from the still's width against the screen's logical width.
fn scale_of(still: &screenshot::Image, screen: Screen) -> f32 {
    let logical = screen.logical.0 as f32;
    if logical <= 0.0 {
        return 1.0;
    }
    (still.width as f32 / logical).max(0.1)
}

fn still_image(
    frozen: &Rc<RefCell<Option<screenshot::Image>>>,
) -> Result<Option<Box<dyn LayoutItem>>, LayoutError> {
    let borrowed = frozen.borrow();
    let Some(still) = borrowed.as_ref() else {
        return Ok(None);
    };
    let data = Arc::new(ImageData::new(
        still.pixels.clone(),
        still.width,
        still.height,
    ));
    let image = Image::new(
        LayoutStyle::new().absolute_fill(),
        move || data.clone(),
        || Raster::Smooth,
        || ObjectFit::Cover,
    )?;
    Ok(Some(Box::new(image)))
}

/// The dim over everything that is not selected, and the selection's own outline.
///
/// Four rectangles around the selection rather than one over the whole screen with a hole in it: the renderer has no way to subtract a shape from a fill, and dimming the selection too would defeat the point of showing the user what they are about to capture.
fn wash(
    selection: telar::ReadSignal<Option<Area>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let canvas = Canvas::new(LayoutStyle::new().absolute_fill(), move |rect: Rect| {
        let shade = Color::rgba(0.0, 0.0, 0.0, DIM);
        let Some(area) = selection.get().filter(|area| !area.is_empty()) else {
            return RenderNode::path(
                Arc::new(box_path(0.0, 0.0, rect.width, rect.height)),
                PathStyle::default().with_fill(shade),
            );
        };
        let (x, y) = (area.x as f32, area.y as f32);
        let (w, h) = (area.width as f32, area.height as f32);
        let dimmed = [
            box_path(0.0, 0.0, rect.width, y),
            box_path(0.0, y + h, rect.width, rect.height - (y + h)),
            box_path(0.0, y, x, h),
            box_path(x + w, y, rect.width - (x + w), h),
        ];
        let mut nodes: Vec<RenderNode> = dimmed
            .into_iter()
            .map(|path| RenderNode::path(Arc::new(path), PathStyle::default().with_fill(shade)))
            .collect();
        nodes.push(RenderNode::path(
            Arc::new(box_path(x, y, w, h)),
            PathStyle::default().with_stroke(Stroke::new(theme.accent, BORDER)),
        ));
        RenderNode::group(nodes)
    })?;
    Ok(Box::new(canvas))
}

fn box_path(x: f32, y: f32, width: f32, height: f32) -> PathData {
    let (width, height) = (width.max(0.0), height.max(0.0));
    PathData::polygon(&[
        Point::new(x, y),
        Point::new(x + width, y),
        Point::new(x + width, y + height),
        Point::new(x, y + height),
    ])
}

/// The size readout, pinned to the top of the screen rather than following the pointer: a label chasing the cursor is the one thing guaranteed to be under whatever the user is trying to look at.
fn readout(
    selection: telar::ReadSignal<Option<Area>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::new(
        move || match selection.get().filter(|area| !area.is_empty()) {
            Some(area) => format!("{} × {}", area.width, area.height),
            None => telar::t!("capture.pick_hint"),
        },
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;
    let pill = StyledContainer::new(
        LayoutStyle::new()
            .padding_horizontal(space::xl())
            .padding_vertical(space::md()),
        paint::xl(theme.surface),
        vec![box_item(text)],
    )?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .absolute_fill()
            .flex_row()
            .align_items(AlignItems::FLEX_START)
            .justify_content(JustifyContent::CENTER)
            .padding_all(space::xxl()),
        vec![Box::new(pill)],
    )?))
}

/// Every window on this output as a surface-local rectangle, front to back, plus the output itself last — so a click that hits no window still has something to select.
fn window_rects(screen: Screen) -> Rc<Vec<Area>> {
    let clients = hyprland::current_clients()
        .or_else(|| hyprland::socket_dir().map(|dir| hyprland::clients(&dir)));
    let mut rects: Vec<Area> = clients
        .unwrap_or_default()
        .iter()
        .filter(|client| client.mapped && client.size.0 > 0 && client.size.1 > 0)
        .map(|client| local_rect(client, screen.origin))
        .rev()
        .collect();
    rects.push(Area {
        x: 0,
        y: 0,
        width: screen.logical.0,
        height: screen.logical.1,
    });
    Rc::new(rects)
}

fn local_rect(client: &Client, origin: (i32, i32)) -> Area {
    Area {
        x: client.at.0 - origin.0,
        y: client.at.1 - origin.1,
        width: client.size.0,
        height: client.size.1,
    }
}

/// The frontmost rectangle containing the point — a window, else the output. `rects` is ordered front to back with the output last, so the first hit is the answer.
fn under_pointer(x: f32, y: f32, rects: &[Area]) -> Area {
    rects
        .iter()
        .find(|rect| {
            (x as i32) >= rect.x
                && (y as i32) >= rect.y
                && (x as i32) <= rect.x + rect.width
                && (y as i32) <= rect.y + rect.height
        })
        .copied()
        .unwrap_or_else(|| whole_output(rects))
}

fn whole_output(rects: &[Area]) -> Area {
    rects.last().copied().unwrap_or_default()
}

/// Pulls each edge of `drawn` onto a window edge it is already within [`SNAP`] of. Each edge snaps on its own, so a box drawn roughly over two tiled windows comes out flush against both.
fn snapped(drawn: Area, rects: &[Area]) -> Area {
    let mut left = drawn.x as f32;
    let mut top = drawn.y as f32;
    let mut right = (drawn.x + drawn.width) as f32;
    let mut bottom = (drawn.y + drawn.height) as f32;
    for rect in rects {
        let (rl, rt) = (rect.x as f32, rect.y as f32);
        let (rr, rb) = ((rect.x + rect.width) as f32, (rect.y + rect.height) as f32);
        for (value, candidates) in [
            (&mut left, [rl, rr]),
            (&mut right, [rl, rr]),
            (&mut top, [rt, rb]),
            (&mut bottom, [rt, rb]),
        ] {
            for candidate in candidates {
                if (*value - candidate).abs() <= SNAP {
                    *value = candidate;
                }
            }
        }
    }
    Area {
        x: left.round() as i32,
        y: top.round() as i32,
        width: (right - left).round().max(0.0) as i32,
        height: (bottom - top).round().max(0.0) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_wayland::{KeyboardInteractivity, Layer};

    fn rects() -> Vec<Area> {
        vec![
            Area {
                x: 100,
                y: 100,
                width: 400,
                height: 300,
            },
            Area {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        ]
    }

    #[test]
    fn a_click_selects_the_window_under_it_and_the_screen_otherwise() {
        let window = under_pointer(200.0, 200.0, &rects());
        assert_eq!(window, rects()[0], "the window in front wins");

        let desktop = under_pointer(1500.0, 900.0, &rects());
        assert_eq!(desktop, rects()[1], "empty desktop is the whole screen");
    }

    #[test]
    fn each_edge_snaps_to_a_window_on_its_own() {
        // Drawn a few pixels off on every side; every edge is within the threshold of the window's own.
        let rough = Area {
            x: 104,
            y: 96,
            width: 392,
            height: 308,
        };
        assert_eq!(
            snapped(rough, &rects()),
            rects()[0],
            "a rough drag over a window comes out flush with it"
        );

        // Far from anything: left alone rather than pulled somewhere the user did not point.
        let deliberate = Area {
            x: 700,
            y: 700,
            width: 200,
            height: 150,
        };
        assert_eq!(snapped(deliberate, &rects()), deliberate);
    }

    #[test]
    fn a_selection_snapping_to_two_windows_keeps_both_edges() {
        let tiled = vec![
            Area {
                x: 0,
                y: 0,
                width: 960,
                height: 1080,
            },
            Area {
                x: 960,
                y: 0,
                width: 960,
                height: 1080,
            },
            Area {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        ];
        let across = Area {
            x: 6,
            y: 4,
            width: 1910,
            height: 1072,
        };
        assert_eq!(
            snapped(across, &tiled),
            Area {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn the_overlay_covers_the_whole_screen_and_takes_the_keyboard() {
        let layer = placement(Some("DP-1".to_string())).layer_config();
        assert_eq!(
            layer.exclusive_zone, -1,
            "a picker ignores the bars' reserved space: the user is selecting what they can see"
        );
        assert!(matches!(layer.layer, Layer::Overlay));
        assert!(matches!(
            layer.keyboard_interactivity,
            KeyboardInteractivity::Exclusive
        ));
        assert!(!layer.input_transparent, "the drag has to land somewhere");
    }

    #[test]
    fn the_overlay_builds_with_and_without_a_still() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let nothing: Rc<RefCell<Option<screenshot::Image>>> = Rc::new(RefCell::new(None));
        assert!(
            overlay(NordTheme::new(), test_screen(), nothing, Rc::new(|_| {})).is_ok(),
            "no freeze: the overlay is a dim over the live screen"
        );

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let still = Rc::new(RefCell::new(Some(screenshot::Image {
            width: 2,
            height: 2,
            pixels: vec![255; 16],
        })));
        assert!(overlay(NordTheme::new(), test_screen(), still, Rc::new(|_| {})).is_ok());
    }

    fn test_screen() -> Screen {
        Screen {
            origin: (0, 0),
            logical: (1920, 1080),
        }
    }

    #[test]
    fn a_still_scales_by_the_ratio_it_actually_has() {
        let hidpi = screenshot::Image {
            width: 3840,
            height: 2160,
            pixels: Vec::new(),
        };
        assert_eq!(scale_of(&hidpi, test_screen()), 2.0);

        // A fractionally-scaled screen: 1.5× reports an integer scale of 2, so deriving the ratio from the image is the only way a crop lands where the user drew it.
        let fractional = screenshot::Image {
            width: 2880,
            height: 1620,
            pixels: Vec::new(),
        };
        assert_eq!(scale_of(&fractional, test_screen()), 1.5);
    }
}
