//! Port of \`ui/src/state/tx.ts\` (+ \`tx.test.ts\`).
//!
//! Transaction mode and state per session. A session is one (connection,
//! database) pair on the backend, shared by every console and table tab on
//! that database, so this is keyed like the workspaces (\`db_key\`). The state
//! is whatever the backend reported after the session's last statement
//! (\`QueryResult.tx\`, \`ApplyOutcome.tx\` / \`CmdError.tx\`): the app feeds every
//! backend outcome, from a console, a table tab or submitted grid edits,
//! through [\`TxSessions::note_outcome\`] (what \`trackTx\` did by wrapping the
//! backend). Connecting or disconnecting drops a connection's sessions
//! (back to Auto, idle): [\`TxSessions::forget\`].

pub use crate::tx::TxState;
use indexmap::IndexMap;

/// Tx: Auto (each statement commits) or Manual (a transaction stays open
/// until Commit / Rollback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TxMode {
    #[default]
    Auto,
    Manual,
}

/// One session's mode and state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct TxSession {
    pub mode: TxMode,
    pub state: TxState,
}

/// A session nobody has reported on: Auto, idle.
pub const IDLE_SESSION: TxSession = TxSession {
    mode: TxMode::Auto,
    state: TxState::Idle,
};

/// A partial update of a [\`TxSession\`] (\`Partial<TxSession>\`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TxPatch {
    pub mode: Option<TxMode>,
    pub state: Option<TxState>,
}

impl TxPatch {
    pub fn state(state: TxState) -> Self {
        Self {
            mode: None,
            state: Some(state),
        }
    }
    pub fn mode(mode: TxMode) -> Self {
        Self {
            mode: Some(mode),
            state: None,
        }
    }
}

/// A session with a transaction open (or aborted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTx {
    pub connection_id: String,
    pub database: String,
    /// \`Active\` or \`Failed\`, never \`Idle\`.
    pub state: TxState,
}

fn key(connection_id: &str, database: &str) -> String {
    super::session::db_key(connection_id, database)
}

/// Every session's transaction mode and state (\`useTx().sessions\`), in the
/// order they were first reported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TxSessions {
    sessions: IndexMap<String, TxSession>,
}

impl TxSessions {
    /// A session's mode and state (\`txOf\`); [\`IDLE_SESSION\`] when unknown.
    pub fn get(&self, connection_id: &str, database: &str) -> TxSession {
        self.sessions
            .get(&key(connection_id, database))
            .copied()
            .unwrap_or(IDLE_SESSION)
    }

    /// Applies \`patch\` (\`noteTx\`). True when something changed (a first
    /// report is a change even when it equals [\`IDLE_SESSION\`]).
    pub fn note(&mut self, connection_id: &str, database: &str, patch: TxPatch) -> bool {
        let k = key(connection_id, database);
        let prev = self.sessions.get(&k).copied();
        let base = prev.unwrap_or(IDLE_SESSION);
        let next = TxSession {
            mode: patch.mode.unwrap_or(base.mode),
            state: patch.state.unwrap_or(base.state),
        };
        if prev == Some(next) {
            return false;
        }
        self.sessions.insert(k, next);
        true
    }

    /// Drops a connection's sessions (its server sessions are gone): after
    /// connect and disconnect (whether they succeed or fail) and after a
    /// connection is deleted (\`forgetTx\`). True when any were dropped.
    pub fn forget(&mut self, connection_id: &str) -> bool {
        let prefix = format!("{connection_id}\u{0}");
        let before = self.sessions.len();
        self.sessions.retain(|k, _| !k.starts_with(&prefix));
        self.sessions.len() != before
    }

    /// Sessions with a transaction open (or aborted), optionally of one
    /// connection (\`openTransactions\`).
    pub fn open_transactions(&self, connection_id: Option<&str>) -> Vec<OpenTx> {
        self.sessions
            .iter()
            .filter(|(_, s)| s.state != TxState::Idle)
            .filter_map(|(k, s)| {
                let (cid, database) = k.split_once('\u{0}').unwrap_or((k.as_str(), ""));
                if connection_id.is_some_and(|c| c != cid) {
                    return None;
                }
                Some(OpenTx {
                    connection_id: cid.to_owned(),
                    database: database.to_owned(),
                    state: s.state,
                })
            })
            .collect()
    }

    /// The tx state a backend call reported, on success or in its error
    /// (\`run_query\`, \`apply_edits\`): noted when present.
    pub fn note_outcome(
        &mut self,
        connection_id: &str,
        database: &str,
        tx: Option<TxState>,
    ) -> bool {
        match tx {
            Some(state) => self.note(connection_id, database, TxPatch::state(state)),
            None => false,
        }
    }

