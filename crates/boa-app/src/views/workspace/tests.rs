//! The consoles in the real main window: tabs through the keymap, database
//! cycling, the status bar's caret, and (with `PGHOST`) running, cancelling,
//! errors in the Output log, paging and table tabs against the local server.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_core::connections::{ConnConfig, SavedConnection};
use boa_core::state::prefs::Scope;
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::app_state::AppState;
use crate::grid::GridEvent;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;
use crate::models::console::Console;
use crate::models::workspaces::Workspaces;
use crate::tests::{FAKE_CONNECTION, TempDir, init, layout_open, open_empty, open_main};
use crate::views::bottom_panel::BottomView;
use crate::views::root::RootView;

fn workspaces(vcx: &mut VisualTestContext) -> Entity<Workspaces> {
    vcx.update(|_, cx| AppState::global(cx).workspaces.clone())
}

fn labels(vcx: &mut VisualTestContext) -> (Vec<String>, Option<String>) {
    vcx.update(|_, cx| {
        let ws = AppState::global(cx).workspaces.read(cx);
        let tabs = ws
            .active_workspace()
            .map(|w| w.tabs.iter().map(|t| t.label().to_string()).collect())
            .unwrap_or_default();
        (tabs, ws.active_tab().map(|t| t.label().to_string()))
    })
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn cmd_t_cmd_w_and_option_digits_drive_the_tabs(cx: &mut TestAppContext) {
    let dir = TempDir::new("tabs");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    assert_eq!(
        labels(&mut vcx),
        (vec!["console_1".into()], Some("console_1".into()))
    );
    vcx.simulate_keystrokes("cmd-t");
    vcx.simulate_keystrokes("cmd-t");
    assert_eq!(
        labels(&mut vcx),
        (
            vec!["console_1".into(), "console_2".into(), "console_3".into()],
            Some("console_3".into())
        )
    );
    // The new console's editor has the focus: typing goes into it.
    draw(&mut vcx);
    vcx.simulate_input("select 3");
    draw(&mut vcx);
    assert_eq!(
        vcx.update(|_, cx| root.read(cx).editor_text(cx)),
        "select 3"
    );
    vcx.simulate_keystrokes("alt-1");
    assert_eq!(labels(&mut vcx).1.as_deref(), Some("console_1"));
    draw(&mut vcx);
    assert!(
        vcx.update(|_, cx| root.read(cx).editor_text(cx))
            .starts_with("-- Boa SQL console")
    );
    vcx.simulate_keystrokes("alt-9");
    assert_eq!(
        labels(&mut vcx).1.as_deref(),
        Some("console_1"),
        "no 9th tab"
    );
    vcx.simulate_keystrokes("alt-3");
    vcx.simulate_keystrokes("cmd-w");
    assert_eq!(
        labels(&mut vcx),
        (
            vec!["console_1".into(), "console_2".into()],
            Some("console_2".into())
        )
    );
    vcx.simulate_keystrokes("cmd-w");
    vcx.simulate_keystrokes("cmd-w");
    assert_eq!(labels(&mut vcx), (vec![], None));
    draw(&mut vcx);
    assert!(
        vcx.update(|_, cx| root.read(cx).workspace().read(cx).editor(cx))
            .is_none()
    );
    // (Nothing left to close: ⌘W closes the window, see views::root's
    // shortcut tests.) ⌥N opens a console on the selected database again.
    vcx.simulate_keystrokes("alt-n");
    assert_eq!(
        labels(&mut vcx),
        (vec!["console_1".into()], Some("console_1".into()))
    );
}

#[gpui::test]
fn option_arrows_cycle_databases(cx: &mut TestAppContext) {
    let dir = TempDir::new("cycle");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    vcx.update(|_, cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                p.scopes.insert(FAKE_CONNECTION.into(), Scope::All);
            })
        });
    });
    let active = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .workspaces
                .read(cx)
                .active
                .clone()
                .map(|k| k.database)
        })
    };
    assert_eq!(active(&mut vcx).as_deref(), Some("boa_dev"));
    vcx.simulate_keystrokes("alt-down");
    assert_eq!(active(&mut vcx).as_deref(), Some("boa_dev2"));
    // Its own (empty) workspace.
    assert_eq!(labels(&mut vcx), (vec![], None));
    vcx.simulate_keystrokes("alt-down");
    assert_eq!(active(&mut vcx).as_deref(), Some("boa_dev"));
    vcx.simulate_keystrokes("alt-up");
    assert_eq!(active(&mut vcx).as_deref(), Some("boa_dev2"));
    vcx.simulate_keystrokes("alt-up");
    assert_eq!(labels(&mut vcx).1.as_deref(), Some("console_1"));
}

