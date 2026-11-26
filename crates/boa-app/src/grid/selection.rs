//! The grid's selection model (DataGrid.tsx): an active cell, an optional
//! anchor (the other corner of a rectangular range) and, in place of a range,
//! whole rows picked by their row numbers (⌘A picks every row).
//!
//! Coordinates are view coordinates: row index into the loaded page, column
//! index into the result's columns. Pure state, no GPUI, so it is unit-tested
//! here and the view only maps mouse / key events onto these methods.

use std::collections::BTreeSet;
use std::ops::Range;

/// A cell: row and column index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

impl Cell {
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// Whole rows picked by their numbers, in place of a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    /// Every row (⌘A). Kept symbolic so 100k rows cost nothing.
    All,
    Rows(BTreeSet<usize>),
}

impl Picked {
    fn contains(&self, row: usize) -> bool {
        match self {
            Self::All => true,
            Self::Rows(rows) => rows.contains(&row),
        }
    }
}

/// The selected rows of a [`Region`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionRows {
    Span(Range<usize>),
    List(Vec<usize>),
}

impl RegionRows {
    pub fn len(&self) -> usize {
        match self {
            Self::Span(r) => r.len(),
            Self::List(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The rows, in ascending order.
    pub fn iter(&self) -> Box<dyn Iterator<Item = usize> + '_> {
        match self {
            Self::Span(r) => Box::new(r.clone()),
            Self::List(v) => Box::new(v.iter().copied()),
        }
    }
}

/// The selected rows × columns (a range, or picked rows × every column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub rows: RegionRows,
    pub cols: Range<usize>,
}

impl Region {
    /// Number of selected cells.
    pub fn cell_count(&self) -> usize {
        self.rows.len() * self.cols.len()
    }
}

/// How a mouse press on a row number combines with the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowClick {
    /// Plain click: just this row.
    Replace,
    /// ⇧-click: the run of rows from the anchor.
    Extend,
    /// ⌘-click: add or remove this row.
    Toggle,
}

/// What a drag started on, while the mouse button is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    Cells,
    Rows,
}

/// Selection state of one grid (DataGrid.tsx's `sel`, `anchor`, `picked`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    active: Option<Cell>,
    anchor: Option<Cell>,
    picked: Option<Picked>,
}

fn span(a: usize, b: usize) -> Range<usize> {
    a.min(b)..a.max(b) + 1
}

impl Selection {
    /// The active cell (keyboard focus, the cell an edit opens on).
    pub fn active(&self) -> Option<Cell> {
        self.active
    }

    pub fn anchor(&self) -> Option<Cell> {
        self.anchor
    }

    pub fn picked(&self) -> Option<&Picked> {
        self.picked.as_ref()
    }

    /// Makes `cell` the active one; `extend` keeps (or sets) the anchor, so
    /// the selection becomes the range between them (`focusCell`).
    pub fn focus_cell(&mut self, cell: Cell, extend: bool) {
        self.anchor = if extend {
            let kept = if self.picked.is_some() {
                None
            } else {
                self.anchor
            };
            Some(kept.or(self.active).unwrap_or(cell))
        } else {
            None
        };
        self.picked = None;
        self.active = Some(cell);
    }

    /// Mouse press on a cell (`cellDown`): ⇧ extends to it, otherwise it
    /// becomes the single selected cell and the anchor of a drag.
    pub fn press_cell(&mut self, cell: Cell, shift: bool) -> DragKind {
        if shift && self.active.is_some() {
            self.focus_cell(cell, true);
        } else {
            self.focus_cell(cell, false);
            self.anchor = Some(cell);
        }
        DragKind::Cells
    }

