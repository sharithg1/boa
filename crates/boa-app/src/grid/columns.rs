//! Column geometry: initial widths (DataGrid.tsx's `initialWidth`), resizing
//! with its minimum, x offsets, hit-testing and the columns a viewport shows.
//! Plain `f32` pixels so it is unit-tested without GPUI.

use std::ops::Range;

use boa_core::model::{ResultColumn, Value};

/// Width of the sticky row-number column (the React grid's `<col width=48>`);
/// wider when the numbers need it (100,000 rows: six digits).
pub const ROW_NUMBER_WIDTH: f32 = 48.;
/// Advance of a Geist Mono digit at 11 px, plus the column's padding.
const DIGIT_WIDTH: f32 = 6.6;
const NUMBER_PADDING: f32 = 18.;
/// Narrowest a column can be dragged (`Math.max(48, …)`).
pub const MIN_WIDTH: f32 = 48.;
/// Initial widths are clamped to 80..=360 px.
pub const MIN_INITIAL_WIDTH: f32 = 80.;
pub const MAX_INITIAL_WIDTH: f32 = 360.;
/// Approximate advance of a Geist Mono character at 13 px, as the React grid assumes.
const CHAR_WIDTH: f32 = 7.8;
/// How many rows the initial width looks at.
const SAMPLE_ROWS: usize = 50;

/// The narrowest a header's name gets (one character and the ellipsis, or
/// the whole name when that is shorter): below it the type label gives way,
/// so a narrow sorted key column still reads `i…` next to its arrow and rank.
pub fn header_name_min_width(name: &str) -> f32 {
    name.chars().count().min(2) as f32 * CHAR_WIDTH
}

/// A column's starting width: wide enough for its header (name + type) and the
/// first 50 values (`<null>` for NULL), within 80..=360 px.
pub fn initial_width(column: &ResultColumn, rows: &[Vec<Value>], index: usize) -> f32 {
    let mut len = column.name.chars().count() + column.ty.chars().count() + 4;
    for row in rows.iter().take(SAMPLE_ROWS) {
        let n = match row.get(index) {
            Some(Some(v)) => v.chars().count(),
            Some(None) => "<null>".len(),
            None => 0,
        };
        len = len.max(n);
    }
    (len as f32 * CHAR_WIDTH + 24.).clamp(MIN_INITIAL_WIDTH, MAX_INITIAL_WIDTH)
}

/// The widths of a result's columns and their running offsets.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnLayout {
    widths: Vec<f32>,
    /// `starts[i]`: x of column `i` from the start of the data area (right
    /// of the row numbers); `starts[len]` is the total width.
    starts: Vec<f32>,
    /// Width of the row-number column.
    numbers: f32,
}

impl ColumnLayout {
    pub fn new(widths: Vec<f32>) -> Self {
        let mut layout = Self {
            widths,
            starts: Vec::new(),
            numbers: ROW_NUMBER_WIDTH,
        };
        layout.recompute();
        layout
    }

    /// Initial widths for a result.
    pub fn for_result(columns: &[ResultColumn], rows: &[Vec<Value>]) -> Self {
        Self::new(
            columns
                .iter()
                .enumerate()
                .map(|(i, c)| initial_width(c, rows, i))
                .collect(),
        )
    }

    fn recompute(&mut self) {
        self.starts.clear();
        self.starts.reserve(self.widths.len() + 1);
        let mut x = 0.;
        self.starts.push(x);
        for w in &self.widths {
            x += w;
            self.starts.push(x);
        }
    }

    pub fn len(&self) -> usize {
        self.widths.len()
    }

    /// Width of the sticky row-number column.
    pub fn numbers_width(&self) -> f32 {
        self.numbers
    }

    /// Widens the row-number column to fit numbers up to `last`.
    pub fn fit_row_numbers(&mut self, last: usize) {
        let digits = last.max(1).ilog10() as f32 + 1.;
        self.numbers = (digits * DIGIT_WIDTH + NUMBER_PADDING).max(ROW_NUMBER_WIDTH);
    }

    pub fn is_empty(&self) -> bool {
        self.widths.is_empty()
    }

    pub fn width(&self, col: usize) -> f32 {
        self.widths[col]
    }

    /// Left edge of a column, from the start of the data area.
    pub fn start(&self, col: usize) -> f32 {
        self.starts[col]
    }

    /// Sum of the column widths (without the row numbers).
    pub fn total(&self) -> f32 {
        self.starts.last().copied().unwrap_or(0.)
    }

    /// Sets a column's width, at least [`MIN_WIDTH`]. Returns the width set.
    pub fn resize(&mut self, col: usize, width: f32) -> f32 {
        let width = if width.is_finite() {
            width.max(MIN_WIDTH)
        } else {
            MIN_WIDTH
        };
        if let Some(w) = self.widths.get_mut(col) {
            *w = width;
            self.recompute();
        }
        width
    }

    /// The column at data-area x, if any.
    pub fn column_at(&self, x: f32) -> Option<usize> {
        if self.is_empty() || x < 0. || x >= self.total() {
            return None;
        }
        // starts is sorted: the last start <= x.
        let i = self.starts.partition_point(|&s| s <= x);
        Some(i - 1)
    }

