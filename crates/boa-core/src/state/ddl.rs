//! "DDL" on a table, the UI half: ported from `ui/src/state/ddl.ts` (no tests
//! in the source). Used by the explorer's context menu (Sidebar.tsx, "DDL")
//! and the table tab's toolbar button (Workspace.tsx).
//!
//! The DDL itself comes from the server: `Backend::table_ddl` (=
//! `crate::pg::catalog`'s `table_ddl`, for tables, views, materialized views
//! and foreign tables). The UI only opens it in a new console on that
//! database (`newQueryTab(connectionId, database, sql)`: the lowest free
//! `console_N` title, selecting the database) or shows an error toast.

use crate::error::CmdError;

use super::reconnect::{Toast, ToastKind};

/// The table whose DDL to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdlRequest {
    pub connection_id: String,
    pub database: String,
    pub schema: String,
    pub table: String,
}

/// What to do with `Backend::table_ddl`'s answer (`openDdl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdlAction {
    /// Open a new console on this connection and database holding `sql`.
    OpenConsole {
        connection_id: String,
        database: String,
        sql: String,
    },
    /// Show this error toast.
    Toast(Toast),
}

/// The error toast's title: "No DDL for {schema}.{table}".
pub fn ddl_error_title(schema: &str, table: &str) -> String {
    format!("No DDL for {schema}.{table}")
}

/// What `openDdl` does with `table_ddl`'s result.
pub fn ddl_action(req: &DdlRequest, result: Result<String, &CmdError>) -> DdlAction {
    match result {
        Ok(sql) => DdlAction::OpenConsole {
            connection_id: req.connection_id.clone(),
            database: req.database.clone(),
            sql,
        },
        Err(e) => DdlAction::Toast(
            Toast::new(ToastKind::Error, ddl_error_title(&req.schema, &req.table))
                .body(e.message.clone()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::reconnect::DEFAULT_TOAST_TTL_MS;

    fn req() -> DdlRequest {
        DdlRequest {
            connection_id: "c1".into(),
            database: "app".into(),
            schema: "public".into(),
            table: "users".into(),
        }
    }

    #[test]
    fn opens_the_ddl_in_a_new_console_on_that_database() {
        assert_eq!(
            ddl_action(&req(), Ok("CREATE TABLE public.users ();".into())),
            DdlAction::OpenConsole {
                connection_id: "c1".into(),
                database: "app".into(),
                sql: "CREATE TABLE public.users ();".into(),
            }
        );
    }

    #[test]
    fn a_failure_shows_an_error_toast_naming_the_table() {
        let err = CmdError::code("42P01", "relation \"public.users\" does not exist");
        let DdlAction::Toast(t) = ddl_action(&req(), Err(&err)) else {
            panic!("expected a toast");
        };
        assert_eq!(t.kind, ToastKind::Error);
        assert_eq!(t.title, "No DDL for public.users");
        assert_eq!(
            t.body.as_deref(),
            Some("relation \"public.users\" does not exist")
        );
        assert_eq!(t.key, None);
        assert_eq!(t.ttl_ms, DEFAULT_TOAST_TTL_MS);
    }
}
