//! Where the panel an instance owns sits, as pure geometry: the rect it covers and the origin of the cell grid inside its padding.

use config::Edge;
use layout::Sides;
use telar::Rect;
use ui::host::Footprint;

use crate::transient::{DEFAULT_GAP, beside_chip, hung_off, kept_inside};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelShape {
    pub cols: u32,
    pub rows: u32,
    pub cell: f32,
    pub gap: f32,
    pub padding: Sides,
}

impl PanelShape {
    pub fn size(self) -> (f32, f32) {
        let cells = |count: u32| u16::try_from(count).unwrap_or(u16::MAX);
        let extent = Footprint {
            columns: cells(self.cols),
            rows: cells(self.rows),
        }
        .extent(self.cell, self.gap);
        (
            extent.width + self.padding.horizontal(),
            extent.height + self.padding.vertical(),
        )
    }
}

/// The bar an owner sits in: the edge it is on and the strip it is drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarSite {
    pub edge: Edge,
    pub strip: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelPlacement {
    pub rect: Rect,
    pub grid_origin: (f32, f32),
}

/// `usable` is the box no reserving strip takes, so a panel placed inside it never covers one. An `along` panel needs a bar to run along and is beside its owner without one.
pub fn place(
    owner: Rect,
    bar: Option<BarSite>,
    along: bool,
    usable: Rect,
    shape: PanelShape,
) -> PanelPlacement {
    let rect = match (bar, along) {
        (Some(bar), true) => along_bar(bar, usable, shape),
        (bar, _) => beside_owner(owner, bar.map(|bar| bar.edge), usable, shape.size()),
    };
    PanelPlacement {
        rect,
        grid_origin: (rect.x + shape.padding.left(), rect.y + shape.padding.top()),
    }
}

fn beside_owner(owner: Rect, edge: Option<Edge>, usable: Rect, size: (f32, f32)) -> Rect {
    let (width, height) = size;
    let (x, y) = match edge {
        Some(edge) => {
            let (x, y) = beside_chip(edge, DEFAULT_GAP, owner, size, usable);
            (
                kept_inside(x, usable.x, usable.width, width, 0.0),
                kept_inside(y, usable.y, usable.height, height, 0.0),
            )
        }
        None => hung_off(owner, DEFAULT_GAP, size, usable),
    };
    Rect::new(x, y, width, height)
}

