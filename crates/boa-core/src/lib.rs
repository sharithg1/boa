//! Boa's backend, without UI: Postgres sessions and query execution
//! (`pg`), saved connections, query history, SSH tunnels, and the async
//! bridge (`backend::Backend`) the GPUI app awaits. See README.md.
//! The catalog and result types (`model`) are shared by all of them.
//! Also the UI-independent logic ported from the React app: `sql`
//! (tokenizer, statements, completion, guards) and `grid` (values, copy-as,
//! CSV, staged edits, table queries, connection tags).

pub mod backend;
pub mod config;
pub mod connections;
pub mod edits;
pub mod error;
pub mod grid;
pub mod history;
pub mod model;
pub mod pg;
pub mod redis;
pub mod sql;
pub mod ssh;
pub mod state;
pub mod tx;
