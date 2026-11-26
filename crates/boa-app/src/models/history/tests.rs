//! History model tests on a temp config dir: what gets recorded (console
//! runs and failures, not page re-fetches), the on-disk format (the Tauri
//! app's `history-<id>.jsonl`), retention, the 5,000 cap, same-query
//! dedupe, delete / clear, and the store-level case of state/history.test.ts
//! live against the dev database (skipped without `PGHOST`).

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_core::connections::{ConnConfig, SslMode};
use boa_core::history::Entry;
use boa_core::state::history::OutputEntry;
use gpui::{Entity, TestAppContext};

use super::*;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;
use crate::models::workspaces::Workspaces;
use crate::tests::{FAKE_CONNECTION, TempDir, fake_connection, init};

const DAY: f64 = 86_400_000.;

fn setup(cx: &mut TestAppContext, name: &str) -> (TempDir, Entity<History>, Entity<Workspaces>) {
    let dir = TempDir::new(name);
    init(cx, dir.path());
    let (history, workspaces) = cx.update(|cx| {
        fake_connection(cx, FAKE_CONNECTION, &["boa_dev"]);
        let s = AppState::global(cx);
        (s.history.clone(), s.workspaces.clone())
    });
    (dir, history, workspaces)
}

/// Waits (really) until `done`: the backend answers on its own threads.
fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Lets queued file operations finish (no condition to wait for).
fn settle(cx: &mut TestAppContext) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.run_until_parked();
}

fn run(sql: &str, at: f64) -> OutputEntry {
    OutputEntry {
        sql: sql.into(),
        database: "boa_dev".into(),
        at,
        ms: 12.,
        rows: Some(1.),
        source: Some("console_1".into()),
        ..OutputEntry::default()
    }
}

fn log(
    cx: &mut TestAppContext,
    ws: &Entity<Workspaces>,
    conn: &str,
    entry: OutputEntry,
    record: bool,
) {
    ws.update(cx, |w, cx| w.log(conn, entry, None, record, cx));
}

/// The connection's file, parsed line by line (oldest first).
fn disk(dir: &Path, conn: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(format!("history-{conn}.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn disk_sql(dir: &Path, conn: &str) -> Vec<String> {
    disk(dir, conn)
        .iter()
        .map(|v| v["sql"].as_str().unwrap().to_owned())
        .collect()
}

fn listed(cx: &mut TestAppContext, h: &Entity<History>, conn: &str) -> Option<Vec<String>> {
    h.read_with(cx, |h, _| {
        h.list(conn)
            .map(|l| l.iter().map(|e| e.sql.clone()).collect())
    })
}

fn load(cx: &mut TestAppContext, h: &Entity<History>, conn: &str, force: bool) {
    h.update(cx, |h, cx| h.load(conn, force, cx));
    let conn = conn.to_owned();
    wait_until(cx, "the load", |cx| {
        h.read_with(cx, |h, _| h.list(&conn).is_some() && !h.is_loading(&conn))
    });
}

/// Writes a history file as the Tauri app would have (oldest first).
fn seed(dir: &Path, conn: &str, entries: &[Entry]) {
    let mut out = String::new();
    for e in entries {
        out.push_str(&serde_json::to_string(e).unwrap());
        out.push('\n');
    }
    std::fs::write(dir.join(format!("history-{conn}.jsonl")), out).unwrap();
}

fn stored(id: &str, sql: &str, at: f64) -> Entry {
    Entry {
        id: id.into(),
        sql: sql.into(),
        key: Some(boa_core::state::history::query_key(sql)),
        database: "boa_dev".into(),
        at,
        ms: 3.,
        rows: Some(1.),
        truncated: None,
        affected: None,
        source: None,
        error: None,
        detail: None,
    }
}

fn set_prefs(cx: &mut TestAppContext, f: impl FnOnce(&mut boa_core::state::prefs::Prefs)) {
    cx.update(|cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| p.update(cx, f));
    });
}

