//! Wallpaper regions as tiles of their screen: one split in two, two that share a whole edge joined back into one, and the edges they share moved so that neighbours stay flush — no gap opens and nothing overlaps (TA-5, F-7).
//!
//! **Exact.** Every position these tools choose is snapped to a grid of 1/4096 of the screen, a fraction `f32` holds exactly, so for regions on that grid — the whole screen, and every region these tools make or move — the sums a split and a join make are exact too: joining the two halves of a split gives back the very rectangle that was split, not one a rounding away from it.
//!
//! **Where it is written.** A change is laid over the layout the way every edit is ([`crate::written::Written`]): each region changed where the rule that decides it writes it, a new region beside the one it came from, and all of it for the workspace that is up alone while the mode says so ([`crate::variant`]).
//!
//! Nothing here reads the pointer or draws: the edit mode's controls and keys ([`super::background`]) call these, and so can any layer whose preview has regions.

use layout::{Area, AreaId, AreaKind, LayerKind, Layout, LayoutOp, Rect, ResolvedAreaKind, Within};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::Node;

use crate::keys::Direction;
use crate::popover::kind_field;
use crate::session::EditError;
use crate::snap;
use crate::written::{Work, known};

/// What every position a region tool chooses is rounded to, as a fraction of the screen: a power of two, so sums of such fractions are exact in `f32`.
pub const GRID: f32 = 1.0 / 4096.0;
/// How narrow a region may be made across, as a fraction of the screen.
pub const SMALLEST: f32 = 1.0 / 64.0;
/// How far one key press moves an edge, as a fraction of the screen.
pub const STEP: f32 = 1.0 / 128.0;
/// How far apart two edges may be and still be one edge: what a hand-written layout's rounding leaves.
const SAME: f32 = 1e-4;

/// Which way a line cuts a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cut {
    /// Upright, leaving the halves side by side.
    SideBySide,
    /// Level, leaving the halves one above the other.
    Stacked,
}

impl Cut {
    /// Where `rect` starts and how far it runs across the cut: the axis a line of this cut is placed along.
    fn along(self, rect: Rect) -> (f32, f32) {
        match self {
            Cut::SideBySide => (rect.x, rect.w),
            Cut::Stacked => (rect.y, rect.h),
        }
    }

    /// Where `rect` starts and how far it runs the other way: the span a line of this cut covers.
    fn across(self, rect: Rect) -> (f32, f32) {
        match self {
            Cut::SideBySide => (rect.y, rect.h),
            Cut::Stacked => (rect.x, rect.w),
        }
    }

    /// `rect` running from `start` to `end` along this cut's axis and as it was the other way.
    fn spanning(self, rect: Rect, start: f32, end: f32) -> Rect {
        match self {
            Cut::SideBySide => Rect {
                x: start,
                w: end - start,
                ..rect
            },
            Cut::Stacked => Rect {
                y: start,
                h: end - start,
                ..rect
            },
        }
    }

    /// The cut whose lines stand across `direction`: an arrow left or right meets an upright line.
    pub fn facing(direction: Direction) -> Self {
        match direction.is_horizontal() {
            true => Cut::SideBySide,
            false => Cut::Stacked,
        }
    }
}

/// One region of the screen, where it is and which box it is measured in.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub id: AreaId,
    pub rect: Rect,
    pub within: Within,
}

/// `value` on the grid every region tool places things on.
pub fn snap(value: f32) -> f32 {
    (value / GRID).round() * GRID
}

fn same(a: f32, b: f32) -> bool {
    (a - b).abs() <= SAME
}

fn end(start: f32, extent: f32) -> f32 {
    start + extent
}

/// `rect` cut in two by a line of `cut` at `at` (a fraction of the screen, snapped to the grid): the half before the line and the half after it, or `None` where either would be narrower than [`SMALLEST`].
pub fn split(rect: Rect, cut: Cut, at: f32) -> Option<(Rect, Rect)> {
    let at = snap(at);
    let (start, extent) = cut.along(rect);
    let stop = end(start, extent);
    if at - start < SMALLEST - SAME || stop - at < SMALLEST - SAME {
        return None;
    }
    Some((cut.spanning(rect, start, at), cut.spanning(rect, at, stop)))
}

