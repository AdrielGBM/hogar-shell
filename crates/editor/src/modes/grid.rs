//! Where widgets go on a grid (TA-5, F-7): the cells each group covers, the room a grid has, and tolerant reflow.
//!
//! **Tolerant reflow, no compaction.** A group put on cells that others cover takes them, and each group it displaced moves to the free cells nearest where it was, one after another in reading order, each around everything already settled. Nothing else moves: a gap left anywhere stays a gap, the way a user left it, rather than the grid closing up as iOS does. Nothing is ever lost either — a group with no free cells inside the grid's room goes below everything instead.
//!
//! **Only explicit cells.** The groups placed here are the ones written at a cell (`place = "cell"`). One a layout leaves to auto-placement is the grid's to put in a free cell as it lays out (F-5.8), so it never overlaps one of these and is left alone.

use layout::{GroupId, GroupKind, ResolvedArea, ResolvedGroup};

/// The cells one group covers: where it starts and how many it spans each way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cells {
    pub col: u32,
    pub row: u32,
    pub cols: u32,
    pub rows: u32,
}

impl Cells {
    /// One cell, at the first.
    pub const ONE: Self = Self {
        col: 0,
        row: 0,
        cols: 1,
        rows: 1,
    };

    /// The same span, starting at `(col, row)`.
    pub fn at(self, col: u32, row: u32) -> Self {
        Self { col, row, ..self }
    }

    pub fn overlaps(&self, other: &Cells) -> bool {
        self.col < other.col + other.cols
            && other.col < self.col + self.cols
            && self.row < other.row + other.rows
            && other.row < self.row + self.rows
    }

    /// Whether it lies inside `room`.
    pub fn fits(&self, room: Room) -> bool {
        self.col + self.cols <= room.cols && self.row + self.rows <= room.rows
    }

    /// Moved as little as it takes to lie inside `room`, where it is small enough to.
    pub fn within(self, room: Room) -> Self {
        let clamp = |at: u32, span: u32, room: u32| at.min(room.saturating_sub(span));
        self.at(
            clamp(self.col, self.cols, room.cols),
            clamp(self.row, self.rows, room.rows),
        )
    }

    /// How far its start is from `(col, row)`, squared: what "nearest" is measured by.
    fn distance(&self, (col, row): (u32, u32)) -> u64 {
        let dx = i64::from(self.col) - i64::from(col);
        let dy = i64::from(self.row) - i64::from(row);
        (dx * dx + dy * dy) as u64
    }
}

/// How many cells a grid has room for each way inside its rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    pub cols: u32,
    pub rows: u32,
}


/// Every group of `area` placed at an explicit cell, with the cells it covers: its written span, widened to hold what is in it.
pub fn placed(area: &ResolvedArea) -> Vec<(GroupId, Cells)> {
    area.groups
        .iter()
        .filter_map(|group| Some((group.id.clone(), cells_of(group)?)))
        .collect()
}

/// The cells `group` covers, where it is placed at one.
pub fn cells_of(group: &ResolvedGroup) -> Option<Cells> {
    let GroupKind::Cell { col, row, .. } = group.kind else {
        return None;
    };
    let span = surfaces::area::cells_of(group);
    Some(Cells {
        col,
        row,
        cols: u32::from(span.columns),
        rows: u32::from(span.rows),
    })
}

/// The free cells nearest `near` that a group of `size` fits on without covering any of `taken`: inside `room` wherever there is space there, else in the first rows below everything.
pub fn nearest_free(taken: &[Cells], size: Cells, near: (u32, u32), room: Room) -> Cells {
    let free = |cells: &Cells| !taken.iter().any(|other| other.overlaps(cells));
    let nearest = |candidates: Vec<Cells>| {
        candidates
            .into_iter()
            .filter(free)
            .min_by_key(|cells| (cells.distance(near), cells.row, cells.col))
    };
    let widest = room.cols.saturating_sub(size.cols);
    let inside = (0..=room.rows.saturating_sub(size.rows))
        .flat_map(|row| (0..=widest).map(move |col| size.at(col, row)))
        .filter(|cells| cells.fits(room))
        .collect();
    if let Some(found) = nearest(inside) {
        return found;
    }
    let below = taken
        .iter()
        .map(|cells| cells.row + cells.rows)
        .max()
        .unwrap_or(0)
        .max(room.rows);
    let spill = (0..=below)
        .flat_map(|row| (0..=widest).map(move |col| size.at(col, row)))
        .collect();
    nearest(spill).unwrap_or_else(|| size.at(0, below))
}

/// A group by its id, on the cells it covers.
type Held = (GroupId, Cells);

/// Where every group is once `moved` is put on `to` (kept inside `room` where it fits), and every other group of `groups` it now covers moved to the free cells nearest where it was, one after another in reading order. `moved` need not be one of `groups` yet: a widget being added is placed the same way.
pub fn reflow(
    groups: &[(GroupId, Cells)],
    moved: &GroupId,
    to: Cells,
    room: Room,
) -> Vec<(GroupId, Cells)> {
    let to = to.within(room);
    let others: Vec<&Held> = groups.iter().filter(|(id, _)| id != moved).collect();
    let (mut displaced, kept): (Vec<&Held>, Vec<&Held>) = others
        .into_iter()
        .partition(|(_, cells)| cells.overlaps(&to));
    displaced.sort_by_key(|(_, cells)| (cells.row, cells.col));
    let mut settled: Vec<Cells> = kept.iter().map(|(_, cells)| *cells).collect();
    settled.push(to);
    let mut moved_to: Vec<(GroupId, Cells)> = Vec::with_capacity(displaced.len());
    for (id, was) in &displaced {
        let landed = nearest_free(&settled, *was, (was.col, was.row), room);
        settled.push(landed);
        moved_to.push((id.clone(), landed));
    }
    let mut placed: Vec<(GroupId, Cells)> = groups
        .iter()
        .map(|(id, cells)| {
            let now = match id == moved {
                true => to,
                false => moved_to
                    .iter()
                    .find(|(held, _)| held == id)
                    .map_or(*cells, |(_, landed)| *landed),
            };
            (id.clone(), now)
        })
        .collect();
    if !groups.iter().any(|(id, _)| id == moved) {
        placed.push((moved.clone(), to));
    }
    placed
}
