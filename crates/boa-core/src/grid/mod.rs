//! Value and result-grid logic, ported from the React UI's pure TypeScript
//! modules (`ui/src/lib/` and `ui/src/state/edits.ts` of the Tauri app):
//! value kinds and JSON, display formatting, CSV, "Copy as", staged grid
//! edits and the SQL that applies them, a table tab's SELECT, table
//! metadata folders and connection tags. No UI or I/O dependencies.

pub mod conn_tag;
pub mod copy_as;
pub mod csv;
pub mod edits;
pub mod format;
pub mod grid_edits;
pub mod table_meta;
pub mod table_query;
pub mod types;
pub mod values;

mod js;

pub use crate::model::{
    Column, Constraint, ConstraintKind, ForeignRef, Index, QueryResult, ResultColumn, Table,
    TableKind, Value,
};
pub use types::GridError;
