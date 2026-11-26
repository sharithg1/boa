//! Staying connected, the pure half: ported from the reconnect parts of
//! `ui/src/state/store.ts` ("Staying connected": `recover`, `attemptRecovery`,
//! `checkConnections`, `watchConnections`, `connectNow`'s error handling,
//! `pushToast` keys), `ui/src/state/links.ts`, and the cases of
//! `ui/src/state/reconnect.test.ts` that need no store.
//!
//! What is here:
//! - the backoff schedule (`RETRY_DELAYS_MS`, 1 s … 30 s, about 2 minutes in
//!   all) and wake detection (`WAKE_TICK_MS`, `WAKE_GAP_MS`, `woke_up`);
//! - which error codes mean what (`codes`, `link_events_for`,
//!   `is_auth_error`, `is_retryable`, `is_commit_unknown`);
//! - the toasts, their texts and the one status-toast key per connection
//!   (`Toast`, `conn_toast_key`, `lost_toast`, …);
//! - `ConnLinkState`, a connection's live state;
//! - `Recoveries`: the reconnect loop as a state machine. It never sleeps
//!   or calls the backend: each method returns `Effect`s (ping, connect,
//!   start a timer, set the state, show a toast, …) that a GPUI model
//!   performs, feeding the outcomes back (`Recoveries::on_ping`,
//!   `Recoveries::on_connect`, `Recoveries::on_timer`).
//!
//! Retry-safety of statements is not a UI decision: `crate::pg` re-runs a
//! statement on a fresh session only when that is safe (`safe_to_retry`:
//! later pages, `TABLE` / `SHOW`), and reports the rest as `connection_lost`,
//! `tx_lost` or commit-unknown. `crate::backend::LinkEvent`s replace
//! `links.ts`'s `trackLinks` (see `link_events_for`).

use std::collections::HashMap;

use crate::backend::LinkEvent;
use crate::error::CmdError;

/// Delays before each retry; once they run out the connection shows as
/// failed (`RETRY_DELAYS_MS`: 120 s in all).
pub const RETRY_DELAYS_MS: [u64; 8] = [1_000, 2_000, 4_000, 8_000, 15_000, 30_000, 30_000, 30_000];

/// How often to tick to notice a wake from sleep (timers stop while asleep).
pub const WAKE_TICK_MS: u64 = 5_000;
/// A tick this late after the previous one means the machine woke up.
pub const WAKE_GAP_MS: u64 = 20_000;

/// The machine slept between two wake ticks (`watchConnections`): call
/// [`Recoveries::check`] (also when the network comes back).
pub fn woke_up(last_tick_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_tick_ms) > WAKE_GAP_MS
}

/// Error codes the reconnect logic looks at (`CmdError::code`): app codes
/// from `crate::pg` / `crate::error` and the SQLSTATEs of refused logins.
pub mod codes {
    /// The server asked for a password and none was given or stored.
    pub const PASSWORD_REQUIRED: &str = "password_required";
    /// The backend has no such connection open (a new launch, or never connected).
    pub const NOT_CONNECTED: &str = "not_connected";
    /// The server could not be reached (refused, timed out, reset).
    pub const UNREACHABLE: &str = crate::error::UNREACHABLE;
    /// The connection dropped during the call (also "commit unknown").
    pub const CONNECTION_LOST: &str = "connection_lost";
    /// The session dropped with a transaction open; the server rolled it back.
    pub const TX_LOST: &str = "tx_lost";
    /// SQLSTATE invalid_password.
    pub const INVALID_PASSWORD: &str = "28P01";
    /// SQLSTATE invalid_authorization_specification.
    pub const INVALID_AUTHORIZATION: &str = "28000";
}

/// Codes of calls that failed because the server couldn't be reached or the
/// connection dropped (`NETWORK_CODES` in links.ts).
pub const NETWORK_CODES: [&str; 2] = [codes::CONNECTION_LOST, codes::UNREACHABLE];

/// What a failed call on an open connection reports (links.ts `report`):
/// `tx_lost` → `TxLost` then `Lost`; `connection_lost` / `unreachable` →
/// `Lost`; anything else → nothing. `crate::backend::Backend` already sends
/// exactly these from every call links.ts wrapped (`list_schemas`,
/// `introspect`, `table_ddl`, `run_query`, `set_tx_mode`, `apply_edits`,
/// `export_csv`); this is the same rule, for callers and tests.
pub fn link_events_for(code: Option<&str>, connection_id: &str, database: &str) -> Vec<LinkEvent> {
    let mut out = Vec::new();
    let Some(code) = code else { return out };
    if code == codes::TX_LOST {
        out.push(LinkEvent::TxLost {
            connection_id: connection_id.to_owned(),
            database: database.to_owned(),
        });
    }
    if code == codes::TX_LOST || NETWORK_CODES.contains(&code) {
        out.push(LinkEvent::Lost {
            connection_id: connection_id.to_owned(),
        });
    }
    out
}

/// A refused login: the password prompt's business, not worth retrying (`isAuthError`).
pub fn is_auth_error(e: &CmdError) -> bool {
    matches!(
        e.code.as_deref(),
        Some(codes::PASSWORD_REQUIRED | codes::INVALID_PASSWORD | codes::INVALID_AUTHORIZATION)
    )
}

/// Worth trying again: the server couldn't be reached (not a refused login
/// or a bad setting) (`isRetryable`).
pub fn is_retryable(e: &CmdError) -> bool {
    matches!(e.code.as_deref(), None | Some(codes::UNREACHABLE))
}

