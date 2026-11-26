//! Pending edits of a table data tab (DataGrip's staged changes) and the SQL
//! that applies them (`gridEdits.ts`). The grid keeps a [`Pending`] per tab,
//! its methods update it, and [`build_statements`] turns it into one
//! statement per changed row:
//!
//! ```text
//! UPDATE t SET c = '…' WHERE pk = '<original>'   edited row
//! INSERT INTO t (c, …) VALUES ('…', …)            new row (untouched columns omitted)
//! DELETE FROM t WHERE pk = '<original>'           deleted row
//! ```
//!
//! Values are the server's text rendering, so they go out as quoted string
//! literals and Postgres coerces them to the column type; NULL stays bare.
//! The WHERE always uses the row's original key values, so editing a key
//! column still finds the row.

use super::types::GridError;
use crate::model::{ResultColumn, Table, TableKind, Value};
use crate::sql::{qualified, quote_ident};
use std::collections::{BTreeMap, BTreeSet};

/// A row of the grid: a loaded row (index into the page's rows) or a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowRef {
    Base(usize),
    New(u64),
}

/// A row to insert; only the columns set in `values` are inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRow {
    pub id: u64,
    /// Column index → value.
    pub values: BTreeMap<usize, Value>,
}

/// A tab's staged changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// Loaded row index → column index → new value.
    pub updates: BTreeMap<usize, BTreeMap<usize, Value>>,
    /// Loaded row indices marked for deletion.
    pub deletes: BTreeSet<usize>,
    /// Rows to insert, in the order they were added.
    pub inserts: Vec<NewRow>,
    /// Next new-row id.
    pub next_id: u64,
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            updates: BTreeMap::new(),
            deletes: BTreeSet::new(),
            inserts: Vec::new(),
            next_id: 1,
        }
    }
}

/// How a row is tinted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowMark {
    Inserted,
    Deleted,
    Updated,
}

impl Pending {
    /// Changed rows: updated, deleted and new ones.
    pub fn change_count(&self) -> usize {
        let updated = self
            .updates
            .keys()
            .filter(|r| !self.deletes.contains(r))
            .count();
        updated + self.deletes.len() + self.inserts.len()
    }

    /// No changes.
    pub fn is_empty(&self) -> bool {
        self.change_count() == 0
    }

    /// A new row by id.
    pub fn new_row(&self, id: u64) -> Option<&NewRow> {
        self.inserts.iter().find(|r| r.id == id)
    }

    /// Sets a cell. Setting a loaded cell back to its original value drops
    /// the edit; cells of deleted or missing rows are left alone.
    pub fn set_cell(&mut self, base: &[Vec<Value>], row: RowRef, col: usize, value: Value) {
        match row {
            RowRef::New(id) => {
                if let Some(r) = self.inserts.iter_mut().find(|r| r.id == id) {
                    r.values.insert(col, value);
                }
            }
            RowRef::Base(index) => {
                let Some(original) = base.get(index).and_then(|r| r.get(col)) else {
                    return;
                };
                if self.deletes.contains(&index) {
                    return;
                }
                let edits = self.updates.entry(index).or_default();
                if &value == original {
                    edits.remove(&col);
                } else {
                    edits.insert(col, value);
                }
                if edits.is_empty() {
                    self.updates.remove(&index);
                }
            }
        }
    }

    /// Undoes one cell: a loaded cell gets its original value, a new row's
    /// cell its default.
    pub fn revert_cell(&mut self, row: RowRef, col: usize) {
        match row {
            RowRef::New(id) => {
                if let Some(r) = self.inserts.iter_mut().find(|r| r.id == id) {
                    r.values.remove(&col);
                }
            }
            RowRef::Base(index) => {
                if let Some(edits) = self.updates.get_mut(&index) {
                    edits.remove(&col);
                    if edits.is_empty() {
                        self.updates.remove(&index);
                    }
                }
            }
        }
    }

    /// Undoes everything on a row: edits and deletion, or drops a new row.
    pub fn revert_row(&mut self, row: RowRef) {
        match row {
            RowRef::New(id) => self.inserts.retain(|r| r.id != id),
            RowRef::Base(index) => {
                self.updates.remove(&index);
                self.deletes.remove(&index);
            }
        }
    }

