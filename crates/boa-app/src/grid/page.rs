//! A loaded page of a result plus the grid state that lives with it: column
//! widths, the local sort, selection and the staged edits
//! (`boa_core::grid::grid_edits::Pending`). Pure logic (view order, values
//! as shown, copy text, edit staging), unit-tested here.
//!
//! Rows are addressed in *view* coordinates, as DataGrid.tsx does: the
//! loaded rows in their (maybe locally sorted) order, without those the
//! table tab's "Filter loaded rows" hides (TableEdits.tsx `rows`), then the
//! new rows of the staged edits. [`Page::row_ref`] maps a view row to the
//! [`RowRef`] the edits use.

use std::borrow::Cow;
use std::sync::Arc;

use boa_core::grid::copy_as::{
    CopyColumn, CopyFormat, CopyKey, CopySelection, copy_as, copy_plain, unavailable,
};
use boa_core::grid::grid_edits::{Pending, RowMark, RowRef};
use boa_core::grid::values::{ValueKind, value_kind};
use boa_core::model::{QueryResult, ResultColumn, Value};

use super::columns::ColumnLayout;
use super::selection::{Cell, Region, Selection};
use super::sort::{LocalSort, next_sort, sorted_order};

/// Longest text drawn in a cell; the rest could never be seen (cells are at
/// most a few hundred px wide unless dragged) and shaping it costs time.
const DISPLAY_CHARS: usize = 1000;

/// What a new row shows in a column it doesn't set (`placeholder`).
pub const DEFAULT_PLACEHOLDER: &str = "<default>";

/// What a copy put on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    pub text: String,
    /// The toast title: "Copied value" / "Copied 3 rows" / "Copied 2 rows as JSON".
    pub title: String,
}

/// What a cell shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown<'a> {
    Null,
    Text(&'a str),
    /// A new row's untouched column (`<default>`).
    Default,
}

/// Table-tab context for "Copy as" (DataGrid's `table` and `pk` props).
#[derive(Debug, Clone, Copy, Default)]
pub struct CopyTarget<'a> {
    /// Qualified, quoted table name (INSERT / UPDATE target).
    pub table: Option<&'a str>,
    /// Primary-key column names.
    pub pk: &'a [String],
}

/// One page of a result as the grid shows it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The statement that produced it.
    pub sql: String,
    pub result: Arc<QueryResult>,
    pub kinds: Vec<ValueKind>,
    /// Rows skipped before this page (row numbers start at offset + 1).
    pub offset: usize,
    /// `prefs.rowLimit` the page was fetched with.
    pub page_size: usize,
    pub pending: Pending,
    pub layout: ColumnLayout,
    pub selection: Selection,
    sort: Option<LocalSort>,
    /// "Filter loaded rows", trimmed and lowercased (empty: every row).
    filter: String,
    /// Columns a new row shows `<default>` in until set (the table's
    /// columns with a default); empty: none.
    defaults: Vec<bool>,
    /// View row → loaded row while sorted or filtered (None: loaded order).
    order: Option<Vec<usize>>,
}

impl Page {
    pub fn new(sql: String, result: Arc<QueryResult>, offset: usize, page_size: usize) -> Self {
        let kinds = result.columns.iter().map(|c| value_kind(&c.ty)).collect();
        let mut layout = ColumnLayout::for_result(&result.columns, &result.rows);
        layout.fit_row_numbers(offset + result.rows.len());
        Self {
            sql,
            result,
            kinds,
            offset,
            page_size,
            pending: Pending::default(),
            layout,
            selection: Selection::default(),
            sort: None,
            filter: String::new(),
            defaults: Vec::new(),
            order: None,
        }
    }

    pub fn columns(&self) -> &[ResultColumn] {
        &self.result.columns
    }

    /// The loaded rows (without staged edits).
    pub fn rows(&self) -> &[Vec<Value>] {
        &self.result.rows
    }

    /// Loaded rows (whatever the filter hides).
    pub fn loaded_count(&self) -> usize {
        self.result.rows.len()
    }

