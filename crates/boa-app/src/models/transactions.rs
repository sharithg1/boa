//! [`Transactions`]: the transaction mode (Auto / Manual) and probed state
//! (idle / open / aborted) per session (state/tx.ts, TxControls.tsx), on
//! `boa_core::state::tx::TxSessions`.
//!
//! A session is one (connection, database) pair on the backend, shared by
//! every console and table tab on that database ([`DbKey`]). Its state is
//! whatever the backend reported after the session's last statement: every
//! backend outcome (`QueryResult.tx`, `ApplyOutcome.tx`, `CmdError.tx`) is fed in
//! through [`report`] / [`Transactions::note_result`] (what `trackTx` did by
//! wrapping the backend). Connecting, disconnecting or deleting a connection
//! drops its sessions (back to Auto, idle); a session that dropped with a
//! transaction open is reported as [`TransactionsEvent::TxLost`].
//!
//! Commit / rollback ([`Transactions::end`]) run `COMMIT` / `ROLLBACK` like a
//! typed statement (logged in the Output log and the history); the mode
//! switch with its "leave Manual with a transaction open" prompt and the
//! shortcuts are in `views::tx_controls`, the quit / disconnect guards in
//! `views::guard_dialogs`.

use std::collections::HashSet;
use std::time::Instant;

use boa_core::edits::ApplyOutcome;
use boa_core::error::CmdError;
use boa_core::model::QueryResult;
use boa_core::state::history::OutputEntry;
pub use boa_core::state::tx::{IDLE_SESSION, OpenTx, TxMode, TxSession, TxSessions};
pub use boa_core::tx::TxState;
use gpui::{App, Context, Entity, EventEmitter, Subscription, Task};

use super::catalogs::DbKey;
use super::connections::{Connections, ConnectionsEvent};
use super::console::Console;
use super::layout::PanelKind;
use super::workspaces::now_ms;
use crate::app_state::AppState;
use crate::grid::error::RunError;

/// What [`Transactions`] tells its observers besides `cx.notify()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionsEvent {
    /// A session's mode or state changed.
    Changed { key: DbKey },
    /// A session dropped with a transaction open; the server rolled it
    /// back (its state is idle now).
    TxLost { key: DbKey },
}

/// A backend outcome that reports the session's transaction state.
pub trait TxOutcome {
    fn tx_state(&self) -> Option<TxState>;
}

impl TxOutcome for QueryResult {
    fn tx_state(&self) -> Option<TxState> {
        Some(self.tx)
    }
}

impl TxOutcome for ApplyOutcome {
    fn tx_state(&self) -> Option<TxState> {
        Some(self.tx)
    }
}

impl TxOutcome for () {
    fn tx_state(&self) -> Option<TxState> {
        None
    }
}

/// The tx state a backend call reported, on success or in its error.
pub fn outcome_tx<T: TxOutcome>(result: &Result<T, CmdError>) -> Option<TxState> {
    match result {
        Ok(v) => v.tx_state(),
        Err(e) => e.tx,
    }
}

/// Feeds a backend outcome of session `key` into the app's [`Transactions`]
/// (no-op without [`AppState`]). Call it wherever `run_query` /
/// `apply_edits` returns.
pub fn report<T: TxOutcome>(key: &DbKey, result: &Result<T, CmdError>, cx: &mut App) {
    let Some(tx) = outcome_tx(result) else {
        return;
    };
    if let Some(state) = AppState::try_global(cx).cloned() {
        state
            .transactions
            .update(cx, |t, cx| t.note_outcome(key, Some(tx), cx));
    }
}

