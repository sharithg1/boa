//! [`History`]: the saved query history (state/history.ts), per connection
//! on disk (`history-<id>.jsonl` in the config dir, the Tauri app's format,
//! through boa-core's `history`, the code behind `Backend::history_*`). The rules live in boa-core
//! ([`boa_core::state::history::QueryHistory`]); this entity does the I/O
//! they ask for and tells views:
//!
//! - every console run and failure is recorded from
//!   [`WorkspacesEvent::OutputLogged`] (`record: true`; page re-fetches are
//!   not), unless prefs `recordHistory` is off; runs past `historyDays` and
//!   the oldest past 5,000 drop out; a query run again replaces its entry;
//! - lists load lazily (the first time a view needs a connection) and then
//!   follow new runs in memory; a new retention period re-reads them;
//! - delete (⌘⌫) / clear update the list at once, then the file (on error: a
//!   toast and a re-read);
//! - a deleted connection's list is forgotten (the backend deletes its file);
//! - [`History::is_open`]: whether the Query History view (⌘⌥E) shows
//!   ([`HistoryEvent::Opened`] / [`HistoryEvent::Closed`]).
//!
//! File operations run one at a time, in the order they were asked for, on
//! the background executor (not the backend's runtime: no foreign wake-ups,
//! so `#[gpui::test]`s that run statements stay deterministic).
//!
//! [`WorkspacesEvent::OutputLogged`]: super::workspaces::WorkspacesEvent::OutputLogged

use std::path::Path;
use std::sync::Arc;

use boa_core::error::CmdError;
use boa_core::history::{Entry, History as FileHistory};
use boa_core::state::history::{
    self as core, CLEAR_FAILED, DELETE_FAILED, HistoryItem, HistoryPrefs, OutputEntry,
    QueryHistory, history_cutoff,
};
use boa_core::state::reconnect::Toast;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, AppContext as _, Context, EventEmitter, Subscription, Task};

use super::connections::ConnectionsEvent;
use super::prefs::PrefsChanged;
use super::workspaces::{WorkspacesEvent, now_ms};
use crate::app_state::AppState;

/// What History tells its subscribers besides notifying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryEvent {
    /// The Query History view should show (⌘⌥E, the console toolbar, ⌘K).
    Opened,
    /// It should close.
    Closed,
}

/// A file operation, run in order by the worker.
#[derive(Debug)]
enum Op {
    Append {
        id: String,
        entry: Box<Entry>,
        days: u32,
    },
    /// `report`: toast + re-read on failure (a user's delete); else ignored
    /// (legacy / stale cleanup).
    Delete {
        id: String,
        ids: Vec<String>,
        report: bool,
    },
    Clear {
        id: String,
    },
    List {
        id: String,
        days: u32,
    },
}

/// The query history.
pub struct History {
    core: QueryHistory,
    ops: Option<UnboundedSender<Op>>,
    _worker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("core", &self.core)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<HistoryEvent> for History {}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// A history without a backend (nothing is read or written).
    pub fn new() -> Self {
        Self {
            core: QueryHistory::new(),
            ops: None,
            _worker: None,
            _subscriptions: Vec::new(),
        }
    }

    /// A history reading and writing `history-<id>.jsonl` files in `dir`
    /// (boa-core's `history`, the code behind `Backend::history_*`), on the
    /// background executor.
    pub fn with_dir(dir: &Path, cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = unbounded::<Op>();
        let files = Arc::new(FileHistory::default());
        let dir = dir.to_owned();
        let worker = cx.spawn(async move |this, cx| {
            while let Some(op) = rx.next().await {
                let (files, dir) = (files.clone(), dir.clone());
                let alive = match op {
                    Op::Append { id, entry, days } => {
                        let result = cx
                            .background_spawn(async move {
                                files.append(&dir, &id, &entry, history_cutoff(days, now_ms()))
                            })
                            .await;
                        this.update(cx, |h, cx| h.append_done(result, cx))
                    }
                    Op::Delete { id, ids, report } => {
                        let file_id = id.clone();
                        let result = cx
                            .background_spawn(async move { files.delete(&dir, &file_id, &ids) })
                            .await;
                        this.update(cx, |h, cx| {
                            if report && let Err(e) = result {
                                h.failed(&id, DELETE_FAILED, &e, cx);
                            }
                        })
                    }
                    Op::Clear { id } => {
                        let file_id = id.clone();
                        let result = cx
                            .background_spawn(async move { files.clear(&dir, &file_id) })
                            .await;
                        this.update(cx, |h, cx| {
                            if let Err(e) = result {
                                h.failed(&id, CLEAR_FAILED, &e, cx);
                            }
                        })
                    }
                    Op::List { id, days } => {
                        let file_id = id.clone();
                        let result = cx
                            .background_spawn(async move {
                                files.list(&dir, &file_id, history_cutoff(days, now_ms()))
                            })
                            .await;
                        this.update(cx, |h, cx| h.loaded(&id, result, days, cx))
                    }
                };
                if alive.is_err() {
                    break;
                }
            }
        });
        Self {
            core: QueryHistory::new(),
            ops: Some(tx),
            _worker: Some(worker),
            _subscriptions: Vec::new(),
        }
    }

