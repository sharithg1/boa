//! Exports: options persisted in csv-export.json, the loaded-rows route,
//! the save panel, failures; live (PGHOST set): streamed exports of
//! public.orders, the delimiter / header options, cancel, and exports inside
//! a manual transaction (the savepoint path).

use std::path::{Path, PathBuf};
use std::time::Duration;

use boa_core::grid::csv::{CsvDelimiter, CsvOptions, to_csv};
use boa_core::state::csv_export::{self as core, ExportSource};
use gpui::TestAppContext;

use super::Exports;
use crate::app_state::AppState;
use crate::models::toasts::{Toast, ToastKind};
use crate::tests::{FAKE_CONNECTION, TempDir, fake_connection, init};

fn toasts(cx: &mut TestAppContext) -> Vec<Toast> {
    cx.update(|cx| AppState::global(cx).toasts.read(cx).toasts().to_vec())
}

fn last_output(cx: &mut TestAppContext, id: &str) -> boa_core::state::history::OutputEntry {
    cx.update(|cx| {
        AppState::global(cx).workspaces.read(cx).output(id)[0]
            .entry
            .clone()
    })
}

fn exports(cx: &mut TestAppContext) -> gpui::Entity<Exports> {
    cx.update(|cx| AppState::global(cx).exports.clone())
}

/// Leftover temp files of an export (`.name.<pid>-<nanos>.part`).
fn parts(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "part"))
        .collect()
}

fn loaded_source() -> ExportSource {
    ExportSource {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev".into(),
        sql: "insert into t values (1), (2) returning *".into(),
        label: "console_1".into(),
        pageable: false,
        columns: Some(vec!["id".into(), "name".into()]),
        rows: Some(vec![
            vec![Some("1".into()), None],
            vec![Some("2".into()), Some(String::new())],
        ]),
        truncated: true,
    }
}

#[gpui::test]
fn options_are_read_from_and_saved_to_csv_export_json(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-options");
    std::fs::write(
        dir.path().join(core::FILE),
        r#"{"delimiter":";","header":false}"#,
    )
    .unwrap();
    init(cx, dir.path());
    let exports = exports(cx);
    assert_eq!(
        exports.read_with(cx, |e, _| e.options()),
        CsvOptions {
            delimiter: CsvDelimiter::Semicolon,
            header: false
        }
    );
    exports.update(cx, |e, cx| e.set_delimiter(CsvDelimiter::Tab, cx));
    assert_eq!(
        std::fs::read_to_string(dir.path().join(core::FILE)).unwrap(),
        r#"{"delimiter":"\t","header":false}"#
    );
    exports.update(cx, |e, cx| e.set_header(true, cx));
    // Read back as the next launch does.
    assert_eq!(
        Exports::load(dir.path()).options(),
        CsvOptions {
            delimiter: CsvDelimiter::Tab,
            header: true
        }
    );
    // A corrupt file: the defaults.
    std::fs::write(dir.path().join(core::FILE), "{nope").unwrap();
    assert_eq!(Exports::load(dir.path()).options(), CsvOptions::default());
}

#[gpui::test]
fn statements_that_change_data_write_their_loaded_rows(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-loaded");
    init(cx, dir.path());
    cx.update(|cx| fake_connection(cx, FAKE_CONNECTION, &["boa_dev"]));
    let exports = exports(cx);
    let dest = dir.path().join("out.csv");
    exports.update(cx, |e, cx| {
        e.export_to(loaded_source(), "q1", dest.clone(), cx)
    });
    cx.run_until_parked();
    let src = loaded_source();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        to_csv(
            &src.columns.unwrap(),
            &src.rows.unwrap(),
            CsvOptions::default()
        )
    );
    assert!(parts(dir.path()).is_empty());
    // Never a running job: nothing was re-run.
    assert!(exports.read_with(cx, |e, _| e.jobs().is_empty()));
    let shown = dest.to_string_lossy().into_owned();
    let entry = last_output(cx, FAKE_CONNECTION);
    assert_eq!(entry.note, Some(core::loaded_rows_note(2, &shown)));
    assert_eq!(entry.source.as_deref(), Some("console_1"));
    assert_eq!(entry.rows, None);
    let toast = toasts(cx).pop().unwrap();
    assert_eq!(toast.kind, ToastKind::Success);
    assert_eq!(toast.title.as_ref(), "Exported 2 rows");
    assert_eq!(
        toast.body.as_deref(),
        Some(core::loaded_rows_toast(2, true, &shown).body.as_str())
    );
    assert_eq!(
        toast.action.map(|a| a.label.to_string()).as_deref(),
        Some("Reveal in Finder")
    );
}

