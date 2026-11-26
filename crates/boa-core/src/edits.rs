//! Submitting grid edits: the statements the data editor generated (one
//! `UPDATE` / `INSERT` / `DELETE` per changed row) run as one unit. Outside
//! a user transaction they get their own `BEGIN … COMMIT`; inside one
//! (`Session.tx`) they run under a savepoint, so a failure undoes only
//! them and leaves the user's transaction usable. In Manual mode a
//! transaction is opened first, as for a typed statement, so the edits stay
//! uncommitted until Commit. Any error, or a statement that must hit exactly
//! one row and doesn't, rolls everything back.

use crate::error::{CmdError, CmdResult};
use crate::tx::{self, TxCell, TxState};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio_postgres::Client;

const SAVEPOINT: &str = "__boa_edit";

/// One generated statement: what `apply_edits` needs of a
/// [`crate::grid::grid_edits::EditStatement`] (`From` converts). (The UI
/// sent more fields; they're ignored.)
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditStatement {
    pub sql: String,
    /// `UPDATE` / `DELETE` by primary key: anything but one row is an error.
    pub exactly_one: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRun {
    pub affected: u64,
    pub ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyFailure {
    /// Index of the statement that failed.
    pub index: usize,
    pub error: CmdError,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    /// The statements that ran (all of them on success), rolled back on failure.
    pub runs: Vec<StatementRun>,
    pub failed: Option<ApplyFailure>,
    /// Ran inside the user's open transaction (not committed yet).
    pub in_transaction: bool,
    /// The session's transaction state afterwards.
    pub tx: TxState,
    pub ms: u64,
}

/// How the batch is opened, kept and undone.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    open: String,
    keep: String,
    undo: String,
}

fn plan(in_tx: bool) -> Plan {
    if in_tx {
        Plan {
            open: format!("savepoint {SAVEPOINT}"),
            keep: format!("release savepoint {SAVEPOINT}"),
            undo: format!("rollback to savepoint {SAVEPOINT}; release savepoint {SAVEPOINT}"),
        }
    } else {
        Plan {
            open: "begin".into(),
            keep: "commit".into(),
            undo: "rollback".into(),
        }
    }
}

/// The error for a statement that touched the wrong number of rows.
fn check_affected(st: &EditStatement, affected: u64) -> Result<(), CmdError> {
    if !st.exactly_one || affected == 1 {
        return Ok(());
    }
    let verb = st
        .sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let what = match verb.as_str() {
        "update" => "updated",
        "delete" => "deleted",
        "insert" => "inserted",
        _ => "affected",
    };
    let mut e = CmdError::code(
        "row_count",
        format!("Expected 1 row to be {what}, but {affected} were"),
    );
    e.hint = Some(if affected == 0 {
        "The row may have been changed or deleted since it was loaded. Reload and try again.".into()
    } else {
        "The key matched several rows; nothing was changed.".into()
    });
    Err(e)
}

fn ms_since(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Runs `statements` as one unit on `client` (see the module docs); `tx`
/// is the session's transaction state, re-probed afterwards.
pub async fn apply(
    client: &Client,
    tx: &TxCell,
    manual: bool,
    statements: &[EditStatement],
) -> CmdResult<ApplyOutcome> {
    let res = run(client, tx, manual, statements).await;
    tx.set(tx::probe(client).await.state);
    res.map(|out| ApplyOutcome {
        tx: tx.get(),
        ..out
    })
}

async fn run(
    client: &Client,
    tx: &TxCell,
    manual: bool,
    statements: &[EditStatement],
) -> CmdResult<ApplyOutcome> {
    let started = Instant::now();
    if manual && tx.get() == TxState::Idle {
        client.batch_execute("begin").await?;
        tx.set(TxState::Active);
    }
    let user_tx = tx.get() != TxState::Idle;
    let p = plan(user_tx);
    client.batch_execute(&p.open).await?;
    // While our own transaction is open, other tabs' reads on this session
    // must not BEGIN/COMMIT around their cursor (that would commit us early).
    if !user_tx {
        tx.set(TxState::Active);
    }
    let mut runs = Vec::with_capacity(statements.len());
    let mut failed = None;
    for (index, st) in statements.iter().enumerate() {
        let t = Instant::now();
        let res = match client.execute(st.sql.as_str(), &[]).await {
            Ok(n) => check_affected(st, n).map(|()| n),
            Err(e) => Err(CmdError::from(e)),
        };
        match res {
            Ok(affected) => runs.push(StatementRun {
                affected,
                ms: ms_since(t),
            }),
            Err(error) => {
                failed = Some(ApplyFailure { index, error });
                break;
            }
        }
    }
    let end = if failed.is_some() { &p.undo } else { &p.keep };
    let ended = client.batch_execute(end).await;
    if let Err(e) = ended {
        // Couldn't commit (e.g. a deferred constraint): nothing was applied.
        if failed.is_none() {
            let _ = client.batch_execute("rollback").await;
            failed = Some(ApplyFailure {
                index: statements.len().saturating_sub(1),
                error: e.into(),
            });
        } else {
            return Err(e.into());
        }
    }
    Ok(ApplyOutcome {
        runs,
        failed,
        in_transaction: user_tx,
        tx: TxState::Idle, // filled in by `apply`
        ms: ms_since(started),
    })
}

#[cfg(test)]
mod tests {
    use super::{EditStatement, check_affected, plan};

    fn st(sql: &str, exactly_one: bool) -> EditStatement {
        EditStatement {
            sql: sql.into(),
            exactly_one,
        }
    }

    #[test]
    fn own_transaction_or_savepoint() {
        let own = plan(false);
        assert_eq!(
            (own.open.as_str(), own.keep.as_str(), own.undo.as_str()),
            ("begin", "commit", "rollback")
        );
        let nested = plan(true);
        assert!(nested.open.starts_with("savepoint "));
        assert!(nested.keep.starts_with("release savepoint "));
        assert!(nested.undo.starts_with("rollback to savepoint "));
        assert!(!nested.undo.contains("rollback;") && !nested.keep.contains("commit"));
    }

    #[test]
    fn exactly_one_row() {
        assert!(check_affected(&st("UPDATE t SET a = '1' WHERE id = '1'", true), 1).is_ok());
        assert!(check_affected(&st("INSERT INTO t DEFAULT VALUES", false), 3).is_ok());
        let e = check_affected(&st("UPDATE t SET a = '1' WHERE id = '1'", true), 0).unwrap_err();
        assert_eq!(e.code.as_deref(), Some("row_count"));
        assert_eq!(e.message, "Expected 1 row to be updated, but 0 were");
        assert!(e.hint.unwrap().contains("Reload"));
        let e = check_affected(&st("DELETE FROM t WHERE id = '1'", true), 2).unwrap_err();
        assert_eq!(e.message, "Expected 1 row to be deleted, but 2 were");
    }

    #[test]
    fn ignores_extra_fields() {
        let s: Vec<EditStatement> = serde_json::from_str(
            r#"[{"sql": "DELETE FROM t WHERE id = '1'", "exactlyOne": true, "change": {"kind": "delete"}}]"#,
        )
        .unwrap();
        assert!(s[0].exactly_one);
    }
}
