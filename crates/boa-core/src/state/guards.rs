//! Port of \`ui/src/state/guards.ts\` (the decisions; the dialogs are
//! \`components/GuardDialogs.tsx\`, whose texts are here too), plus the quit
//! rule of the Tauri \`lib.rs\` (\`RunEvent::ExitRequested\`).
//!
//! Confirmations the guard rails ask for:
//! - before a destructive statement (\`sql::guard::destructive_risks\`), unless
//!   turned off in Settings; production-tagged connections always ask;
//! - nothing written on a read-only connection may turn read-only off
//!   (\`sql::guard::read_only_escape\`): not run, with a message;
//! - before disconnecting, reconnecting, quitting or restarting to update
//!   with a transaction open (rolled back if the user goes ahead);
//! - leaving Manual mode with a transaction open: commit or roll back first.
//!
//! The GPUI \`Transactions\` / guard dialogs show one prompt at a time; a new
//! one answers the earlier one "no" (\`ask\` in guards.ts).

use super::tx::{OpenTx, TxSessions, TxState};
use crate::sql::guard::{ConfirmOptions, Risk, destructive_risks, needs_confirm, read_only_escape};

// ── Running a statement ───────────────────────────────────────────────────

/// What the guard rails know about a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunContext {
    /// The connection is read-only.
    pub read_only: bool,
    /// The connection is tagged Production (\`grid::conn_tag::is_production\`).
    pub production: bool,
    /// The "confirm destructive statements" preference.
    pub confirm_destructive: bool,
}

/// The guard rails' answer before running a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunGuard {
    /// Nothing risky (or confirmation off): run it.
    Go,
    /// Not run, logged and shown as this error (\`Not run: …\`).
    Blocked(String),
    /// Ask first ("Run a destructive statement?"), listing these risks.
    Confirm(Vec<Risk>),
}

/// Checks \`sql\` (already trimmed) before running it, as \`runQuery\` does:
/// the read-only escape first, then the destructive-statement confirmation
/// (\`confirmRun\`).
pub fn check_run(sql: &str, ctx: RunContext) -> RunGuard {
    if ctx.read_only
        && let Some(why) = read_only_escape(sql)
    {
        return RunGuard::Blocked(format!("Not run: {why}"));
    }
    let risks = destructive_risks(sql);
    let confirm = needs_confirm(
        &risks,
        ConfirmOptions {
            confirm_destructive: ctx.confirm_destructive,
            production: ctx.production,
        },
    );
    if confirm {
        RunGuard::Confirm(risks)
    } else {
        RunGuard::Go
    }
}

/// [`check_run`] for a Redis console: read-only is the backend's to enforce
/// (it knows which commands write); flushing / swapping databases (and, on
/// production, deleting keys) asks first.
pub fn check_redis_run(text: &str, ctx: RunContext) -> RunGuard {
    let risks = crate::redis::destructive_risks(text, ctx.production);
    let confirm = needs_confirm(
        &risks,
        ConfirmOptions {
            confirm_destructive: ctx.confirm_destructive,
            production: ctx.production,
        },
    );
    if confirm {
        RunGuard::Confirm(risks)
    } else {
        RunGuard::Go
    }
}

/// Title of the run confirmation.
pub fn run_confirm_title(production: bool) -> &'static str {
    if production {
        "Run a destructive statement on production?"
    } else {
        "Run a destructive statement?"
    }
}

/// The run confirmation's statement box: each risky statement once, in
/// order, separated by \`;\` and a blank line.
pub fn risk_statements(risks: &[Risk]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for r in risks {
        if !seen.contains(&r.statement.as_str()) {
            seen.push(&r.statement);
        }
    }
    seen.join(";\n\n")
}

/// The run confirmation's buttons: Cancel (focused) and this (destructive).
pub const RUN_CONFIRM_LABEL: &str = "Run";

// ── Closing sessions with a transaction open ──────────────────────────────

/// What closes the sessions (\`CloseAction\`; \`Update\`: restarting to install an update).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseAction {
    Disconnect,
    Reconnect,
    Quit,
    Update,
}

impl CloseAction {
    /// Dialog title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Disconnect => "Disconnect with an open transaction?",
            Self::Reconnect => "Reconnect with an open transaction?",
            Self::Quit => "Quit with an open transaction?",
            Self::Update => "Restart to update with an open transaction?",
        }
    }

    /// The destructive button.
    pub fn confirm_label(self) -> &'static str {
        match self {
            Self::Disconnect => "Roll back and disconnect",
            Self::Reconnect => "Roll back and reconnect",
            Self::Quit => "Roll back and quit",
            Self::Update => "Roll back and restart",
        }
    }
}

