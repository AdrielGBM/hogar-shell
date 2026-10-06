//! Container geometry kept apart from building it, so the editor can ask where a container's children sit without drawing it.

use layout::{Arrange, ChildCell, Placement, Rect, Representation as Placed, ResolvedGroup};
use ui::descriptor::Input;
use ui::host::{Audience, Size, WidgetSize};

const WIDGETS: [Placed; 3] = [Placed::WidgetL, Placed::WidgetM, Placed::WidgetS];

/// A share is a box divided by weights, which lands a hair under a whole pixel of the extent it was meant to hold.
const FIT_SLACK: f32 = 0.5;

/// About a bar's thickness on the default grid, so a widget collapsed to a chip reads as one.
const CHIP_OF_CELL: f32 = 0.5;

pub fn arranges(group: &ResolvedGroup) -> bool {
    matches!(
        group.arrange,
        Some(Arrange::Row | Arrange::Column | Arrange::Grid | Arrange::Free)
    )
}

/// The gap between a group's children: its own, else the plate scale's, which is tighter on a bar.
pub fn gap_of(group: &ResolvedGroup, in_bar: bool) -> f32 {
    group.gap.unwrap_or_else(|| ui::scale::plate::gap(in_bar))
}

/// How far a group holds its children off the edges of its plate: its style's, else the plate scale's.
pub fn padding_of(group: &ResolvedGroup, in_bar: bool) -> layout::Sides {
    group
        .style
        .padding
        .unwrap_or(layout::Sides::all(ui::scale::plate::padding(in_bar)))
}

/// Relative to the container's own top left corner, padding included, so every share lies inside the padded box.
pub fn shares(group: &ResolvedGroup, size: Size, gap: f32) -> Vec<telar::Rect> {
    let pad = padding_of(group, false);
    let inner = telar::Rect::new(
        pad.left(),
        pad.top(),
        (size.width - pad.horizontal()).max(0.0),
        (size.height - pad.vertical()).max(0.0),
    );
    let placements: Vec<Option<Placement>> =
        group.children.iter().map(|child| child.placement).collect();
    match group.arrange {
        Some(Arrange::Row) => weighted(&placements, inner, gap, Axis::Across),
        Some(Arrange::Column) => weighted(&placements, inner, gap, Axis::Down),
        Some(Arrange::Grid) => placements
            .iter()
            .map(|placement| on_tracks(cell_of(*placement), (group.cols, group.rows), inner, gap))
            .collect(),
        Some(Arrange::Free) => placements
            .iter()
            .map(|placement| fraction_of(rect_of(*placement), inner))
            .collect(),
        Some(Arrange::Pages) | None => vec![inner; placements.len()],
    }
}

#[derive(Clone, Copy)]
enum Axis {
    Across,
    Down,
}

fn weighted(
    placements: &[Option<Placement>],
    inner: telar::Rect,
    gap: f32,
    axis: Axis,
) -> Vec<telar::Rect> {
    let weights: Vec<f32> = placements
        .iter()
        .map(|placement| weight_of(*placement))
        .collect();
    let total: f32 = weights.iter().sum();
    let length = match axis {
        Axis::Across => inner.width,
        Axis::Down => inner.height,
    };
    let shared = (length - gap * weights.len().saturating_sub(1) as f32).max(0.0);
    let mut at = 0.0;
    weights
        .iter()
        .map(|weight| {
            let share = shared * weight / total;
            let rect = match axis {
                Axis::Across => telar::Rect::new(inner.x + at, inner.y, share, inner.height),
                Axis::Down => telar::Rect::new(inner.x, inner.y + at, inner.width, share),
            };
            at += share + gap;
            rect
        })
        .collect()
}

pub fn weight_of(placement: Option<Placement>) -> f32 {
    match placement {
        Some(Placement::Weight(weight)) if weight > 0.0 && weight.is_finite() => weight,
        _ => 1.0,
    }
}

fn cell_of(placement: Option<Placement>) -> ChildCell {
    match placement {
        Some(Placement::Cell(cell)) => cell,
        _ => ChildCell::at(0, 0),
    }
}

fn rect_of(placement: Option<Placement>) -> Rect {
    match placement {
        Some(Placement::Rect(rect)) => rect,
        _ => Rect::default(),
    }
}

