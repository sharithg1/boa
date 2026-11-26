//! Several operations on one session at once (live; skipped without PGHOST):
//! they take turns (`ops`) instead of interleaving their steps, and each
//! one can still be cancelled, running or waiting.

use super::tests::live_config;
use super::{CancelHandle, CsvOptions, Pg};
use crate::error::CmdResult;
use crate::tx::TxState;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A cursor statement whose fetch takes about half a second (3 rows).
const SLOW_ROWS: &str = "select g from generate_series(1, 3) g, pg_sleep(0.5)";
const LONG: &str = "select pg_sleep(30)";
const CSV: CsvOptions = CsvOptions {
    delimiter: ',',
    header: true,
};

async fn connected() -> Option<(Arc<Pg>, String)> {
    let Some((config, password)) = live_config() else {
        eprintln!("PGHOST not set: skipping live Postgres test");
        return None;
    };
    let db = config.database.clone();
    let pg = Arc::new(Pg::default());
    pg.connect(config, password).await.unwrap();
    Some((pg, db))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("boa-conc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn export(
    pg: &Arc<Pg>,
    db: &str,
    sql: &str,
    dest: &Path,
    cancel: &CancelHandle,
) -> tokio::task::JoinHandle<CmdResult<super::ExportResult>> {
    let (pg, db, sql, dest, cancel) = (
        pg.clone(),
        db.to_owned(),
        sql.to_owned(),
        dest.to_owned(),
        cancel.clone(),
    );
    tokio::spawn(async move {
        pg.export_csv_with("test", &db, &sql, &dest, CSV, |_| {}, &cancel)
            .await
    })
}

fn query(
    pg: &Arc<Pg>,
    db: &str,
    sql: &str,
    offset: usize,
    cancel: &CancelHandle,
) -> tokio::task::JoinHandle<CmdResult<super::QueryResult>> {
    let (pg, db, sql, cancel) = (pg.clone(), db.to_owned(), sql.to_owned(), cancel.clone());
    tokio::spawn(async move { pg.query_with("test", &db, &sql, 10, offset, &cancel).await })
}

/// Waits (≤ 5 s) until `h`'s operation holds its session.
async fn until_running(h: &CancelHandle) {
    for _ in 0..250 {
        if h.is_running() {
            // Past the claim: give the statement time to reach the server.
            tokio::time::sleep(Duration::from_millis(150)).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the operation never started");
}

async fn within<T>(secs: u64, f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(secs), f)
        .await
        .expect("timed out")
}

fn csv_rows(path: &Path) -> usize {
    std::fs::read_to_string(path).unwrap().lines().count() - 1
}

/// The reported bug: an export started while a console statement runs on
/// the same database broke it ("cursor \"__boa\" does not exist"). Now
/// both succeed, in either order, with explorer reads and pages mixed in.
#[tokio::test(flavor = "multi_thread")]
async fn live_run_and_export_on_one_database_both_succeed() {
    let Some((pg, db)) = connected().await else {
        return;
    };
    let dir = temp_dir("both");
    let none = CancelHandle::new();
    for round in 0..3 {
        let dest = dir.join(format!("out{round}.csv"));
        let src = "select g, md5(g::text) from generate_series(1, 20000) g";
        let (run, exp) = if round % 2 == 0 {
            let run = query(&pg, &db, SLOW_ROWS, 0, &none);
            tokio::time::sleep(Duration::from_millis(50)).await;
            (run, export(&pg, &db, src, &dest, &none))
        } else {
            let exp = export(&pg, &db, src, &dest, &none);
            tokio::time::sleep(Duration::from_millis(5)).await;
            (query(&pg, &db, SLOW_ROWS, 0, &none), exp)
        };
        let page = query(&pg, &db, SLOW_ROWS, 2, &none);
        let schemas = {
            let (pg, db) = (pg.clone(), db.clone());
            tokio::spawn(async move { pg.list_schemas("test", &db).await })
        };
        let run = within(20, run).await.unwrap().unwrap();
        assert_eq!(run.rows.len(), 3);
        assert!(run.pageable && run.tx == TxState::Idle);
        let page = within(20, page).await.unwrap().unwrap();
        assert_eq!(page.rows, vec![vec![Some("3".into())]]);
        let exp = within(20, exp).await.unwrap().unwrap();
        assert_eq!(exp.rows, 20000);
        assert_eq!(csv_rows(&dest), 20000);
        assert!(
            within(20, schemas)
                .await
                .unwrap()
                .unwrap()
                .contains(&"public".into())
        );
    }
    let r = pg.query("test", &db, "select 1", 10, 0).await.unwrap();
    assert_eq!(r.tx, TxState::Idle);
    pg.disconnect("test").await;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Cancelling a waiting operation drops it without touching the running
/// one; cancelling the running one (by handle or by database) lets the
/// waiting one run; a running export is still cancelled by the server.
#[tokio::test(flavor = "multi_thread")]
async fn live_cancel_while_another_operation_waits() {
    let Some((pg, db)) = connected().await else {
        return;
    };
    let dir = temp_dir("cancel");
    let dest = dir.join("out.csv");
    let src = "select g from generate_series(1, 1000) g";

    // The waiting export is cancelled; the running statement goes on.
    let (ha, hb) = (CancelHandle::new(), CancelHandle::new());
    let a = query(&pg, &db, LONG, 0, &ha);
    until_running(&ha).await;
    let b = export(&pg, &db, src, &dest, &hb);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!b.is_finished() && !hb.is_running());
    let at = Instant::now();
    hb.cancel().await.unwrap();
    let e = within(5, b).await.unwrap().unwrap_err();
    assert_eq!(e.code.as_deref(), Some("57014"));
    assert!(at.elapsed() < Duration::from_secs(2));
    assert!(!dest.exists());
    assert!(!a.is_finished() && ha.is_running());
    // …then the running one, by handle.
    ha.cancel().await.unwrap();
    let e = within(5, a).await.unwrap().unwrap_err();
    assert_eq!(e.code.as_deref(), Some("57014"));
    assert_eq!(e.tx, Some(TxState::Idle));

    // Cancelling the running statement lets the waiting export run.
    let (ha, hb) = (CancelHandle::new(), CancelHandle::new());
    let a = query(&pg, &db, LONG, 0, &ha);
    until_running(&ha).await;
    let b = export(&pg, &db, src, &dest, &hb);
    tokio::time::sleep(Duration::from_millis(100)).await;
    ha.cancel().await.unwrap();
    let e = within(5, a).await.unwrap().unwrap_err();
    assert_eq!(e.code.as_deref(), Some("57014"));
    assert_eq!(within(10, b).await.unwrap().unwrap().rows, 1000);
    assert_eq!(csv_rows(&dest), 1000);

    // The same by database (`cancel_query`): only the running one stops.
    let ha = CancelHandle::new();
    let a = query(&pg, &db, LONG, 0, &ha);
    until_running(&ha).await;
    let b = query(&pg, &db, "select 42", 0, &CancelHandle::new());
    tokio::time::sleep(Duration::from_millis(100)).await;
    pg.cancel("test", &db).await.unwrap();
    let e = within(5, a).await.unwrap().unwrap_err();
    assert_eq!(e.code.as_deref(), Some("57014"));
    let r = within(5, b).await.unwrap().unwrap();
    assert_eq!(r.rows, vec![vec![Some("42".into())]]);

    // A running export is cancelled by the server; the statement waiting
    // behind it then runs.
    let he = CancelHandle::new();
    let slow_src = "select g, pg_sleep(0.001) from generate_series(1, 100000) g";
    let e = export(&pg, &db, slow_src, &dir.join("slow.csv"), &he);
    until_running(&he).await;
    let b = query(&pg, &db, "select 7", 0, &CancelHandle::new());
    he.cancel().await.unwrap();
    let err = within(5, e).await.unwrap().unwrap_err();
    assert_eq!(err.code.as_deref(), Some("57014"));
    assert!(!dir.join("slow.csv").exists());
    let r = within(5, b).await.unwrap().unwrap();
    assert_eq!(r.rows, vec![vec![Some("7".into())]]);
    assert_eq!(r.tx, TxState::Idle);

    // A handle cancelled after its operation ended changes nothing.
    let h = CancelHandle::new();
    query(&pg, &db, "select 1", 0, &h).await.unwrap().unwrap();
    h.cancel().await.unwrap();
    assert!(pg.query("test", &db, "select 1", 10, 0).await.is_ok());

    pg.disconnect("test").await;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Inside a user transaction (typed BEGIN, then Manual mode), concurrent
/// runs, pages and exports all see the transaction's rows and leave it open.
#[tokio::test(flavor = "multi_thread")]
async fn live_concurrent_operations_keep_the_transaction() {
    let Some((pg, db)) = connected().await else {
        return;
    };
    let dir = temp_dir("tx");
    let none = CancelHandle::new();
    let q = |sql: &'static str| pg.query("test", &db, sql, 10, 0);
    for manual in [false, true] {
        if manual {
            pg.set_tx_mode("test", &db, true).await.unwrap();
        } else {
            q("begin").await.unwrap();
        }
        let r = q("create temp table conc_t as select g from generate_series(1, 100) g")
            .await
            .unwrap();
        assert_eq!(r.tx, TxState::Active);
        let dest = dir.join("t.csv");
        let run = query(
            &pg,
            &db,
            "select count(*) from conc_t, pg_sleep(0.3)",
            0,
            &none,
        );
        let exp = export(&pg, &db, "select * from conc_t", &dest, &none);
        let page = query(&pg, &db, "select g from conc_t order by g", 95, &none);
        let run = within(10, run).await.unwrap().unwrap();
        assert_eq!(run.rows, vec![vec![Some("100".into())]]);
        assert_eq!(run.tx, TxState::Active);
        assert_eq!(within(10, exp).await.unwrap().unwrap().rows, 100);
        let page = within(10, page).await.unwrap().unwrap();
        assert_eq!(page.rows.len(), 5);
        assert_eq!(page.tx, TxState::Active);
        // A failing export (savepoint) next to a run leaves the block usable.
        let bad = export(&pg, &db, "select 1 / 0", &dest, &none);
        let ok = query(&pg, &db, "select count(*) from conc_t", 0, &none);
        assert!(within(10, bad).await.unwrap().is_err());
        let ok = within(10, ok).await.unwrap().unwrap();
        assert_eq!(ok.tx, TxState::Active);
        assert_eq!(q("rollback").await.unwrap().tx, TxState::Idle);
        // The temp table went with the transaction.
        assert!(q("select * from conc_t").await.is_err());
    }
    pg.set_tx_mode("test", &db, false).await.unwrap();
    pg.disconnect("test").await;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Pages of one statement fetched at once each get their own rows.
#[tokio::test(flavor = "multi_thread")]
async fn live_concurrent_pages() {
    let Some((pg, db)) = connected().await else {
        return;
    };
    let none = CancelHandle::new();
    let sql = "select g from generate_series(1, 100) g order by g";
    let pages: Vec<_> = (0..8)
        .map(|i| query(&pg, &db, sql, i * 10, &none))
        .collect();
    for (i, p) in pages.into_iter().enumerate() {
        let r = within(10, p).await.unwrap().unwrap();
        assert_eq!(r.rows.len(), 10);
        assert_eq!(
            r.rows[0][0].as_deref(),
            Some((i * 10 + 1).to_string().as_str())
        );
    }
    pg.disconnect("test").await;
}

/// Disconnecting while operations wait: they fail `not_connected` instead
/// of running on the dropped session.
#[tokio::test(flavor = "multi_thread")]
async fn live_waiting_operations_after_disconnect() {
    let Some((pg, db)) = connected().await else {
        return;
    };
    let ha = CancelHandle::new();
    let a = query(&pg, &db, LONG, 0, &ha);
    until_running(&ha).await;
    let b = query(&pg, &db, "select 1", 0, &CancelHandle::new());
    tokio::time::sleep(Duration::from_millis(100)).await;
    pg.disconnect("test").await;
    ha.cancel().await.unwrap();
    assert!(within(5, a).await.unwrap().is_err());
    let e = within(5, b).await.unwrap().unwrap_err();
    assert_eq!(e.code.as_deref(), Some("not_connected"));
}