    /// Like [`Self::column_at`], clamped to the first / last column (dragging
    /// past the edges).
    pub fn column_at_clamped(&self, x: f32) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        Some(
            self.column_at(x)
                .unwrap_or(if x < 0. { 0 } else { self.len() - 1 }),
        )
    }

    /// Columns overlapping data-area x in `from..to` (the visible ones).
    pub fn visible(&self, from: f32, to: f32) -> Range<usize> {
        if self.is_empty() || to <= from {
            return 0..0;
        }
        let first = self
            .starts
            .partition_point(|&s| s <= from.max(0.))
            .saturating_sub(1);
        let end = self.starts.partition_point(|&s| s < to).min(self.len());
        first.min(end)..end
    }

    /// The data-area scroll offset that brings `col` into a viewport `width`
    /// wide currently scrolled to `scroll` (minimal move; left edge wins when
    /// the column is wider than the viewport).
    pub fn scroll_to_reveal(&self, col: usize, scroll: f32, width: f32) -> f32 {
        let (left, right) = (self.start(col), self.start(col + 1));
        if left < scroll {
            left
        } else if right > scroll + width {
            (right - width).min(left)
        } else {
            scroll
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str) -> ResultColumn {
        ResultColumn::new(name, ty)
    }

    #[test]
    fn initial_widths_follow_header_and_values() {
        // "id" + "int4" + 4 = 10 chars -> 102 px.
        assert_eq!(initial_width(&col("id", "int4"), &[], 0), 10. * 7.8 + 24.);
        // Never below 80.
        assert_eq!(initial_width(&col("a", "b"), &[], 0), MIN_INITIAL_WIDTH);
        // Values widen it; NULL counts as "<null>".
        let rows = vec![vec![Some("x".repeat(20))], vec![None]];
        assert_eq!(initial_width(&col("a", "b"), &rows, 0), 20. * 7.8 + 24.);
        // Capped at 360.
        let rows = vec![vec![Some("x".repeat(500))]];
        assert_eq!(initial_width(&col("a", "b"), &rows, 0), MAX_INITIAL_WIDTH);
        // Only the first 50 rows count.
        let mut rows = vec![vec![Some("x".to_owned())]; 50];
        rows.push(vec![Some("x".repeat(100))]);
        assert_eq!(initial_width(&col("a", "b"), &rows, 0), MIN_INITIAL_WIDTH);
    }

    #[test]
    fn resize_clamps_to_the_minimum() {
        let mut l = ColumnLayout::new(vec![100., 100., 100.]);
        assert_eq!(l.resize(1, 10.), MIN_WIDTH);
        assert_eq!(l.width(1), MIN_WIDTH);
        assert_eq!(l.start(2), 100. + MIN_WIDTH);
        assert_eq!(l.total(), 200. + MIN_WIDTH);
        assert_eq!(l.resize(1, -50.), MIN_WIDTH);
        assert_eq!(l.resize(1, f32::NAN), MIN_WIDTH);
        assert_eq!(l.resize(0, 400.), 400.);
        assert_eq!(l.start(1), 400.);
        // No maximum when dragging.
        assert_eq!(l.resize(2, 5000.), 5000.);
        // Out of range: ignored.
        l.resize(9, 300.);
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn row_numbers_fit_their_digits() {
        let mut l = ColumnLayout::new(vec![]);
        assert_eq!(l.numbers_width(), ROW_NUMBER_WIDTH);
        l.fit_row_numbers(500);
        assert_eq!(l.numbers_width(), ROW_NUMBER_WIDTH);
        l.fit_row_numbers(100_000);
        assert!(l.numbers_width() > ROW_NUMBER_WIDTH);
        assert_eq!(l.numbers_width(), 6. * DIGIT_WIDTH + NUMBER_PADDING);
        l.fit_row_numbers(0);
        assert_eq!(l.numbers_width(), ROW_NUMBER_WIDTH);
    }

    #[test]
    fn hit_testing() {
        let l = ColumnLayout::new(vec![100., 50., 200.]);
        assert_eq!(l.column_at(-1.), None);
        assert_eq!(l.column_at(0.), Some(0));
        assert_eq!(l.column_at(99.9), Some(0));
        assert_eq!(l.column_at(100.), Some(1));
        assert_eq!(l.column_at(149.), Some(1));
        assert_eq!(l.column_at(150.), Some(2));
        assert_eq!(l.column_at(350.), None);
        assert_eq!(l.column_at_clamped(-10.), Some(0));
        assert_eq!(l.column_at_clamped(1000.), Some(2));
        assert_eq!(ColumnLayout::new(vec![]).column_at_clamped(5.), None);
    }

    #[test]
    fn visible_columns() {
        let l = ColumnLayout::new(vec![100.; 10]);
        assert_eq!(l.visible(0., 250.), 0..3);
        assert_eq!(l.visible(150., 300.), 1..3);
        assert_eq!(l.visible(150., 301.), 1..4);
        assert_eq!(l.visible(900., 2000.), 9..10);
        assert_eq!(l.visible(5000., 6000.), 10..10);
        assert_eq!(l.visible(0., 0.), 0..0);
    }

    #[test]
    fn reveal_scrolls_minimally() {
        let l = ColumnLayout::new(vec![100.; 10]);
        assert_eq!(l.scroll_to_reveal(1, 0., 300.), 0.);
        assert_eq!(l.scroll_to_reveal(5, 0., 300.), 300.);
        assert_eq!(l.scroll_to_reveal(0, 300., 300.), 0.);
        // Wider than the viewport: show its left edge.
        let wide = ColumnLayout::new(vec![100., 800.]);
        assert_eq!(wide.scroll_to_reveal(1, 0., 300.), 100.);
    }
}