#[gpui::test]
fn without_a_database_the_shortcuts_do_nothing(cx: &mut TestAppContext) {
    let dir = TempDir::new("empty");
    init(cx, dir.path());
    let (root, mut vcx) = open_empty(cx);
    vcx.simulate_keystrokes("cmd-t");
    vcx.simulate_keystrokes("alt-n");
    assert_eq!(labels(&mut vcx), (vec![], None));
    assert!(
        vcx.update(|_, cx| root.read(cx).workspace().read(cx).editor(cx))
            .is_none()
    );
    let crumbs = vcx.update(|_, cx| crate::views::status_bar::StatusBar::crumbs(cx));
    assert!(crumbs.is_empty());
}

#[gpui::test]
fn the_status_bar_shows_the_crumbs_and_the_caret(cx: &mut TestAppContext) {
    let dir = TempDir::new("caret");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    draw(&mut vcx);
    let crumbs = vcx.update(|_, cx| crate::views::status_bar::StatusBar::crumbs(cx));
    assert_eq!(
        crumbs.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
        ["fake server", "boa_dev", "console_1"]
    );
    let editor = vcx.update(|_, cx| root.read(cx).workspace().read(cx).editor(cx).unwrap());
    let state = vcx.update(|_, cx| editor.read(cx).state().clone());
    vcx.update(|window, cx| {
        state.update(cx, |s, cx| {
            s.focus(window, cx);
            s.set_selected_range(3..3, cx)
        })
    });
    draw(&mut vcx);
    let caret = vcx.update(|_, cx| root.read(cx).status_bar().read(cx).caret);
    assert_eq!(caret.map(|c| c.to_string()).as_deref(), Some("1:4"));
    // A table tab: schema › table crumbs, no caret.
    let key = DbKey {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev".into(),
    };
    let ws = workspaces(&mut vcx);
    vcx.update(|_, cx| ws.update(cx, |w, cx| w.open_table(key, "public", "orders", true, cx)));
    draw(&mut vcx);
    let crumbs = vcx.update(|_, cx| crate::views::status_bar::StatusBar::crumbs(cx));
    assert_eq!(crumbs.len(), 4);
    assert_eq!(crumbs[3].as_ref(), "orders");
    let caret = vcx.update(|_, cx| root.read(cx).status_bar().read(cx).caret);
    assert_eq!(caret, None);
}

// ── Live (PGHOST) ─────────────────────────────────────────────────────────

/// Waits (real time) for `done`, letting GPUI and the backend work.
fn wait_until(
    vcx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let start = Instant::now();
    loop {
        vcx.run_until_parked();
        if done(vcx) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn await_task<T: 'static>(
    vcx: &mut VisualTestContext,
    task: impl std::future::Future<Output = T> + 'static,
) -> T {
    let slot: Rc<RefCell<Option<T>>> = Rc::default();
    let out = slot.clone();
    vcx.update(|_, cx| {
        cx.spawn(async move |_| {
            let value = task.await;
            *out.borrow_mut() = Some(value);
        })
        .detach()
    });
    wait_until(vcx, "a backend call", |_| slot.borrow().is_some());
    slot.borrow_mut().take().unwrap()
}

const LIVE: &str = "live";

/// Saves and connects the local server (PGHOST …) as connection "live" and
/// tells the Connections model; None without PGHOST.
fn live_database(vcx: &mut VisualTestContext) -> Option<DbKey> {
    let env = |k: &str, d: &str| {
        std::env::var(k)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| d.to_owned())
    };
    let host = std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
    let database = env("PGDATABASE", "boa_dev");
    let config = ConnConfig {
        kind: Default::default(),
        id: LIVE.into(),
        name: "Local".into(),
        host,
        port: env("PGPORT", "5432").parse().unwrap_or(5432),
        user: env("PGUSER", "boa"),
        database: database.clone(),
        ssl_mode: Default::default(),
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    };
    let backend = vcx.update(|_, cx| AppState::global(cx).backend.clone());
    await_task(
        vcx,
        backend.save_connection(config.clone(), Some(env("PGPASSWORD", "boa")), true),
    )
    .expect("save");
    let info = await_task(vcx, backend.connect(LIVE, None)).expect("connect");
    vcx.update(|_, cx| {
        let conns = AppState::global(cx).connections.clone();
        conns.update(cx, |c, cx| {
            c.saved.push(SavedConnection {
                config,
                has_password: true,
            });
            c.state.insert(
                LIVE.into(),
                ConnState::Connected {
                    server_version: info.server_version,
                    databases: info.databases,
                },
            );
            cx.notify();
        });
    });
    Some(DbKey {
        connection_id: LIVE.into(),
        database,
    })
}