    /// \`set_tx_mode\` succeeded with the session now in \`state\`.
    pub fn note_mode_set(
        &mut self,
        connection_id: &str,
        database: &str,
        manual: bool,
        state: TxState,
    ) -> bool {
        let mode = if manual { TxMode::Manual } else { TxMode::Auto };
        self.note(
            connection_id,
            database,
            TxPatch {
                mode: Some(mode),
                state: Some(state),
            },
        )
    }

    /// Every known session by \`db_key\`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &TxSession)> {
        self.sessions.iter().map(|(k, s)| (k.as_str(), s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(mode: TxMode, state: TxState) -> TxSession {
        TxSession { mode, state }
    }

    #[test]
    fn is_kept_per_session_and_listed_while_open() {
        let mut tx = TxSessions::default();
        assert_eq!(tx.get("c", "db"), session(TxMode::Auto, TxState::Idle));
        tx.note("c", "db", TxPatch::state(TxState::Active));
        tx.note("c", "other", TxPatch::mode(TxMode::Manual));
        tx.note("d", "db", TxPatch::state(TxState::Failed));
        assert_eq!(
            tx.open_transactions(None),
            [
                OpenTx {
                    connection_id: "c".into(),
                    database: "db".into(),
                    state: TxState::Active
                },
                OpenTx {
                    connection_id: "d".into(),
                    database: "db".into(),
                    state: TxState::Failed
                },
            ]
        );
        assert_eq!(tx.open_transactions(Some("c")).len(), 1);
        tx.forget("c");
        assert_eq!(tx.get("c", "db").state, TxState::Idle);
        assert_eq!(tx.get("c", "other").mode, TxMode::Auto);
        assert_eq!(tx.get("d", "db").state, TxState::Failed);
    }

    /// The app reports each backend outcome as \`trackTx\` did.
    #[test]
    fn follows_what_the_backend_reports_on_success_and_on_error() {
        let mut tx = TxSessions::default();
        // run_query("begin") → Ok(QueryResult { tx: Some(Active) })
        tx.note_outcome("c", "db", Some(TxState::Active));
        assert_eq!(tx.get("c", "db").state, TxState::Active);
        // run_query("select 1") → Err(CmdError { code: 25P02, tx: Some(Failed) })
        tx.note_outcome("c", "db", Some(TxState::Failed));
        assert_eq!(tx.get("c", "db").state, TxState::Failed);
        // set_tx_mode(manual) → Ok(Active)
        tx.note_mode_set("c", "db", true, TxState::Active);
        assert_eq!(tx.get("c", "db"), session(TxMode::Manual, TxState::Active));
        // disconnect
        tx.forget("c");
        assert_eq!(tx.get("c", "db"), session(TxMode::Auto, TxState::Idle));
    }

    #[test]
    fn follows_submitted_grid_edits_manual_mode_leaves_a_transaction_open() {
        let mut tx = TxSessions::default();
        // apply_edits → Ok(ApplyOutcome { in_transaction: true, tx: Some(Active) })
        tx.note_outcome("c", "db", Some(TxState::Active));
        assert_eq!(tx.get("c", "db").state, TxState::Active);
    }

    // Beyond the TypeScript cases.

    #[test]
    fn reports_whether_anything_changed() {
        let mut tx = TxSessions::default();
        assert!(
            tx.note("c", "db", TxPatch::default()),
            "a first report is a change"
        );
        assert!(!tx.note("c", "db", TxPatch::state(TxState::Idle)));
        assert!(!tx.note_outcome("c", "db", None));
        assert!(tx.note_outcome("c", "db", Some(TxState::Active)));
        assert!(!tx.note_outcome("c", "db", Some(TxState::Active)));
        assert!(tx.forget("c"));
        assert!(!tx.forget("c"));
        assert_eq!(tx.iter().count(), 0);
    }

    #[test]
    fn forgetting_a_connection_spares_ids_with_the_same_prefix() {
        let mut tx = TxSessions::default();
        tx.note("c", "db", TxPatch::state(TxState::Active));
        tx.note("c2", "db", TxPatch::state(TxState::Active));
        tx.forget("c");
        assert_eq!(tx.open_transactions(None)[0].connection_id, "c2");
    }

    #[test]
    fn serializes_like_the_typescript_values() {
        assert_eq!(
            serde_json::to_string(&session(TxMode::Manual, TxState::Failed)).unwrap(),
            r#"{"mode":"manual","state":"failed"}"#
        );
    }
}
