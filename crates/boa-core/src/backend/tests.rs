use super::{Backend, BackendTask, LinkEvent};
use crate::connections::{ConnConfig, SslMode};
use crate::error::CmdError;
use crate::history::Entry;
use crate::model::ConstraintKind;
use crate::pg::{CsvOptions, QueryResult};
use crate::tx::TxState;
use futures_executor::block_on;
use std::path::PathBuf;
use std::time::Duration;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "boa-backend-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(id: &str) -> ConnConfig {
    ConnConfig {
        kind: Default::default(),
        id: id.into(),
        name: id.into(),
        host: "localhost".into(),
        port: 5432,
        user: "u".into(),
        database: "d".into(),
        ssl_mode: SslMode::Prefer,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    }
}

fn entry(n: u32) -> Entry {
    Entry {
        id: format!("e{n}"),
        sql: format!("select {n}"),
        key: None,
        database: "d".into(),
        at: 1_700_000_000_000.0 + f64::from(n),
        ms: 1.0,
        rows: None,
        truncated: None,
        affected: None,
        source: None,
        error: None,
        detail: None,
    }
}

/// Files commands, awaited from a plain thread (no tokio context).
#[test]
fn file_commands_from_a_plain_thread() {
    let dir = temp_dir("files");
    let b = Backend::with_config_dir(&dir).unwrap();
    assert_eq!(b.config_dir(), dir);
    assert!(block_on(b.list_connections()).unwrap().is_empty());

    block_on(b.save_connection(config("a"), Some("pw".into()), true)).unwrap();
    block_on(b.save_connection(config("b"), None, false)).unwrap();
    let list = block_on(b.list_connections()).unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].has_password && !list[1].has_password);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("connections.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    for n in 0..3 {
        block_on(b.history_append("a", entry(n), None)).unwrap();
    }
    let ids = |v: Vec<Entry>| v.into_iter().map(|e| e.id).collect::<Vec<_>>();
    assert_eq!(
        ids(block_on(b.history_list("a", None)).unwrap()),
        ["e0", "e1", "e2"]
    );
    // A 1-day retention drops these 2023 runs from the list.
    assert!(block_on(b.history_list("a", Some(1))).unwrap().is_empty());
    block_on(b.history_delete("a", vec!["e1".into()])).unwrap();
    assert_eq!(
        ids(block_on(b.history_list("a", Some(0))).unwrap()),
        ["e0", "e2"]
    );

    // Deleting a connection forgets its history too.
    block_on(b.delete_connection("a")).unwrap();
    assert!(!dir.join("history-a.jsonl").exists());
    let list = block_on(b.list_connections()).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].config.id, "b");

    let csv = dir.join("out.csv");
    block_on(b.write_csv_file(&csv, "a,b\n1,2\n".into())).unwrap();
    assert_eq!(std::fs::read_to_string(&csv).unwrap(), "a,b\n1,2\n");

    // Session commands on a connection that isn't open.
    let e = block_on(b.run_query("b", "d", "select 1", 10, 0)).unwrap_err();
    assert_eq!(e.code.as_deref(), Some("not_connected"));
    let e = block_on(b.connect("zz", None)).unwrap_err();
    assert_eq!(e.message, "Unknown connection");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A task still running when the last handle drops resolves to an error
/// instead of hanging; dropping inside the runtime's own threads is fine.
#[test]
fn tasks_fail_once_the_backend_is_gone() {
    let b = Backend::with_config_dir(temp_dir("gone")).unwrap();
    let slow = b.spawn(|_| async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(())
    });
    let clone = b.clone();
    // The last clone dropped on a runtime thread.
    let dropped = b.spawn(move |_| async move {
        drop(clone);
        Ok(())
    });
    drop(b);
    // `dropped` may lose the race with the shutdown: either is fine.
    let _ = block_on(dropped);
    let e = block_on(slow).unwrap_err();
    assert!(e.message.contains("backend stopped"), "{e:?}");
}

#[test]
fn link_events_mirror_track_links() {
    let b = Backend::with_config_dir(temp_dir("links")).unwrap();
    let rx = b.link_events();
    let other = b.link_events();
    drop(other);
    let err = |code: &str| Err::<(), _>(CmdError::code(code, "x"));
    b.inner.watch("c", "db", &err("42P01"));
    b.inner.watch("c", "db", &Ok(()));
    b.inner.watch("c", "db", &err("unreachable"));
    b.inner.watch("c", "db", &err("tx_lost"));
    let lost = LinkEvent::Lost {
        connection_id: "c".into(),
    };
    let got: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(
        got,
        [
            lost.clone(),
            LinkEvent::TxLost {
                connection_id: "c".into(),
                database: "db".into()
            },
            lost
        ]
    );
    // The dropped subscriber was pruned.
    assert_eq!(b.inner.links.lock().unwrap().len(), 1);
}