/// The close dialog's description, above the list of open transactions.
pub const CLOSE_TX_DESCRIPTION: &str =
    "Uncommitted changes in these transactions will be rolled back:";

/// The transactions to warn about before closing the sessions of
/// \`connection_id\` (all connections when \`None\`): empty means go ahead
/// without asking (\`confirmCloseTx\`).
pub fn close_tx_open(sessions: &TxSessions, connection_id: Option<&str>) -> Vec<OpenTx> {
    sessions.open_transactions(connection_id)
}

/// The state shown next to a listed transaction: "aborted" or "open".
pub fn open_tx_label(state: TxState) -> &'static str {
    if state == TxState::Failed {
        "aborted"
    } else {
        "open"
    }
}

// ── Leaving Manual mode ───────────────────────────────────────────────────

/// The user's answer when leaving Manual mode with a transaction open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndTxChoice {
    Commit,
    Rollback,
}

/// Leaving Manual mode needs a choice first (\`chooseEndTx\` is only asked then).
pub fn end_tx_needed(state: TxState) -> bool {
    state != TxState::Idle
}

/// Title of the end-transaction prompt.
pub const END_TX_TITLE: &str = "Switch to auto-commit?";

/// Its description.
pub fn end_tx_description(database: &str, state: TxState) -> String {
    if state == TxState::Failed {
        format!("The transaction on {database} failed; it is rolled back first.")
    } else {
        format!("A transaction is open on {database}. Commit or roll it back first.")
    }
}

/// Its choices besides Cancel: an aborted transaction can only be rolled back.
pub fn end_tx_choices(state: TxState) -> &'static [EndTxChoice] {
    if state == TxState::Failed {
        &[EndTxChoice::Rollback]
    } else {
        &[EndTxChoice::Rollback, EndTxChoice::Commit]
    }
}

/// ⌘⇧↵ (commit) / ⌘⇧⌫ (rollback) act on the active database's transaction:
/// only when one is open, and an aborted one can't be committed.
pub fn tx_shortcut_applies(state: TxState, commit: bool) -> bool {
    match state {
        TxState::Idle => false,
        TxState::Failed => !commit,
        TxState::Active => true,
    }
}

// ── Quitting ──────────────────────────────────────────────────────────────

/// A second quit request within this many ms goes through without asking,
/// so a stuck UI can't keep the app open.
pub const QUIT_AGAIN_MS: u64 = 3000;

/// What to do with a quit request (⌘Q, the Quit menu, system logout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitDecision {
    /// Hold the exit; check open transactions and ask ([\`CloseAction::Quit\`]),
    /// then quit (rolling them back) unless cancelled.
    Ask,
    /// Asked less than [\`QUIT_AGAIN_MS\`] ago: quit now.
    Quit,
}

/// The quit rule of the Tauri app's \`RunEvent::ExitRequested\` handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuitGate {
    /// When the UI was last asked (ms since the epoch); 0: never.
    last_asked_ms: u64,
}