/// Where a split of `rect` by a line of `cut` goes when nothing says where: its middle, on the grid.
pub fn middle(rect: Rect, cut: Cut) -> f32 {
    let (start, extent) = cut.along(rect);
    snap(start + extent / 2.0)
}

/// The one rectangle `a` and `b` make together, where they share a whole edge — side by side at the same height, or one on the other at the same width; `None` otherwise.
pub fn join(a: Rect, b: Rect) -> Option<Rect> {
    [Cut::SideBySide, Cut::Stacked].into_iter().find_map(|cut| {
        let ((a_start, a_extent), (b_start, b_extent)) = (cut.along(a), cut.along(b));
        let ((a_from, a_span), (b_from, b_span)) = (cut.across(a), cut.across(b));
        let level = same(a_from, b_from) && same(a_span, b_span);
        let touching =
            same(end(a_start, a_extent), b_start) || same(end(b_start, b_extent), a_start);
        (level && touching).then(|| {
            let start = a_start.min(b_start);
            let stop = end(a_start, a_extent).max(end(b_start, b_extent));
            cut.spanning(a, start, stop)
        })
    })
}

/// The region that shares the whole edge of `from` facing `direction`, among `tiles`.
pub fn beside<'a>(tiles: &'a [Tile], from: &Tile, direction: Direction) -> Option<&'a Tile> {
    let cut = Cut::facing(direction);
    let (start, extent) = cut.along(from.rect);
    let edge = match direction.is_forward() {
        true => end(start, extent),
        false => start,
    };
    tiles.iter().find(|tile| {
        let (at, span) = cut.along(tile.rect);
        let meets = match direction.is_forward() {
            true => same(at, edge),
            false => same(end(at, span), edge),
        };
        tile.id != from.id
            && tile.within == from.within
            && meets
            && join(from.rect, tile.rect).is_some()
    })
}

/// One edge the regions of a screen share, or that one of them has in the open: the regions that end at it, the regions that start at it, and how far along it runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub cut: Cut,
    /// Where it is, as a fraction of the screen.
    pub at: f32,
    /// The regions before it — to its left, or above it.
    pub before: Vec<AreaId>,
    /// The regions after it.
    pub after: Vec<AreaId>,
    /// Where along the other way it starts and ends.
    pub span: (f32, f32),
}

impl Line {
    /// What names this edge across an edit that moves it: which way it cuts and which regions it lies between.
    pub fn key(&self) -> (Cut, Vec<AreaId>, Vec<AreaId>) {
        (self.cut, self.before.clone(), self.after.clone())
    }
}

/// Every edge of `tiles` that can be moved: those inside the screen, where moving one never opens the screen's own edge.
pub fn lines(tiles: &[Tile]) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for cut in [Cut::SideBySide, Cut::Stacked] {
        for tile in tiles {
            let (start, extent) = cut.along(tile.rect);
            for (at, before) in [(start, false), (end(start, extent), true)] {
                if same(at, 0.0) || same(at, 1.0) {
                    continue;
                }
                let (from, span) = cut.across(tile.rect);
                let line = match lines
                    .iter_mut()
                    .find(|line| line.cut == cut && same(line.at, at))
                {
                    Some(line) => line,
                    None => {
                        lines.push(Line {
                            cut,
                            at,
                            before: Vec::new(),
                            after: Vec::new(),
                            span: (from, end(from, span)),
                        });
                        lines.last_mut().expect("a line was just pushed")
                    }
                };
                line.span = (line.span.0.min(from), line.span.1.max(end(from, span)));
                match before {
                    true => line.before.push(tile.id.clone()),
                    false => line.after.push(tile.id.clone()),
                }
            }
        }
    }
    for line in &mut lines {
        line.before.sort();
        line.after.sort();
    }
    lines
}

