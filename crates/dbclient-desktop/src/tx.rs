//! Transaction state of a session, as the server sees it.
//!
//! tokio-postgres does not expose the ReadyForQuery status byte, so after
//! every statement the session is probed with one cheap query:
//! `now()` (the transaction's start) differs from `statement_timestamp()`
//! only inside an explicit transaction block, and any query fails with
//! 25P02 while that block is aborted. The same probe reads the read-only
//! settings, so a read-only session that was talked out of it (`SET
//! default_transaction_read_only = off`, `BEGIN READ WRITE`) is put back.

use serde::Serialize;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio_postgres::{Client, SimpleQueryMessage};

/// What the UI shows next to "Tx: Auto / Manual".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TxState {
    /// No transaction block open (auto-commit).
    #[default]
    Idle,
    /// Inside BEGIN … COMMIT.
    Active,
    /// Inside a block that hit an error: everything fails with 25P02 until ROLLBACK.
    Failed,
}

impl TxState {
    const fn to_u8(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Active => 1,
            Self::Failed => 2,
        }
    }

    const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Active,
            2 => Self::Failed,
            _ => Self::Idle,
        }
    }
}

/// A `TxState` shared between the tasks using one session.
#[derive(Debug, Default)]
pub struct TxCell(AtomicU8);

impl TxCell {
    pub fn get(&self) -> TxState {
        TxState::from_u8(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, s: TxState) {
        self.0.store(s.to_u8(), Ordering::Relaxed);
    }
}

pub const PROBE: &str = "select now() <> statement_timestamp(), \
     current_setting('transaction_read_only'), \
     current_setting('default_transaction_read_only')";

/// Result of `PROBE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub state: TxState,
    /// The session or its open transaction can write although it should not.
    pub writable: bool,
}

/// Reads one `PROBE` reply (`t`/`f`, `on`/`off`, `on`/`off`).
pub fn parse_probe(in_block: Option<&str>, tx_ro: Option<&str>, default_ro: Option<&str>) -> Probe {
    let state = if in_block == Some("t") {
        TxState::Active
    } else {
        TxState::Idle
    };
    let writable = default_ro == Some("off") || (state == TxState::Active && tx_ro == Some("off"));
    Probe { state, writable }
}

/// Probes the session. Aborted block: `Failed`; a dead connection reads as `Idle`.
pub async fn probe(client: &Client) -> Probe {
    match client.simple_query(PROBE).await {
        Ok(msgs) => {
            for m in msgs {
                if let SimpleQueryMessage::Row(r) = m {
                    return parse_probe(r.get(0), r.get(1), r.get(2));
                }
            }
            Probe {
                state: TxState::Idle,
                writable: false,
            }
        }
        Err(e) => Probe {
            state: if e.code().is_some_and(|c| c.code() == "25P02") {
                TxState::Failed
            } else {
                TxState::Idle
            },
            writable: false,
        },
    }
}

/// In Manual mode a transaction is opened before every statement that runs
/// outside one, except transaction control itself and the commands Postgres
/// refuses inside a block (`kw`: first keyword, `sql`: the statement).
pub fn opens_tx(kw: &str, sql: &str) -> bool {
    match kw {
        "begin" | "start" | "commit" | "end" | "rollback" | "abort" | "savepoint" | "release"
        | "prepare" | "vacuum" | "checkpoint" | "discard" => false,
        "create" | "drop" | "alter" | "reindex" | "cluster" => {
            let words: Vec<String> = sql
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .filter(|w| !w.is_empty())
                .take(4)
                .map(str::to_ascii_lowercase)
                .collect();
            let has = |w: &str| words.iter().any(|x| x == w);
            !(has("database") || has("tablespace") || has("concurrently") || has("system"))
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{TxCell, TxState, opens_tx, parse_probe};

    #[test]
    fn probe_reply() {
        let p = parse_probe(Some("t"), Some("off"), Some("off"));
        assert_eq!(p.state, TxState::Active);
        assert!(p.writable);
        let p = parse_probe(Some("f"), Some("on"), Some("on"));
        assert_eq!(p.state, TxState::Idle);
        assert!(!p.writable);
        // A read-only session inside BEGIN READ WRITE.
        assert!(parse_probe(Some("t"), Some("off"), Some("on")).writable);
        // Outside a block, transaction_read_only is the probe's own transaction.
        assert!(!parse_probe(Some("f"), Some("off"), Some("on")).writable);
    }

    #[test]
    fn cell_round_trips() {
        let c = TxCell::default();
        assert_eq!(c.get(), TxState::Idle);
        for s in [TxState::Active, TxState::Failed, TxState::Idle] {
            c.set(s);
            assert_eq!(c.get(), s);
        }
    }

    #[test]
    fn manual_mode_skips_what_cannot_run_in_a_block() {
        assert!(opens_tx("insert", "insert into t values (1)"));
        assert!(opens_tx("select", "select 1"));
        assert!(opens_tx("create", "create table t (id int)"));
        assert!(!opens_tx("commit", "commit"));
        assert!(!opens_tx("begin", "begin"));
        assert!(!opens_tx("vacuum", "vacuum full t"));
        assert!(!opens_tx("create", "CREATE DATABASE x"));
        assert!(!opens_tx("create", "create index concurrently i on t (a)"));
        assert!(!opens_tx("alter", "alter system set work_mem = '8MB'"));
        assert!(!opens_tx("drop", "drop database if exists x"));
    }
}