    /// Follows the app: records runs from Workspaces, re-reads on a new
    /// retention period, forgets deleted connections. Call once
    /// [`AppState`] is installed.
    pub fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        self._subscriptions = vec![
            cx.subscribe(&state.workspaces, |this, _, event: &WorkspacesEvent, cx| {
                let WorkspacesEvent::OutputLogged {
                    connection_id,
                    entry,
                    record,
                } = event;
                if *record {
                    this.record(connection_id, entry, cx);
                }
            }),
            cx.subscribe(&state.prefs, |this, prefs, event: &PrefsChanged, cx| {
                let next = prefs.read(cx).get().history_days;
                for id in this.core.days_changed(event.prev.history_days, next) {
                    this.load(&id, true, cx);
                }
            }),
            cx.subscribe(
                &state.connections,
                |this, _, event: &ConnectionsEvent, cx| {
                    if let ConnectionsEvent::Deleted { id } = event {
                        this.forget(id, cx);
                    }
                },
            ),
        ];
    }

    /// prefs `recordHistory` / `historyDays` (the defaults without AppState).
    pub fn prefs(cx: &App) -> HistoryPrefs {
        AppState::try_global(cx).map_or_else(HistoryPrefs::default, |s| {
            let p = s.prefs.read(cx).get();
            HistoryPrefs {
                record: p.record_history,
                days: p.history_days,
            }
        })
    }

    fn send(&self, op: Op) {
        if let Some(tx) = &self.ops {
            let _ = tx.unbounded_send(op);
        }
    }

    fn toast(toast: Toast, cx: &mut App) {
        if let Some(state) = AppState::try_global(cx).cloned() {
            state.toasts.update(cx, |t, cx| {
                t.report_error(toast.title, toast.body.unwrap_or_default(), cx);
            });
        }
    }

    // ── The view (openHistory / toggleHistory) ─────────────────────────────

    /// The Query History view shows.
    pub fn is_open(&self) -> bool {
        self.core.open
    }

    /// Opens or closes the Query History view.
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.core.open == open {
            return;
        }
        self.core.open = open;
        cx.emit(if open {
            HistoryEvent::Opened
        } else {
            HistoryEvent::Closed
        });
        cx.notify();
    }

    /// ⌘⌥E (`toggleHistory`).
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.set_open(!self.core.open, cx);
    }

    // ── Lists ──────────────────────────────────────────────────────────────

    /// A connection's loaded list, newest first (None: not loaded).
    pub fn list(&self, connection_id: &str) -> Option<&[Entry]> {
        self.core.list(connection_id)
    }

    /// Its history is being read.
    pub fn is_loading(&self, connection_id: &str) -> bool {
        self.core.is_loading(connection_id)
    }

    /// The loaded lists of `ids` merged newest first, with their connection.
    pub fn items(&self, ids: &[String]) -> Vec<HistoryItem> {
        self.core.items(ids)
    }

    /// Reads a connection's history once (`force`: again). (`loadHistory`)
    pub fn load(&mut self, connection_id: &str, force: bool, cx: &mut Context<Self>) {
        if self.ops.is_none() || !self.core.begin_load(connection_id, force) {
            return;
        }
        let days = Self::prefs(cx).days;
        self.send(Op::List {
            id: connection_id.to_owned(),
            days,
        });
        cx.notify();
    }

    fn loaded(
        &mut self,
        connection_id: &str,
        result: Result<Vec<Entry>, CmdError>,
        days: u32,
        cx: &mut Context<Self>,
    ) {
        let err;
        let result = match result {
            Ok(list) => Ok(list),
            Err(e) => {
                err = e;
                Err(&err)
            }
        };
        let outcome = self.core.finish_load(connection_id, result, days, now_ms());
        if !outcome.stale.is_empty() {
            self.send(Op::Delete {
                id: connection_id.to_owned(),
                ids: outcome.stale,
                report: false,
            });
        }
        if let Some(toast) = outcome.toast {
            Self::toast(toast, cx);
        }
        cx.notify();
    }

    /// Saves a statement the user ran to its connection's history, unless
    /// recording is off. (`recordHistory`)
    pub fn record(&mut self, connection_id: &str, entry: &OutputEntry, cx: &mut Context<Self>) {
        let prefs = Self::prefs(cx);
        let Some(plan) = self.core.record(connection_id, entry, prefs, now_ms()) else {
            return;
        };
        if !plan.delete_legacy.is_empty() {
            self.send(Op::Delete {
                id: connection_id.to_owned(),
                ids: plan.delete_legacy,
                report: false,
            });
        }
        self.send(Op::Append {
            id: connection_id.to_owned(),
            entry: Box::new(plan.append),
            days: plan.days,
        });
        cx.notify();
    }

    fn append_done(&mut self, result: Result<(), CmdError>, cx: &mut Context<Self>) {
        if let Some(toast) = self.core.append_done(result.as_ref().map(|_| ())) {
            Self::toast(toast, cx);
        }
    }

    fn failed(&mut self, connection_id: &str, title: &str, err: &CmdError, cx: &mut Context<Self>) {
        Self::toast(core::error_toast(title, err), cx);
        self.load(connection_id, true, cx);
    }

    /// Deletes entries (⌘⌫ in the view). (`deleteHistoryEntries`)
    pub fn delete(&mut self, connection_id: &str, ids: &[String], cx: &mut Context<Self>) {
        self.core.delete_entries(connection_id, ids);
        self.send(Op::Delete {
            id: connection_id.to_owned(),
            ids: ids.to_vec(),
            report: true,
        });
        cx.notify();
    }

    /// Deletes a connection's whole history ("Clear history…"). (`clearHistory`)
    pub fn clear(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        self.core.clear(connection_id);
        self.send(Op::Clear {
            id: connection_id.to_owned(),
        });
        cx.notify();
    }

    /// Drops a deleted connection's list. (`forgetHistory`)
    pub fn forget(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        self.core.forget(connection_id);
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
