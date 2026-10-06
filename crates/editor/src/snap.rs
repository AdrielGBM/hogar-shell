//! Snapping what a drag moves, and the guide lines that say where it snapped.
//!
//! **Wired today.** Dragging the lock prompt snaps its edges and centre to the edges and centres of the regions, textures and free areas of its layer measured in the same box ([`area_siblings`]) and to the start, middle and end of that box, within [`SNAP_PX`] logical pixels ([`snap`] with [`Moving::BODY`]). Region edges and the cuts of a split snap as described next.
//!
//! **Region edges.** An edge two wallpaper regions share, and the cut a split makes, snap to the cell lines of the layer's main grid — the desktop's for the background, the lock's own for the lock — in the middle of the gap between two cells: within [`EDGE_TOLERANCE`] of the box for an edge, [`CUT_TOLERANCE`] for a cut. What that gives is then put on [`regions::GRID`], so splits and joins stay exact.
//!
//! **Alt drags free.** While Alt is held nothing snaps ([`free`]).
//!
//! **Not wired yet.** [`snap`] also takes a resize (`Motion::Start` and `Motion::End`) and [`free_siblings`] lists the other children of a `free` container; no drag calls them until resizing a rectangle and carrying free children use them.
use layout::{
    AreaId, InstanceId, LayerKind, Placement, Rect, ResolvedAreaKind, ResolvedGroup, ResolvedLayer,
};
use surfaces::reconcile::Desktop;

use crate::modes::regions::{self, Cut};
use crate::modes::widgets::Geometry;

/// How near, in logical pixels, an edge or centre has to come to a line to snap to it.
pub const SNAP_PX: f32 = 6.0;
/// How near a dragged region edge has to come to a cell line to snap to it, as a fraction of its box.
pub const EDGE_TOLERANCE: f32 = 0.012;
/// How near a split's cut has to come to a cell line to snap to it, as a fraction of its box.
pub const CUT_TOLERANCE: f32 = 0.04;

/// Which way along the box a line is placed: an `X` line stands upright at a place across.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

impl From<Cut> for Axis {
    fn from(cut: Cut) -> Self {
        match cut {
            Cut::SideBySide => Axis::X,
            Cut::Stacked => Axis::Y,
        }
    }
}

impl Axis {
    fn along(self, rect: Rect) -> (f32, f32) {
        match self {
            Axis::X => (rect.x, rect.w),
            Axis::Y => (rect.y, rect.h),
        }
    }

    fn placed(self, rect: Rect, start: f32, extent: f32) -> Rect {
        match self {
            Axis::X => Rect {
                x: start,
                w: extent,
                ..rect
            },
            Axis::Y => Rect {
                y: start,
                h: extent,
                ..rect
            },
        }
    }

    fn of_size(self, (width, height): (f32, f32)) -> f32 {
        match self {
            Axis::X => width,
            Axis::Y => height,
        }
    }
}

/// What a drag does to a rectangle along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Carries it whole: its start, its centre and its end may each snap.
    Move,
    /// Drags its start, its end staying put.
    Start,
    /// Drags its end, its start staying put.
    End,
}

impl Motion {
    fn edges(self) -> &'static [f32] {
        match self {
            Motion::Move => &[0.0, 0.5, 1.0],
            Motion::Start => &[0.0],
            Motion::End => &[1.0],
        }
    }
}

/// What a drag does to a rectangle each way; `None` leaves that way alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moving {
    pub x: Option<Motion>,
    pub y: Option<Motion>,
}

impl Moving {
    /// A rectangle carried whole.
    pub const BODY: Self = Self {
        x: Some(Motion::Move),
        y: Some(Motion::Move),
    };

    fn on(self, axis: Axis) -> Option<Motion> {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }
}

/// A line something snapped to, as a fraction of the box it is measured in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub axis: Axis,
    pub at: f32,
}

impl Guide {
    /// Where the line is drawn across `bounds`, the box in pixels: as wide or as tall as it, and no thicker than nothing.
    pub fn line(self, bounds: telar::Rect) -> telar::Rect {
        match self.axis {
            Axis::X => telar::Rect::new(
                bounds.x + self.at * bounds.width,
                bounds.y,
                0.0,
                bounds.height,
            ),
            Axis::Y => telar::Rect::new(
                bounds.x,
                bounds.y + self.at * bounds.height,
                bounds.width,
                0.0,
            ),
        }
    }
}

/// Every one of `guides` drawn across `bounds`.
pub fn lines(guides: &[Guide], bounds: telar::Rect) -> Vec<telar::Rect> {
    guides.iter().map(|guide| guide.line(bounds)).collect()
}

/// A rectangle after snapping, and the lines it snapped to.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapped {
    pub rect: Rect,
    pub guides: Vec<Guide>,
}

/// Whether the drag under way goes free of every snap: while Alt is held.
pub fn free() -> bool {
    telar::modifiers().is_alt
}