/// The edge of `tile` that faces `direction`, as one of [`lines`].
pub fn line_of(tiles: &[Tile], tile: &Tile, direction: Direction) -> Option<Line> {
    let cut = Cut::facing(direction);
    let (start, extent) = cut.along(tile.rect);
    let at = match direction.is_forward() {
        true => end(start, extent),
        false => start,
    };
    lines(tiles)
        .into_iter()
        .find(|line| line.cut == cut && same(line.at, at))
}

/// How far `line` can move and leave every region on either side of it at least [`SMALLEST`] across.
pub fn range(tiles: &[Tile], line: &Line) -> (f32, f32) {
    let reach = |ids: &[AreaId], far: bool| {
        tiles
            .iter()
            .filter(|tile| ids.contains(&tile.id))
            .map(|tile| {
                let (start, extent) = line.cut.along(tile.rect);
                match far {
                    true => end(start, extent),
                    false => start,
                }
            })
            .collect::<Vec<f32>>()
    };
    let low = reach(&line.before, false)
        .into_iter()
        .map(|start| start + SMALLEST)
        .fold(0.0, f32::max);
    let high = reach(&line.after, true)
        .into_iter()
        .map(|stop| stop - SMALLEST)
        .fold(1.0, f32::min);
    ((low / GRID).ceil() * GRID, (high / GRID).floor() * GRID)
}

/// Every region `line` moving to `to` changes, with where it is then: those before it end there and those after it start there, so they stay flush. `to` is snapped to the grid and kept within [`range`].
pub fn moved(tiles: &[Tile], line: &Line, to: f32) -> Vec<(AreaId, Rect)> {
    let (low, high) = range(tiles, line);
    if low > high {
        return Vec::new();
    }
    let to = snap(to).clamp(low, high);
    tiles
        .iter()
        .filter_map(|tile| {
            let (start, extent) = line.cut.along(tile.rect);
            let stop = end(start, extent);
            let rect = if line.before.contains(&tile.id) {
                line.cut.spanning(tile.rect, start, to)
            } else if line.after.contains(&tile.id) {
                line.cut.spanning(tile.rect, to, stop)
            } else {
                return None;
            };
            (rect != tile.rect).then(|| (tile.id.clone(), rect))
        })
        .collect()
}
/// The screen a region edit is planned for: its arrangement as a layout resolves there, and what it takes to write an edit back.
pub struct Plan<'a> {
    pub layout: &'a Layout,
    pub desktop: Desktop,
    pub layer: LayerKind,
}

impl<'a> Plan<'a> {
    /// The screen and layer `node` is on, as its windows show it now, for an edit of `layout`.
    pub fn for_node(layout: &'a Layout, node: &Node) -> Result<Self, EditError> {
        let desktop =
            reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
        Ok(Self {
            layout,
            desktop,
            layer: node.layer,
        })
    }