fn on_tracks(
    cell: ChildCell,
    (cols, rows): (u32, u32),
    inner: telar::Rect,
    gap: f32,
) -> telar::Rect {
    let along = |start: u32, span: u32, tracks: u32, length: f32| {
        let tracks = tracks.max(1);
        let start = start.min(tracks - 1);
        let span = span.clamp(1, tracks - start);
        let track = ((length - gap * (tracks - 1) as f32) / tracks as f32).max(0.0);
        (
            start as f32 * (track + gap),
            span as f32 * track + (span - 1) as f32 * gap,
        )
    };
    let (x, width) = along(cell.col, cell.col_span, cols, inner.width);
    let (y, height) = along(cell.row, cell.row_span, rows, inner.height);
    telar::Rect::new(inner.x + x, inner.y + y, width, height)
}

fn fraction_of(rect: Rect, inner: telar::Rect) -> telar::Rect {
    let x = rect.x.clamp(0.0, 1.0);
    let y = rect.y.clamp(0.0, 1.0);
    let w = rect.w.clamp(0.0, 1.0 - x);
    let h = rect.h.clamp(0.0, 1.0 - y);
    telar::Rect::new(
        inner.x + x * inner.width,
        inner.y + y * inner.height,
        w * inner.width,
        h * inner.height,
    )
}

/// On the lock screen only representations that only read count as declared (TA-8).
pub fn fitted(module: &str, share: Size, grid: (f32, f32), audience: Audience) -> Placed {
    let descriptor = ui::descriptor::find(module);
    fitting(share, grid, |placed| {
        descriptor
            .and_then(|descriptor| descriptor.input(crate::area::representation(placed)))
            .is_some_and(|input| audience == Audience::Owner || input == Input::ReadOnly)
    })
}

/// A module with no chip is squeezed into its smallest widget rather than drawn as a placeholder.
pub fn fitting(share: Size, (cell, gap): (f32, f32), declared: impl Fn(Placed) -> bool) -> Placed {
    let fits = |placed: Placed| {
        widget_size(placed).is_some_and(|size| {
            let extent = size.footprint().extent(cell, gap);
            extent.width <= share.width + FIT_SLACK && extent.height <= share.height + FIT_SLACK
        })
    };
    WIDGETS
        .into_iter()
        .find(|placed| declared(*placed) && fits(*placed))
        .or_else(|| declared(Placed::Chip).then_some(Placed::Chip))
        .or_else(|| WIDGETS.into_iter().rev().find(|placed| declared(*placed)))
        .unwrap_or(Placed::Chip)
}

fn widget_size(placed: Placed) -> Option<WidgetSize> {
    match placed {
        Placed::WidgetS => Some(WidgetSize::S),
        Placed::WidgetM => Some(WidgetSize::M),
        Placed::WidgetL => Some(WidgetSize::L),
        Placed::Chip | Placed::Card => None,
    }
}