#[gpui::test]
fn records_runs_and_failures_from_the_output_log_not_page_fetches(cx: &mut TestAppContext) {
    let (dir, h, ws) = setup(cx, "history-record");
    let now = now_ms();
    log(
        cx,
        &ws,
        FAKE_CONNECTION,
        run("select * from big", now),
        true,
    );
    let mut page = run("select * from big", now + 1.);
    page.offset = Some(500.);
    log(cx, &ws, FAKE_CONNECTION, page, false);
    let mut bad = run("select * from bad", now + 2.);
    bad.rows = None;
    bad.error = Some("[42P01] relation \"bad\" does not exist".into());
    log(cx, &ws, FAKE_CONNECTION, bad, true);
    wait_until(cx, "two saved", |_| {
        disk(dir.path(), FAKE_CONNECTION).len() == 2
    });
    settle(cx);
    assert_eq!(
        disk_sql(dir.path(), FAKE_CONNECTION),
        ["select * from big", "select * from bad"]
    );
    // Not loaded yet: nothing listed until a view asks.
    assert_eq!(listed(cx, &h, FAKE_CONNECTION), None);
    load(cx, &h, FAKE_CONNECTION, false);
    assert_eq!(
        listed(cx, &h, FAKE_CONNECTION).unwrap(),
        ["select * from bad", "select * from big"]
    );
    // Loaded lists follow new runs in memory.
    log(cx, &ws, FAKE_CONNECTION, run("select 3", now + 3.), true);
    assert_eq!(listed(cx, &h, FAKE_CONNECTION).unwrap()[0], "select 3");
    // The Output log still shows every fetch.
    assert_eq!(ws.read_with(cx, |w, _| w.output(FAKE_CONNECTION).len()), 4);
}

#[gpui::test]
fn writes_the_tauri_apps_jsonl_format(cx: &mut TestAppContext) {
    let (dir, _h, ws) = setup(cx, "history-format");
    let at = now_ms().floor();
    let mut e = run("select *\nfrom orders where id = 42", at);
    e.truncated = true;
    e.ms = 8.5;
    e.offset = Some(0.);
    log(cx, &ws, FAKE_CONNECTION, e, true);
    let mut failed = run("delete from x", at + 1.);
    failed.rows = None;
    failed.error = Some("[42P01] relation \"x\" does not exist".into());
    failed.detail = Some("Hint: check the name".into());
    log(cx, &ws, FAKE_CONNECTION, failed, true);
    wait_until(cx, "saved", |_| {
        disk(dir.path(), FAKE_CONNECTION).len() == 2
    });
    let raw = std::fs::read_to_string(dir.path().join("history-fake.jsonl")).unwrap();
    assert!(raw.ends_with('\n'));
    let lines = disk(dir.path(), FAKE_CONNECTION);
    let mut first = lines[0].as_object().unwrap().clone();
    let id = first.remove("id").unwrap();
    assert!(id.as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(
        serde_json::Value::Object(first),
        serde_json::json!({
            "sql": "select *\nfrom orders where id = 42",
            "key": boa_core::state::history::query_key("select *\nfrom orders where id = 42"),
            "database": "boa_dev",
            "at": at,
            "ms": 8.5,
            "rows": 1.0,
            "truncated": true,
            "source": "console_1",
        })
    );
    let mut second = lines[1].as_object().unwrap().clone();
    second.remove("id");
    second.remove("key");
    assert_eq!(
        serde_json::Value::Object(second),
        serde_json::json!({
            "sql": "delete from x",
            "database": "boa_dev",
            "at": at + 1.,
            "ms": 12.0,
            "source": "console_1",
            "error": "[42P01] relation \"x\" does not exist",
            "detail": "Hint: check the name",
        })
    );
    // The Tauri app reads the file with this same code (boa-core's history).
    let back = boa_core::history::History::default()
        .list(dir.path(), FAKE_CONNECTION, None)
        .unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].truncated, Some(true));
    // Owner-only, like connections.json.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.path().join("history-fake.jsonl"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[gpui::test]