    /// Mouse press on a row number (`rowDown`). Returns the drag it starts.
    pub fn press_row(&mut self, row: usize, how: RowClick) -> Option<DragKind> {
        let col = self.active.map_or(0, |c| c.col);
        let mut drag = None;
        match (how, self.active) {
            (RowClick::Extend, Some(active)) => {
                let from = self.anchor.map_or(active.row, |a| a.row);
                self.picked = Some(Picked::Rows(span(from, row).collect()));
                self.anchor = self.anchor.or(Some(active));
            }
            (RowClick::Toggle, _) => {
                let mut base: BTreeSet<usize> = match &self.picked {
                    Some(Picked::Rows(rows)) => rows.clone(),
                    // Turning ⌘A's "every row" into a list would need the row
                    // count; the view resolves All before toggling.
                    Some(Picked::All) => BTreeSet::new(),
                    None => self.range_rows().map(|r| r.collect()).unwrap_or_default(),
                };
                if !base.remove(&row) {
                    base.insert(row);
                }
                self.picked = Some(Picked::Rows(base));
                self.anchor = Some(Cell::new(row, col));
            }
            _ => {
                self.picked = Some(Picked::Rows(BTreeSet::from([row])));
                self.anchor = Some(Cell::new(row, col));
                drag = Some(DragKind::Rows);
            }
        }
        self.active = Some(Cell::new(row, col));
        drag
    }

    /// Resolves ⌘A's "every row" into the list of `row_count` rows (before a
    /// ⌘-click removes one of them).
    pub fn expand_all(&mut self, row_count: usize) {
        if self.picked == Some(Picked::All) {
            self.picked = Some(Picked::Rows((0..row_count).collect()));
        }
    }

    /// The mouse, with the button down, is over `cell` (`dragOver`).
    pub fn drag_to(&mut self, kind: DragKind, cell: Cell) {
        match kind {
            DragKind::Cells => self.active = Some(cell),
            DragKind::Rows => {
                if let Some(anchor) = self.anchor {
                    self.picked = Some(Picked::Rows(span(anchor.row, cell.row).collect()));
                    let col = self.active.map_or(0, |c| c.col);
                    self.active = Some(Cell::new(cell.row, col));
                }
            }
        }
    }

    /// Arrow keys: moves the active cell by (`dr`, `dc`), clamped to the
    /// grid; `extend` (⇧) grows the range instead. Returns the new active cell.
    pub fn move_by(
        &mut self,
        dr: isize,
        dc: isize,
        extend: bool,
        row_count: usize,
        col_count: usize,
    ) -> Option<Cell> {
        let active = self.active?;
        if row_count == 0 || col_count == 0 {
            return None;
        }
        let clamp = |v: usize, d: isize, n: usize| v.saturating_add_signed(d).min(n - 1);
        let cell = Cell::new(
            clamp(active.row, dr, row_count),
            clamp(active.col, dc, col_count),
        );
        self.focus_cell(cell, extend);
        Some(cell)
    }

    /// ⌘A: every row (needs an active cell, like the React grid).
    pub fn select_all(&mut self) -> bool {
        if self.active.is_none() {
            return false;
        }
        self.anchor = None;
        self.picked = Some(Picked::All);
        true
    }

    /// Esc: back to the active cell alone. False when there was no range.
    pub fn collapse(&mut self) -> bool {
        if self.anchor.is_none() && self.picked.is_none() {
            return false;
        }
        self.anchor = None;
        self.picked = None;
        true
    }