    /// Loaded rows in the view (those the filter keeps).
    pub fn base_count(&self) -> usize {
        self.order.as_ref().map_or(self.result.rows.len(), Vec::len)
    }

    /// Rows in the view: the loaded ones plus the new ones.
    pub fn row_count(&self) -> usize {
        self.base_count() + self.pending.inserts.len()
    }

    pub fn col_count(&self) -> usize {
        self.result.columns.len()
    }

    /// The same columns (names and types) as `other`: widths carry over.
    pub fn same_columns(&self, other: &[ResultColumn]) -> bool {
        self.columns() == other
    }

    // ── View order ────────────────────────────────────────────────────────

    /// The local sort (console results).
    pub fn sort(&self) -> Option<LocalSort> {
        self.sort
    }

    /// Header click on a console result: asc → desc → loaded order. Drops
    /// a range / picked rows (their rows move), keeps the active cell.
    pub fn cycle_sort(&mut self, col: usize) {
        if col >= self.col_count() {
            return;
        }
        self.set_sort(next_sort(self.sort, col));
    }

    pub fn set_sort(&mut self, sort: Option<LocalSort>) {
        self.selection.collapse();
        self.sort = sort;
        self.refresh_order();
    }

    /// The "Filter loaded rows" text (as given; matched trimmed, ignoring case).
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Table tabs' "Filter loaded rows" (Workspace.tsx): only loaded rows with
    /// a value (staged edits included) containing the text, ignoring case,
    /// stay in the view; new rows always do. The selection stays where it
    /// was, inside the rows.
    pub fn set_filter(&mut self, filter: &str) {
        let filter = filter.trim().to_lowercase();
        if filter == self.filter {
            return;
        }
        self.filter = filter;
        self.refresh_order();
        self.clamp_selection();
    }

    /// Recomputes the view order from the sort and the filter.
    fn refresh_order(&mut self) {
        let sorted = self
            .sort
            .map(|s| sorted_order(self.rows(), s, self.kinds[s.col]));
        self.order = if self.filter.is_empty() {
            sorted
        } else {
            let keep = |i: &usize| self.matches_filter(*i);
            Some(match sorted {
                Some(order) => order.into_iter().filter(keep).collect(),
                None => (0..self.loaded_count()).filter(keep).collect(),
            })
        };
    }

    /// Loaded row `i`, with its staged values, has a value containing the filter.
    fn matches_filter(&self, i: usize) -> bool {
        let Some(row) = self.rows().get(i) else {
            return false;
        };
        let staged = self.pending.updates.get(&i);
        row.iter().enumerate().any(|(c, v)| {
            let v = staged.and_then(|s| s.get(&c)).unwrap_or(v);
            v.as_deref()
                .is_some_and(|v| v.to_lowercase().contains(&self.filter))
        })
    }

    /// The staged edits changed: rows may enter or leave the filtered view.
    fn edits_changed(&mut self) {
        if !self.filter.is_empty() {
            self.refresh_order();
        }
        self.clamp_selection();
    }

    /// Marks the columns named in `defaults` as having a default.
    pub fn set_column_defaults(&mut self, defaults: &[String]) {
        self.defaults = self
            .columns()
            .iter()
            .map(|c| defaults.contains(&c.name))
            .collect();
    }

    /// The column has a default (a new row shows `<default>` until it is set).
    pub fn has_default(&self, col: usize) -> bool {
        self.defaults.get(col).copied().unwrap_or(false)
    }

    /// The row a view row stands for.
    pub fn row_ref(&self, view: usize) -> Option<RowRef> {
        let base = self.base_count();
        if view < base {
            Some(RowRef::Base(
                self.order.as_ref().map_or(view, |order| order[view]),
            ))
        } else {
            self.pending
                .inserts
                .get(view - base)
                .map(|row| RowRef::New(row.id))
        }
    }