#[gpui::test]
fn export_asks_where_to_save(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-prompt");
    init(cx, dir.path());
    cx.update(|cx| fake_connection(cx, FAKE_CONNECTION, &["boa_dev"]));
    let exports = exports(cx);
    // Cancelled save panel: nothing happens.
    exports.update(cx, |e, cx| e.export(loaded_source(), "q1", cx));
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert!(toasts(cx).is_empty());
    // A chosen file is written, and the panel opens there next time.
    let sub = dir.path().join("exports");
    std::fs::create_dir_all(&sub).unwrap();
    let dest = sub.join("x.csv");
    exports.update(cx, |e, cx| e.export(loaded_source(), "q1", cx));
    cx.run_until_parked();
    let chosen = dest.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    cx.run_until_parked();
    assert!(dest.exists());
    assert_eq!(exports.read_with(cx, |e, _| e.save_dir()), sub);
    exports.update(cx, |e, cx| e.export(loaded_source(), "q1", cx));
    cx.run_until_parked();
    cx.simulate_new_path_selection(|opened| {
        assert_eq!(opened, sub.as_path());
        None
    });
}

#[gpui::test]
fn a_read_only_source_needs_its_connection(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-closed");
    init(cx, dir.path());
    let exports = exports(cx);
    let src = core::table_source("nope", "db", "public", "orders");
    exports.update(cx, |e, cx| e.export(src, "t", cx));
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    let toast = toasts(cx).pop().unwrap();
    assert_eq!(toast.kind, ToastKind::Error);
    assert_eq!(toast.title.as_ref(), core::FAILED_TITLE);
    assert_eq!(toast.body.as_deref(), Some("Connection is not connected"));
}

#[gpui::test]
fn a_failed_write_is_logged_and_toasted(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-fail");
    init(cx, dir.path());
    cx.update(|cx| fake_connection(cx, FAKE_CONNECTION, &["boa_dev"]));
    let exports = exports(cx);
    let dest = dir.path().join("missing/out.csv");
    exports.update(cx, |e, cx| e.export_to(loaded_source(), "q1", dest, cx));
    cx.run_until_parked();
    let entry = last_output(cx, FAKE_CONNECTION);
    assert!(
        entry
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("Export failed: ")),
        "{entry:?}"
    );
    let toast = toasts(cx).pop().unwrap();
    assert_eq!(
        (toast.kind, toast.title.as_ref()),
        (ToastKind::Error, core::FAILED_TITLE)
    );
}

#[gpui::test]
fn quitting_cancels_running_exports_and_removes_their_part_files(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-quit");
    init(cx, dir.path());
    cx.update(|cx| fake_connection(cx, FAKE_CONNECTION, &["boa_dev"]));
    // The cancel goes to the backend (on its own threads).
    cx.executor().allow_parking();
    let out = dir.path().join("out.csv");
    let pid = std::process::id();
    let ours = dir.path().join(format!(".out.csv.{pid}-123456.part"));
    let theirs = dir.path().join(".out.csv.1-123456.part");
    let other = dir.path().join(format!(".other.csv.{pid}-1.part"));
    for f in [&ours, &theirs, &other] {
        std::fs::write(f, "partial").unwrap();
    }
    std::fs::write(&out, "an older export").unwrap();
    assert_eq!(super::part_files(&out), vec![ours.clone()]);
    let exports = exports(cx);
    let src = ExportSource {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev".into(),
        sql: "select * from big".into(),
        label: "big".into(),
        pageable: true,
        ..ExportSource::default()
    };
    exports.update(cx, |e, _| {
        let id = e
            .jobs_mut()
            .start("console_1", &src, &out.to_string_lossy());
        e.paths.insert(id, out.clone());
    });
    let quit = exports.update(cx, |e, cx| e.quit(cx));
    // Removed at once.
    assert!(!ours.exists());
    // One the export only creates afterwards goes in the second sweep.
    let late = dir.path().join(format!(".out.csv.{pid}-999.part"));
    std::fs::write(&late, "partial").unwrap();
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = done.clone();
    cx.update(|cx| {
        cx.spawn(async move |_| {
            quit.await;
            flag.set(true);
        })
        .detach()
    });
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert!(done.get());
    assert!(!late.exists());
    // Never the destination, another process's or another file's part files.
    assert!(out.exists());
    assert!(theirs.exists());
    assert!(other.exists());
    // Nothing running: nothing to do.
    let ids: Vec<u64> = exports.read_with(cx, |e, _| e.jobs().iter().map(|j| j.id).collect());
    exports.update(cx, |e, _| {
        for id in ids {
            e.jobs_mut().finish(id);
        }
        e.paths.clear();
    });
    let empty = exports.update(cx, |e, cx| e.quit(cx));
    assert_eq!(futures::FutureExt::now_or_never(Box::pin(empty)), Some(()));
}

