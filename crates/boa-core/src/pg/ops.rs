//! One operation at a time per session.
//!
//! A session is one Postgres connection, shared by every console, table tab,
//! export and explorer read of its database. Most operations are several
//! round trips that depend on each other (`begin`, `declare __boa`,
//! `fetch`, `close`, `commit`, then the transaction probe; or a savepoint
//! around a `COPY`). tokio-postgres pipelines whatever is sent on a client,
//! so two operations started together interleave their steps on the server:
//! one's `commit` closes the other's cursor ("cursor \"__boa\" does not
//! exist"), or both declare `__boa`. (The Tauri app has the same race; its
//! UI only refuses a second run in the same tab.)
//!
//! So every operation first *claims* its session ([`Pg::claim`]): a fair
//! (FIFO) async mutex per session, held until the operation's last step,
//! including the transaction probe. Operations on different databases or
//! connections still run in parallel. Waiting adds no latency over the old
//! pipelining: one connection runs one statement at a time anyway.
//!
//! Each operation can be given a [`CancelHandle`]: cancelling it while it
//! waits makes it give up without running (57014, as a server cancel), and
//! while it runs sends the server a cancel request for its session, as
//! before. [`Pg::cancel`] (by database) still cancels whatever is running.

use super::{Pg, Session};
use crate::error::{CmdError, CmdResult};
use futures_util::future::{Either, select};
use futures_util::pin_mut;
use postgres_native_tls::MakeTlsConnector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use tokio::sync::{Notify, OwnedMutexGuard};
use tokio_postgres::CancelToken;

/// Cancels one operation (a statement, a page, an export), whether it is
/// still waiting for its session or already running. Cheap to clone.
#[derive(Clone, Default)]
pub struct CancelHandle(Arc<CancelState>);

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    wake: Notify,
    /// The session's cancel token while the operation holds it.
    running: std::sync::Mutex<Option<(CancelToken, MakeTlsConnector)>>,
}

impl std::fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelHandle")
            .field("cancelled", &self.is_cancelled())
            .field("running", &self.is_running())
            .finish()
    }
}

impl CancelHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// `cancel` was called.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// The operation holds its session (it is past waiting, not yet done).
    pub fn is_running(&self) -> bool {
        self.running().is_some()
    }

    fn running(&self) -> Option<(CancelToken, MakeTlsConnector)> {
        self.0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_running(&self, r: Option<(CancelToken, MakeTlsConnector)>) {
        *self
            .0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = r;
    }

    /// Cancels the operation: one still waiting for its session fails at
    /// once without running; a running one's statement is cancelled by the
    /// server (it then fails with 57014). Nothing happens once it ended.
    /// Like any server-side cancel, a request that arrives just as the
    /// statement ends is lost (or, rarely, hits the next statement).
    pub async fn cancel(&self) -> CmdResult<()> {
        self.0.cancelled.store(true, Ordering::SeqCst);
        // Stores a permit when nobody waits yet, so a later wait sees it.
        self.0.wake.notify_one();
        if let Some((token, tls)) = self.running() {
            token.cancel_query(tls).await?;
        }
        Ok(())
    }

    /// Resolves once `cancel` was called.
    async fn cancelled(&self) {
        while !self.is_cancelled() {
            self.0.wake.notified().await;
        }
    }
}

/// The error of an operation cancelled before it ran: the server's own
/// (SQLSTATE 57014), so callers treat it like a cancelled statement.
pub fn cancelled_error() -> CmdError {
    CmdError::code("57014", "canceling statement due to user request")
}

/// An operation's hold on its session; dropping it lets the next one run.
pub(super) struct Claim {
    pub(super) s: Arc<Session>,
    cancel: CancelHandle,
    _turn: OwnedMutexGuard<()>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.cancel.set_running(None);
    }
}

impl Pg {
    /// Waits for the database's session to be free (in arrival order) and
    /// holds it for one operation. A session that dropped, was found dead or
    /// was disconnected while this waited is not used: its replacement is
    /// claimed instead (or `not_connected`).
    pub(super) async fn claim(
        &self,
        id: &str,
        database: &str,
        cancel: &CancelHandle,
    ) -> CmdResult<Claim> {
        let key = (id.to_owned(), database.to_owned());
        let mut attempts = 0;
        loop {
            attempts += 1;
            let s = self.session(id, database).await?;
            let wait = s.ops.clone().lock_owned();
            let stop = cancel.cancelled();
            pin_mut!(wait, stop);
            // Polled in this order: a free session is taken even if
            // cancelled meanwhile (checked again below).
            let turn = match select(wait, stop).await {
                Either::Left((turn, _)) => turn,
                Either::Right(_) => return Err(cancelled_error()),
            };
            let current = self
                .sessions
                .lock()
                .await
                .get(&key)
                .is_some_and(|c| Arc::ptr_eq(c, &s));
            let usable = !s.client.is_closed() && !s.dead.load(Ordering::Relaxed);
            if !(current && usable) && attempts < 3 {
                continue;
            }
            cancel.set_running(Some((s.cancel.clone(), s.tls.clone())));
            let claim = Claim {
                s,
                cancel: cancel.clone(),
                _turn: turn,
            };
            if cancel.is_cancelled() {
                return Err(cancelled_error());
            }
            return Ok(claim);
        }
    }
}