fn records_nothing_when_turned_off(cx: &mut TestAppContext) {
    let (dir, h, ws) = setup(cx, "history-off");
    set_prefs(cx, |p| p.record_history = false);
    log(cx, &ws, FAKE_CONNECTION, run("select 1", now_ms()), true);
    settle(cx);
    assert!(!dir.path().join("history-fake.jsonl").exists());
    load(cx, &h, FAKE_CONNECTION, false);
    assert_eq!(
        listed(cx, &h, FAKE_CONNECTION).unwrap(),
        Vec::<String>::new()
    );
}

#[gpui::test]
fn a_query_run_again_replaces_its_entry(cx: &mut TestAppContext) {
    let (dir, h, ws) = setup(cx, "history-dedupe");
    load(cx, &h, FAKE_CONNECTION, false);
    let now = now_ms();
    log(
        cx,
        &ws,
        FAKE_CONNECTION,
        run("select * from t where id = 1", now),
        true,
    );
    log(cx, &ws, FAKE_CONNECTION, run("select 2", now + 1.), true);
    log(
        cx,
        &ws,
        FAKE_CONNECTION,
        run("SELECT *  FROM t WHERE id = 7", now + 2.),
        true,
    );
    assert_eq!(
        listed(cx, &h, FAKE_CONNECTION).unwrap(),
        ["SELECT *  FROM t WHERE id = 7", "select 2"]
    );
    wait_until(cx, "deduped on disk", |_| {
        disk_sql(dir.path(), FAKE_CONNECTION) == ["select 2", "SELECT *  FROM t WHERE id = 7"]
    });
}

#[gpui::test]
fn retention_drops_old_runs_and_follows_the_setting(cx: &mut TestAppContext) {
    let (dir, h, ws) = setup(cx, "history-retention");
    let now = now_ms();
    seed(
        dir.path(),
        FAKE_CONNECTION,
        &[
            stored("a", "select * from old", now - 40. * DAY),
            stored("b", "select * from week", now - 8. * DAY),
            stored("c", "select * from new", now - DAY),
        ],
    );
    // 30 days by default.
    load(cx, &h, FAKE_CONNECTION, false);
    assert_eq!(
        listed(cx, &h, FAKE_CONNECTION).unwrap(),
        ["select * from new", "select * from week"]
    );
    // Forever: everything comes back (the file keeps old runs until the next append).
    set_prefs(cx, |p| p.history_days = 0);
    wait_until(cx, "re-read forever", |cx| {
        listed(cx, &h, FAKE_CONNECTION).is_some_and(|l| l.len() == 3)
    });
    // 7 days: re-read.
    set_prefs(cx, |p| p.history_days = 7);
    wait_until(cx, "re-read 7 days", |cx| {
        listed(cx, &h, FAKE_CONNECTION).is_some_and(|l| l == ["select * from new"])
    });
    // The next append drops what is past the period from the file.
    log(
        cx,
        &ws,
        FAKE_CONNECTION,
        run("select * from now", now),
        true,
    );
    wait_until(cx, "appended", |_| {
        disk_sql(dir.path(), FAKE_CONNECTION) == ["select * from new", "select * from now"]
    });
}

#[gpui::test]
fn keeps_at_most_five_thousand_per_connection(cx: &mut TestAppContext) {
    let (dir, h, ws) = setup(cx, "history-cap");
    let now = now_ms();
    let entries: Vec<Entry> = (0..5001)
        .map(|i| {
            stored(
                &format!("e{i}"),
                &format!("select {i} as c{i}"),
                now - 10_000. + f64::from(i),
            )
        })
        .collect();
    seed(dir.path(), FAKE_CONNECTION, &entries);
    load(cx, &h, FAKE_CONNECTION, false);
    let list = listed(cx, &h, FAKE_CONNECTION).unwrap();
    assert_eq!(list.len(), CAP);
    assert_eq!(list[0], "select 5000 as c5000");
    assert_eq!(list[4999], "select 1 as c1");
    log(cx, &ws, FAKE_CONNECTION, run("select 'latest'", now), true);
    wait_until(cx, "capped on disk", |_| {
        let sql = disk_sql(dir.path(), FAKE_CONNECTION);
        sql.len() == CAP && sql.last().map(String::as_str) == Some("select 'latest'")
    });
    let list = listed(cx, &h, FAKE_CONNECTION).unwrap();
    assert_eq!(list.len(), CAP);
    assert_eq!(list[0], "select 'latest'");
    assert_eq!(list[CAP - 1], "select 2 as c2");
}