/// `rect` as `moving` snaps it, each way on its own, to `siblings` and to the start, middle and end of its box, which is `size` logical pixels; `free` leaves it as it is. All of it is in fractions of the box.
pub fn snap(
    rect: Rect,
    siblings: &[Rect],
    size: (f32, f32),
    moving: Moving,
    free: bool,
) -> Snapped {
    let mut snapped = Snapped {
        rect,
        guides: Vec::new(),
    };
    if free {
        return snapped;
    }
    for axis in [Axis::X, Axis::Y] {
        let Some(motion) = moving.on(axis) else {
            continue;
        };
        let tolerance = SNAP_PX / axis.of_size(size).max(1.0);
        let Some((by, line)) = nearest(snapped.rect, axis, siblings, motion.edges(), tolerance)
        else {
            continue;
        };
        let (start, extent) = axis.along(snapped.rect);
        let (start, extent) = match motion {
            Motion::Move => (start + by, extent),
            Motion::Start => (start + by, extent - by),
            Motion::End => (start, extent + by),
        };
        snapped.rect = axis.placed(snapped.rect, start, extent);
        snapped.guides.push(Guide { axis, at: line });
    }
    snapped
}

/// How far `rect` moves along `axis` for the nearest of its `edges` (0 its start, ½ its centre, 1 its end) to land on a line within `tolerance`, and that line.
fn nearest(
    rect: Rect,
    axis: Axis,
    siblings: &[Rect],
    edges: &[f32],
    tolerance: f32,
) -> Option<(f32, f32)> {
    let (start, extent) = axis.along(rect);
    let lines = [0.0, 0.5, 1.0]
        .into_iter()
        .chain(siblings.iter().flat_map(|sibling| {
            let (from, across) = axis.along(*sibling);
            [from, from + across / 2.0, from + across]
        }));
    let lines: Vec<f32> = lines.collect();
    edges
        .iter()
        .flat_map(|edge| {
            let at = start + extent * edge;
            lines.iter().map(move |line| (line - at, *line))
        })
        .filter(|(by, _)| by.abs() < tolerance)
        .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
}

/// The line of `lines` nearest `value`, where one is within `tolerance` of it.
pub fn nearest_line(value: f32, lines: &[f32], tolerance: f32) -> Option<f32> {
    lines
        .iter()
        .copied()
        .filter(|line| (line - value).abs() < tolerance)
        .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
}

/// Where a region edge or cut dragged to `value` goes: onto the nearest of `lines` within `tolerance` unless `free`, then onto [`regions::GRID`].
pub fn region_line(value: f32, lines: &[f32], tolerance: f32, free: bool) -> f32 {
    let snapped = match free {
        true => None,
        false => nearest_line(value, lines, tolerance),
    };
    regions::snap(snapped.unwrap_or(value))
}

/// The layer whose grid the regions of `layer` line up with: the lock's own on the lock, the desktop's everywhere else.
pub fn main_grid_layer(layer: LayerKind) -> LayerKind {
    match layer {
        LayerKind::Lock => LayerKind::Lock,
        _ => LayerKind::Desktop,
    }
}

/// The cell lines of the main grid of `layer` on `desktop`'s screen along `axis`, each in the middle of the gap between two cells, as fractions of `bounds` and only those inside it; none where the layer has no grid.
pub fn grid_lines(
    desktop: &Desktop,
    layer: LayerKind,
    axis: Axis,
    bounds: telar::Rect,
) -> Vec<f32> {
    let Some(geometry) = desktop
        .resolved
        .layer(main_grid_layer(layer))
        .and_then(|held| {
            held.areas
                .iter()
                .find_map(|area| Geometry::of(desktop, area))
        })
    else {
        return Vec::new();
    };
    cell_lines(&geometry, axis, bounds)
}

/// The cell lines of the grid `geometry` along `axis`, as [`grid_lines`] gives them.
pub fn cell_lines(geometry: &Geometry, axis: Axis, bounds: telar::Rect) -> Vec<f32> {
    let gap = geometry.pitch - geometry.cell;
    let (origin, cells, from, extent) = match axis {
        Axis::X => (
            geometry.origin.0,
            geometry.room.cols,
            bounds.x,
            bounds.width,
        ),
        Axis::Y => (
            geometry.origin.1,
            geometry.room.rows,
            bounds.y,
            bounds.height,
        ),
    };
    if extent <= 0.0 {
        return Vec::new();
    }
    (0..=cells)
        .map(|line| (origin + line as f32 * geometry.pitch - gap / 2.0 - from) / extent)
        .filter(|at| (0.0..=1.0).contains(at))
        .collect()
}

/// Every line of `lines` as a guide along `axis`.
pub fn guides(axis: Axis, lines: &[f32]) -> Vec<Guide> {
    lines.iter().map(|at| Guide { axis, at: *at }).collect()
}

/// What the area `id` of `layer` snaps to: every other region, texture and free area measured in the same box.
pub fn area_siblings(layer: &ResolvedLayer, id: &AreaId) -> Vec<Rect> {
    let Some(within) = layer
        .areas
        .iter()
        .find(|area| area.id == *id)
        .map(|area| area.within)
    else {
        return Vec::new();
    };
    layer
        .areas
        .iter()
        .filter(|area| area.id != *id && area.within == within)
        .filter(|area| {
            !matches!(
                area.kind,
                ResolvedAreaKind::Grid { .. } | ResolvedAreaKind::Prompt { .. }
            )
        })
        .filter_map(|area| area.kind.rect())
        .collect()
}

/// What the child `id` of the `free` container `group` snaps to: every other child's rectangle in it.
pub fn free_siblings(group: &ResolvedGroup, id: &InstanceId) -> Vec<Rect> {
    group
        .children
        .iter()
        .filter(|child| child.id != *id)
        .filter_map(|child| match child.placement {
            Some(Placement::Rect(rect)) => Some(rect),
            _ => None,
        })
        .collect()
}