/// The connection dropped while a statement was ending the transaction: it
/// may or may not have been committed (`crate::pg`'s `commit_unknown`, a
/// `connection_lost` whose hint says to check the data). Shown like any
/// error; the GPUI app may want to flag it.
pub fn is_commit_unknown(e: &CmdError) -> bool {
    e.code.as_deref() == Some(codes::CONNECTION_LOST) && e.message.contains("while committing")
}

// ── Toasts ────────────────────────────────────────────────────────────────

/// How long a toast shows by default (`pushToast`'s `ttlMs`).
pub const DEFAULT_TOAST_TTL_MS: u64 = 6_000;

/// A toast's colour (`Toast['kind']`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

/// A toast's button. The store's toasts held a closure; here it is data the
/// Toasts model maps to an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastAction {
    /// "Retry": `recover(connection_id)` (a foreground recovery).
    Retry { connection_id: String },
}

impl ToastAction {
    /// The button label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Retry { .. } => "Retry",
        }
    }
}

/// A toast to push (`Omit<Toast, 'id'>` plus `pushToast`'s `ttlMs`). One with
/// the `key` of a toast still showing replaces it in place (keeping its id
/// and getting this one's lifetime) instead of stacking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub key: Option<String>,
    pub kind: ToastKind,
    pub title: String,
    pub body: Option<String>,
    pub action: Option<ToastAction>,
    /// Shown for this long; 0 until dismissed.
    pub ttl_ms: u64,
}

impl Toast {
    pub fn new(kind: ToastKind, title: impl Into<String>) -> Self {
        Self {
            key: None,
            kind,
            title: title.into(),
            body: None,
            action: None,
            ttl_ms: DEFAULT_TOAST_TTL_MS,
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn action(mut self, action: ToastAction) -> Self {
        self.action = Some(action);
        self
    }

    pub fn ttl(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }
}

/// The key of a connection's one status toast (lost, could not connect,
/// reconnected) (`connToastKey`).
pub fn conn_toast_key(connection_id: &str) -> String {
    format!("conn:{connection_id}")
}

/// "Could not connect to {name}" (status toast, default lifetime).
pub fn could_not_connect_toast(connection_id: &str, name: &str, message: &str) -> Toast {
    Toast::new(ToastKind::Error, format!("Could not connect to {name}"))
        .key(conn_toast_key(connection_id))
        .body(message)
}

/// Given up after the last retry: "Lost connection to {name}" ("Could not
/// connect to {name}" when restoring at launch), with Retry, until dismissed.
pub fn lost_toast(connection_id: &str, name: &str, reason: RecoverReason, message: &str) -> Toast {
    let title = if reason == RecoverReason::Restore {
        format!("Could not connect to {name}")
    } else {
        format!("Lost connection to {name}")
    };
    Toast::new(ToastKind::Error, title)
        .key(conn_toast_key(connection_id))
        .body(message)
        .action(ToastAction::Retry {
            connection_id: connection_id.to_owned(),
        })
        .ttl(0)
}

/// "Reconnected to {name}" (replaces the connection's failure toast).
pub fn reconnected_toast(connection_id: &str, name: &str) -> Toast {
    Toast::new(ToastKind::Success, format!("Reconnected to {name}"))
        .key(conn_toast_key(connection_id))
}

/// A transaction the server rolled back when its session dropped
/// (`noteLostTransactions`; also set that session's tx state to idle). Not
/// keyed: one per database, until dismissed.
pub fn tx_lost_toast(name: &str, database: &str) -> Toast {
    Toast::new(ToastKind::Error, "Transaction rolled back")
        .body(format!(
            "The connection to {name} ({database}) was lost, so the server rolled back the open \
             transaction. Uncommitted changes are gone."
        ))
        .ttl(0)
}

// ── Connection state ──────────────────────────────────────────────────────

/// A connection's live state (`ConnState` in state/types.ts), with the
/// retry schedule while reconnecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnLinkState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    /// Waiting to retry: `attempt` retries scheduled so far (1-based), the
    /// next at `next_at_ms` (the caller's clock, as passed in).
    Reconnecting {
        attempt: u32,
        next_at_ms: u64,
    },
    /// Given up (or a connect failed); Retry / a wake-up tries again.
    Failed,
}

impl ConnLinkState {
    /// The TypeScript name (`connected`, `reconnecting`, …).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Reconnecting { .. } => "reconnecting",
            Self::Failed => "failed",
        }
    }

    pub fn is_connected(&self) -> bool {
        *self == Self::Connected
    }
}

/// A password prompt to open (`passwordPrompt`): `error` under the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPrompt {
    pub error: Option<String>,
}

/// How a connect was asked for (`connectNow`'s arguments).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConnectAttempt {
    /// A password was typed for this attempt (not the stored one).
    pub password_given: bool,
    /// The connection has a stored password.
    pub has_password: bool,
    /// Restoring in the background: never open the password prompt.
    pub quiet: bool,
    /// Show a toast for other errors (`false` from the reconnect loop).
    pub toast: bool,
}

/// What a failed connect does (`connectNow`'s `catch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectFailure {
    /// `Disconnected` after a refused login, else `Failed`.
    pub state: ConnLinkState,
    pub prompt: Option<PasswordPrompt>,
    pub toast: Option<Toast>,
}

