//! Where each child of a group that arranges its children sits, as resolution answers it.
//!
//! A file may leave a child's place out, or write one its group cannot hold — a `cell` past the inner grid after `cols` shrank. Both are answered here rather than refused, so a container whose inner grid shrinks keeps every child on it: a child with no cell takes the first free one in reading order, one past the inner grid is pulled back onto it, and a `free` child with no `rect` is put a step down from the one before.

use std::collections::BTreeSet;

use crate::model::{Arrange, ChildCell, Instance, Rect};

/// Where a child sits in a group that arranges its children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// Its share of a `row` or `column` group's length, against its siblings' weights.
    Weight(f32),
    /// The cells of a `grid` group's inner grid it covers, all of them on it.
    Cell(ChildCell),
    /// Its box in fractions of a `free` group's box.
    Rect(Rect),
}

/// How far each `free` child that names no `rect` is put from the one before it, as a fraction of the box.
const FREE_STEP: f32 = 0.05;

/// How many `free` children in a row step away from the top left corner before the next starts over there.
const FREE_STEPS: usize = 8;

/// Where each of `children` sits in a group arranged as `arrange`, with an inner grid of `cols` × `rows` where it is a `grid`. `None` for each child of a `pages` group or of one that arranges nothing.
pub fn placements(
    arrange: Option<Arrange>,
    (cols, rows): (u32, u32),
    children: &[&Instance],
) -> Vec<Option<Placement>> {
    match arrange {
        Some(Arrange::Row | Arrange::Column) => children
            .iter()
            .map(|child| Some(Placement::Weight(weight_of(child))))
            .collect(),
        Some(Arrange::Grid) => {
            let written: Vec<Option<ChildCell>> = children.iter().map(|child| child.cell).collect();
            cells(&written, cols, rows)
                .into_iter()
                .map(|cell| Some(Placement::Cell(cell)))
                .collect()
        }
        Some(Arrange::Free) => children
            .iter()
            .enumerate()
            .map(|(index, child)| Some(Placement::Rect(child.rect.unwrap_or(free_rect(index)))))
            .collect(),
        Some(Arrange::Pages) | None => vec![None; children.len()],
    }
}

/// A child's `weight`, or 1 where it names none or one that is not above 0.
fn weight_of(child: &Instance) -> f32 {
    child.weight.filter(|weight| *weight > 0.0).unwrap_or(1.0)
}

/// The cell each child covers on an inner grid of `cols` × `rows`, given the ones `written` names: each written cell pulled back onto the grid, then each child that names none on the first cell in reading order that no other child covers — or the top left one, where every cell is taken.
pub fn cells(written: &[Option<ChildCell>], cols: u32, rows: u32) -> Vec<ChildCell> {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let kept: Vec<Option<ChildCell>> = written
        .iter()
        .map(|cell| cell.map(|cell| kept_on(cell, cols, rows)))
        .collect();
    let mut covered: BTreeSet<(u32, u32)> = kept.iter().flatten().flat_map(covers).collect();
    kept.into_iter()
        .map(|cell| {
            cell.unwrap_or_else(|| {
                let free = (0..rows)
                    .flat_map(|row| (0..cols).map(move |col| (col, row)))
                    .find(|spot| !covered.contains(spot))
                    .unwrap_or((0, 0));
                covered.insert(free);
                ChildCell::at(free.0, free.1)
            })
        })
        .collect()
}

fn kept_on(cell: ChildCell, cols: u32, rows: u32) -> ChildCell {
    let col = cell.col.min(cols - 1);
    let row = cell.row.min(rows - 1);
    ChildCell {
        col,
        row,
        col_span: cell.col_span.clamp(1, cols - col),
        row_span: cell.row_span.clamp(1, rows - row),
    }
}

fn covers(cell: &ChildCell) -> impl Iterator<Item = (u32, u32)> {
    let (cols, rows) = (
        cell.col..cell.col + cell.col_span,
        cell.row..cell.row + cell.row_span,
    );
    rows.flat_map(move |row| cols.clone().map(move |col| (col, row)))
}

/// Where the `index`th child of a `free` group sits when it names no `rect`.
fn free_rect(index: usize) -> Rect {
    let step = FREE_STEP * (index % FREE_STEPS) as f32;
    Rect {
        x: step,
        y: step,
        w: 0.5,
        h: 0.5,
    }
}