    /// Forgets everything (a new result).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn range_rows(&self) -> Option<Range<usize>> {
        let active = self.active?;
        let a = self.anchor.unwrap_or(active);
        Some(span(a.row, active.row))
    }

    /// The selected rows and columns, limited to the grid's size.
    pub fn region(&self, row_count: usize, col_count: usize) -> Option<Region> {
        let active = self.active?;
        if let Some(picked) = &self.picked {
            let rows = match picked {
                Picked::All => RegionRows::Span(0..row_count),
                Picked::Rows(rows) => {
                    RegionRows::List(rows.iter().copied().filter(|&r| r < row_count).collect())
                }
            };
            return Some(Region {
                rows,
                cols: 0..col_count,
            });
        }
        let a = self.anchor.unwrap_or(active);
        let rows = span(a.row, active.row);
        let cols = span(a.col, active.col);
        Some(Region {
            rows: RegionRows::Span(rows.start.min(row_count)..rows.end.min(row_count)),
            cols: cols.start.min(col_count)..cols.end.min(col_count),
        })
    }

    /// More than one cell is selected (ranges are only tinted then). Called
    /// every frame, so it counts without building the region.
    pub fn is_multi(&self, row_count: usize, col_count: usize) -> bool {
        let Some(active) = self.active else {
            return false;
        };
        let cells = match &self.picked {
            Some(Picked::All) => row_count * col_count,
            Some(Picked::Rows(rows)) => {
                // At most two rows matter for "more than one cell".
                rows.range(..row_count).take(2).count() * col_count
            }
            None => {
                let a = self.anchor.unwrap_or(active);
                let rows = span(a.row, active.row);
                let cols = span(a.col, active.col);
                (rows.start.min(row_count)..rows.end.min(row_count)).len()
                    * (cols.start.min(col_count)..cols.end.min(col_count)).len()
            }
        };
        cells > 1
    }

    /// The cell is in the selection (`inRange`).
    pub fn contains(&self, cell: Cell) -> bool {
        let Some(active) = self.active else {
            return false;
        };
        if let Some(picked) = &self.picked {
            return picked.contains(cell.row);
        }
        let a = self.anchor.unwrap_or(active);
        span(a.row, active.row).contains(&cell.row) && span(a.col, active.col).contains(&cell.col)
    }

    /// The row is picked by its number, or crossed by the range in the
    /// active column (the row-number tint).
    pub fn row_marked(&self, row: usize) -> bool {
        match (&self.picked, self.active) {
            (Some(picked), _) => picked.contains(row),
            (None, Some(active)) => self.contains(Cell::new(row, active.col)),
            (None, None) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: usize = 100;
    const COLS: usize = 5;

    fn region(s: &Selection) -> (Vec<usize>, Range<usize>) {
        let r = s.region(ROWS, COLS).expect("a selection");
        (r.rows.iter().collect(), r.cols)
    }

    #[test]
    fn click_selects_one_cell() {
        let mut s = Selection::default();
        assert_eq!(s.region(ROWS, COLS), None);
        assert_eq!(s.press_cell(Cell::new(3, 2), false), DragKind::Cells);
        assert_eq!(s.active(), Some(Cell::new(3, 2)));
        assert_eq!(region(&s), (vec![3], 2..3));
        assert!(!s.is_multi(ROWS, COLS));
        assert!(s.contains(Cell::new(3, 2)));
        assert!(!s.contains(Cell::new(3, 1)));
    }

    #[test]
    fn shift_click_extends_from_the_anchor() {
        let mut s = Selection::default();
        s.press_cell(Cell::new(3, 2), false);
        s.press_cell(Cell::new(6, 0), true);
        assert_eq!(s.active(), Some(Cell::new(6, 0)));
        assert_eq!(region(&s), (vec![3, 4, 5, 6], 0..3));
        assert!(s.is_multi(ROWS, COLS));
        assert!(s.contains(Cell::new(5, 1)));
        assert!(!s.contains(Cell::new(7, 1)));
        // A second ⇧-click keeps the original anchor.
        s.press_cell(Cell::new(1, 4), true);
        assert_eq!(region(&s), (vec![1, 2, 3], 2..5));
        // ⇧-click without an active cell is a plain click.
        let mut fresh = Selection::default();
        fresh.press_cell(Cell::new(2, 2), true);
        assert_eq!(region(&fresh), (vec![2], 2..3));
    }

    #[test]
    fn drag_extends_the_range() {
        let mut s = Selection::default();
        let kind = s.press_cell(Cell::new(10, 1), false);
        s.drag_to(kind, Cell::new(12, 3));
        assert_eq!(region(&s), (vec![10, 11, 12], 1..4));
        s.drag_to(kind, Cell::new(8, 1));
        assert_eq!(region(&s), (vec![8, 9, 10], 1..2));
    }

    #[test]
    fn shift_arrows_extend_and_arrows_collapse() {
        let mut s = Selection::default();
        assert_eq!(s.move_by(1, 0, false, ROWS, COLS), None);
        s.press_cell(Cell::new(0, 0), false);
        s.move_by(1, 0, true, ROWS, COLS);
        s.move_by(0, 1, true, ROWS, COLS);
        assert_eq!(region(&s), (vec![0, 1], 0..2));
        // Clamped at the edges.
        assert_eq!(s.move_by(-5, -5, true, ROWS, COLS), Some(Cell::new(0, 0)));
        assert_eq!(
            s.move_by(500, 500, false, ROWS, COLS),
            Some(Cell::new(ROWS - 1, COLS - 1))
        );
        assert_eq!(region(&s), (vec![ROWS - 1], COLS - 1..COLS));
    }

    #[test]
    fn cmd_a_picks_every_row_and_esc_collapses() {
        let mut s = Selection::default();
        assert!(!s.select_all(), "⌘A needs an active cell");
        s.press_cell(Cell::new(4, 2), false);
        assert!(s.select_all());
        let r = s.region(ROWS, COLS).unwrap();
        assert_eq!(r.rows, RegionRows::Span(0..ROWS));
        assert_eq!(r.cols, 0..COLS);
        assert_eq!(r.cell_count(), ROWS * COLS);
        assert!(s.contains(Cell::new(99, 0)));
        assert!(s.row_marked(50));
        assert!(s.collapse());
        assert_eq!(region(&s), (vec![4], 2..3));
        assert_eq!(s.active(), Some(Cell::new(4, 2)));
        assert!(!s.collapse(), "nothing left to collapse");
    }

    #[test]
    fn esc_after_a_range_keeps_the_active_cell() {
        let mut s = Selection::default();
        s.press_cell(Cell::new(1, 1), false);
        s.press_cell(Cell::new(3, 3), true);
        assert!(s.collapse());
        assert_eq!(region(&s), (vec![3], 3..4));
    }

    #[test]
    fn row_numbers_pick_rows() {
        let mut s = Selection::default();
        s.press_cell(Cell::new(0, 2), false);
        assert_eq!(s.press_row(5, RowClick::Replace), Some(DragKind::Rows));
        assert_eq!(region(&s), (vec![5], 0..COLS));
        assert_eq!(s.active(), Some(Cell::new(5, 2)));
        // ⇧-click: the run from the anchor.
        s.press_row(8, RowClick::Extend);
        assert_eq!(region(&s), (vec![5, 6, 7, 8], 0..COLS));
        // ⌘-click toggles one.
        s.press_row(6, RowClick::Toggle);
        assert_eq!(region(&s), (vec![5, 7, 8], 0..COLS));
        s.press_row(20, RowClick::Toggle);
        assert_eq!(region(&s), (vec![5, 7, 8, 20], 0..COLS));
        assert!(s.row_marked(20));
        assert!(!s.row_marked(6));
        // Dragging over row numbers picks the run from the anchor.
        s.press_row(2, RowClick::Replace);
        s.drag_to(DragKind::Rows, Cell::new(4, 0));
        assert_eq!(region(&s), (vec![2, 3, 4], 0..COLS));
        // Clicking a cell drops the picked rows.
        s.press_cell(Cell::new(9, 1), false);
        assert_eq!(region(&s), (vec![9], 1..2));
    }

    #[test]
    fn cmd_click_after_cmd_a_removes_one_row() {
        let mut s = Selection::default();
        s.press_cell(Cell::new(0, 0), false);
        s.select_all();
        s.expand_all(4);
        s.press_row(2, RowClick::Toggle);
        assert_eq!(
            s.region(4, 2).unwrap().rows.iter().collect::<Vec<_>>(),
            [0, 1, 3]
        );
    }

    #[test]
    fn region_is_clamped_to_the_grid() {
        let mut s = Selection::default();
        s.press_cell(Cell::new(1, 1), false);
        s.press_cell(Cell::new(50, 4), true);
        let r = s.region(10, 3).unwrap();
        assert_eq!(r.rows, RegionRows::Span(1..10));
        assert_eq!(r.cols, 1..3);
    }
}