/// A connect failed: the state to show, and the password prompt or toast.
/// A refused login opens the prompt (with the error when a typed password
/// or the stored one was wrong, `28P01`), unless `quiet`; other errors show
/// the connection's status toast, unless `toast` is false.
pub fn connect_failure(
    connection_id: &str,
    name: &str,
    err: &CmdError,
    how: ConnectAttempt,
) -> ConnectFailure {
    let auth = is_auth_error(err);
    let wrong = err.code.as_deref() == Some(codes::INVALID_PASSWORD);
    let mut out = ConnectFailure {
        state: if auth {
            ConnLinkState::Disconnected
        } else {
            ConnLinkState::Failed
        },
        prompt: None,
        toast: None,
    };
    if auth && how.quiet {
        // Restoring at launch: don't ask for passwords in the background.
    } else if auth && (how.password_given || !how.has_password || wrong) {
        out.prompt = Some(PasswordPrompt {
            error: (how.password_given || wrong).then(|| err.message.clone()),
        });
    } else if how.toast {
        out.toast = Some(could_not_connect_toast(connection_id, name, &err.message));
    }
    out
}

// ── The reconnect loop ────────────────────────────────────────────────────

/// Why a connection is being recovered: a call failed, the machine woke up,
/// or the app launched (`RecoverReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecoverReason {
    #[default]
    Lost,
    Wake,
    Restore,
}

/// Options of [`Recoveries::recover`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecoverOpts {
    pub reason: RecoverReason,
    /// Don't ask for a password (connections restored in the background).
    pub quiet: bool,
    /// A background retry of a connection already shown as failed: giving
    /// up again says nothing.
    pub background: bool,
}

/// Something the GPUI model must do for a recovery. `token` identifies the
/// recovery: pass it back with the outcome, so a stale outcome (the
/// recovery was stopped or replaced meanwhile) is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Call `Backend::ping(id)`, then [`Recoveries::on_ping`].
    Ping { token: u64 },
    /// Connect again with the stored password (`connectNow(id, None,
    /// { quiet, toast: false })`: state `Connecting`, and on failure apply
    /// [`connect_failure`]), then [`Recoveries::on_connect`].
    Connect { token: u64, quiet: bool },
    /// Start a timer; when it fires call [`Recoveries::on_timer`].
    Schedule { token: u64, delay_ms: u64 },
    /// Cancel the recovery's timer, if one is running.
    CancelTimer,
    /// Show this state (on success also measure the latency).
    SetState(ConnLinkState),
    /// Push this toast.
    Toast(Toast),
    /// Dismiss the connection's status toast if it is an error (`clearFailureToast`).
    ClearFailureToast,
    /// These databases' sessions dropped with a transaction open: set their
    /// tx state to idle and push a [`tx_lost_toast`] for each.
    TxLost { databases: Vec<String> },
    /// Forget that the user wants this connection open (`unwant`).
    Unwant,
    /// The recovery ended: connected (`true`) or given up (`false`); resolve
    /// whoever awaits it.
    Finish { ok: bool },
}

#[derive(Debug, Clone)]
struct Recovery {
    token: u64,
    reason: RecoverReason,
    quiet: bool,
    tries: usize,
    /// A ping or connect is in flight.
    attempt: bool,
    /// It showed as reconnecting (so getting back is worth a toast).
    flagged: bool,
    background: bool,
}

/// The connections being brought back (`recoveries` in store.ts).
///
/// A wanted connection that drops is checked with `ping` (which keeps the
/// sessions that still work, and their transactions), or connected again
/// if the backend no longer has it (a new launch), retrying after each of
/// [`RETRY_DELAYS_MS`] while the server can't be reached. Meanwhile it shows
/// as `Reconnecting`; after the last attempt as `Failed`, with a Retry toast.
/// A connection has one status toast at a time ([`conn_toast_key`]), and a
/// background retry of one already shown as failed doesn't announce failing
/// again.
#[derive(Debug, Default, Clone)]
pub struct Recoveries {
    map: HashMap<String, Recovery>,
    next_token: u64,
}

impl Recoveries {
    pub fn new() -> Self {
        Self::default()
    }

    /// A recovery of this connection is running.
    pub fn is_recovering(&self, connection_id: &str) -> bool {
        self.map.contains_key(connection_id)
    }

    /// Brings a wanted connection back. Joins a recovery in progress (no
    /// effects; a foreground request makes a background one announce its
    /// outcome). Otherwise starts one: `[Ping]`. The recovery ends with
    /// `Effect::Finish`.
    pub fn recover(&mut self, connection_id: &str, opts: RecoverOpts) -> Vec<Effect> {
        if let Some(r) = self.map.get_mut(connection_id) {
            // Asked for (Retry) while a background retry runs: its outcome is shown.
            if !opts.background {
                r.background = false;
            }
            return Vec::new();
        }
        self.next_token += 1;
        let token = self.next_token;
        self.map.insert(
            connection_id.to_owned(),
            Recovery {
                token,
                reason: opts.reason,
                quiet: opts.quiet,
                tries: 0,
                attempt: true,
                flagged: false,
                background: opts.background,
            },
        );
        vec![Effect::Ping { token }]
    }

    /// Runs the pending attempt of a recovery now (`retryNow`): nothing when
    /// none is running (the caller answers "is it connected") or an attempt
    /// is already in flight (join it).
    pub fn retry_now(&mut self, connection_id: &str) -> Vec<Effect> {
        match self.map.get_mut(connection_id) {
            Some(r) if !r.attempt => {
                r.attempt = true;
                vec![Effect::CancelTimer, Effect::Ping { token: r.token }]
            }
            _ => Vec::new(),
        }
    }