pub fn chip_extent(share: Size, cell: f32) -> Size {
    Size {
        width: share.width,
        height: share.height.min(share.width).min(cell * CHIP_OF_CELL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use layout::{GroupId, GroupKind, Instance, InstanceId, ResolvedInstance, Sides, Style};

    fn child(placement: Placement) -> ResolvedInstance {
        ResolvedInstance {
            id: InstanceId::new("child"),
            module: "probe".to_string(),
            representation: Placed::WidgetS,
            options: toml::Table::new(),
            bindings: BTreeMap::new(),
            style: Style::default(),
            placement: Some(placement),
            actions: BTreeMap::new(),
        }
    }

    fn container(arrange: Arrange, children: Vec<ResolvedInstance>) -> ResolvedGroup {
        ResolvedGroup {
            id: GroupId::new("box"),
            kind: GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 6,
                row_span: 2,
            },
            arrange: Some(arrange),
            cols: Arrange::TRACKS,
            rows: Arrange::TRACKS,
            gap: Some(8.0),
            repeat: None,
            komponent: None,
            style: unpadded(),
            children,
        }
    }

    fn unpadded() -> Style {
        Style {
            padding: Some(Sides::all(0.0)),
            ..Style::default()
        }
    }

    fn size(width: f32, height: f32) -> Size {
        Size { width, height }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    const BOX: Size = Size {
        width: 560.0,
        height: 176.0,
    };

    #[test]
    fn a_row_splits_its_box_minus_the_gaps_by_weight() {
        let group = container(
            Arrange::Row,
            vec![
                child(Placement::Weight(1.0)),
                child(Placement::Weight(2.0)),
                child(Placement::Weight(1.0)),
            ],
        );
        assert_eq!(
            shares(&group, BOX, 8.0),
            vec![
                telar::Rect::new(0.0, 0.0, 136.0, 176.0),
                telar::Rect::new(144.0, 0.0, 272.0, 176.0),
                telar::Rect::new(424.0, 0.0, 136.0, 176.0),
            ]
        );
    }

    #[test]
    fn a_column_splits_its_height_and_takes_the_whole_width() {
        let group = container(
            Arrange::Column,
            vec![child(Placement::Weight(1.0)), child(Placement::Weight(1.0))],
        );
        assert_eq!(
            shares(&group, BOX, 8.0),
            vec![
                telar::Rect::new(0.0, 0.0, 560.0, 84.0),
                telar::Rect::new(0.0, 92.0, 560.0, 84.0),
            ]
        );
    }

    #[test]
    fn a_weight_shares_the_length_in_proportion() {
        let group = container(
            Arrange::Row,
            vec![
                child(Placement::Weight(Instance::WEIGHTS.0)),
                child(Placement::Weight(Instance::WEIGHTS.1)),
            ],
        );
        let shares = shares(&group, size(33.0 * 4.0 + 8.0, 10.0), 8.0);
        assert_eq!(
            (shares[0].width, shares[1].width),
            (4.0, 128.0),
            "the least against the most"
        );
    }

    #[test]
    fn a_grid_lays_equal_tracks_and_a_spanning_child_covers_the_gap_between() {
        let mut group = container(
            Arrange::Grid,
            vec![
                child(Placement::Cell(ChildCell::at(0, 0))),
                child(Placement::Cell(ChildCell {
                    col: 1,
                    row: 0,
                    col_span: 2,
                    row_span: 2,
                })),
            ],
        );
        group.cols = 3;
        assert_eq!(
            shares(&group, size(316.0, 208.0), 8.0),
            vec![
                telar::Rect::new(0.0, 0.0, 100.0, 100.0),
                telar::Rect::new(108.0, 0.0, 208.0, 208.0),
            ]
        );
    }

    #[test]
    fn free_children_are_fractions_of_the_padded_box_and_never_reach_past_it() {
        let mut group = container(
            Arrange::Free,
            vec![
                child(Placement::Rect(rect(0.5, 0.0, 0.5, 0.5))),
                child(Placement::Rect(rect(0.75, 0.75, 0.5, 0.5))),
            ],
        );
        group.style.padding = Some(Sides::all(10.0));
        assert_eq!(
            shares(&group, size(220.0, 120.0), 8.0),
            vec![
                telar::Rect::new(110.0, 10.0, 100.0, 50.0),
                telar::Rect::new(160.0, 85.0, 50.0, 25.0),
            ]
        );
    }

    #[test]
    fn padding_comes_off_the_box_before_it_is_shared() {
        let mut group = container(Arrange::Row, vec![child(Placement::Weight(1.0))]);
        group.style.padding = Some(Sides::all(8.0));
        assert_eq!(
            shares(&group, BOX, 8.0),
            vec![telar::Rect::new(8.0, 8.0, 544.0, 160.0)]
        );
    }

    #[test]
    fn a_container_that_names_no_padding_holds_its_children_off_its_plate_by_the_plates() {
        let mut group = container(Arrange::Row, vec![child(Placement::Weight(1.0))]);
        group.style.padding = None;
        let pad = ui::scale::plate::padding(false);
        assert!(pad > 0.0);
        assert_eq!(
            shares(&group, BOX, 8.0),
            vec![telar::Rect::new(
                pad,
                pad,
                BOX.width - 2.0 * pad,
                BOX.height - 2.0 * pad
            )]
        );
    }

    fn everything(_: Placed) -> bool {
        true
    }

    const GRID: (f32, f32) = (80.0, 16.0);

    #[test]
    fn a_child_takes_the_largest_widget_its_share_holds() {
        let at = |width, height| fitting(size(width, height), GRID, everything);
        assert_eq!(at(368.0, 368.0), Placed::WidgetL);
        assert_eq!(at(368.0, 367.0), Placed::WidgetM);
        assert_eq!(at(181.3, 176.0), Placed::WidgetS);
        assert_eq!(at(175.4, 400.0), Placed::Chip);
    }

    #[test]
    fn a_child_too_small_for_any_widget_is_a_chip() {
        assert_eq!(fitting(size(181.0, 80.0), GRID, everything), Placed::Chip);
    }

    #[test]
    fn only_what_the_module_declares_is_fitted() {
        assert_eq!(
            fitting(size(400.0, 400.0), GRID, |placed| matches!(
                placed,
                Placed::WidgetS | Placed::Chip
            )),
            Placed::WidgetS
        );
        assert_eq!(
            fitting(size(50.0, 50.0), GRID, |placed| placed == Placed::WidgetM),
            Placed::WidgetM,
            "with no chip, the smallest widget it declares is squeezed in"
        );
    }

    #[test]
    fn a_chip_is_no_thicker_than_its_share_or_half_a_cell() {
        assert_eq!(chip_extent(size(181.0, 80.0), 80.0).height, 40.0);
        assert_eq!(chip_extent(size(30.0, 80.0), 80.0).height, 30.0);
        assert_eq!(chip_extent(size(181.0, 20.0), 80.0).height, 20.0);
    }
}

#[cfg(test)]
mod built {
    use super::*;

    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use config::Config;
    use layout::{
        Anchor, AreaId, GroupId, GroupKind, InstanceId, LayerKind, ResolvedArea, ResolvedAreaKind,
        ResolvedInstance, Style,
    };
    use telar::{
        AvailableSpace, Container, LayoutStyle, compute_layout, new_container,
        reset_layout_runtime, set_theme,
    };
    use ui::descriptor::{
        Built, Category, ChipDef, ChipFrame, ModuleDescriptor, Representations, WidgetDef,
    };
    use ui::host::{Host, Representation};

    use crate::area::Surround;
    use crate::layer_window::Reserved;
    use crate::rects;

    thread_local! {
        static DRAWN: RefCell<Vec<(Representation, Size)>> = const { RefCell::new(Vec::new()) };
    }

    /// Far larger than any share it is given, so a slot that let it spill would show.
    fn oversized(host: &Host) -> Built {
        DRAWN.with(|drawn| drawn.borrow_mut().push((host.representation, host.extent)));
        Ok(Box::new(Container::new(
            LayoutStyle::new().width(2000.0).height(2000.0),
            Vec::new(),
        )?))
    }

    static FITTED: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "fitted",
        name: "fitted",
        icon: "circle",
        category: Category::Info,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef {
                build: oversized,
                input: Input::ReadOnly,
                frame: ChipFrame::SelfManaged,
                square: false,
                press: None,
                scroll: None,
                elastic: false,
            }),
            widget: Some(WidgetDef {
                sizes: &WidgetSize::ALL,
                build: oversized,
                input: Input::ReadOnly,
            }),
            card: None,
            panel: None,
            popout: None,
        },
        actions: &[],
        sources: &[],
    }];

    /// Sixteen 80 px cells 16 px apart across, and room below for a container four cells tall.
    const PAGE: (f32, f32) = (1536.0, 864.0);

    fn row_of_three(col_span: u32, row_span: u32) -> ResolvedArea {
        let children = (0..3)
            .map(|at| ResolvedInstance {
                id: InstanceId::new(format!("child-{at}")),
                module: "fitted".to_string(),
                representation: Placed::WidgetM,
                options: toml::Table::new(),
                bindings: BTreeMap::new(),
                style: Style::default(),
                placement: Some(Placement::Weight(1.0)),
                actions: BTreeMap::new(),
            })
            .collect();
        ResolvedArea {
            id: AreaId::new("widgets"),
            kind: ResolvedAreaKind::Grid {
                rect: Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            reserve: false,
            above_fullscreen: false,
            within: layout::Within::Output,
            style: Style::default(),
            visible: None,
            actions: Default::default(),
            groups: vec![ResolvedGroup {
                id: GroupId::new("shelf"),
                kind: GroupKind::Cell {
                    col: 0,
                    row: 0,
                    col_span,
                    row_span,
                },
                arrange: Some(Arrange::Row),
                cols: Arrange::TRACKS,
                rows: Arrange::TRACKS,
                gap: Some(8.0),
                repeat: None,
                komponent: None,
                style: Style {
                    padding: Some(layout::Sides::all(0.0)),
                    ..Style::default()
                },
                children,
            }],
        }
    }

    /// What each child of a three-child row container `col_span` × `row_span` cells big was drawn as and told it has, and where its slot landed.
    fn drawn(col_span: u32, row_span: u32) -> Vec<(Representation, Size, telar::Rect)> {
        reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        ui::descriptor::install(FITTED);
        DRAWN.with(|drawn| drawn.borrow_mut().clear());
        let _scope = telar::owner_scope();
        let area = row_of_three(col_span, row_span);
        let surround = Surround {
            config: &config,
            theme: config.resolve_theme(),
            output: None,
            layer: LayerKind::Desktop,
            bounds: telar::Rect::new(0.0, 0.0, PAGE.0, PAGE.1),
            reserved: Reserved::default(),
            audience: Audience::Owner,
        };
        let built = crate::area::grid(
            &area,
            Rect::default(),
            80.0,
            16.0,
            Anchor::TopLeft,
            surround,
        )
        .expect("the grid builds");
        let root = new_container(
            LayoutStyle::new().width(PAGE.0).height(PAGE.1),
            &[built.layout_node()],
        )
        .expect("a page");
        compute_layout(
            root,
            AvailableSpace::Definite(PAGE.0),
            AvailableSpace::Definite(PAGE.1),
        )
        .expect("a laid out page");
        let at = rects::Node::area(None, LayerKind::Desktop, &area.id);
        let group = &area.groups[0];
        let slots: Vec<telar::Rect> = group
            .children
            .iter()
            .map(|child| rects::rect(&at.instance(&group.id, &child.id)).expect("tracked"))
            .collect();
        DRAWN
            .with(|drawn| drawn.borrow().clone())
            .into_iter()
            .zip(slots)
            .map(|((representation, extent), slot)| (representation, extent, slot))
            .collect()
    }

    fn assert_inside(slots: &[(Representation, Size, telar::Rect)], (width, height): (f32, f32)) {
        for (_, _, slot) in slots {
            assert!(
                slot.x >= 0.0
                    && slot.y >= 0.0
                    && slot.x + slot.width <= width + 0.01
                    && slot.y + slot.height <= height + 0.01,
                "{slot:?} reaches past the {width}x{height} container"
            );
        }
    }

    #[test]
    fn a_row_container_draws_the_largest_widget_its_shares_hold_and_chips_once_none_fits() {
        for (span, box_size, expected) in [
            (
                (12, 4),
                (1136.0, 368.0),
                Representation::Widget(WidgetSize::L),
            ),
            (
                (6, 2),
                (560.0, 176.0),
                Representation::Widget(WidgetSize::S),
            ),
            ((6, 1), (560.0, 80.0), Representation::Chip),
            ((3, 1), (272.0, 80.0), Representation::Chip),
        ] {
            let drawn = drawn(span.0, span.1);
            assert_eq!(drawn.len(), 3, "{span:?}: every child is drawn");
            let share = Size {
                width: (box_size.0 - 16.0) / 3.0,
                height: box_size.1,
            };
            for (representation, extent, slot) in &drawn {
                assert_eq!(*representation, expected, "{span:?}");
                let told = match expected {
                    Representation::Chip => chip_extent(share, 80.0),
                    _ => share,
                };
                assert_eq!(*extent, told, "{span:?}: the box it was told it has");
                assert!(
                    (slot.width - share.width).abs() < 1.0 && slot.height == share.height,
                    "{span:?}: {slot:?} is laid out at its share, to the pixel layout rounds to"
                );
            }
            assert_inside(&drawn, box_size);
        }
    }
}