/// Transaction mode and state per session.
#[derive(Debug, Default)]
pub struct Transactions {
    sessions: TxSessions,
    /// Sessions whose COMMIT / ROLLBACK or mode switch is in flight.
    busy: HashSet<DbKey>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TransactionsEvent> for Transactions {}

impl Transactions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Follows `connections`: connect / disconnect / delete drop the
    /// connection's sessions; a lost transaction makes its session idle.
    pub fn watch(&mut self, connections: &Entity<Connections>, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.subscribe(
            connections,
            |this, _, event: &ConnectionsEvent, cx| match event {
                ConnectionsEvent::Connected { id }
                | ConnectionsEvent::Disconnected { id }
                | ConnectionsEvent::Deleted { id } => this.forget(id, cx),
                ConnectionsEvent::TxLost { id, database } => {
                    let key = DbKey {
                        connection_id: id.clone(),
                        database: database.clone(),
                    };
                    this.note_lost(&key, cx);
                }
                ConnectionsEvent::Saved { .. } | ConnectionsEvent::PasswordPrompt { .. } => {}
            },
        ));
    }

    /// A session's mode and state (`txOf`); Auto / idle when unknown.
    pub fn get(&self, key: &DbKey) -> TxSession {
        self.sessions.get(&key.connection_id, &key.database)
    }

    pub fn mode(&self, key: &DbKey) -> TxMode {
        self.get(key).mode
    }

    pub fn state(&self, key: &DbKey) -> TxState {
        self.get(key).state
    }

    /// Every known session.
    pub fn sessions(&self) -> &TxSessions {
        &self.sessions
    }

    /// Sessions with a transaction open (or aborted), optionally of one
    /// connection: what the guards ask about.
    pub fn open_transactions(&self, connection_id: Option<&str>) -> Vec<OpenTx> {
        self.sessions.open_transactions(connection_id)
    }

    /// Sessions with a transaction open (or aborted).
    pub fn open_sessions(&self) -> Vec<DbKey> {
        self.open_transactions(None)
            .into_iter()
            .map(|o| DbKey {
                connection_id: o.connection_id,
                database: o.database,
            })
            .collect()
    }

    /// A COMMIT / ROLLBACK or mode switch of the session is in flight.
    pub fn is_busy(&self, key: &DbKey) -> bool {
        self.busy.contains(key)
    }

    fn changed(&mut self, key: &DbKey, cx: &mut Context<Self>) {
        cx.emit(TransactionsEvent::Changed { key: key.clone() });
        cx.notify();
    }

    /// The tx state a backend call reported (`noteTx`; None: nothing to note).
    pub fn note_outcome(&mut self, key: &DbKey, tx: Option<TxState>, cx: &mut Context<Self>) {
        if self
            .sessions
            .note_outcome(&key.connection_id, &key.database, tx)
        {
            self.changed(key, cx);
        }
    }

    /// [`Transactions::note_outcome`] of a backend result.
    pub fn note_result<T: TxOutcome>(
        &mut self,
        key: &DbKey,
        result: &Result<T, CmdError>,
        cx: &mut Context<Self>,
    ) {
        self.note_outcome(key, outcome_tx(result), cx);
    }

    /// `set_tx_mode` succeeded with the session now in `state`.
    pub fn note_mode_set(
        &mut self,
        key: &DbKey,
        mode: TxMode,
        state: TxState,
        cx: &mut Context<Self>,
    ) {
        let manual = mode == TxMode::Manual;
        if self
            .sessions
            .note_mode_set(&key.connection_id, &key.database, manual, state)
        {
            self.changed(key, cx);
        }
    }

    /// Drops a connection's sessions (its server sessions are gone; `forgetTx`).
    pub fn forget(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        self.busy.retain(|k| k.connection_id != connection_id);
        if self.sessions.forget(connection_id) {
            cx.notify();
        }
    }

    /// The session dropped with a transaction open: the server rolled it back.
    pub fn note_lost(&mut self, key: &DbKey, cx: &mut Context<Self>) {
        self.sessions.note(
            &key.connection_id,
            &key.database,
            boa_core::state::tx::TxPatch::state(TxState::Idle),
        );
        cx.emit(TransactionsEvent::TxLost { key: key.clone() });
        cx.notify();
    }

    /// Switches the session to `mode` on the backend (`set_tx_mode`) and
    /// notes the state it reports. No questions asked: leaving Manual with
    /// a transaction open asks first in `views::tx_controls::switch_mode`.
    pub fn set_mode(
        &mut self,
        key: &DbKey,
        mode: TxMode,
        cx: &mut Context<Self>,
    ) -> Task<Result<TxState, CmdError>> {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return Task::ready(Err(CmdError::msg("No backend")));
        };
        let call =
            state
                .backend
                .set_tx_mode(&key.connection_id, &key.database, mode == TxMode::Manual);
        self.busy.insert(key.clone());
        cx.notify();
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                this.busy.remove(&key);
                match &result {
                    Ok(tx) => this.note_mode_set(&key, mode, *tx, cx),
                    Err(e) => this.note_outcome(&key, e.tx, cx),
                }
                cx.notify();
            })
            .ok();
            result
        })
    }

    /// Ends the session's transaction (`endTransaction`): runs `COMMIT` or
    /// `ROLLBACK` like a typed statement of `console` (the active console of
    /// the database, if any: it shows as running, a failure in its strip),
    /// logged in the Output log and the history. Resolves true when the
    /// session is idle afterwards. Nothing while `console` runs a statement
    /// or another COMMIT / ROLLBACK of the session is in flight.
    pub fn end(
        &mut self,
        key: &DbKey,
        commit: bool,
        console: Option<Entity<Console>>,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return Task::ready(false);
        };
        if self.busy.contains(key)
            || console
                .as_ref()
                .is_some_and(|c| c.read(cx).running.is_some())
        {
            return Task::ready(false);
        }
        let sql = if commit { "COMMIT" } else { "ROLLBACK" };
        let source = console
            .as_ref()
            .map_or_else(|| key.database.clone(), |c| c.read(cx).title.to_string());
        if let Some(console) = &console {
            console.update(cx, |c, cx| {
                c.failed = None;
                c.running = Some(Instant::now());
                cx.notify();
            });
        }
        let page_size = state.prefs.read(cx).get().page_size() as usize;
        let at = now_ms();
        let call = state
            .backend
            .run_query(&key.connection_id, &key.database, sql, page_size, 0);
        self.busy.insert(key.clone());
        cx.notify();
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                this.busy.remove(&key);
                this.note_result(&key, &result, cx);
                let mut entry = OutputEntry {
                    sql: sql.to_owned(),
                    database: key.database.clone(),
                    at,
                    source: Some(source),
                    ..OutputEntry::default()
                };
                let failure = match &result {
                    Ok(res) => {
                        entry.ms = res.ms as f64;
                        entry.affected = Some(res.affected.unwrap_or(0) as f64);
                        None
                    }
                    Err(err) => {
                        let error = RunError::from_cmd(err, sql);
                        entry.ms = (now_ms() - at).max(0.);
                        entry.error = Some(error.message.clone());
                        entry.detail = error.detail.clone();
                        Some(error)
                    }
                };
                let excerpt = failure.as_ref().and_then(|f| f.excerpt.clone());
                let failed = failure.is_some();
                state.workspaces.update(cx, |w, cx| {
                    w.log(&key.connection_id, entry, excerpt, true, cx);
                    w.set_bottom_view(None, cx);
                });
                state
                    .layout
                    .update(cx, |l, cx| l.set_open(PanelKind::Bottom, true, cx));
                if let Some(console) = &console {
                    console.update(cx, |c, cx| {
                        c.running = None;
                        c.failed = failure;
                        cx.notify();
                    });
                }
                cx.notify();
                !failed && this.state(&key) == TxState::Idle
            })
            .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests;