const CAP: usize = boa_core::state::history::HISTORY_CAP;

#[gpui::test]
fn deletes_entries_and_clears_a_connection(cx: &mut TestAppContext) {
    let (dir, h, ws) = setup(cx, "history-delete");
    cx.update(|cx| {
        fake_connection(cx, "other", &["boa_dev"]);
    });
    let now = now_ms();
    log(cx, &ws, FAKE_CONNECTION, run("select 'a'", now), true);
    log(
        cx,
        &ws,
        FAKE_CONNECTION,
        run("select 'b' from t", now + 1.),
        true,
    );
    log(cx, &ws, "other", run("select 'c' from u", now + 2.), true);
    wait_until(cx, "saved", |_| {
        disk(dir.path(), FAKE_CONNECTION).len() == 2 && disk(dir.path(), "other").len() == 1
    });
    load(cx, &h, FAKE_CONNECTION, false);
    let first = h.read_with(cx, |h, _| h.list(FAKE_CONNECTION).unwrap()[1].clone());
    assert_eq!(first.sql, "select 'a'");
    h.update(cx, |h, cx| {
        h.delete(FAKE_CONNECTION, std::slice::from_ref(&first.id), cx)
    });
    assert_eq!(
        listed(cx, &h, FAKE_CONNECTION).unwrap(),
        ["select 'b' from t"]
    );
    wait_until(cx, "deleted on disk", |_| {
        disk_sql(dir.path(), FAKE_CONNECTION) == ["select 'b' from t"]
    });
    h.update(cx, |h, cx| h.clear(FAKE_CONNECTION, cx));
    assert_eq!(
        listed(cx, &h, FAKE_CONNECTION).unwrap(),
        Vec::<String>::new()
    );
    wait_until(cx, "file gone", |_| {
        !dir.path().join("history-fake.jsonl").exists()
    });
    assert_eq!(disk(dir.path(), "other").len(), 1);
    // All connections, newest first.
    load(cx, &h, "other", false);
    log(
        cx,
        &ws,
        FAKE_CONNECTION,
        run("select 'd' from v", now + 3.),
        true,
    );
    let all = h.read_with(cx, |h, _| {
        h.items(&[FAKE_CONNECTION.to_owned(), "other".to_owned()])
            .into_iter()
            .map(|i| (i.connection_id, i.entry.sql))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        all,
        [
            (FAKE_CONNECTION.to_owned(), "select 'd' from v".to_owned()),
            ("other".to_owned(), "select 'c' from u".to_owned()),
        ]
    );
    // A deleted connection's list goes.
    h.update(cx, |h, cx| h.forget("other", cx));
    assert_eq!(listed(cx, &h, "other"), None);
}

#[gpui::test]
fn opens_closes_and_toggles(cx: &mut TestAppContext) {
    let (_dir, h, _ws) = setup(cx, "history-open");
    let events: Rc<RefCell<Vec<HistoryEvent>>> = Rc::default();
    let _sub = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&h, move |_, e: &HistoryEvent, _| {
            events.borrow_mut().push(*e)
        })
    });
    h.update(cx, |h, cx| h.toggle(cx));
    assert!(h.read_with(cx, |h, _| h.is_open()));
    h.update(cx, |h, cx| h.set_open(true, cx));
    h.update(cx, |h, cx| h.toggle(cx));
    cx.run_until_parked();
    assert!(!h.read_with(cx, |h, _| h.is_open()));
    assert_eq!(
        *events.borrow(),
        [HistoryEvent::Opened, HistoryEvent::Closed]
    );
}

// ── Live: the store-level case of state/history.test.ts ──────────────────────