    /// Marks a loaded row for deletion (its edits are dropped); a new row
    /// just goes away.
    pub fn delete_row(&mut self, row: RowRef) {
        match row {
            RowRef::New(_) => self.revert_row(row),
            RowRef::Base(index) => {
                self.updates.remove(&index);
                self.deletes.insert(index);
            }
        }
    }

    /// Adds an empty new row; returns its id.
    pub fn add_row(&mut self) -> u64 {
        let id = self.next_id;
        self.inserts.push(NewRow {
            id,
            values: BTreeMap::new(),
        });
        self.next_id += 1;
        id
    }

    /// How the row is tinted, if it changed.
    pub fn row_mark(&self, row: RowRef) -> Option<RowMark> {
        match row {
            RowRef::New(_) => Some(RowMark::Inserted),
            RowRef::Base(i) if self.deletes.contains(&i) => Some(RowMark::Deleted),
            RowRef::Base(i) => self.updates.contains_key(&i).then_some(RowMark::Updated),
        }
    }

    /// The cell holds a staged value.
    pub fn cell_edited(&self, row: RowRef, col: usize) -> bool {
        match row {
            RowRef::New(id) => self
                .new_row(id)
                .is_some_and(|r| r.values.contains_key(&col)),
            RowRef::Base(i) => {
                !self.deletes.contains(&i)
                    && self.updates.get(&i).is_some_and(|e| e.contains_key(&col))
            }
        }
    }
}

// ── Editability ───────────────────────────────────────────────────────────

/// Whether a table tab's rows can be edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Editability {
    /// Editable, by this primary key.
    Editable { pk: Vec<String> },
    /// Read-only, and why ("Read-only: views are not editable").
    ReadOnly { reason: &'static str },
}

/// Whether a table tab's rows can be edited: a table with a primary key,
/// all of it in the result (exactly once).
pub fn editability(table: Option<&Table>, columns: &[ResultColumn]) -> Editability {
    let read_only = |reason| Editability::ReadOnly { reason };
    let Some(table) = table else {
        return read_only("Read-only: table not introspected");
    };
    if table.kind == TableKind::View {
        return read_only("Read-only: views are not editable");
    }
    let pk: Vec<String> = table
        .columns
        .iter()
        .filter(|c| c.pk)
        .map(|c| c.name.clone())
        .collect();
    if pk.is_empty() {
        return read_only("Read-only: the table has no primary key");
    }
    if pk
        .iter()
        .any(|k| columns.iter().filter(|c| &c.name == k).count() != 1)
    {
        return read_only("Read-only: the primary key is not in the result");
    }
    Editability::Editable { pk }
}

// ── SQL ───────────────────────────────────────────────────────────────────

/// A string literal for `v`, or `NULL`. Backslashes switch to an `E''`
/// literal (with `\\`), which reads the same whatever
/// `standard_conforming_strings` is.
pub fn sql_literal(v: Option<&str>) -> String {
    let Some(v) = v else {
        return "NULL".into();
    };
    let quoted = v.replace('\'', "''");
    if v.contains('\\') {
        format!("E'{}'", quoted.replace('\\', "\\\\"))
    } else {
        format!("'{quoted}'")
    }
}

/// The table being edited and the result's shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditTarget {
    pub schema: String,
    pub table: String,
    /// Result columns, in grid order.
    pub columns: Vec<String>,
    /// Primary-key column names.
    pub pk: Vec<String>,
}

/// Column name → value, in column order.
pub type NamedValues = Vec<(String, Value)>;

/// What a statement does, by column name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowChange {
    Update {
        key: NamedValues,
        values: NamedValues,
    },
    Insert {
        values: NamedValues,
    },
    Delete {
        key: NamedValues,
    },
}