    /// The view row of a [`RowRef`] (after a sort, a loaded row moves).
    pub fn view_of(&self, row: RowRef) -> Option<usize> {
        match row {
            RowRef::Base(i) => match &self.order {
                None => (i < self.loaded_count()).then_some(i),
                Some(order) => order.iter().position(|&o| o == i),
            },
            RowRef::New(id) => self
                .pending
                .inserts
                .iter()
                .position(|r| r.id == id)
                .map(|i| self.base_count() + i),
        }
    }

    /// The number of view row `view` (DataGrid's `firstRow + r`): from the
    /// page's offset + 1, or from 1 while filtered.
    pub fn row_number(&self, view: usize) -> usize {
        let first = if self.filter.is_empty() {
            self.offset + 1
        } else {
            1
        };
        first + view
    }

    /// The number in the row-number column: `+` for new rows.
    pub fn row_label(&self, view: usize) -> String {
        if view >= self.base_count() {
            "+".to_owned()
        } else {
            self.row_number(view).to_string()
        }
    }

    // ── Values ────────────────────────────────────────────────────────────

    /// What a cell shows: the staged value if edited, else the loaded one;
    /// a new row's untouched column is `<default>` when the column has a
    /// default, else NULL.
    pub fn shown(&self, view: usize, col: usize) -> Shown<'_> {
        fn as_shown(v: &Value) -> Shown<'_> {
            v.as_deref().map_or(Shown::Null, Shown::Text)
        }
        match self.row_ref(view) {
            Some(RowRef::Base(i)) => {
                if let Some(staged) = self.pending.updates.get(&i).and_then(|r| r.get(&col)) {
                    return as_shown(staged);
                }
                self.rows()
                    .get(i)
                    .and_then(|r| r.get(col))
                    .map_or(Shown::Null, as_shown)
            }
            Some(RowRef::New(id)) => {
                match self.pending.new_row(id).and_then(|r| r.values.get(&col)) {
                    Some(v) => as_shown(v),
                    None if self.has_default(col) => Shown::Default,
                    None => Shown::Null,
                }
            }
            None => Shown::Null,
        }
    }

    /// The value shown in a cell (`<default>` counts as NULL).
    pub fn value(&self, view: usize, col: usize) -> Option<&str> {
        match self.shown(view, col) {
            Shown::Text(v) => Some(v),
            Shown::Null | Shown::Default => None,
        }
    }

    /// The rows as the view shows them (TableEdits.tsx `rows`: the filter
    /// applied, staged values, new rows with NULL where untouched), for
    /// "Copy page as CSV".
    pub fn shown_rows(&self) -> Vec<Vec<Value>> {
        (0..self.row_count())
            .map(|r| {
                (0..self.col_count())
                    .map(|c| self.value(r, c).map(str::to_owned))
                    .collect()
            })
            .collect()
    }

    /// A new row's untouched column that has a default.
    pub fn is_default(&self, view: usize, col: usize) -> bool {
        self.shown(view, col) == Shown::Default
    }

    pub fn row_mark(&self, view: usize) -> Option<RowMark> {
        self.pending.row_mark(self.row_ref(view)?)
    }

    pub fn cell_edited(&self, cell: Cell) -> bool {
        self.row_ref(cell.row)
            .is_some_and(|r| self.pending.cell_edited(r, cell.col))
    }

    /// Rows can be edited unless deleted (`canEdit`).
    pub fn can_edit(&self, view: usize) -> bool {
        view < self.row_count() && self.row_mark(view) != Some(RowMark::Deleted)
    }

    // ── Staging ───────────────────────────────────────────────────────────

    /// Stages a new value for a cell (setting it back to the loaded value
    /// drops the edit). Deleted rows are left alone.
    pub fn stage(&mut self, cell: Cell, value: Value) {
        if let Some(row) = self.row_ref(cell.row) {
            let rows = &self.result.rows;
            self.pending.set_cell(rows, row, cell.col, value);
            self.edits_changed();
        }
    }

    /// ⌘⌥Z: reverts the cell if it is edited, else the whole row.
    pub fn revert(&mut self, cell: Cell) {
        let Some(row) = self.row_ref(cell.row) else {
            return;
        };
        if self.pending.cell_edited(row, cell.col) {
            self.pending.revert_cell(row, cell.col);
        } else {
            self.pending.revert_row(row);
        }
        self.edits_changed();
    }

    pub fn revert_cell(&mut self, cell: Cell) {
        if let Some(row) = self.row_ref(cell.row) {
            self.pending.revert_cell(row, cell.col);
            self.edits_changed();
        }
    }

    pub fn revert_row(&mut self, view: usize) {
        if let Some(row) = self.row_ref(view) {
            self.pending.revert_row(row);
            self.edits_changed();
        }
    }

    /// ⌘⌫: marks the row for deletion (a new row just goes away).
    pub fn delete_row(&mut self, view: usize) {
        if let Some(row) = self.row_ref(view) {
            self.pending.delete_row(row);
            self.edits_changed();
        }
    }

    /// ⌘N: adds a new row at the end; returns its view row.
    pub fn add_row(&mut self) -> usize {
        self.pending.add_row();
        self.row_count() - 1
    }

    /// Replaces the staged edits (the owner restoring a tab's edits).
    pub fn set_pending(&mut self, pending: Pending) {
        self.pending = pending;
        self.edits_changed();
    }

    /// Takes the staged edits, leaving none.
    pub fn take_pending(&mut self) -> Pending {
        let taken = std::mem::take(&mut self.pending);
        self.edits_changed();
        taken
    }

    /// Keeps the selection inside the rows (new rows can go away).
    pub fn clamp_selection(&mut self) {
        let rows = self.row_count();
        let Some(active) = self.selection.active() else {
            return;
        };
        if active.row < rows {
            return;
        }
        if rows == 0 {
            self.selection.clear();
        } else {
            self.selection
                .focus_cell(Cell::new(rows - 1, active.col), false);
        }
    }

    // ── Copying ───────────────────────────────────────────────────────────

    fn region(&self) -> Option<Region> {
        let region = self.selection.region(self.row_count(), self.col_count())?;
        (!region.rows.is_empty() && !region.cols.is_empty()).then_some(region)
    }

    /// The selected columns (for the "Copy as" menu: which formats apply).
    pub fn selected_columns(&self) -> Option<Vec<CopyColumn>> {
        let region = self.region()?;
        Some(
            region
                .cols
                .map(|c| CopyColumn::new(&self.columns()[c].name, &self.columns()[c].ty))
                .collect(),
        )
    }

    /// Why `format` can't copy the selection (None: it can). Only the shape
    /// matters (columns, table, key), as DataGrid's `selection(false)`.
    pub fn copy_unavailable(
        &self,
        format: CopyFormat,
        target: CopyTarget<'_>,
    ) -> Option<&'static str> {
        let columns = self.selected_columns()?;
        let key_cols = self.key_columns(target.pk);
        let key_columns: Vec<CopyColumn> = key_cols
            .iter()
            .flatten()
            .map(|&c| CopyColumn::new(&self.columns()[c].name, &self.columns()[c].ty))
            .collect();
        let selection = CopySelection {
            table: target.table,
            key: key_cols.as_ref().map(|_| CopyKey {
                columns: &key_columns,
                rows: &[],
            }),
            ..CopySelection::new(&columns, &[])
        };
        unavailable(format, &selection)
    }

    /// Primary-key column indices, when every key column is in the result.
    fn key_columns(&self, pk: &[String]) -> Option<Vec<usize>> {
        let idx: Option<Vec<usize>> = pk
            .iter()
            .map(|k| self.columns().iter().position(|c| &c.name == k))
            .collect();
        idx.filter(|i| !i.is_empty())
    }

    /// ⌘C: TSV of the displayed values (staged edits included; a single
    /// cell is its raw text), via `copy_as::copy_plain`.
    pub fn copy_selection(&self) -> Option<Copied> {
        self.copy(None, CopyTarget::default()).and_then(Result::ok)
    }

    /// "Copy as ›" `format` (or ⌘C's TSV with None). Err: why it failed.
    pub fn copy(
        &self,
        format: Option<CopyFormat>,
        target: CopyTarget<'_>,
    ) -> Option<Result<Copied, String>> {
        let region = self.region()?;
        let view_rows: Vec<usize> = region.rows.iter().collect();
        let cols: Vec<usize> = region.cols.clone().collect();
        let columns: Vec<CopyColumn> = cols
            .iter()
            .map(|&c| CopyColumn::new(&self.columns()[c].name, &self.columns()[c].ty))
            .collect();
        let row_values = |cols: &[usize]| -> Vec<Vec<Value>> {
            view_rows
                .iter()
                .map(|&r| {
                    cols.iter()
                        .map(|&c| self.value(r, c).map(str::to_owned))
                        .collect()
                })
                .collect()
        };
        let rows = row_values(&cols);
        let key_cols = self.key_columns(target.pk);
        let key_columns: Vec<CopyColumn> = key_cols
            .iter()
            .flatten()
            .map(|&c| CopyColumn::new(&self.columns()[c].name, &self.columns()[c].ty))
            .collect();
        let key_rows = key_cols.as_deref().map(row_values).unwrap_or_default();
        let is_default = |i: usize, j: usize| self.is_default(view_rows[i], cols[j]);
        let has_new_rows = !self.pending.inserts.is_empty();
        let selection = CopySelection {
            table: target.table,
            key: key_cols.as_ref().map(|_| CopyKey {
                columns: &key_columns,
                rows: &key_rows,
            }),
            is_default: has_new_rows.then_some(&is_default as &dyn Fn(usize, usize) -> bool),
            ..CopySelection::new(&columns, &rows)
        };
        let text = match format {
            None => copy_plain(&selection),
            Some(format) => match copy_as(format, &selection) {
                Ok(text) => text,
                Err(err) => return Some(Err(err.to_string())),
            },
        };
        let n = rows.len();
        let title = match format {
            None if n == 1 && columns.len() == 1 => "Copied value".to_owned(),
            Some(CopyFormat::SqlIn) => format!("Copied {} as an SQL IN list", plural(n, "value")),
            Some(format) => format!("Copied {} as {}", plural(n, "row"), format.name()),
            None => format!("Copied {}", plural(n, "row")),
        };
        Some(Ok(Copied { text, title }))
    }
}