    fn work(&self) -> Work<'_> {
        Work::new(self.layout, &self.desktop, self.layer)
    }

    /// Every region of the layer as the layout resolves on this screen, in z-order, measured in the box the one named `like` is.
    pub fn tiles(&self, like: &AreaId) -> Result<Vec<Tile>, EditError> {
        let resolved = self.desktop.resolving(self.layout, &known()).resolved;
        let all = tiles_of(&resolved, self.layer);
        let within = tile(&all, like)?.within;
        Ok(all
            .into_iter()
            .filter(|tile| tile.within == within)
            .collect())
    }

    /// `id` split in two by a line of `cut` at `at`: it keeps the part before the line, and a new region after it takes the rest, showing the same picture the same way.
    pub fn split(&self, id: &AreaId, cut: Cut, at: f32) -> Result<Vec<LayoutOp>, EditError> {
        let tiles = self.tiles(id)?;
        let tile = tile(&tiles, id)?;
        let (kept, taken) = split(tile.rect, cut, at).ok_or_else(|| {
            EditError::refused(util::message!(
                "editor.region.too_small",
                name = id.to_string()
            ))
        })?;
        let mut work = self.work();
        let ResolvedAreaKind::WallpaperRegion {
            source,
            fit,
            transition,
            ..
        } = work.area(self.layer, id)?.kind
        else {
            return Err(EditError::gone(id));
        };
        let region = Area {
            id: layout::ops::free_area_id(self.layout, &work.known, self.layer, id.as_str()),
            kind: Some(AreaKind::WallpaperRegion {
                rect: Some(taken),
                source: (!source.is_empty()).then_some(source),
                fit: Some(fit),
                transition: Some(transition),
            }),
            within: (tile.within != Within::Output).then_some(tile.within),
            ..Area::default()
        };
        placed_at(&mut work, id, kept)?;
        work.insert_after(self.layer, id, region)?;
        Ok(work.done())
    }

    /// `id` split in two at its middle by a line of `cut`, or on the cell line of the main grid within [`snap::CUT_TOLERANCE`] of it.
    pub fn split_in_half(&self, id: &AreaId, cut: Cut) -> Result<Vec<LayoutOp>, EditError> {
        let rect = tile(&self.tiles(id)?, id)?.rect;
        let lines = self.cut_lines(id, cut)?;
        let at = snap::region_line(middle(rect, cut), &lines, snap::CUT_TOLERANCE, false);
        self.split(id, cut, at)
    }

    /// The box the region `id` is measured in, in pixels.
    fn bounds(&self, id: &AreaId) -> Result<telar::Rect, EditError> {
        let within = tile(&self.tiles(id)?, id)?.within;
        Ok(self.desktop.reserved.box_of(within, self.desktop.size))
    }

    /// The cell lines of the layer's main grid a line of `cut` across `id` can snap to: those that leave it at least [`SMALLEST`] on either side.
    pub fn cut_lines(&self, id: &AreaId, cut: Cut) -> Result<Vec<f32>, EditError> {
        let (start, extent) = cut.along(tile(&self.tiles(id)?, id)?.rect);
        let stop = end(start, extent);
        let bounds = self.bounds(id)?;
        Ok(snap::grid_lines_in(
            &self.desktop,
            self.layer,
            cut.into(),
            bounds,
            (start, stop),
            SMALLEST,
        ))
    }

    /// `a` and `b` made one region, which is the one of them first in the layer's z-order, showing its picture; the other is taken out.
    pub fn join(&self, a: &AreaId, b: &AreaId) -> Result<Vec<LayoutOp>, EditError> {
        let tiles = self.tiles(a)?;
        let at = |id: &AreaId| tiles.iter().position(|tile| tile.id == *id);
        let (first, second) = match (at(a), at(b)) {
            (Some(one), Some(other)) if one < other => (&tiles[one], &tiles[other]),
            (Some(one), Some(other)) => (&tiles[other], &tiles[one]),
            _ => return Err(no_neighbour(a)),
        };
        let whole = join(first.rect, second.rect).ok_or_else(|| no_neighbour(a))?;
        let mut work = self.work();
        placed_at(&mut work, &first.id, whole)?;
        work.remove(self.layer, &second.id)?;
        Ok(work.done())
    }

    /// The region `id` joined with the one sharing its whole edge that way.
    pub fn join_toward(
        &self,
        id: &AreaId,
        direction: Direction,
    ) -> Result<Vec<LayoutOp>, EditError> {
        let tiles = self.tiles(id)?;
        let other = beside(&tiles, tile(&tiles, id)?, direction).ok_or_else(|| no_neighbour(id))?;
        self.join(id, &other.id)
    }

    /// The edge `key` names moved to `to`, every region on either side of it following.
    pub fn move_line(
        &self,
        like: &AreaId,
        key: &(Cut, Vec<AreaId>, Vec<AreaId>),
        to: f32,
    ) -> Result<Vec<LayoutOp>, EditError> {
        let tiles = self.tiles(like)?;
        let line = lines(&tiles)
            .into_iter()
            .find(|line| line.key() == *key)
            .ok_or_else(|| EditError::gone(like))?;
        self.placed(moved(&tiles, &line, to))
    }

    /// One key press of a resize: the edge of `id` ahead of `direction` pushed one [`STEP`] that way, or, where that edge is the screen's, the one behind it pulled after it.
    pub fn resize_step(
        &self,
        id: &AreaId,
        direction: Direction,
    ) -> Result<Vec<LayoutOp>, EditError> {
        let tiles = self.tiles(id)?;
        let tile = tile(&tiles, id)?;
        let behind = match direction {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        };
        let line = line_of(&tiles, tile, direction)
            .or_else(|| line_of(&tiles, tile, behind))
            .ok_or_else(|| EditError::no_way(id))?;
        self.step_lines(&tiles, id, &[line], direction)
    }

    /// One key press of a move: both edges of `id` across `direction` shifted one [`STEP`] that way, its neighbours taking up and giving back what it left and took. A region whose edge is the screen's cannot move off it.
    pub fn move_step(&self, id: &AreaId, direction: Direction) -> Result<Vec<LayoutOp>, EditError> {
        let tiles = self.tiles(id)?;
        let tile = tile(&tiles, id)?;
        let behind = match direction.is_horizontal() {
            true => [Direction::Left, Direction::Right],
            false => [Direction::Up, Direction::Down],
        };
        let lines = behind
            .into_iter()
            .map(|side| line_of(&tiles, tile, side))
            .collect::<Option<Vec<Line>>>()
            .ok_or_else(|| EditError::no_way(id))?;
        self.step_lines(&tiles, id, &lines, direction)
    }

    /// Every line of `lines` moved one step `direction` together, refused where any of them cannot go that far.
    fn step_lines(
        &self,
        tiles: &[Tile],
        id: &AreaId,
        lines: &[Line],
        direction: Direction,
    ) -> Result<Vec<LayoutOp>, EditError> {
        let by = match direction.is_forward() {
            true => STEP,
            false => -STEP,
        };
        let mut placed = tiles.to_vec();
        let mut ordered = lines.to_vec();
        if by > 0.0 {
            ordered.reverse();
        }
        for line in ordered {
            let wanted = snap(line.at + by);
            let (low, high) = range(&placed, &line);
            if wanted < low || wanted > high {
                return Err(EditError::no_way(id));
            }
            for (moved_id, rect) in moved(&placed, &line, wanted) {
                if let Some(tile) = placed.iter_mut().find(|tile| tile.id == moved_id) {
                    tile.rect = rect;
                }
            }
        }
        self.placed(
            placed
                .into_iter()
                .zip(tiles)
                .filter(|(now, was)| now.rect != was.rect)
                .map(|(now, _)| (now.id, now.rect)),
        )
    }

    /// Each region of `rects` placed where it says, one after another.
    fn placed(
        &self,
        rects: impl IntoIterator<Item = (AreaId, Rect)>,
    ) -> Result<Vec<LayoutOp>, EditError> {
        let mut work = self.work();
        for (id, rect) in rects {
            placed_at(&mut work, &id, rect)?;
        }
        Ok(work.done())
    }
}

/// The region `id` placed at `rect`, where the layout writes it.
fn placed_at(work: &mut Work, id: &AreaId, rect: Rect) -> Result<(), EditError> {
    work.rewrite(work.layer, id, |area| {
        kind_field!(area, "wallpaper_region", WallpaperRegion { rect }, rect)
    })
}

fn tile<'t>(tiles: &'t [Tile], id: &AreaId) -> Result<&'t Tile, EditError> {
    tiles
        .iter()
        .find(|tile| tile.id == *id)
        .ok_or_else(|| EditError::gone(id))
}

/// Every wallpaper region `layer` resolves to in `resolved`, in z-order.
pub fn tiles_of(resolved: &layout::Resolved, layer: LayerKind) -> Vec<Tile> {
    resolved
        .layer(layer)
        .map(|layer| {
            layer
                .areas
                .iter()
                .filter_map(|area| match area.kind {
                    ResolvedAreaKind::WallpaperRegion { rect, .. } => Some(Tile {
                        id: area.id.clone(),
                        rect,
                        within: area.within,
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn no_neighbour(id: &AreaId) -> EditError {
    EditError::refused(util::message!(
        "editor.region.no_neighbour",
        name = id.to_string()
    ))
}