/// One generated statement. The backend's `apply_edits` takes
/// [`crate::edits::EditStatement`] (`sql` and `exactly_one`): convert with
/// `From` / `Into`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditStatement {
    pub sql: String,
    pub schema: String,
    pub table: String,
    /// Must affect exactly one row, or the whole submit rolls back.
    pub exactly_one: bool,
    pub change: RowChange,
}

impl From<&EditStatement> for crate::edits::EditStatement {
    fn from(s: &EditStatement) -> Self {
        Self {
            sql: s.sql.clone(),
            exactly_one: s.exactly_one,
        }
    }
}

impl From<EditStatement> for crate::edits::EditStatement {
    fn from(s: EditStatement) -> Self {
        Self {
            sql: s.sql,
            exactly_one: s.exactly_one,
        }
    }
}

/// Sets `name` in `values`, keeping the first position of a repeated name
/// (as assigning a JS object key does).
fn put(values: &mut NamedValues, name: &str, v: Value) {
    match values.iter_mut().find(|(k, _)| k == name) {
        Some(slot) => slot.1 = v,
        None => values.push((name.to_owned(), v)),
    }
}

fn key_of(target: &EditTarget, row: &[Value]) -> Result<NamedValues, GridError> {
    let mut key = NamedValues::new();
    for k in &target.pk {
        let Some(i) = target.columns.iter().position(|c| c == k) else {
            return Err(GridError::new(format!(
                "Primary key column {k} is not in the result"
            )));
        };
        put(&mut key, k, row.get(i).cloned().flatten());
    }
    Ok(key)
}