/// `1,234` (the React app's `toLocaleString()` in an English locale).
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `1 row`, `1,234 rows`.
pub fn plural(n: usize, word: &str) -> String {
    format!("{} {word}{}", grouped(n), if n == 1 { "" } else { "s" })
}

/// A value as one line of cell text: line breaks and tabs become spaces (as
/// the React grid's `white-space: nowrap` collapses them), cut at 1,000 chars.
pub fn cell_text(value: &str) -> Cow<'_, str> {
    let cut = value
        .char_indices()
        .nth(DISPLAY_CHARS)
        .map_or(value, |(i, _)| &value[..i]);
    if cut.contains(['\n', '\r', '\t']) {
        Cow::Owned(cut.replace(['\n', '\r', '\t'], " "))
    } else {
        Cow::Borrowed(cut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_core::tx::TxState;

    fn result(rows: Vec<Vec<Value>>) -> QueryResult {
        QueryResult {
            columns: vec![
                ResultColumn::new("id", "int4"),
                ResultColumn::new("name", "text"),
                ResultColumn::new("note", "text"),
            ],
            rows,
            affected: None,
            truncated: false,
            pageable: true,
            ms: 3,
            tx: TxState::default(),
        }
    }

    fn s(v: &str) -> Value {
        Some(v.to_owned())
    }

    fn page() -> Page {
        Page::new(
            "select".into(),
            Arc::new(result(vec![
                vec![s("1"), s("ann"), None],
                vec![s("2"), s("bob"), s("a\tb")],
                vec![s("3"), s("cy"), s("x")],
            ])),
            0,
            500,
        )
    }

    fn select(p: &mut Page, a: (usize, usize), b: (usize, usize)) {
        p.selection.press_cell(Cell::new(a.0, a.1), false);
        p.selection.press_cell(Cell::new(b.0, b.1), true);
    }

    #[test]
    fn tsv_of_a_range() {
        let mut p = page();
        assert_eq!(p.copy_selection(), None);
        select(&mut p, (0, 0), (1, 2));
        let copied = p.copy_selection().unwrap();
        // NULL is an empty field; a tab inside a value is quoted (copy_plain's CSV rules).
        assert_eq!(copied.text, "1\tann\t\n2\tbob\t\"a\tb\"\n");
        assert_eq!(copied.title, "Copied 2 rows");
    }

    #[test]
    fn one_cell_copies_its_text() {
        let mut p = page();
        p.selection.press_cell(Cell::new(1, 1), false);
        let copied = p.copy_selection().unwrap();
        assert_eq!(copied.text, "bob");
        assert_eq!(copied.title, "Copied value");
        // A NULL cell copies as nothing.
        p.selection.press_cell(Cell::new(0, 2), false);
        assert_eq!(p.copy_selection().unwrap().text, "");
    }

    #[test]
    fn cmd_a_copies_every_row() {
        let mut p = page();
        p.selection.press_cell(Cell::new(0, 0), false);
        p.selection.select_all();
        let copied = p.copy_selection().unwrap();
        assert_eq!(copied.text.lines().count(), 3);
        assert_eq!(copied.title, "Copied 3 rows");
    }

    #[test]
    fn copy_as_titles_and_formats() {
        let mut p = page();
        select(&mut p, (0, 0), (1, 1));
        let t = CopyTarget::default();
        let json = p.copy(Some(CopyFormat::Json), t).unwrap().unwrap();
        assert_eq!(json.title, "Copied 2 rows as JSON");
        assert!(json.text.contains("\"name\": \"ann\""));
        let insert = p.copy(Some(CopyFormat::SqlInsert), t).unwrap().unwrap();
        assert!(
            insert
                .text
                .starts_with("INSERT INTO MY_TABLE (id, name) VALUES (1, 'ann');")
        );
        // IN list: one column only.
        assert_eq!(
            p.copy_unavailable(CopyFormat::SqlIn, t),
            Some("Select a single column")
        );
        assert!(p.copy(Some(CopyFormat::SqlIn), t).unwrap().is_err());
        select(&mut p, (0, 1), (2, 1));
        let list = p.copy(Some(CopyFormat::SqlIn), t).unwrap().unwrap();
        assert_eq!(list.text, "'ann', 'bob', 'cy'\n");
        assert_eq!(list.title, "Copied 3 values as an SQL IN list");
        // UPDATE needs a table with a key.
        assert!(p.copy_unavailable(CopyFormat::SqlUpdate, t).is_some());
        let pk = vec!["id".to_owned()];
        let table = CopyTarget {
            table: Some("public.people"),
            pk: &pk,
        };
        assert_eq!(p.copy_unavailable(CopyFormat::SqlUpdate, table), None);
        let update = p.copy(Some(CopyFormat::SqlUpdate), table).unwrap().unwrap();
        assert_eq!(
            update.text.lines().next(),
            Some("UPDATE public.people SET name = 'ann' WHERE id = 1;")
        );
    }

    #[test]
    fn staged_edits_show_and_copy() {
        let mut p = page();
        let cell = Cell::new(1, 1);
        p.stage(cell, s("robert"));
        assert_eq!(p.value(1, 1), Some("robert"));
        assert!(p.cell_edited(cell));
        assert_eq!(p.row_mark(1), Some(RowMark::Updated));
        assert_eq!(p.pending.change_count(), 1);
        p.selection.press_cell(cell, false);
        assert_eq!(p.copy_selection().unwrap().text, "robert");
        // NULL is not an empty string.
        p.stage(Cell::new(0, 1), None);
        assert_eq!(p.value(0, 1), None);
        assert_eq!(p.pending.change_count(), 2);
        // Setting the original value back drops the edit.
        p.stage(cell, s("bob"));
        assert!(!p.cell_edited(cell));
        assert_eq!(p.row_mark(1), None);
        assert_eq!(p.pending.change_count(), 1);
    }

    #[test]
    fn revert_and_delete() {
        let mut p = page();
        p.stage(Cell::new(2, 1), s("z"));
        p.stage(Cell::new(2, 2), s("z"));
        p.revert(Cell::new(2, 1));
        assert!(!p.cell_edited(Cell::new(2, 1)));
        assert!(p.cell_edited(Cell::new(2, 2)));
        // On an unedited cell it reverts the row.
        p.revert(Cell::new(2, 0));
        assert_eq!(p.row_mark(2), None);
        p.delete_row(0);
        assert_eq!(p.row_mark(0), Some(RowMark::Deleted));
        assert!(!p.can_edit(0));
        // Deleted rows can't be edited.
        p.stage(Cell::new(0, 1), s("q"));
        assert_eq!(p.value(0, 1), Some("ann"));
        p.revert(Cell::new(0, 1));
        assert!(p.can_edit(0));
        p.delete_row(99);
        assert!(p.pending.is_empty());
    }

    #[test]
    fn new_rows_show_defaults_only_where_the_column_has_one() {
        let mut p = page();
        p.set_column_defaults(&["id".to_owned()]);
        let row = p.add_row();
        assert_eq!(row, 3);
        assert_eq!(p.row_count(), 4);
        assert_eq!(p.row_label(row), "+");
        assert_eq!(p.row_label(0), "1");
        assert_eq!(p.row_mark(row), Some(RowMark::Inserted));
        // TableEdits.tsx `placeholder`: <default> only in a column with a
        // default; the others are NULL (DataGrid shows <null>).
        assert_eq!(p.shown(row, 0), Shown::Default);
        assert!(p.is_default(row, 0));
        assert_eq!(p.shown(row, 1), Shown::Null);
        assert!(!p.is_default(row, 1));
        assert_eq!(p.shown(row, 2), Shown::Null);
        p.stage(Cell::new(row, 1), s("dee"));
        assert_eq!(p.shown(row, 1), Shown::Text("dee"));
        assert!(p.cell_edited(Cell::new(row, 1)));
        select(&mut p, (row, 0), (row, 2));
        let insert = p
            .copy(Some(CopyFormat::SqlInsert), CopyTarget::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            insert.text,
            "INSERT INTO MY_TABLE (id, name, note) VALUES (DEFAULT, 'dee', NULL);\n"
        );
        // Deleting a new row drops it; the selection moves back inside.
        p.delete_row(row);
        assert_eq!(p.row_count(), 3);
        assert_eq!(p.selection.active(), Some(Cell::new(2, 2)));
        assert!(p.pending.is_empty());
    }

    #[test]
    fn without_defaults_a_new_row_is_all_null() {
        let mut p = page();
        let row = p.add_row();
        for col in 0..3 {
            assert_eq!(p.shown(row, col), Shown::Null);
        }
        // Columns are matched by name; unknown names change nothing.
        p.set_column_defaults(&["missing".to_owned(), "note".to_owned()]);
        assert_eq!(p.shown(row, 2), Shown::Default);
        assert_eq!(p.shown(row, 0), Shown::Null);
    }

    #[test]
    fn the_row_filter_keeps_matching_rows_and_numbers_them_from_one() {
        let mut p = Page::new(
            "select".into(),
            Arc::new(result(vec![
                vec![s("1"), s("Ann"), None],
                vec![s("2"), s("bob"), s("a\tb")],
                vec![s("3"), s("cy"), s("ANNEX")],
            ])),
            500,
            500,
        );
        assert_eq!(p.row_label(0), "501");
        // Trimmed, any column, ignoring case.
        p.set_filter("  ann ");
        assert_eq!(p.filter(), "ann");
        assert_eq!(p.base_count(), 2);
        assert_eq!(p.loaded_count(), 3);
        assert_eq!(p.row_ref(0), Some(RowRef::Base(0)));
        assert_eq!(p.row_ref(1), Some(RowRef::Base(2)));
        assert_eq!(p.view_of(RowRef::Base(1)), None);
        assert_eq!(p.view_of(RowRef::Base(2)), Some(1));
        // Numbered from 1 while filtered (`firstRow={filter ? 1 : offset + 1}`).
        assert_eq!(p.row_label(1), "2");
        assert_eq!(p.row_number(1), 2);
        // NULL never matches.
        p.set_filter("null");
        assert_eq!(p.base_count(), 0);
        // New rows always show, after the filtered rows.
        let row = p.add_row();
        assert_eq!(row, 0);
        assert_eq!(p.row_label(row), "+");
        p.set_filter("");
        assert_eq!(p.base_count(), 3);
        assert_eq!(p.row_count(), 4);
        assert_eq!(p.row_label(0), "501");
    }

    #[test]
    fn the_row_filter_follows_staged_values_and_copies_what_it_shows() {
        let mut p = page();
        p.set_filter("bo");
        assert_eq!(p.base_count(), 1);
        p.selection.press_cell(Cell::new(0, 1), false);
        // Editing the only match away: the row leaves the view, the selection stays inside.
        p.stage(Cell::new(0, 1), s("zed"));
        assert_eq!(p.base_count(), 0);
        assert_eq!(p.selection.active(), None);
        // A staged value that matches brings a row in.
        p.set_filter("ze");
        assert_eq!(p.base_count(), 1);
        assert_eq!(p.value(0, 1), Some("zed"));
        p.revert_cell(Cell::new(0, 1));
        assert_eq!(p.base_count(), 0);
        // What "Copy page as CSV" gets: the rows shown, new ones included.
        p.set_filter("c");
        p.add_row();
        assert_eq!(
            p.shown_rows(),
            vec![vec![s("3"), s("cy"), s("x")], vec![None, None, None]]
        );
        // A local sort and the filter together.
        p.set_filter("");
        p.take_pending();
        p.cycle_sort(1);
        p.cycle_sort(1);
        p.set_filter("b");
        assert_eq!(p.row_ref(0), Some(RowRef::Base(1)));
        assert_eq!(p.base_count(), 1);
    }

    #[test]
    fn local_sort_reorders_the_view_and_keeps_edits_on_their_rows() {
        let mut p = page();
        p.stage(Cell::new(0, 1), s("zed"));
        select(&mut p, (0, 0), (1, 1));
        p.cycle_sort(1);
        // Range dropped, active cell kept.
        assert_eq!(p.selection.anchor(), None);
        // By name ascending: bob, cy, zed (the edit is on the loaded row 0... sorted by loaded values).
        assert_eq!(p.row_ref(0), Some(RowRef::Base(0)));
        assert_eq!(p.value(0, 1), Some("zed"));
        assert_eq!(p.value(1, 1), Some("bob"));
        assert_eq!(p.view_of(RowRef::Base(2)), Some(2));
        p.cycle_sort(1);
        assert_eq!(p.value(0, 1), Some("cy"));
        assert_eq!(p.row_label(0), "1");
        p.cycle_sort(1);
        assert_eq!(p.sort(), None);
        assert_eq!(p.value(0, 1), Some("zed"));
    }

    #[test]
    fn numbers_and_text() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(100_000), "100,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(plural(1, "row"), "1 row");
        assert_eq!(plural(2500, "row"), "2,500 rows");
        assert_eq!(cell_text("a\nb\tc"), "a b c");
        assert!(matches!(cell_text("plain"), Cow::Borrowed("plain")));
        assert_eq!(cell_text(&"é".repeat(2000)).chars().count(), 1000);
    }
}