/// A console on the live database holding `sql`, shown and focused.
fn live_console(vcx: &mut VisualTestContext, key: &DbKey, sql: &str) -> Entity<Console> {
    let ws = workspaces(vcx);
    let (key, sql) = (key.clone(), sql.to_owned());
    vcx.update(|_, cx| ws.update(cx, |w, cx| w.new_query_tab_with(key, sql, cx)));
    draw(vcx);
    vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .read(cx)
            .active_tab()
            .and_then(|t| t.console().cloned())
            .unwrap()
    })
}

/// ⌘↵ twice (arm, run) in the focused console.
fn arm_and_run(vcx: &mut VisualTestContext) {
    vcx.simulate_keystrokes("cmd-enter");
    draw(vcx);
    vcx.simulate_keystrokes("cmd-enter");
    draw(vcx);
}

fn live_window(
    cx: &mut TestAppContext,
    name: &str,
) -> Option<(TempDir, Entity<RootView>, VisualTestContext, DbKey)> {
    std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
    let dir = TempDir::new(name);
    init(cx, dir.path());
    // The backend answers from its own threads.
    cx.executor().allow_parking();
    let (root, mut vcx) = open_main(cx);
    let key = live_database(&mut vcx)?;
    Some((dir, root, vcx, key))
}

#[gpui::test]
fn live_run_shows_the_result_in_the_database_output(cx: &mut TestAppContext) {
    let Some((_dir, root, mut vcx, key)) = live_window(cx, "live-run") else {
        return;
    };
    let console = live_console(
        &mut vcx,
        &key,
        "select id, name from public.customers order by id",
    );
    arm_and_run(&mut vcx);
    wait_until(&mut vcx, "the result", |vcx| {
        vcx.update(|_, cx| console.read(cx).result.is_some())
    });
    let (rows, truncated, page_size) = vcx.update(|_, cx| {
        let r = console.read(cx).result.clone().unwrap();
        (r.result.rows.len(), r.result.truncated, r.page_size)
    });
    // prefs.rowLimit (500 by default), not the backend's maximum.
    assert_eq!((rows, truncated, page_size), (500, true, 500));
    draw(&mut vcx);
    let tab_id = vcx.update(|_, cx| console.read(cx).tab_id.clone());
    let bottom = vcx.update(|_, cx| root.read(cx).bottom_panel().clone());
    assert_eq!(
        vcx.update(|_, cx| bottom.read(cx).view(cx)),
        BottomView::Result(tab_id)
    );
    assert!(
        vcx.update(|_, cx| bottom.read(cx).shown().is_some()),
        "the grid got the page"
    );
    assert!(layout_open(
        &mut vcx,
        crate::models::layout::PanelKind::Bottom
    ));
    let line = vcx.update(|_, cx| AppState::global(cx).workspaces.read(cx).output(LIVE)[0].clone());
    let outcome = crate::views::bottom_panel::output_outcome(&line.entry);
    assert!(
        outcome.starts_with("500 rows retrieved starting from 1 in "),
        "{outcome}"
    );
    assert!(outcome.ends_with(" (more available)"), "{outcome}");
    assert_eq!(line.entry.source.as_deref(), Some("console_1"));
    // A statement without rows: "N rows affected", the Output log shows.
    let console = live_console(
        &mut vcx,
        &key,
        "create temporary table boa_consoles_tmp as select 1 as x union all select 2",
    );
    arm_and_run(&mut vcx);
    wait_until(&mut vcx, "the statement", |vcx| {
        vcx.update(|_, cx| console.read(cx).running.is_none())
            && vcx.update(|_, cx| AppState::global(cx).workspaces.read(cx).output(LIVE).len()) >= 2
    });
    let line = vcx.update(|_, cx| AppState::global(cx).workspaces.read(cx).output(LIVE)[0].clone());
    assert_eq!(line.entry.affected, Some(2.), "{line:?}");
    assert_eq!(
        vcx.update(|_, cx| bottom.read(cx).view(cx)),
        BottomView::Output
    );
}

#[gpui::test]
fn live_cancel_stops_a_slow_statement(cx: &mut TestAppContext) {
    let Some((_dir, _root, mut vcx, key)) = live_window(cx, "live-cancel") else {
        return;
    };
    let console = live_console(&mut vcx, &key, "select boa_fx.slow(5)");
    let started = Instant::now();
    arm_and_run(&mut vcx);
    wait_until(&mut vcx, "the run to start", |vcx| {
        vcx.update(|_, cx| console.read(cx).running.is_some())
    });
    std::thread::sleep(Duration::from_millis(300));
    vcx.update(|_, cx| console.update(cx, |c, cx| c.cancel(cx)));
    wait_until(&mut vcx, "the cancel", |vcx| {
        vcx.update(|_, cx| console.read(cx).running.is_none())
    });
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "cancelled, not finished: {:?}",
        started.elapsed()
    );
    let failed = vcx
        .update(|_, cx| console.read(cx).failed.clone())
        .expect("failed");
    assert!(failed.message.starts_with("[57014]"), "{}", failed.message);
}