impl QuitGate {
    /// A quit request at \`now_ms\`.
    pub fn request(&mut self, now_ms: u64) -> QuitDecision {
        let last = std::mem::replace(&mut self.last_asked_ms, now_ms);
        if now_ms.saturating_sub(last) > QUIT_AGAIN_MS {
            QuitDecision::Ask
        } else {
            QuitDecision::Quit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tx::TxPatch;
    use super::*;
    use crate::sql::guard::RiskKind;

    const ON: RunContext = RunContext {
        read_only: false,
        production: false,
        confirm_destructive: true,
    };

    #[test]
    fn destructive_statements_ask_unless_turned_off_production_always_asks() {
        let sql = "delete from users";
        let RunGuard::Confirm(risks) = check_run(sql, ON) else {
            panic!("should ask");
        };
        assert_eq!(risks[0].kind, RiskKind::NoWhere);
        let off = RunContext {
            confirm_destructive: false,
            ..ON
        };
        assert_eq!(check_run(sql, off), RunGuard::Go);
        let prod = RunContext {
            production: true,
            ..off
        };
        assert!(matches!(check_run(sql, prod), RunGuard::Confirm(_)));
        assert_eq!(
            check_run("delete from users where id = 1", prod),
            RunGuard::Go
        );
        assert_eq!(check_run("select 1", ON), RunGuard::Go);
    }

    #[test]
    fn read_only_connections_refuse_escapes_before_anything_else() {
        let ro = RunContext {
            read_only: true,
            ..ON
        };
        let RunGuard::Blocked(msg) = check_run("set default_transaction_read_only = off", ro)
        else {
            panic!("should block");
        };
        assert!(msg.starts_with("Not run: "), "{msg}");
        assert_eq!(
            msg,
            format!(
                "Not run: {}",
                read_only_escape("set default_transaction_read_only = off").unwrap()
            )
        );
        // A writable connection may.
        assert_eq!(
            check_run("set default_transaction_read_only = off", ON),
            RunGuard::Go
        );
        // Destructive statements on a read-only connection still ask (the server refuses them anyway).
        assert!(matches!(
            check_run("drop table t", ro),
            RunGuard::Confirm(_)
        ));
    }

    #[test]
    fn confirmation_texts() {
        assert_eq!(
            run_confirm_title(true),
            "Run a destructive statement on production?"
        );
        assert_eq!(run_confirm_title(false), "Run a destructive statement?");
        let risks = destructive_risks("drop table a, b; truncate c");
        assert!(risks.len() >= 2);
        let text = risk_statements(&risks);
        assert_eq!(text.matches("drop table a, b").count(), 1, "{text}");
        assert!(text.contains(";\n\ntruncate c"), "{text}");
    }

    #[test]
    fn closing_asks_only_with_a_transaction_open() {
        let mut tx = TxSessions::default();
        tx.note("c1", "app", TxPatch::state(TxState::Idle));
        assert!(close_tx_open(&tx, None).is_empty());
        tx.note("c1", "app", TxPatch::state(TxState::Active));
        tx.note("c2", "db", TxPatch::state(TxState::Failed));
        assert_eq!(close_tx_open(&tx, None).len(), 2);
        let c2 = close_tx_open(&tx, Some("c2"));
        assert_eq!(c2.len(), 1);
        assert_eq!(open_tx_label(c2[0].state), "aborted");
        assert_eq!(open_tx_label(TxState::Active), "open");
        assert!(close_tx_open(&tx, Some("c3")).is_empty());
        assert_eq!(CloseAction::Quit.title(), "Quit with an open transaction?");
        assert_eq!(CloseAction::Update.confirm_label(), "Roll back and restart");
        assert_eq!(
            CloseAction::Reconnect.confirm_label(),
            "Roll back and reconnect"
        );
        assert_eq!(
            CloseAction::Disconnect.title(),
            "Disconnect with an open transaction?"
        );
    }

    #[test]
    fn leaving_manual_mode() {
        assert!(!end_tx_needed(TxState::Idle));
        assert!(end_tx_needed(TxState::Active) && end_tx_needed(TxState::Failed));
        assert_eq!(end_tx_choices(TxState::Failed), [EndTxChoice::Rollback]);
        assert_eq!(
            end_tx_choices(TxState::Active),
            [EndTxChoice::Rollback, EndTxChoice::Commit]
        );
        assert_eq!(
            end_tx_description("app", TxState::Failed),
            "The transaction on app failed; it is rolled back first."
        );
        assert_eq!(
            end_tx_description("app", TxState::Active),
            "A transaction is open on app. Commit or roll it back first."
        );
    }

    #[test]
    fn commit_and_rollback_shortcuts() {
        assert!(!tx_shortcut_applies(TxState::Idle, true));
        assert!(!tx_shortcut_applies(TxState::Idle, false));
        assert!(tx_shortcut_applies(TxState::Active, true));
        assert!(tx_shortcut_applies(TxState::Active, false));
        assert!(!tx_shortcut_applies(TxState::Failed, true));
        assert!(tx_shortcut_applies(TxState::Failed, false));
    }

    #[test]
    fn a_second_quit_within_3_s_goes_through() {
        let mut gate = QuitGate::default();
        let t = 1_700_000_000_000;
        assert_eq!(gate.request(t), QuitDecision::Ask);
        assert_eq!(gate.request(t + 2_999), QuitDecision::Quit);
        // Each request restarts the window.
        assert_eq!(gate.request(t + 5_000), QuitDecision::Quit);
        assert_eq!(gate.request(t + 8_001), QuitDecision::Ask);
        assert_eq!(gate.request(t + 8_001 + QUIT_AGAIN_MS), QuitDecision::Quit);
    }
}