/// The Tauri app's real config dir parses (read-only; skipped if absent).
#[test]
fn reads_the_real_config_dir() {
    let Some(dir) = crate::config::config_dir().filter(|d| d.join("connections.json").exists())
    else {
        eprintln!("no Boa config dir: skipping");
        return;
    };
    let saved = crate::connections::list(&dir).unwrap();
    let history = crate::history::History::default();
    for c in &saved {
        history.list(&dir, &c.config.id, None).unwrap();
    }
}

/// The libpq variables' server (PGHOST, …) as a saved connection, or None.
fn live(dir: &std::path::Path, id: &str) -> Option<ConnConfig> {
    let host = std::env::var("PGHOST").ok()?;
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    let config = ConnConfig {
        kind: Default::default(),
        id: id.into(),
        name: id.into(),
        host,
        port: env("PGPORT", "5432").parse().unwrap(),
        user: env("PGUSER", "postgres"),
        database: env("PGDATABASE", "postgres"),
        ssl_mode: SslMode::Prefer,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    };
    let pw = std::env::var("PGPASSWORD").ok();
    let b = Backend::with_config_dir(dir).unwrap();
    block_on(b.save_connection(config.clone(), pw, true)).unwrap();
    Some(config)
}

/// The bridge against a real server (skipped without PGHOST), awaited from
/// a plain thread: connect, paging, introspection, DDL, cancel, CSV export
/// with progress, link events. Reads `public.orders` / `public.customers`
/// when they exist (the dev database), else a generated series.
#[test]
fn live_backend() {
    let dir = temp_dir("live");
    let Some(config) = live(&dir, "a") else {
        eprintln!("PGHOST not set: skipping live Postgres test");
        return;
    };
    live(&dir, "killer").unwrap();
    let db = config.database.clone();
    let b = Backend::with_config_dir(&dir).unwrap();
    let links = b.link_events();

    let info = block_on(b.connect("a", None)).unwrap();
    assert!(info.server_version.starts_with("PostgreSQL"));
    assert!(info.databases.contains(&db));
    assert!(
        block_on(b.list_schemas("a", &db))
            .unwrap()
            .contains(&"public".to_owned())
    );

    // Paging over a cursor.
    let src = "select g as n, 'r' || g as label from generate_series(1, 1000) g";
    let page = block_on(b.run_query("a", &db, src, 50, 0)).unwrap();
    assert_eq!(page.rows.len(), 50);
    assert!(page.truncated && page.pageable);
    assert_eq!(page.columns[0].ty, "int4");
    assert_eq!(page.tx, TxState::Idle);
    let last = block_on(b.run_query("a", &db, src, 50, 990)).unwrap();
    assert_eq!(last.rows.len(), 10);
    assert!(!last.truncated);
    assert_eq!(last.rows[0][0].as_deref(), Some("991"));

    // The dev database's tables, when present.
    let schemas = block_on(b.introspect("a", &db, vec!["public".into()])).unwrap();
    let public = &schemas[0];
    let orders = public.tables.iter().find(|t| t.name == "orders");
    let source = if let Some(orders) = orders {
        assert!(orders.constraints.iter().any(|c| {
            c.kind == ConstraintKind::ForeignKey
                && c.references
                    .as_ref()
                    .is_some_and(|r| r.table == "customers")
        }));
        let ddl = block_on(b.table_ddl("a", &db, "public", "orders")).unwrap();
        assert!(ddl.contains("CREATE TABLE"), "{ddl}");
        "select * from public.orders".to_owned()
    } else {
        "select g, md5(g::text) from generate_series(1, 100000) g".to_owned()
    };

    // CSV export with progress, awaited while the progress is drained.
    let dest = dir.join("export.csv");
    let export = b.export_csv(
        "a",
        &db,
        &source,
        &dest,
        CsvOptions {
            delimiter: ',',
            header: true,
        },
    );
    let progress = std::thread::spawn(move || {
        let mut seen = Vec::new();
        while let Ok(p) = block_on(export.progress.recv()) {
            seen.push(p.rows);
        }
        seen
    });
    let done = block_on(export.result).unwrap();
    let seen = progress.join().unwrap();
    let total = block_on(b.run_query(
        "a",
        &db,
        &format!("select count(*)::text from ({source}) s"),
        1,
        0,
    ))
    .unwrap();
    assert_eq!(
        total.rows[0][0].as_deref(),
        Some(done.rows.to_string().as_str())
    );
    eprintln!("exported {} rows in {} ms: {source}", done.rows, done.ms);
    assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    let text = std::fs::read_to_string(&dest).unwrap();
    assert_eq!(text.lines().count() as u64, done.rows + 1);
    assert!(
        std::fs::read_dir(&dir).unwrap().all(|f| !f
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part"))
    );

    // Cancel from another call while a statement runs.
    let slow = b.run_query("a", &db, "select pg_sleep(30)", 1, 0);
    std::thread::sleep(Duration::from_millis(300));
    block_on(b.cancel_query("a", &db)).unwrap();
    let e = block_on(slow).unwrap_err();
    assert_eq!(e.code.as_deref(), Some("57014"));
    assert_eq!(e.tx, Some(TxState::Idle));

    // A transaction lost with its session: reported, and a link event sent.
    assert!(links.try_recv().is_err());
    assert_eq!(
        block_on(b.set_tx_mode("a", &db, false)).unwrap(),
        TxState::Idle
    );
    block_on(b.run_query("a", &db, "begin", 1, 0)).unwrap();
    let pid = block_on(b.run_query("a", &db, "select pg_backend_pid()", 1, 0)).unwrap();
    let pid = pid.rows[0][0].clone().unwrap();
    block_on(b.connect("killer", None)).unwrap();
    block_on(b.run_query(
        "killer",
        &db,
        &format!("select pg_terminate_backend({pid})"),
        1,
        0,
    ))
    .unwrap();
    let e = block_on(b.run_query("a", &db, "select 1", 1, 0)).unwrap_err();
    assert_eq!(e.code.as_deref(), Some("tx_lost"));
    assert_eq!(
        links.try_recv().unwrap(),
        LinkEvent::TxLost {
            connection_id: "a".into(),
            database: db.clone()
        }
    );
    assert_eq!(
        links.try_recv().unwrap(),
        LinkEvent::Lost {
            connection_id: "a".into()
        }
    );
    // The next statement runs on a new session.
    let one = block_on(b.run_query("a", &db, "select 1", 1, 0)).unwrap();
    assert_eq!(one.rows, vec![vec![Some("1".to_owned())]]);
    assert!(block_on(b.ping("a")).unwrap().lost_tx.is_empty());

    block_on(b.disconnect("killer")).unwrap();
    block_on(b.disconnect("a")).unwrap();
    let e = block_on(b.ping("a")).unwrap_err();
    assert_eq!(e.code.as_deref(), Some("not_connected"));
    drop(b);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Through the bridge: a console run and an export on one database take
/// turns, and each task's cancel handle stops only its own operation.
#[test]
fn live_backend_one_operation_at_a_time() {
    let dir = temp_dir("turns");
    let Some(config) = live(&dir, "a") else {
        eprintln!("PGHOST not set: skipping live Postgres test");
        return;
    };
    let db = config.database.clone();
    let b = Backend::with_config_dir(&dir).unwrap();
    block_on(b.connect("a", None)).unwrap();
    let csv = CsvOptions {
        delimiter: ',',
        header: false,
    };
    let src = "select g from generate_series(1, 5000) g";
    let running = |task: &BackendTask<QueryResult>| {
        let h = task.cancel_handle().unwrap();
        for _ in 0..250 {
            if h.is_running() {
                std::thread::sleep(Duration::from_millis(150));
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("never started");
    };

    // Both succeed (was: "cursor __boa does not exist").
    let run = b.run_query(
        "a",
        &db,
        "select g from generate_series(1, 3) g, pg_sleep(0.5)",
        10,
        0,
    );
    std::thread::sleep(Duration::from_millis(50));
    let export = b.export_csv("a", &db, src, dir.join("a.csv"), csv);
    assert_eq!(block_on(run).unwrap().rows.len(), 3);
    assert_eq!(block_on(export.result).unwrap().rows, 5000);

    // Cancelling the waiting export leaves the running statement alone.
    let slow = b.run_query("a", &db, "select pg_sleep(30)", 1, 0);
    running(&slow);
    let export = b.export_csv("a", &db, src, dir.join("b.csv"), csv);
    std::thread::sleep(Duration::from_millis(100));
    block_on(b.cancel(&export.result.cancel_handle().unwrap())).unwrap();
    let e = block_on(export.result).unwrap_err();
    assert_eq!(e.code.as_deref(), Some("57014"));
    assert!(!dir.join("b.csv").exists());
    assert!(slow.cancel_handle().unwrap().is_running());
    // …and cancelling the statement lets a waiting export run.
    let export = b.export_csv("a", &db, src, dir.join("c.csv"), csv);
    block_on(b.cancel(&slow.cancel_handle().unwrap())).unwrap();
    assert_eq!(block_on(slow).unwrap_err().code.as_deref(), Some("57014"));
    assert_eq!(block_on(export.result).unwrap().rows, 5000);

    block_on(b.disconnect("a")).unwrap();
    drop(b);
    std::fs::remove_dir_all(&dir).unwrap();
}