#[gpui::test]
fn running_jobs_are_found_by_owner(cx: &mut TestAppContext) {
    let dir = TempDir::new("csv-jobs");
    init(cx, dir.path());
    let exports = exports(cx);
    exports.update(cx, |e, _| {
        let src = core::table_source("c", "db", "public", "orders");
        let id = e.jobs.start("q1", &src, "/tmp/orders.csv");
        e.paths.insert(id, "/tmp/orders.csv".into());
        assert_eq!(e.job_of("q1").map(|j| j.id), Some(id));
        assert!(e.job_of("q2").is_none());
        assert_eq!(e.path_of(id), Some(Path::new("/tmp/orders.csv")));
        assert_eq!(e.jobs()[0].file, "orders.csv");
    });
}

#[test]
fn table_tab_sources_export_what_the_tab_shows() {
    use crate::models::catalogs::DbKey;
    let key = DbKey::new("c", "db");
    let src = super::table_tab_source(
        &key,
        "public",
        "orders",
        "select * from public.orders where id > 3",
        None,
    );
    assert_eq!(src.sql, "select * from public.orders where id > 3");
    assert!(src.pageable && src.label == "orders" && src.rows.is_none());
    let whole = super::table_tab_source(&key, "public", "orders", "", None);
    assert_eq!(whole.sql, "select * from public.orders");
    assert_eq!(
        super::pg_options(CsvOptions {
            delimiter: CsvDelimiter::Tab,
            header: false
        })
        .delimiter,
        '\t'
    );
}

// ── Live (PGHOST set) ─────────────────────────────────────────────────────

mod live {
    use super::*;
    use crate::models::catalogs::DbKey;
    use boa_core::backend::Backend;
    use boa_core::connections::ConnConfig;

    const ID: &str = "live";

