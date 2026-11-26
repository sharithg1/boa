//! The catalog and result shapes shared by the whole crate: what the
//! backend returns (`pg`: `Pg::query` / `Backend::run_query`,
//! `introspect`) is what completion (`sql`) and the grid logic (`grid`:
//! copy-as, edits, table metadata) take, without conversion.
//!
//! They serialize (camelCase) as the Tauri app's IPC did: `Table::kind` is
//! `"table"` / `"view"`, `Constraint::kind` the `contype` letter.

use crate::tx::TxState;
use serde::Serialize;

/// One cell: Postgres's text rendering of the value, or `None` for NULL.
pub type Value = Option<String>;

/// A result column: its name and the driver's type name (`int4`, `_text`…).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

impl ResultColumn {
    pub fn new(name: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
        }
    }
}

/// A page of a query's result (`Backend::run_query`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    /// Rows affected (or returned) as the server reported it.
    pub affected: Option<u64>,
    /// More rows existed after this page.
    pub truncated: bool,
    /// Read-only cursor statement: other pages can be fetched by re-running
    /// it with an offset. False for DML (`INSERT … RETURNING`), which must
    /// never be re-run.
    pub pageable: bool,
    pub ms: u64,
    /// Transaction state after the statement.
    pub tx: TxState,
}

/// A schema and the tables / views in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub name: String,
    pub tables: Vec<Table>,
}

/// Table or view (materialized views count as views). Serializes as
/// `"table"` / `"view"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TableKind {
    #[default]
    Table,
    View,
}

impl TableKind {
    /// From `pg_class.relkind`: `v` and `m` are views, anything else a table.
    pub fn from_relkind(relkind: &str) -> Self {
        if relkind == "v" || relkind == "m" {
            Self::View
        } else {
            Self::Table
        }
    }

    /// `"table"` or `"view"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::View => "view",
        }
    }
}

/// A table or view with its columns and (tables) keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub name: String,
    pub kind: TableKind,
    /// Approximate row count (planner estimate).
    pub rows: i64,
    pub size_bytes: i64,
    pub columns: Vec<Column>,
    /// Tables and materialized views: every index, the primary key's too.
    pub indexes: Vec<Index>,
    /// Tables: primary key, unique, foreign key, check and exclusion constraints.
    pub constraints: Vec<Constraint>,
}

/// A column of a table or view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// Type as Postgres formats it (`integer`, `character varying(20)`…).
    #[serde(rename = "type")]
    pub ty: String,
    pub nullable: bool,
    /// Part of the primary key.
    pub pk: bool,
    pub default: Option<String>,
}

/// An index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub name: String,
    /// Key columns in order: a column name, or the expression for an
    /// expression column (`lower(email)`). INCLUDE columns are left out.
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    /// `CREATE [UNIQUE] INDEX … ON … USING …` (`pg_get_indexdef`).
    pub definition: String,
}

/// `pg_constraint.contype`. Serializes as the letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ConstraintKind {
    /// `p`
    #[serde(rename = "p")]
    PrimaryKey,
    /// `u`
    #[serde(rename = "u")]
    Unique,
    /// `f`
    #[serde(rename = "f")]
    ForeignKey,
    /// `c`
    #[serde(rename = "c")]
    Check,
    /// `x`
    #[serde(rename = "x")]
    Exclusion,
}

impl ConstraintKind {
    /// The `contype` letter: `p`, `u`, `f`, `c` or `x`.
    pub fn code(self) -> char {
        match self {
            Self::PrimaryKey => 'p',
            Self::Unique => 'u',
            Self::ForeignKey => 'f',
            Self::Check => 'c',
            Self::Exclusion => 'x',
        }
    }

    /// From a `contype` letter (`p`, `u`, `f`, `c`, `x`).
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "p" => Self::PrimaryKey,
            "u" => Self::Unique,
            "f" => Self::ForeignKey,
            "c" => Self::Check,
            "x" => Self::Exclusion,
            _ => return None,
        })
    }
}

/// A table constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Constraint {
    pub name: String,
    pub kind: ConstraintKind,
    /// `PRIMARY KEY (id)`, `FOREIGN KEY (org_id) REFERENCES orgs(id)`, …
    /// (`pg_get_constraintdef`).
    pub definition: String,
    /// The constrained columns (`conkey`), in order.
    pub columns: Vec<String>,
    /// Foreign keys only: the referenced table and columns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<ForeignRef>,
}

/// The target of a foreign key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignRef {
    pub schema: String,
    pub table: String,
    /// Referenced columns, in the order of the constraint's local columns.
    pub columns: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_serialize_as_before() {
        for k in [
            ConstraintKind::PrimaryKey,
            ConstraintKind::Unique,
            ConstraintKind::ForeignKey,
            ConstraintKind::Check,
            ConstraintKind::Exclusion,
        ] {
            let code = k.code().to_string();
            assert_eq!(ConstraintKind::from_code(&code), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), code.as_str());
        }
        assert_eq!(ConstraintKind::from_code("n"), None);
        for (relkind, kind) in [
            ("r", TableKind::Table),
            ("p", TableKind::Table),
            ("f", TableKind::Table),
            ("v", TableKind::View),
            ("m", TableKind::View),
        ] {
            assert_eq!(TableKind::from_relkind(relkind), kind);
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
        let t = serde_json::to_value(Table {
            name: "t".into(),
            columns: vec![Column {
                name: "id".into(),
                ty: "int".into(),
                ..Column::default()
            }],
            ..Table::default()
        })
        .unwrap();
        assert_eq!(t["kind"], "table");
        assert_eq!(t["sizeBytes"], 0);
        assert_eq!(t["columns"][0]["type"], "int");
    }
}