fn along_bar(bar: BarSite, usable: Rect, shape: PanelShape) -> Rect {
    let (width, height) = shape.size();
    let strip = bar.strip;
    let (start, end) = match bar.edge.is_vertical() {
        true => (
            strip.y.max(usable.y),
            (strip.y + strip.height).min(usable.y + usable.height),
        ),
        false => (
            strip.x.max(usable.x),
            (strip.x + strip.width).min(usable.x + usable.width),
        ),
    };
    let length = (end - start).max(0.0);
    let (usable_right, usable_bottom) = (usable.x + usable.width, usable.y + usable.height);
    match bar.edge {
        Edge::Top => {
            let y = (strip.y + strip.height).max(usable.y);
            Rect::new(start, y, length, height.min(usable_bottom - y).max(0.0))
        }
        Edge::Bottom => {
            let bottom = strip.y.min(usable_bottom);
            let y = (bottom - height).max(usable.y);
            Rect::new(start, y, length, bottom - y)
        }
        Edge::Left => {
            let x = (strip.x + strip.width).max(usable.x);
            Rect::new(x, start, width.min(usable_right - x).max(0.0), length)
        }
        Edge::Right => {
            let right = strip.x.min(usable_right);
            let x = (right - width).max(usable.x);
            Rect::new(x, start, right - x, length)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Rect {
        Rect::new(0.0, 0.0, 1920.0, 1080.0)
    }

    fn shape(cols: u32, rows: u32) -> PanelShape {
        PanelShape {
            cols,
            rows,
            cell: 40.0,
            gap: 8.0,
            padding: Sides::all(12.0),
        }
    }

    fn strip(edge: Edge, thickness: f32) -> Rect {
        match edge {
            Edge::Top => Rect::new(0.0, 0.0, 1920.0, thickness),
            Edge::Bottom => Rect::new(0.0, 1080.0 - thickness, 1920.0, thickness),
            Edge::Left => Rect::new(0.0, 0.0, thickness, 1080.0),
            Edge::Right => Rect::new(1920.0 - thickness, 0.0, thickness, 1080.0),
        }
    }

    fn usable(edge: Edge, thickness: f32) -> Rect {
        match edge {
            Edge::Top => Rect::new(0.0, thickness, 1920.0, 1080.0 - thickness),
            Edge::Bottom => Rect::new(0.0, 0.0, 1920.0, 1080.0 - thickness),
            Edge::Left => Rect::new(thickness, 0.0, 1920.0 - thickness, 1080.0),
            Edge::Right => Rect::new(0.0, 0.0, 1920.0 - thickness, 1080.0),
        }
    }

    fn inside(rect: Rect, bounds: Rect) -> bool {
        rect.x >= bounds.x - 0.01
            && rect.y >= bounds.y - 0.01
            && rect.x + rect.width <= bounds.x + bounds.width + 0.01
            && rect.y + rect.height <= bounds.y + bounds.height + 0.01
    }

    fn beside_in_bar(edge: Edge, owner: Rect) -> Rect {
        let bar = BarSite {
            edge,
            strip: strip(edge, 40.0),
        };
        place(owner, Some(bar), false, usable(edge, 40.0), shape(6, 4)).rect
    }

    #[test]
    fn the_size_is_every_cell_and_gap_plus_the_padding() {
        assert_eq!(
            shape(6, 4).size(),
            (6.0 * 40.0 + 5.0 * 8.0 + 24.0, 4.0 * 40.0 + 3.0 * 8.0 + 24.0)
        );
    }

    #[test]
    fn the_grid_starts_inside_the_padding() {
        let placed = place(
            Rect::new(900.0, 8.0, 60.0, 24.0),
            None,
            false,
            output(),
            shape(2, 2),
        );
        assert_eq!(
            placed.grid_origin,
            (placed.rect.x + 12.0, placed.rect.y + 12.0)
        );
    }

    #[test]
    fn beside_a_top_bar_hangs_below_the_owner_centred_on_it() {
        let owner = Rect::new(900.0, 8.0, 60.0, 24.0);
        let rect = beside_in_bar(Edge::Top, owner);
        let (width, height) = shape(6, 4).size();
        assert_eq!(
            rect,
            Rect::new(930.0 - width / 2.0, 32.0 + 8.0, width, height)
        );
    }

    #[test]
    fn beside_a_bottom_bar_rises_above_the_owner() {
        let owner = Rect::new(900.0, 1048.0, 60.0, 24.0);
        let rect = beside_in_bar(Edge::Bottom, owner);
        assert_eq!(rect.y + rect.height, 1048.0 - 8.0);
        assert_eq!(rect.x + rect.width / 2.0, 930.0);
    }

    #[test]
    fn beside_a_left_bar_opens_to_its_right_top_aligned() {
        let owner = Rect::new(8.0, 300.0, 24.0, 24.0);
        let rect = beside_in_bar(Edge::Left, owner);
        assert_eq!((rect.x, rect.y), (32.0 + 8.0, 300.0));
    }

    #[test]
    fn beside_a_right_bar_opens_to_its_left_top_aligned() {
        let owner = Rect::new(1888.0, 300.0, 24.0, 24.0);
        let rect = beside_in_bar(Edge::Right, owner);
        assert_eq!((rect.x + rect.width, rect.y), (1888.0 - 8.0, 300.0));
    }

    #[test]
    fn beside_is_kept_inside_the_usable_box_at_the_ends_of_a_bar() {
        let near_start = beside_in_bar(Edge::Top, Rect::new(0.0, 8.0, 40.0, 24.0));
        assert_eq!(near_start.x, 8.0);
        let near_end = beside_in_bar(Edge::Top, Rect::new(1880.0, 8.0, 40.0, 24.0));
        assert_eq!(near_end.x + near_end.width, 1920.0 - 8.0);
        let low = beside_in_bar(Edge::Left, Rect::new(8.0, 1060.0, 24.0, 24.0));
        assert_eq!(low.y + low.height, 1080.0 - 8.0);
    }

    #[test]
    fn a_desktop_owner_gets_its_panel_below_when_it_fits() {
        let owner = Rect::new(800.0, 300.0, 160.0, 160.0);
        let rect = place(owner, None, false, usable(Edge::Top, 40.0), shape(6, 4)).rect;
        assert_eq!(rect.y, 300.0 + 160.0 + 8.0);
        assert_eq!(rect.x + rect.width / 2.0, 880.0);
    }

    #[test]
    fn a_desktop_owner_near_the_bottom_flips_its_panel_above() {
        let owner = Rect::new(800.0, 880.0, 160.0, 160.0);
        let rect = place(owner, None, false, usable(Edge::Bottom, 40.0), shape(6, 4)).rect;
        assert_eq!(rect.y + rect.height, 880.0 - 8.0);
    }

    #[test]
    fn a_desktop_owner_near_each_side_clamps_its_panel() {
        let bounds = usable(Edge::Top, 40.0);
        let left = place(
            Rect::new(0.0, 300.0, 80.0, 80.0),
            None,
            false,
            bounds,
            shape(6, 4),
        )
        .rect;
        assert_eq!(left.x, 8.0);
        let right = place(
            Rect::new(1840.0, 300.0, 80.0, 80.0),
            None,
            false,
            bounds,
            shape(6, 4),
        )
        .rect;
        assert_eq!(right.x + right.width, 1920.0 - 8.0);
        let top = place(
            Rect::new(800.0, 40.0, 80.0, 80.0),
            None,
            false,
            usable(Edge::Top, 40.0),
            shape(6, 16),
        )
        .rect;
        assert!(inside(top, Rect::new(8.0, 48.0, 1904.0, 1024.0)));
    }

    #[test]
    fn along_fills_the_strip_and_sits_flush_against_a_horizontal_bar() {
        let (_, height) = shape(16, 2).size();
        let top = BarSite {
            edge: Edge::Top,
            strip: strip(Edge::Top, 40.0),
        };
        let placed = place(
            Rect::new(900.0, 8.0, 60.0, 24.0),
            Some(top),
            true,
            usable(Edge::Top, 40.0),
            shape(16, 2),
        );
        assert_eq!(placed.rect, Rect::new(0.0, 40.0, 1920.0, height));
        let bottom = BarSite {
            edge: Edge::Bottom,
            strip: strip(Edge::Bottom, 40.0),
        };
        let placed = place(
            Rect::new(900.0, 1048.0, 60.0, 24.0),
            Some(bottom),
            true,
            usable(Edge::Bottom, 40.0),
            shape(16, 2),
        );
        assert_eq!(placed.rect, Rect::new(0.0, 1040.0 - height, 1920.0, height));
    }

    #[test]
    fn along_fills_the_strip_and_sits_flush_against_a_vertical_bar() {
        let (width, _) = shape(2, 9).size();
        let left = BarSite {
            edge: Edge::Left,
            strip: strip(Edge::Left, 40.0),
        };
        let placed = place(
            Rect::new(8.0, 300.0, 24.0, 24.0),
            Some(left),
            true,
            usable(Edge::Left, 40.0),
            shape(2, 9),
        );
        assert_eq!(placed.rect, Rect::new(40.0, 0.0, width, 1080.0));
        let right = BarSite {
            edge: Edge::Right,
            strip: strip(Edge::Right, 40.0),
        };
        let placed = place(
            Rect::new(1888.0, 300.0, 24.0, 24.0),
            Some(right),
            true,
            usable(Edge::Right, 40.0),
            shape(2, 9),
        );
        assert_eq!(placed.rect, Rect::new(1880.0 - width, 0.0, width, 1080.0));
        assert_eq!(placed.grid_origin, (1880.0 - width + 12.0, 12.0));
    }

    #[test]
    fn along_without_a_bar_falls_back_to_beside() {
        let owner = Rect::new(800.0, 300.0, 160.0, 160.0);
        let along = place(owner, None, true, output(), shape(6, 4));
        assert_eq!(along, place(owner, None, false, output(), shape(6, 4)));
    }

    #[test]
    fn a_panel_never_crosses_a_reserving_strip() {
        let owners = [
            Rect::new(0.0, 0.0, 30.0, 30.0),
            Rect::new(900.0, 8.0, 60.0, 24.0),
            Rect::new(1890.0, 1050.0, 30.0, 30.0),
            Rect::new(8.0, 540.0, 24.0, 24.0),
        ];
        let big = shape(30, 20);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            let bounds = usable(edge, 40.0);
            let bar = BarSite {
                edge,
                strip: strip(edge, 40.0),
            };
            for owner in owners {
                for along in [false, true] {
                    for shape in [shape(6, 4), big] {
                        let rect = place(owner, Some(bar), along, bounds, shape).rect;
                        assert!(
                            inside(rect, bounds),
                            "{edge:?} {owner:?} along={along} {rect:?}"
                        );
                    }
                }
                let desktop = place(owner, None, false, bounds, big).rect;
                assert!(
                    inside(desktop, bounds),
                    "desktop {edge:?} {owner:?} {desktop:?}"
                );
            }
        }
    }

    /// A drawer and an owned panel opened from the same chip on no edge open on the same side of it, whatever its size and wherever the chip is.
    #[test]
    fn a_drawer_and_an_owned_panel_from_one_chip_on_no_edge_open_on_the_same_side() {
        let bounds = usable(Edge::Top, 40.0);
        for y in [40.0, 300.0, 500.0, 620.0, 760.0, 1040.0] {
            let owner = Rect::new(800.0, y, 40.0, 40.0);
            for rows in [1, 4, 8, 12, 19] {
                let shape = shape(6, rows);
                let panel = place(owner, None, false, bounds, shape).rect;
                let drawer = crate::transient::chips::Site {
                    output: None,
                    layer: layout::LayerKind::Top,
                    edge: None,
                    chrome: ui::chrome::Chrome::global(
                        std::sync::Arc::new(config::Config::default()),
                        None,
                    ),
                    gap: DEFAULT_GAP,
                }
                .beside(owner);
                let laid = Rect::new(0.0, 0.0, panel.width, panel.height);
                assert_eq!(
                    crate::transient::settled_in(&drawer, laid, bounds),
                    panel,
                    "owner at {y}, {rows} rows"
                );
            }
        }
    }
}