#[gpui::test]
fn live_errors_show_their_position_in_the_output_log(cx: &mut TestAppContext) {
    let Some((_dir, _root, mut vcx, key)) = live_window(cx, "live-error") else {
        return;
    };
    let console = live_console(&mut vcx, &key, "select *\nfrom boa_no_such_table");
    arm_and_run(&mut vcx);
    wait_until(&mut vcx, "the error", |vcx| {
        vcx.update(|_, cx| console.read(cx).failed.is_some())
    });
    let failed = vcx.update(|_, cx| console.read(cx).failed.clone()).unwrap();
    assert!(failed.message.starts_with("[42P01] "), "{}", failed.message);
    let line = vcx.update(|_, cx| AppState::global(cx).workspaces.read(cx).output(LIVE)[0].clone());
    assert_eq!(line.entry.error.as_deref(), Some(failed.message.as_str()));
    let (text, caret) = line.excerpt.expect("position");
    assert_eq!(text, "LINE 2: from boa_no_such_table");
    assert_eq!(caret, format!("{}^", " ".repeat("LINE 2: from ".len())));
    assert_eq!(
        crate::views::bottom_panel::output_outcome(&line.entry),
        format!("ERROR: {}", failed.message)
    );
}

#[gpui::test]
fn live_paging_fetches_the_next_page(cx: &mut TestAppContext) {
    let Some((_dir, root, mut vcx, key)) = live_window(cx, "live-paging") else {
        return;
    };
    vcx.update(|_, cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| p.update(cx, |p| p.row_limit = 100));
    });
    let console = live_console(&mut vcx, &key, "select id from public.orders order by id");
    arm_and_run(&mut vcx);
    wait_until(&mut vcx, "the first page", |vcx| {
        vcx.update(|_, cx| console.read(cx).result.is_some())
    });
    let first = vcx.update(|_, cx| console.read(cx).result.clone().unwrap());
    assert_eq!(
        (first.result.rows.len(), first.offset, first.result.pageable),
        (100, 0, true)
    );
    // The grid's pager asks; the console fetches.
    let grid = vcx.update(|_, cx| root.read(cx).bottom_panel().read(cx).grid().clone());
    vcx.update(|_, cx| {
        grid.update(cx, |_, cx| {
            cx.emit(GridEvent::PageRequested { offset: 100 })
        })
    });
    wait_until(&mut vcx, "the second page", |vcx| {
        vcx.update(|_, cx| {
            console
                .read(cx)
                .result
                .as_ref()
                .is_some_and(|r| r.offset == 100)
        })
    });
    let second = vcx.update(|_, cx| console.read(cx).result.clone().unwrap());
    assert_eq!(second.run, first.run, "a page of the same run");
    assert_ne!(second.result.rows[0][0], first.result.rows[0][0]);
    let line = vcx.update(|_, cx| AppState::global(cx).workspaces.read(cx).output(LIVE)[0].clone());
    assert_eq!(line.entry.offset, Some(100.));
    assert!(crate::views::bottom_panel::output_outcome(&line.entry).contains("starting from 101"));
}

#[gpui::test]
fn live_table_tabs_load_their_first_page(cx: &mut TestAppContext) {
    let Some((_dir, root, mut vcx, key)) = live_window(cx, "live-table") else {
        return;
    };
    let ws = workspaces(&mut vcx);
    let k = key.clone();
    vcx.update(|_, cx| ws.update(cx, |w, cx| w.open_table(k, "public", "customers", true, cx)));
    draw(&mut vcx);
    let table = vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .read(cx)
            .active_tab()
            .unwrap()
            .table()
            .unwrap()
            .clone()
    });
    wait_until(&mut vcx, "the table's rows", |vcx| {
        vcx.update(|_, cx| table.read(cx).result.is_some())
    });
    let (rows, sql) = vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.result.as_ref().unwrap().rows.len(), t.sql.clone())
    });
    assert_eq!(rows, 500);
    assert_eq!(sql, "select * from public.customers");
    draw(&mut vcx);
    let grid = vcx.update(|_, cx| root.read(cx).workspace().read(cx).grid_of(&table));
    assert!(grid.is_some(), "the table tab has its grid");
}