fn pg_config(id: &str) -> Option<ConnConfig> {
    let host = std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    Some(ConnConfig {
        id: id.into(),
        name: "local".into(),
        host,
        port: var("PGPORT", "5432").parse().unwrap_or(5432),
        user: var("PGUSER", "boa"),
        database: var("PGDATABASE", "boa_dev"),
        ssl_mode: SslMode::Disable,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    })
}

#[gpui::test]
fn records_console_runs_and_failures_not_page_refetches(cx: &mut TestAppContext) {
    let Some(config) = pg_config("c1") else {
        return;
    };
    cx.executor().allow_parking();
    let dir = TempDir::new("history-live");
    let password = std::env::var("PGPASSWORD").unwrap_or_else(|_| "boa".into());
    let database = config.database.clone();
    boa_core::connections::upsert(dir.path(), config, Some(password), true).unwrap();
    init(cx, dir.path());
    // The saved list, then a real connection.
    let saved = boa_core::connections::list(dir.path()).unwrap();
    let (backend, connections, workspaces) = cx.update(|cx| {
        let s = AppState::global(cx).clone();
        s.connections.update(cx, |c, cx| {
            c.saved = saved;
            cx.notify();
        });
        (s.backend, s.connections, s.workspaces)
    });
    let result = Rc::new(RefCell::new(None));
    {
        let result = result.clone();
        cx.update(|cx| {
            cx.spawn(async move |_| {
                *result.borrow_mut() = Some(backend.connect("c1", None).await);
            })
            .detach()
        });
    }
    wait_until(cx, "connect", |_| result.borrow().is_some());
    let info = result.borrow_mut().take().unwrap().expect("connects");
    connections.update(cx, |c, cx| {
        c.state.insert(
            "c1".into(),
            ConnState::Connected {
                server_version: info.server_version,
                databases: info.databases,
            },
        );
        cx.notify();
    });
    // One row per page, so "big" is truncated and pageable.
    set_prefs(cx, |p| p.row_limit = 1);
    let key = DbKey {
        connection_id: "c1".into(),
        database,
    };
    workspaces.update(cx, |w, cx| w.new_query_tab_with(key, String::new(), cx));
    let console = workspaces.read_with(cx, |w, _| {
        w.active_tab().unwrap().console().unwrap().clone()
    });
    let idle = |cx: &mut TestAppContext| console.read_with(cx, |c, _| c.running.is_none());
    let big = "select * from generate_series(1, 3) as big";
    console.update(cx, |c, cx| c.run(big.into(), cx));
    wait_until(cx, "the run", |cx| {
        idle(cx) && console.read_with(cx, |c, _| c.result.is_some())
    });
    console.update(cx, |c, cx| c.page(1, cx));
    wait_until(cx, "page 2", |cx| {
        idle(cx) && console.read_with(cx, |c, _| c.result.as_ref().is_some_and(|r| r.offset == 1))
    });
    console.update(cx, |c, cx| c.page(2, cx));
    wait_until(cx, "page 3", |cx| {
        idle(cx) && console.read_with(cx, |c, _| c.result.as_ref().is_some_and(|r| r.offset == 2))
    });
    console.update(cx, |c, cx| c.run("select * from bad".into(), cx));
    wait_until(cx, "the failure", |cx| {
        idle(cx) && console.read_with(cx, |c, _| c.failed.is_some())
    });
    wait_until(cx, "two saved", |_| disk(dir.path(), "c1").len() == 2);
    settle(cx);
    let saved = disk(dir.path(), "c1");
    assert_eq!(disk_sql(dir.path(), "c1"), [big, "select * from bad"]);
    assert_eq!(saved[0]["rows"], 1.0);
    assert_eq!(saved[0]["truncated"], true);
    assert_eq!(saved[0]["source"], "console_1");
    assert_eq!(saved[1]["error"], "[42P01] relation \"bad\" does not exist");
    // The Output log still shows every fetch.
    assert_eq!(workspaces.read_with(cx, |w, _| w.output("c1").len()), 4);
}