fn where_clause(key: &NamedValues) -> String {
    key.iter()
        .map(|(k, v)| match v {
            None => format!("{} IS NULL", quote_ident(k)),
            Some(v) => format!("{} = {}", quote_ident(k), sql_literal(Some(v))),
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The statements that apply `p` to the loaded `base` rows: deletes first
/// (so a new row may reuse a deleted key), then updates, then inserts, each
/// group in row order. Fails for a target without a primary key.
pub fn build_statements(
    target: &EditTarget,
    base: &[Vec<Value>],
    p: &Pending,
) -> Result<Vec<EditStatement>, GridError> {
    if target.pk.is_empty() {
        return Err(GridError::new(
            "A table without a primary key cannot be edited",
        ));
    }
    let table = qualified(&target.schema, &target.table);
    let statement = |sql: String, change| EditStatement {
        sql,
        schema: target.schema.clone(),
        table: target.table.clone(),
        exactly_one: true,
        change,
    };
    let mut out = Vec::new();

    for &r in &p.deletes {
        let Some(row) = base.get(r) else { continue };
        let key = key_of(target, row)?;
        out.push(statement(
            format!("DELETE FROM {table} WHERE {}", where_clause(&key)),
            RowChange::Delete { key },
        ));
    }

    for (&r, edits) in &p.updates {
        let Some(row) = base.get(r) else { continue };
        if p.deletes.contains(&r) {
            continue;
        }
        let mut values = NamedValues::new();
        for (&c, v) in edits {
            if c < target.columns.len() && row.get(c) != Some(v) {
                put(&mut values, &target.columns[c], v.clone());
            }
        }
        if values.is_empty() {
            continue;
        }
        let key = key_of(target, row)?;
        let set = values
            .iter()
            .map(|(k, v)| format!("{} = {}", quote_ident(k), sql_literal(v.as_deref())))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(statement(
            format!("UPDATE {table} SET {set} WHERE {}", where_clause(&key)),
            RowChange::Update { key, values },
        ));
    }

    for n in &p.inserts {
        let mut values = NamedValues::new();
        for (&c, v) in &n.values {
            if c < target.columns.len() {
                put(&mut values, &target.columns[c], v.clone());
            }
        }
        let sql = if values.is_empty() {
            format!("INSERT INTO {table} DEFAULT VALUES")
        } else {
            let names: Vec<String> = values.iter().map(|(k, _)| quote_ident(k)).collect();
            let literals: Vec<String> = values
                .iter()
                .map(|(_, v)| sql_literal(v.as_deref()))
                .collect();
            format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                names.join(", "),
                literals.join(", ")
            )
        };
        out.push(statement(sql, RowChange::Insert { values }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Column;

    fn v(s: &str) -> Value {
        Some(s.to_owned())
    }

    fn base() -> Vec<Vec<Value>> {
        vec![
            vec![v("1"), v("ada"), v("a@x.io")],
            vec![v("2"), v("o'brien"), None],
            vec![v("3"), v("grace"), v("g@x.io")],
        ]
    }

    fn target() -> EditTarget {
        EditTarget {
            schema: "public".into(),
            table: "users".into(),
            columns: vec!["id".into(), "name".into(), "email".into()],
            pk: vec!["id".into()],
        }
    }

    fn b(i: usize) -> RowRef {
        RowRef::Base(i)
    }

    fn sqls_with(p: &Pending, t: &EditTarget, rows: &[Vec<Value>]) -> Vec<String> {
        build_statements(t, rows, p)
            .unwrap()
            .into_iter()
            .map(|s| s.sql)
            .collect()
    }

    fn sqls(p: &Pending) -> Vec<String> {
        sqls_with(p, &target(), &base())
    }

    fn updates(entries: &[(usize, &[(usize, Value)])]) -> BTreeMap<usize, BTreeMap<usize, Value>> {
        entries
            .iter()
            .map(|(r, cols)| (*r, cols.iter().cloned().collect()))
            .collect()
    }

    fn named(pairs: &[(&str, Value)]) -> NamedValues {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    // ── sqlLiteral ──

    #[test]
    fn sql_literal_quotes_and_doubles_single_quotes() {
        assert_eq!(sql_literal(Some("abc")), "'abc'");
        assert_eq!(sql_literal(Some("it's")), "'it''s'");
        assert_eq!(sql_literal(Some("")), "''");
        assert_eq!(sql_literal(Some("''")), "''''''");
    }

    #[test]
    fn sql_literal_leaves_null_bare_and_distinct_from_the_text_null() {
        assert_eq!(sql_literal(None), "NULL");
        assert_eq!(sql_literal(Some("NULL")), "'NULL'");
    }

    #[test]
    fn sql_literal_uses_an_e_literal_when_there_are_backslashes() {
        assert_eq!(sql_literal(Some("C:\\dir")), "E'C:\\\\dir'");
        assert_eq!(sql_literal(Some("a\\'b")), "E'a\\\\''b'");
    }

    #[test]
    fn sql_literal_keeps_newlines_unicode_and_semicolons() {
        assert_eq!(
            sql_literal(Some("a\nb; drop table x; --")),
            "'a\nb; drop table x; --'"
        );
        assert_eq!(sql_literal(Some("héllo ✓")), "'héllo ✓'");
    }

    // ── pending state ──

    #[test]
    fn records_an_edit_and_drops_it_when_set_back() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(0), 1, v("lovelace"));
        assert_eq!(p.updates, updates(&[(0, &[(1, v("lovelace"))])]));
        assert!(p.cell_edited(b(0), 1));
        assert_eq!(p.row_mark(b(0)), Some(RowMark::Updated));
        p.set_cell(&base(), b(0), 1, v("ada"));
        assert!(p.updates.is_empty());
        assert_eq!(p.change_count(), 0);
    }

    #[test]
    fn treats_null_and_empty_string_as_different_values() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(1), 2, v(""));
        assert_eq!(p.updates, updates(&[(1, &[(2, v(""))])]));
        p.set_cell(&base(), b(1), 2, None);
        assert!(p.updates.is_empty());
        let mut q = Pending::default();
        q.set_cell(&base(), b(0), 2, None);
        assert_eq!(q.updates, updates(&[(0, &[(2, None)])]));
    }

    #[test]
    fn reverts_a_cell_and_a_row() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(0), 1, v("x"));
        p.set_cell(&base(), b(0), 2, v("y"));
        let mut one = p.clone();
        one.revert_cell(b(0), 1);
        assert_eq!(one.updates, updates(&[(0, &[(2, v("y"))])]));
        one.revert_cell(b(0), 2);
        assert!(one.updates.is_empty());
        let mut del = p.clone();
        del.delete_row(b(0));
        del.revert_row(b(0));
        assert!(del.updates.is_empty() && del.deletes.is_empty());
    }

    #[test]
    fn deleting_a_row_drops_its_edits_and_blocks_new_ones() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(2), 1, v("x"));
        p.delete_row(b(2));
        assert!(p.updates.is_empty());
        assert_eq!(p.row_mark(b(2)), Some(RowMark::Deleted));
        let before = p.clone();
        p.set_cell(&base(), b(2), 1, v("y"));
        assert_eq!(p, before);
        assert_eq!(p.change_count(), 1);
    }

    #[test]
    fn adds_edits_reverts_and_deletes_new_rows() {
        let mut p = Pending::default();
        let id = p.add_row();
        let r = RowRef::New(id);
        p.set_cell(&base(), r, 1, v("new"));
        assert_eq!(
            p.inserts,
            vec![NewRow {
                id,
                values: [(1, v("new"))].into()
            }]
        );
        assert!(p.cell_edited(r, 1));
        assert!(!p.cell_edited(r, 2));
        assert_eq!(p.row_mark(r), Some(RowMark::Inserted));
        let mut reverted = p.clone();
        reverted.revert_cell(r, 1);
        assert_eq!(
            reverted.inserts,
            vec![NewRow {
                id,
                values: BTreeMap::new()
            }]
        );
        assert_eq!(p.change_count(), 1);
        p.delete_row(r);
        assert!(p.inserts.is_empty());
        assert_eq!(p.add_row(), id + 1);
    }

    // ── buildStatements ──

    #[test]
    fn emits_nothing_for_no_changes() {
        assert!(sqls(&Pending::default()).is_empty());
    }

    #[test]
    fn updates_only_the_changed_columns_keyed_by_the_primary_key() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(1), 1, v("obrien"));
        p.set_cell(&base(), b(1), 2, v("o'b@x.io"));
        assert_eq!(
            sqls(&p),
            ["UPDATE public.users SET name = 'obrien', email = 'o''b@x.io' WHERE id = '2'"]
        );
        let s = &build_statements(&target(), &base(), &p).unwrap()[0];
        assert!(s.exactly_one);
        assert_eq!(
            s.change,
            RowChange::Update {
                key: named(&[("id", v("2"))]),
                values: named(&[("name", v("obrien")), ("email", v("o'b@x.io"))]),
            }
        );
    }

    #[test]
    fn sets_null_unquoted_and_empty_strings_as_quotes() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(0), 2, None);
        p.set_cell(&base(), b(2), 2, v(""));
        assert_eq!(
            sqls(&p),
            [
                "UPDATE public.users SET email = NULL WHERE id = '1'",
                "UPDATE public.users SET email = '' WHERE id = '3'",
            ]
        );
    }

    #[test]
    fn uses_the_original_key_when_the_key_itself_is_edited() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(0), 0, v("10"));
        assert_eq!(
            sqls(&p),
            ["UPDATE public.users SET id = '10' WHERE id = '1'"]
        );
    }

    #[test]
    fn handles_composite_keys_quoted_identifiers_and_key_edits() {
        let t = EditTarget {
            schema: "Sales".into(),
            table: "order items".into(),
            columns: vec![
                "order_id".into(),
                "Line".into(),
                "qty".into(),
                "select".into(),
            ],
            pk: vec!["order_id".into(), "Line".into()],
        };
        let rows = vec![vec![v("7"), v("1"), v("3"), v("x")]];
        let mut p = Pending::default();
        p.set_cell(&rows, b(0), 1, v("2"));
        p.set_cell(&rows, b(0), 3, v("y"));
        assert_eq!(
            sqls_with(&p, &t, &rows),
            [
                r#"UPDATE "Sales"."order items" SET "Line" = '2', "select" = 'y' WHERE order_id = '7' AND "Line" = '1'"#
            ]
        );
        let mut d = Pending::default();
        d.delete_row(b(0));
        assert_eq!(
            sqls_with(&d, &t, &rows),
            [r#"DELETE FROM "Sales"."order items" WHERE order_id = '7' AND "Line" = '1'"#]
        );
    }

    #[test]
    fn inserts_only_the_columns_that_were_set_or_default_values() {
        let mut p = Pending::default();
        let a = p.add_row();
        let _c = p.add_row();
        p.set_cell(&base(), RowRef::New(a), 2, v("x@y.z"));
        p.set_cell(&base(), RowRef::New(a), 1, None);
        assert_eq!(
            sqls(&p),
            [
                "INSERT INTO public.users (name, email) VALUES (NULL, 'x@y.z')",
                "INSERT INTO public.users DEFAULT VALUES",
            ]
        );
        assert_eq!(
            build_statements(&target(), &base(), &p).unwrap()[1].change,
            RowChange::Insert { values: vec![] }
        );
    }

    #[test]
    fn orders_deletes_then_updates_then_inserts() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(2), 1, v("g"));
        p.delete_row(b(1));
        let n = p.add_row();
        p.set_cell(&base(), RowRef::New(n), 0, v("2"));
        p.set_cell(&base(), b(0), 1, v("a"));
        assert_eq!(
            sqls(&p),
            [
                "DELETE FROM public.users WHERE id = '2'",
                "UPDATE public.users SET name = 'a' WHERE id = '1'",
                "UPDATE public.users SET name = 'g' WHERE id = '3'",
                "INSERT INTO public.users (id) VALUES ('2')",
            ]
        );
    }

    #[test]
    fn skips_no_op_updates_and_rows_missing_from_the_page() {
        let p = Pending {
            updates: updates(&[(0, &[(1, v("ada"))]), (9, &[(1, v("x"))])]),
            deletes: [9].into(),
            ..Pending::default()
        };
        assert!(sqls(&p).is_empty());
    }

    #[test]
    fn escapes_values_that_look_like_sql() {
        let mut p = Pending::default();
        p.set_cell(&base(), b(0), 1, v("'); drop table users; --"));
        assert_eq!(
            sqls(&p),
            ["UPDATE public.users SET name = '''); drop table users; --' WHERE id = '1'"]
        );
    }

    #[test]
    fn refuses_a_target_without_a_primary_key() {
        let t = EditTarget {
            pk: vec![],
            ..target()
        };
        assert!(build_statements(&t, &base(), &Pending::default()).is_err());
    }

    // ── editability ──

    fn col(name: &str, pk: bool) -> Column {
        Column {
            name: name.into(),
            ty: "text".into(),
            nullable: true,
            pk,
            default: None,
        }
    }

    fn table(columns: Vec<Column>, kind: TableKind) -> Table {
        Table {
            name: "t".into(),
            kind,
            rows: 0,
            size_bytes: 0,
            columns,
            indexes: vec![],
            constraints: vec![],
        }
    }

    fn rc(names: &[&str]) -> Vec<ResultColumn> {
        names
            .iter()
            .map(|n| ResultColumn::new(*n, "text"))
            .collect()
    }

    #[test]
    fn editability_allows_tables_with_a_primary_key_in_the_result() {
        let t = table(vec![col("id", true), col("v", false)], TableKind::Table);
        assert_eq!(
            editability(Some(&t), &rc(&["id", "v"])),
            Editability::Editable {
                pk: vec!["id".into()]
            }
        );
    }

    #[test]
    fn editability_explains_why_other_tabs_are_read_only() {
        assert!(matches!(
            editability(None, &rc(&["id"])),
            Editability::ReadOnly { .. }
        ));
        let view = table(vec![col("id", true)], TableKind::View);
        assert!(matches!(
            editability(Some(&view), &rc(&["id"])),
            Editability::ReadOnly { reason } if reason.contains("view")
        ));
        let no_pk = table(vec![col("v", false)], TableKind::Table);
        assert!(matches!(
            editability(Some(&no_pk), &rc(&["v"])),
            Editability::ReadOnly { reason } if reason.contains("primary key")
        ));
        let t = table(vec![col("id", true), col("v", false)], TableKind::Table);
        assert!(matches!(
            editability(Some(&t), &rc(&["v"])),
            Editability::ReadOnly { .. }
        ));
    }
}