    /// The timer of [`Effect::Schedule`] fired.
    pub fn on_timer(&mut self, connection_id: &str, token: u64) -> Vec<Effect> {
        match self.map.get_mut(connection_id) {
            Some(r) if r.token == token && !r.attempt => {
                r.attempt = true;
                vec![Effect::Ping { token }]
            }
            _ => Vec::new(),
        }
    }

    /// Ends a recovery (the user connected, disconnected or deleted the
    /// connection) (`stopRecovery`). True if one ran: cancel its timer and
    /// resolve its waiters with whether the connection is connected. Later
    /// outcomes and timers of it are ignored.
    pub fn stop(&mut self, connection_id: &str) -> bool {
        self.map.remove(connection_id).is_some()
    }

    fn live(&mut self, connection_id: &str, token: u64) -> Option<&mut Recovery> {
        self.map.get_mut(connection_id).filter(|r| r.token == token)
    }

    /// `ping`'s outcome: `Ok(lost_tx)` (databases whose transaction was
    /// rolled back) or the error. `name`: the connection's display name.
    pub fn on_ping(
        &mut self,
        connection_id: &str,
        token: u64,
        name: &str,
        result: Result<&[String], &CmdError>,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(r) = self.live(connection_id, token) else {
            return Vec::new();
        };
        match result {
            Ok(lost_tx) => {
                let mut out = Vec::new();
                if !lost_tx.is_empty() {
                    out.push(Effect::TxLost {
                        databases: lost_tx.to_vec(),
                    });
                }
                out.push(Effect::SetState(ConnLinkState::Connected));
                self.finish_ok(connection_id, name, out)
            }
            Err(e) if e.code.as_deref() == Some(codes::NOT_CONNECTED) => {
                // The backend doesn't have it open (a new launch, or it was never connected).
                vec![Effect::Connect {
                    token,
                    quiet: r.quiet,
                }]
            }
            Err(e) => self.failed(connection_id, name, e, now_ms),
        }
    }

    /// The outcome of [`Effect::Connect`] (the caller already applied
    /// `Connected`, or [`connect_failure`]'s state and prompt).
    pub fn on_connect(
        &mut self,
        connection_id: &str,
        token: u64,
        name: &str,
        result: Result<(), &CmdError>,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(r) = self.live(connection_id, token) else {
            return Vec::new();
        };
        let e = match result {
            Ok(()) => return self.finish_ok(connection_id, name, Vec::new()),
            Err(e) => e,
        };
        if is_retryable(e) {
            return self.failed(connection_id, name, e, now_ms);
        }
        // A refused login (the prompt is up, or skipped in the background) or a bad setting.
        let mut out = Vec::new();
        if is_auth_error(e) {
            if r.quiet {
                out.push(Effect::Unwant);
            }
        } else {
            out.push(Effect::Toast(could_not_connect_toast(
                connection_id,
                name,
                &e.message,
            )));
        }
        self.map.remove(connection_id);
        out.push(Effect::CancelTimer);
        out.push(Effect::Finish { ok: false });
        out
    }

    /// Back: finish, and say so if it showed as reconnecting (`announceBack`).
    fn finish_ok(&mut self, connection_id: &str, name: &str, mut out: Vec<Effect>) -> Vec<Effect> {
        let Some(r) = self.map.remove(connection_id) else {
            return out;
        };
        out.push(Effect::CancelTimer);
        out.push(Effect::Finish { ok: true });
        if r.flagged && r.reason != RecoverReason::Restore {
            out.push(Effect::Toast(reconnected_toast(connection_id, name)));
        } else {
            out.push(Effect::ClearFailureToast);
        }
        out
    }

    /// An attempt failed with a retryable error: schedule the next one, or give up.
    fn failed(
        &mut self,
        connection_id: &str,
        name: &str,
        e: &CmdError,
        now_ms: u64,
    ) -> Vec<Effect> {
        let Some(r) = self.map.get_mut(connection_id) else {
            return Vec::new();
        };
        r.attempt = false;
        if let Some(&delay_ms) = RETRY_DELAYS_MS.get(r.tries) {
            r.tries += 1;
            r.flagged = true;
            return vec![
                Effect::SetState(ConnLinkState::Reconnecting {
                    attempt: u32::try_from(r.tries).unwrap_or(u32::MAX),
                    next_at_ms: now_ms + delay_ms,
                }),
                Effect::Schedule {
                    token: r.token,
                    delay_ms,
                },
            ];
        }
        let Some(r) = self.map.remove(connection_id) else {
            return Vec::new();
        };
        let mut out = vec![
            Effect::SetState(ConnLinkState::Failed),
            Effect::CancelTimer,
            Effect::Finish { ok: false },
        ];
        if !r.background {
            out.push(Effect::Toast(lost_toast(
                connection_id,
                name,
                r.reason,
                &e.message,
            )));
        }
        out
    }