    fn pg() -> Option<(String, u16)> {
        let host = std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
        let port = std::env::var("PGPORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5432);
        Some((host, port))
    }

    /// Waits (really) until `f` holds: the backend answers on its own threads.
    fn wait_for(cx: &mut TestAppContext, what: &str, f: impl Fn(&mut TestAppContext) -> bool) {
        for _ in 0..3000 {
            cx.run_until_parked();
            if f(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    /// The app on a temp config dir with a connected "live" data source.
    fn live_app(cx: &mut TestAppContext, name: &str) -> Option<(TempDir, Backend)> {
        let (host, port) = pg()?;
        let dir = TempDir::new(name);
        {
            let backend = Backend::with_config_dir(dir.path()).unwrap();
            let cfg = ConnConfig {
                id: ID.into(),
                name: "Live".into(),
                host,
                port,
                user: "boa".into(),
                database: "boa_dev".into(),
                ssl_mode: Default::default(),
                read_only: false,
                environment: None,
                color: None,
                ssh: None,
            };
            futures::executor::block_on(backend.save_connection(cfg, Some("boa".into()), true))
                .unwrap();
        }
        cx.executor().allow_parking();
        init(cx, dir.path());
        let conns = cx.update(|cx| AppState::global(cx).connections.clone());
        wait_for(cx, "the list", |cx| {
            conns.read_with(cx, |c, _| c.get(ID).is_some())
        });
        conns.update(cx, |c, cx| c.connect(ID, cx));
        wait_for(cx, "connected", |cx| {
            conns.read_with(cx, |c, _| c.is_connected(ID))
        });
        let backend = cx.update(|cx| AppState::global(cx).backend.clone());
        Some((dir, backend))
    }

    fn run(backend: &Backend, sql: &str) -> boa_core::model::QueryResult {
        futures::executor::block_on(backend.run_query(ID, "boa_dev", sql, 100, 0)).unwrap()
    }

    fn source(sql: &str) -> ExportSource {
        ExportSource {
            connection_id: ID.into(),
            database: "boa_dev".into(),
            sql: sql.into(),
            label: "console_1".into(),
            pageable: true,
            ..ExportSource::default()
        }
    }

    fn idle(cx: &mut TestAppContext) -> bool {
        let exports = exports(cx);
        exports.read_with(cx, |e, _| e.jobs().is_empty())
    }

    #[gpui::test]
    fn exports_the_whole_orders_table(cx: &mut TestAppContext) {
        let Some((dir, backend)) = live_app(cx, "csv-live-orders") else {
            return;
        };
        let exports = exports(cx);
        // The explorer's "Export to CSV…": save panel, then the streamed COPY.
        let key = DbKey::new(ID, "boa_dev");
        cx.update(|cx| super::super::export_table(&key, "public", "orders", cx));
        wait_for(cx, "the save panel", |cx| cx.did_prompt_for_new_path());
        let dest = dir.path().join("orders.csv");
        let chosen = dest.clone();
        cx.simulate_new_path_selection(move |_| Some(chosen));
        wait_for(cx, "the job", |cx| !idle(cx));
        let owner = core::table_owner(ID, "boa_dev", "public", "orders");
        assert!(exports.read_with(cx, |e, _| e.job_of(&owner).is_some()));
        wait_for(cx, "the export", idle);

        let columns: Vec<String> = run(&backend, "select * from public.orders limit 0")
            .columns
            .into_iter()
            .map(|c| c.name)
            .collect();
        let text = std::fs::read_to_string(&dest).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), columns.join(","));
        assert_eq!(lines.count(), 100_000);
        assert!(parts(dir.path()).is_empty());
        let size = std::fs::metadata(&dest).unwrap().len();
        let shown = dest.to_string_lossy().into_owned();
        let toast = toasts(cx).pop().unwrap();
        assert_eq!(toast.kind, ToastKind::Success);
        assert_eq!(toast.title.as_ref(), "Exported 100,000 rows");
        assert_eq!(
            toast.body.as_deref(),
            Some(core::streamed_toast(100_000, size, &shown).body.as_str())
        );
        assert_eq!(
            toast.action.map(|a| a.label.to_string()).as_deref(),
            Some(core::REVEAL_LABEL)
        );
        let entry = last_output(cx, ID);
        let note = entry.note.unwrap();
        assert!(
            note.starts_with(&format!("100,000 rows exported to {shown} (")),
            "{note}"
        );
        assert_eq!(entry.sql, "select * from public.orders");
        assert_eq!(entry.source.as_deref(), Some("orders"));

        // Semicolons, no header row.
        exports.update(cx, |e, cx| {
            e.set_delimiter(CsvDelimiter::Semicolon, cx);
            e.set_header(false, cx);
        });
        let small = dir.path().join("small.csv");
        let src = source("select id, 'a;b' as s from public.orders order by id limit 3");
        exports.update(cx, |e, cx| e.export_to(src, "q1", small.clone(), cx));
        wait_for(cx, "the second export", |cx| {
            idle(cx)
                && toasts(cx)
                    .last()
                    .is_some_and(|t| t.title.as_ref() == "Exported 3 rows")
        });
        let ids: Vec<String> = run(&backend, "select id from public.orders order by id limit 3")
            .rows
            .into_iter()
            .map(|r| r[0].clone().unwrap())
            .collect();
        let want: String = ids.iter().map(|id| format!("{id};\"a;b\"\n")).collect();
        assert_eq!(std::fs::read_to_string(&small).unwrap(), want);
    }

    #[gpui::test]
    fn cancel_leaves_no_file(cx: &mut TestAppContext) {
        let Some((dir, _backend)) = live_app(cx, "csv-live-cancel") else {
            return;
        };
        let exports = exports(cx);
        let dest = dir.path().join("big.csv");
        let src = source("select g, repeat('x', 100) from generate_series(1, 3000000) g");
        exports.update(cx, |e, cx| e.export_to(src, "q1", dest.clone(), cx));
        // Progress arrives every 250 ms while rows stream.
        wait_for(cx, "progress", |cx| {
            let exports = exports.clone();
            exports.read_with(cx, |e, _| e.jobs().first().is_some_and(|j| j.rows > 0))
        });
        let (id, bytes) = exports.read_with(cx, |e, _| (e.jobs()[0].id, e.jobs()[0].bytes));
        assert!(bytes > 0);
        exports.update(cx, |e, cx| e.cancel(id, cx));
        wait_for(cx, "the cancel", idle);
        assert!(!dest.exists());
        assert!(parts(dir.path()).is_empty(), "{:?}", parts(dir.path()));
        let toast = toasts(cx).pop().unwrap();
        assert_eq!(
            (toast.kind, toast.title.as_ref()),
            (ToastKind::Info, core::CANCELLED)
        );
        let entry = last_output(cx, ID);
        assert_eq!(entry.note.as_deref(), Some(core::CANCELLED));
        assert_eq!(entry.error, None);
    }

    #[gpui::test]
    fn inside_a_transaction_a_savepoint_keeps_it_intact(cx: &mut TestAppContext) {
        let Some((dir, backend)) = live_app(cx, "csv-live-tx") else {
            return;
        };
        let exports = exports(cx);
        run(&backend, "begin");
        run(
            &backend,
            "create temp table boa_csv_tx as select g from generate_series(1, 5) g",
        );
        run(&backend, "insert into boa_csv_tx values (6)");

        // Uncommitted rows are exported as the session sees them.
        let dest = dir.path().join("tx.csv");
        let src = source("select g from boa_csv_tx order by g");
        exports.update(cx, |e, cx| e.export_to(src, "q1", dest.clone(), cx));
        wait_for(cx, "the export", |cx| {
            idle(cx)
                && toasts(cx)
                    .last()
                    .is_some_and(|t| t.title.as_ref() == "Exported 6 rows")
        });
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "g\n1\n2\n3\n4\n5\n6\n"
        );

        // A failing export is rolled back to its savepoint: the transaction
        // goes on (no 25P02 "current transaction is aborted").
        exports.update(cx, |e, cx| {
            e.export_to(source("select 1/0"), "q1", dir.path().join("x.csv"), cx)
        });
        wait_for(cx, "the failure", |cx| {
            idle(cx)
                && toasts(cx)
                    .last()
                    .is_some_and(|t| t.kind == ToastKind::Error)
        });
        let toast = toasts(cx).pop().unwrap();
        assert_eq!(toast.title.as_ref(), core::FAILED_TITLE);
        assert_eq!(toast.body.as_deref(), Some("[22012] division by zero"));
        assert_eq!(
            last_output(cx, ID).error.as_deref(),
            Some("Export failed: [22012] division by zero")
        );

        // Statements that write are refused, not run.
        exports.update(cx, |e, cx| {
            e.export_to(
                source("with d as (delete from boa_csv_tx returning *) select * from d"),
                "q1",
                dir.path().join("y.csv"),
                cx,
            )
        });
        wait_for(cx, "the refusal", |cx| {
            idle(cx)
                && last_output(cx, ID)
                    .error
                    .is_some_and(|e| e.ends_with("this statement modifies data"))
        });
        assert_eq!(
            run(&backend, "select count(*)::text from boa_csv_tx").rows[0][0].as_deref(),
            Some("6")
        );
        assert!(!dir.path().join("x.csv").exists() && !dir.path().join("y.csv").exists());
        assert!(parts(dir.path()).is_empty());
        run(&backend, "rollback");
    }
}