    /// A [`LinkEvent`] from `Backend::link_events` (`onLinkEvent` in store.ts):
    /// `TxLost` → `[TxLost]`; `Lost` of a connection shown as connected →
    /// [`recover`](Self::recover) (reason `Lost`). Returns the connection id
    /// and the effects.
    pub fn on_link_event<'a>(
        &mut self,
        event: &'a LinkEvent,
        state: ConnLinkState,
    ) -> (&'a str, Vec<Effect>) {
        match event {
            LinkEvent::TxLost {
                connection_id,
                database,
            } => (
                connection_id,
                vec![Effect::TxLost {
                    databases: vec![database.clone()],
                }],
            ),
            LinkEvent::Lost { connection_id } => {
                let effects = if state.is_connected() {
                    self.recover(connection_id, RecoverOpts::default())
                } else {
                    Vec::new()
                };
                (connection_id, effects)
            }
        }
    }

    /// Checks every wanted connection (after sleep, or when the network
    /// comes back) (`checkConnections`): ones being retried are retried now,
    /// open ones are pinged (reason `Wake`), failed or reconnecting ones get
    /// another round in the background. `wanted`: (id, state) of each.
    pub fn check(&mut self, wanted: &[(String, ConnLinkState)]) -> Vec<(String, Vec<Effect>)> {
        let mut out = Vec::new();
        for (id, state) in wanted {
            let effects = if self.is_recovering(id) {
                self.retry_now(id)
            } else {
                match state {
                    ConnLinkState::Connected => self.recover(
                        id,
                        RecoverOpts {
                            reason: RecoverReason::Wake,
                            ..RecoverOpts::default()
                        },
                    ),
                    ConnLinkState::Failed | ConnLinkState::Reconnecting { .. } => self.recover(
                        id,
                        RecoverOpts {
                            background: true,
                            ..RecoverOpts::default()
                        },
                    ),
                    _ => Vec::new(),
                }
            };
            out.push((id.clone(), effects));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // ── Rules ──

    #[test]
    fn the_backoff_lasts_about_two_minutes() {
        assert_eq!(RETRY_DELAYS_MS.iter().sum::<u64>(), 120_000);
        assert!(RETRY_DELAYS_MS.windows(2).all(|w| w[0] <= w[1]));
        assert!(!woke_up(0, WAKE_TICK_MS));
        assert!(!woke_up(0, WAKE_GAP_MS));
        assert!(woke_up(0, WAKE_GAP_MS + 1));
    }

    #[test]
    fn which_failures_mean_the_link_dropped() {
        let lost = LinkEvent::Lost {
            connection_id: "c1".into(),
        };
        let tx = LinkEvent::TxLost {
            connection_id: "c1".into(),
            database: "app".into(),
        };
        assert_eq!(
            link_events_for(Some("tx_lost"), "c1", "app"),
            [tx, lost.clone()]
        );
        assert_eq!(
            link_events_for(Some("connection_lost"), "c1", "app"),
            std::slice::from_ref(&lost)
        );
        assert_eq!(link_events_for(Some("unreachable"), "c1", "app"), [lost]);
        for code in [
            None,
            Some("42P01"),
            Some("not_connected"),
            Some("password_required"),
        ] {
            assert!(link_events_for(code, "c1", "app").is_empty(), "{code:?}");
        }
        let e = |code: Option<&str>| CmdError {
            code: code.map(Into::into),
            ..CmdError::msg("x")
        };
        for code in ["password_required", "28P01", "28000"] {
            assert!(is_auth_error(&e(Some(code))));
            assert!(!is_retryable(&e(Some(code))));
        }
        assert!(is_retryable(&e(None)));
        assert!(is_retryable(&e(Some("unreachable"))));
        assert!(!is_retryable(&e(Some("3D000"))));
        let commit = CmdError::code(
            "connection_lost",
            "The connection to the server was lost while committing: the transaction may or may \
             not have been committed",
        );
        assert!(is_commit_unknown(&commit));
        assert!(!is_commit_unknown(&CmdError::code(
            "connection_lost",
            "The connection to the server was lost"
        )));
    }

    #[test]
    fn a_failed_connect_prompts_for_a_password_or_shows_one_status_toast() {
        let how = ConnectAttempt {
            password_given: false,
            has_password: true,
            quiet: false,
            toast: true,
        };
        let refused = CmdError::code("28P01", "password authentication failed");
        let f = connect_failure("c1", "Local", &refused, how);
        assert_eq!(f.state, ConnLinkState::Disconnected);
        assert_eq!(
            f.prompt,
            Some(PasswordPrompt {
                error: Some("password authentication failed".into())
            })
        );
        assert_eq!(f.toast, None);
        // No stored password: ask, without an error line.
        let need = CmdError::code("password_required", "The server asked for a password");
        let f = connect_failure(
            "c1",
            "Local",
            &need,
            ConnectAttempt {
                has_password: false,
                ..how
            },
        );
        assert_eq!(f.prompt, Some(PasswordPrompt { error: None }));
        // In the background: never ask.
        let f = connect_failure(
            "c1",
            "Local",
            &refused,
            ConnectAttempt { quiet: true, ..how },
        );
        assert_eq!((f.prompt, f.toast), (None, None));
        // Other errors: the connection's status toast (unless the reconnect loop connects).
        let down = CmdError::code("unreachable", "Connection refused");
        let f = connect_failure("c1", "Local", &down, how);
        assert_eq!(f.state, ConnLinkState::Failed);
        let t = f.toast.unwrap();
        assert_eq!(
            (t.key.as_deref(), t.title.as_str(), t.body.as_deref()),
            (
                Some("conn:c1"),
                "Could not connect to Local",
                Some("Connection refused")
            )
        );
        let f = connect_failure(
            "c1",
            "Local",
            &down,
            ConnectAttempt {
                toast: false,
                ..how
            },
        );
        assert_eq!(f.toast, None);
    }

    // ── describe('reconnecting') (state/reconnect.test.ts) ──
    //
    // A small driver plays the store's part: it performs the effects at
    // once (ping / connect answer from a script), keeps timers on a fake
    // clock, and holds keyed toasts like pushToast (same key: replaced).

    type Ping = Result<Vec<String>, CmdError>;

    struct Sim {
        rec: Recoveries,
        state: ConnLinkState,
        wanted: bool,
        prompt: Option<PasswordPrompt>,
        has_password: bool,
        toasts: Vec<Toast>,
        tx_idle: Vec<String>,
        now: u64,
        timer: Option<(u64, u64)>,
        pings: VecDeque<Ping>,
        ping_default: Ping,
        ping_calls: usize,
        connect: Result<(), CmdError>,
        connect_calls: usize,
        finished: Vec<bool>,
    }

    fn unreachable() -> CmdError {
        CmdError::code("unreachable", "Connection refused")
    }

    impl Sim {
        fn new(state: ConnLinkState) -> Self {
            Self {
                rec: Recoveries::new(),
                state,
                wanted: true,
                prompt: None,
                has_password: true,
                toasts: Vec::new(),
                tx_idle: Vec::new(),
                now: 0,
                timer: None,
                pings: VecDeque::new(),
                ping_default: Ok(Vec::new()),
                ping_calls: 0,
                connect: Ok(()),
                connect_calls: 0,
                finished: Vec::new(),
            }
        }

        fn push_toast(&mut self, t: Toast) {
            match t
                .key
                .as_ref()
                .and_then(|k| self.toasts.iter().position(|x| x.key.as_ref() == Some(k)))
            {
                Some(i) => self.toasts[i] = t,
                None => self.toasts.push(t),
            }
        }

        fn titles(&self) -> Vec<&str> {
            self.toasts.iter().map(|t| t.title.as_str()).collect()
        }

        fn run(&mut self, effects: Vec<Effect>) {
            for e in effects {
                match e {
                    Effect::Ping { token } => {
                        self.ping_calls += 1;
                        let answer = self
                            .pings
                            .pop_front()
                            .unwrap_or_else(|| self.ping_default.clone());
                        let next = match &answer {
                            Ok(lost) => self.rec.on_ping("c1", token, "Local", Ok(lost), self.now),
                            Err(e) => self.rec.on_ping("c1", token, "Local", Err(e), self.now),
                        };
                        self.run(next);
                    }
                    Effect::Connect { token, quiet } => {
                        self.connect_calls += 1;
                        self.state = ConnLinkState::Connecting;
                        let answer = self.connect.clone();
                        match &answer {
                            Ok(()) => {
                                self.state = ConnLinkState::Connected;
                                self.wanted = true;
                                self.clear_failure_toast();
                            }
                            Err(e) => {
                                let f = connect_failure(
                                    "c1",
                                    "Local",
                                    e,
                                    ConnectAttempt {
                                        password_given: false,
                                        has_password: self.has_password,
                                        quiet,
                                        toast: false,
                                    },
                                );
                                self.state = f.state;
                                self.prompt = f.prompt;
                                assert_eq!(f.toast, None);
                            }
                        }
                        let next = self.rec.on_connect(
                            "c1",
                            token,
                            "Local",
                            answer.as_ref().map(|_| ()),
                            self.now,
                        );
                        self.run(next);
                    }
                    Effect::Schedule { token, delay_ms } => {
                        self.timer = Some((self.now + delay_ms, token))
                    }
                    Effect::CancelTimer => self.timer = None,
                    Effect::SetState(s) => self.state = s,
                    Effect::Toast(t) => self.push_toast(t),
                    Effect::ClearFailureToast => self.clear_failure_toast(),
                    Effect::TxLost { databases } => {
                        for db in databases {
                            self.push_toast(tx_lost_toast("Local", &db));
                            self.tx_idle.push(db);
                        }
                    }
                    Effect::Unwant => self.wanted = false,
                    Effect::Finish { ok } => self.finished.push(ok),
                }
            }
        }

        fn clear_failure_toast(&mut self) {
            let key = conn_toast_key("c1");
            self.toasts.retain(|t| {
                !(t.key.as_deref() == Some(key.as_str()) && t.kind == ToastKind::Error)
            });
        }

        fn recover(&mut self, opts: RecoverOpts) {
            let e = self.rec.recover("c1", opts);
            self.run(e);
        }

        fn check(&mut self) {
            let wanted = if self.wanted {
                vec![("c1".to_owned(), self.state)]
            } else {
                Vec::new()
            };
            for (_, effects) in self.rec.check(&wanted) {
                self.run(effects);
            }
        }

        fn advance(&mut self, ms: u64) {
            let end = self.now + ms;
            while let Some((due, token)) = self.timer.filter(|(due, _)| *due <= end) {
                self.now = due;
                self.timer = None;
                let e = self.rec.on_timer("c1", token);
                self.run(e);
            }
            self.now = end;
        }

        /// `disconnect`: stop recovering, forget it is wanted.
        fn disconnect(&mut self) {
            if self.rec.stop("c1") {
                self.timer = None;
                self.finished.push(self.state.is_connected());
            }
            self.wanted = false;
            self.clear_failure_toast();
            self.state = ConnLinkState::Disconnected;
        }

        fn run_out_of_retries(&mut self) {
            for ms in RETRY_DELAYS_MS {
                self.advance(ms);
            }
        }
    }

    fn opts(reason: RecoverReason) -> RecoverOpts {
        RecoverOpts {
            reason,
            ..RecoverOpts::default()
        }
    }

    #[test]
    fn keeps_a_connection_that_still_answers_without_showing_anything() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.recover(opts(RecoverReason::Wake));
        assert_eq!(s.ping_calls, 1);
        assert_eq!(s.state, ConnLinkState::Connected);
        assert!(s.toasts.is_empty());
        assert_eq!(s.finished, [true]);
        assert!(!s.rec.is_recovering("c1"));
    }

    #[test]
    fn shows_reconnecting_while_the_server_is_away_and_says_when_it_is_back() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.pings = VecDeque::from([Err(unreachable()), Err(unreachable())]);
        s.recover(RecoverOpts::default());
        assert_eq!(
            s.state,
            ConnLinkState::Reconnecting {
                attempt: 1,
                next_at_ms: RETRY_DELAYS_MS[0]
            }
        );
        s.advance(RETRY_DELAYS_MS[0]);
        assert_eq!(s.state.as_str(), "reconnecting");
        assert_eq!(
            s.state,
            ConnLinkState::Reconnecting {
                attempt: 2,
                next_at_ms: RETRY_DELAYS_MS[0] + RETRY_DELAYS_MS[1]
            }
        );
        s.advance(RETRY_DELAYS_MS[1]);
        assert_eq!(s.finished, [true]);
        assert_eq!(s.ping_calls, 3);
        assert_eq!(s.state, ConnLinkState::Connected);
        assert_eq!(s.titles(), ["Reconnected to Local"]);
        assert_eq!(s.toasts[0].kind, ToastKind::Success);
    }

    #[test]
    fn gives_up_after_the_last_retry_offering_to_retry() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.ping_default = Err(unreachable());
        s.recover(RecoverOpts::default());
        s.run_out_of_retries();
        assert_eq!(s.finished, [false]);
        assert_eq!(s.state, ConnLinkState::Failed);
        let t = &s.toasts[0];
        assert_eq!(t.title, "Lost connection to Local");
        assert_eq!(t.body.as_deref(), Some("Connection refused"));
        assert_eq!(t.action.as_ref().map(ToastAction::label), Some("Retry"));
        assert_eq!(
            t.action,
            Some(ToastAction::Retry {
                connection_id: "c1".into()
            })
        );
        assert_eq!(t.ttl_ms, 0);
        assert_eq!(s.ping_calls, RETRY_DELAYS_MS.len() + 1);
    }

    #[test]
    fn failing_again_replaces_the_lost_connection_toast_instead_of_stacking_another() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.ping_default = Err(unreachable());
        for _ in 0..2 {
            s.recover(RecoverOpts::default());
            s.run_out_of_retries();
        }
        assert_eq!(s.finished, [false, false]);
        assert_eq!(s.titles(), ["Lost connection to Local"]);
    }

    #[test]
    fn a_background_retry_of_a_failed_connection_gives_up_without_a_new_toast() {
        let mut s = Sim::new(ConnLinkState::Failed);
        s.ping_default = Err(unreachable());
        s.check();
        s.run_out_of_retries();
        assert_eq!(s.ping_calls, RETRY_DELAYS_MS.len() + 1);
        assert_eq!(s.state, ConnLinkState::Failed);
        assert!(s.toasts.is_empty());
    }

    #[test]
    fn retry_during_a_background_retry_shows_how_it_ends() {
        let mut s = Sim::new(ConnLinkState::Failed);
        s.ping_default = Err(unreachable());
        s.check();
        // Joins the running recovery (no second ping) and makes it say how it ends.
        s.recover(RecoverOpts::default());
        assert_eq!(s.ping_calls, 1);
        s.run_out_of_retries();
        assert_eq!(s.finished, [false]);
        assert_eq!(s.titles(), ["Lost connection to Local"]);
    }

    #[test]
    fn getting_back_replaces_the_lost_connection_toast() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.ping_default = Err(unreachable());
        s.recover(RecoverOpts::default());
        s.run_out_of_retries();
        s.ping_default = Ok(Vec::new());
        s.pings = VecDeque::from([Err(unreachable())]);
        s.check();
        s.advance(RETRY_DELAYS_MS[0]);
        assert_eq!(s.state, ConnLinkState::Connected);
        assert_eq!(s.titles(), ["Reconnected to Local"]);
    }

    #[test]
    fn back_on_the_first_try_drops_the_lost_connection_toast() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.ping_default = Err(unreachable());
        s.recover(RecoverOpts::default());
        s.run_out_of_retries();
        s.ping_default = Ok(Vec::new());
        s.check();
        assert_eq!(s.state, ConnLinkState::Connected);
        assert!(s.toasts.is_empty());
    }

    #[test]
    fn connects_again_when_the_backend_no_longer_has_the_connection_open() {
        let mut s = Sim::new(ConnLinkState::Disconnected);
        s.ping_default = Err(CmdError::code("not_connected", "Not connected"));
        s.recover(opts(RecoverReason::Restore));
        assert_eq!(s.finished, [true]);
        assert_eq!(s.connect_calls, 1);
        assert_eq!(s.state, ConnLinkState::Connected);
        assert!(s.toasts.is_empty());
    }

    #[test]
    fn stops_on_a_refused_login_skipping_the_password_prompt_in_the_background() {
        let mut s = Sim::new(ConnLinkState::Disconnected);
        s.has_password = false;
        s.ping_default = Err(CmdError::code("not_connected", "Not connected"));
        s.connect = Err(CmdError::code("password_required", "no password"));
        s.recover(RecoverOpts {
            reason: RecoverReason::Restore,
            quiet: true,
            background: false,
        });
        assert_eq!(s.finished, [false]);
        assert_eq!(s.prompt, None);
        assert!(!s.wanted);
        assert!(!s.rec.is_recovering("c1"));
    }

    #[test]
    fn warns_about_transactions_rolled_back_with_a_dropped_session() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.ping_default = Ok(vec!["app".into()]);
        s.recover(opts(RecoverReason::Wake));
        assert_eq!(s.tx_idle, ["app"]);
        assert_eq!(s.titles(), ["Transaction rolled back"]);
        assert_eq!(
            s.toasts[0].body.as_deref(),
            Some(
                "The connection to Local (app) was lost, so the server rolled back the open \
                 transaction. Uncommitted changes are gone."
            )
        );
        assert_eq!(s.toasts[0].ttl_ms, 0);
    }

    #[test]
    fn starts_reconnecting_when_a_call_fails_on_the_network() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.ping_default = Err(unreachable());
        // What Backend sends for a run_query that failed with connection_lost.
        for event in link_events_for(Some("connection_lost"), "c1", "app") {
            let (id, effects) = s.rec.on_link_event(&event, s.state);
            assert_eq!(id, "c1");
            s.run(effects);
        }
        assert_eq!(s.state.as_str(), "reconnecting");
        // More failures meanwhile join the same recovery.
        let (_, effects) = s.rec.on_link_event(
            &LinkEvent::Lost {
                connection_id: "c1".into(),
            },
            ConnLinkState::Connected,
        );
        assert!(effects.is_empty());
        // Not shown as connected: a lost link starts nothing.
        let mut idle = Recoveries::new();
        let (_, effects) = idle.on_link_event(
            &LinkEvent::Lost {
                connection_id: "c2".into(),
            },
            ConnLinkState::Disconnected,
        );
        assert!(effects.is_empty());
        // tx_lost also reports the rolled-back transaction.
        let (_, effects) = idle.on_link_event(
            &LinkEvent::TxLost {
                connection_id: "c2".into(),
                database: "db".into(),
            },
            ConnLinkState::Connected,
        );
        assert_eq!(
            effects,
            [Effect::TxLost {
                databases: vec!["db".into()]
            }]
        );
    }

    #[test]
    fn a_wake_up_retries_a_failed_wanted_connection_right_away() {
        let mut s = Sim::new(ConnLinkState::Failed);
        s.check();
        assert_eq!(s.ping_calls, 1);
        assert_eq!(s.state, ConnLinkState::Connected);
    }

    #[test]
    fn disconnecting_stops_reconnecting_and_forgets_the_connection_was_wanted() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.ping_default = Err(unreachable());
        s.recover(RecoverOpts::default());
        let Some((_, token)) = s.timer else {
            panic!("a retry is scheduled")
        };
        s.disconnect();
        assert_eq!(s.finished, [false]);
        s.advance(60_000);
        // A timer that still fired is ignored.
        assert!(s.rec.on_timer("c1", token).is_empty());
        assert!(s.rec.on_ping("c1", token, "Local", Ok(&[]), 0).is_empty());
        assert_eq!(s.ping_calls, 1);
        assert_eq!(s.state, ConnLinkState::Disconnected);
        assert!(!s.wanted);
    }

    // Beyond the TS cases.

    #[test]
    fn retry_now_runs_the_pending_attempt_at_once() {
        let mut s = Sim::new(ConnLinkState::Connected);
        s.pings = VecDeque::from([Err(unreachable())]);
        s.recover(RecoverOpts::default());
        assert!(s.timer.is_some());
        let e = s.rec.retry_now("c1");
        assert_eq!(e[0], Effect::CancelTimer);
        s.run(e);
        assert_eq!(s.ping_calls, 2);
        assert_eq!(s.state, ConnLinkState::Connected);
        assert_eq!(s.titles(), ["Reconnected to Local"]);
        // Nothing running: nothing to do.
        assert!(s.rec.retry_now("c1").is_empty());
    }

    #[test]
    fn a_bad_setting_during_recovery_shows_could_not_connect_and_stops() {
        let mut s = Sim::new(ConnLinkState::Disconnected);
        s.ping_default = Err(CmdError::code("not_connected", "Not connected"));
        s.connect = Err(CmdError::code("3D000", "database \"gone\" does not exist"));
        s.recover(opts(RecoverReason::Restore));
        assert_eq!(s.finished, [false]);
        assert_eq!(s.state, ConnLinkState::Failed);
        assert_eq!(s.titles(), ["Could not connect to Local"]);
        assert_eq!(s.toasts[0].key.as_deref(), Some("conn:c1"));
        assert!(s.wanted);
    }

    #[test]
    fn restoring_at_launch_gives_up_with_could_not_connect_and_no_reconnected_toast() {
        let mut s = Sim::new(ConnLinkState::Disconnected);
        s.ping_default = Err(CmdError::code("not_connected", "Not connected"));
        s.connect = Err(unreachable());
        s.recover(RecoverOpts {
            reason: RecoverReason::Restore,
            quiet: true,
            background: false,
        });
        assert_eq!(s.state.as_str(), "reconnecting");
        s.connect = Ok(());
        s.advance(RETRY_DELAYS_MS[0]);
        assert_eq!(s.state, ConnLinkState::Connected);
        assert!(
            s.toasts.is_empty(),
            "restoring says nothing when it gets there"
        );
        s.connect = Err(unreachable());
        s.state = ConnLinkState::Disconnected;
        s.recover(RecoverOpts {
            reason: RecoverReason::Restore,
            quiet: true,
            background: false,
        });
        s.run_out_of_retries();
        assert_eq!(s.titles(), ["Could not connect to Local"]);
        assert_eq!(s.connect_calls, 3 + RETRY_DELAYS_MS.len());
    }
}
