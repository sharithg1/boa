//! Workspaces / Console model tests (store.ts tab and run behaviour), on a
//! temp config dir with pretend connections (no server).

use std::cell::RefCell;
use std::rc::Rc;

use boa_core::state::prefs::Scope;
use gpui::{Entity, TestAppContext};

use super::*;
use crate::models::console::ConsoleEvent;
use crate::tests::{FAKE_CONNECTION, TempDir, fake_connection, init};

fn setup(cx: &mut TestAppContext) -> (TempDir, Entity<Workspaces>, DbKey) {
    let dir = TempDir::new("workspaces");
    // Table tabs of the pretend connection still ask the real backend (and
    // fail): its answer arrives from the backend's thread.
    cx.executor().allow_parking();
    init(cx, dir.path());
    let (ws, key) = cx.update(|cx| {
        let key = fake_connection(cx, FAKE_CONNECTION, &["boa_dev", "boa_dev2"]);
        (AppState::global(cx).workspaces.clone(), key)
    });
    (dir, ws, key)
}

fn titles(ws: &Entity<Workspaces>, key: &DbKey, cx: &mut TestAppContext) -> Vec<String> {
    ws.read_with(cx, |w, _| {
        w.workspace(key)
            .map(|w| w.tabs.iter().map(|t| t.label().to_string()).collect())
            .unwrap_or_default()
    })
}

fn active_label(ws: &Entity<Workspaces>, cx: &mut TestAppContext) -> Option<String> {
    ws.read_with(cx, |w, _| w.active_tab().map(|t| t.label().to_string()))
}

fn tab_id(ws: &Entity<Workspaces>, key: &DbKey, label: &str, cx: &mut TestAppContext) -> TabId {
    ws.read_with(cx, |w, _| {
        w.workspace(key)
            .and_then(|w| w.tabs.iter().find(|t| t.label().as_ref() == label))
            .map(|t| t.id.clone())
            .expect(label)
    })
}

#[gpui::test]
fn consoles_take_the_lowest_free_console_n(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    for _ in 0..3 {
        ws.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    }
    assert_eq!(
        titles(&ws, &key, cx),
        ["console_1", "console_2", "console_3"]
    );
    assert_eq!(active_label(&ws, cx).as_deref(), Some("console_3"));
    let ids: Vec<String> = ws.read_with(cx, |w, _| {
        w.workspace(&key)
            .unwrap()
            .tabs
            .iter()
            .map(|t| t.id.to_string())
            .collect()
    });
    assert_eq!(ids, ["q1", "q2", "q3"]);
    let second = tab_id(&ws, &key, "console_2", cx);
    ws.update(cx, |w, cx| w.close_tab(key.clone(), second, cx));
    ws.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    assert_eq!(
        titles(&ws, &key, cx),
        ["console_1", "console_3", "console_2"]
    );
    // Ids keep counting.
    assert_eq!(
        ws.read_with(cx, |w, _| w.active_tab().unwrap().id.to_string()),
        "q4"
    );
    // A console with SQL (Show DDL, history).
    ws.update(cx, |w, cx| {
        w.new_query_tab_with(key.clone(), "select 42".into(), cx)
    });
    let sql = ws.read_with(cx, |w, cx| {
        w.active_tab()
            .unwrap()
            .console()
            .unwrap()
            .read(cx)
            .sql
            .clone()
    });
    assert_eq!(sql, "select 42");
}

#[gpui::test]
fn closing_a_tab_activates_its_right_neighbour_else_the_left(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    for _ in 0..3 {
        ws.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    }
    let b = tab_id(&ws, &key, "console_2", cx);
    ws.update(cx, |w, cx| w.activate(&key, &b, cx));
    ws.update(cx, |w, cx| w.close_tab(key.clone(), b, cx));
    assert_eq!(active_label(&ws, cx).as_deref(), Some("console_3"));
    assert!(ws.update(cx, |w, cx| w.close_active(cx)));
    assert_eq!(active_label(&ws, cx).as_deref(), Some("console_1"));
    assert!(ws.update(cx, |w, cx| w.close_active(cx)));
    assert_eq!(active_label(&ws, cx), None);
    assert!(
        !ws.update(cx, |w, cx| w.close_active(cx)),
        "nothing to close"
    );
    // ⌥n beyond the tabs does nothing.
    assert!(!ws.update(cx, |w, cx| w.activate_index(3, cx)));
}

#[gpui::test]
fn preview_tabs_are_replaced_until_pinned(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    ws.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    ws.update(cx, |w, cx| {
        w.open_table(key.clone(), "public", "customers", true, cx)
    });
    ws.update(cx, |w, cx| {
        w.open_table(key.clone(), "public", "orders", true, cx)
    });
    assert_eq!(
        titles(&ws, &key, cx),
        ["console_1", "orders"],
        "the preview was replaced"
    );
    // Opening it pinned (double click) pins the same tab.
    ws.update(cx, |w, cx| {
        w.open_table(key.clone(), "public", "orders", false, cx)
    });
    let preview = ws.read_with(cx, |w, _| {
        matches!(
            w.active_tab().unwrap().kind,
            TabKind::Table { preview: true, .. }
        )
    });
    assert!(!preview);
    ws.update(cx, |w, cx| {
        w.open_table(key.clone(), "public", "customers", true, cx)
    });
    assert_eq!(titles(&ws, &key, cx), ["console_1", "orders", "customers"]);
    // Reopening an open table activates it.
    ws.update(cx, |w, cx| {
        w.open_table(key.clone(), "public", "orders", true, cx)
    });
    assert_eq!(active_label(&ws, cx).as_deref(), Some("orders"));
    assert_eq!(titles(&ws, &key, cx).len(), 3);
    // Pin by double click.
    let customers = tab_id(&ws, &key, "customers", cx);
    ws.update(cx, |w, cx| w.pin_tab(&key, &customers, cx));
    let kinds = ws.read_with(cx, |w, _| {
        w.workspace(&key)
            .unwrap()
            .tabs
            .iter()
            .filter(|t| matches!(t.kind, TabKind::Table { preview: true, .. }))
            .count()
    });
    assert_eq!(kinds, 0);
    let mru = ws.read_with(cx, |w, _| w.mru.clone());
    let tk = |t: &str| core_session::table_key(FAKE_CONNECTION, "boa_dev", "public", t);
    assert_eq!(mru, vec![tk("orders"), tk("customers")]);
    assert!(ws.read_with(cx, |w, _| w.seen.contains_key(&tk("customers"))));
    // Reorder by drag.
    ws.update(cx, |w, cx| w.move_tab(&key, &customers, 0, cx));
    assert_eq!(titles(&ws, &key, cx), ["customers", "console_1", "orders"]);
}

#[gpui::test]
fn the_mru_keeps_thirty_tables(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    for i in 0..35 {
        ws.update(cx, |w, cx| {
            w.open_table(key.clone(), "public", &format!("t{i}"), true, cx)
        });
    }
    let mru = ws.read_with(cx, |w, _| w.mru.clone());
    assert_eq!(mru.len(), MRU_CAP);
    assert!(mru[0].ends_with("\u{0}t34"));
}

#[gpui::test]
fn databases_cycle_in_sidebar_order(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    let other = cx.update(|cx| {
        let other = fake_connection(cx, "second", &["app"]);
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                p.scopes.insert(FAKE_CONNECTION.into(), Scope::All);
            })
        });
        other
    });
    let dev2 = DbKey {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev2".into(),
    };
    let active = |cx: &mut TestAppContext| ws.read_with(cx, |w, _| w.active.clone());
    ws.update(cx, |w, cx| w.cycle_database(1, cx));
    assert_eq!(active(cx), Some(key.clone()), "nothing selected: the first");
    ws.update(cx, |w, cx| w.cycle_database(1, cx));
    assert_eq!(active(cx), Some(dev2.clone()));
    ws.update(cx, |w, cx| w.cycle_database(1, cx));
    assert_eq!(active(cx), Some(other.clone()));
    ws.update(cx, |w, cx| w.cycle_database(1, cx));
    assert_eq!(active(cx), Some(key.clone()), "wraps around");
    ws.update(cx, |w, cx| w.cycle_database(-1, cx));
    assert_eq!(active(cx), Some(other));
    // A database's tabs are its own.
    ws.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    ws.update(cx, |w, cx| w.new_query_tab(dev2.clone(), cx));
    assert_eq!(titles(&ws, &dev2, cx), ["console_1"]);
    assert_eq!(titles(&ws, &key, cx), ["console_1"]);
}

#[gpui::test]
fn new_query_goes_to_the_selected_else_the_first_connected_database(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    assert_eq!(
        ws.read_with(cx, |w, cx| w.new_query_target(cx)),
        Some(key.clone())
    );
    let offline = DbKey {
        connection_id: "offline".into(),
        database: "x".into(),
    };
    ws.update(cx, |w, _| w.active = Some(offline));
    assert_eq!(ws.read_with(cx, |w, cx| w.new_query_target(cx)), Some(key));
}

#[gpui::test]
fn the_output_log_keeps_two_hundred_lines_newest_first(cx: &mut TestAppContext) {
    let (_dir, ws, _key) = setup(cx);
    let events: Rc<RefCell<Vec<bool>>> = Rc::default();
    let _sub = cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&ws, move |_, e: &WorkspacesEvent, _| {
            let WorkspacesEvent::OutputLogged { record, .. } = e;
            events.borrow_mut().push(*record);
        })
    });
    for i in 0..205 {
        let entry = OutputEntry {
            sql: format!("select {i}"),
            ..OutputEntry::default()
        };
        ws.update(cx, |w, cx| {
            w.log(FAKE_CONNECTION, entry, None, i % 2 == 0, cx)
        });
    }
    cx.run_until_parked();
    let (len, first, ids) = ws.read_with(cx, |w, _| {
        let out = w.output(FAKE_CONNECTION);
        (
            out.len(),
            out[0].entry.sql.clone(),
            (out[0].entry.id, out[199].entry.id),
        )
    });
    assert_eq!(len, OUTPUT_CAP);
    assert_eq!(first, "select 204");
    assert_eq!(ids, (205, 6));
    assert_eq!(events.borrow().len(), 205);
    assert!(events.borrow()[0] && !events.borrow()[1]);
    ws.update(cx, |w, cx| w.clear_output(FAKE_CONNECTION, cx));
    assert!(ws.read_with(cx, |w, _| w.output(FAKE_CONNECTION).is_empty()));
}

#[gpui::test]
fn tabs_round_trip_through_the_session(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    ws.update(cx, |w, cx| {
        w.new_query_tab_with(key.clone(), "select 1".into(), cx)
    });
    ws.update(cx, |w, cx| {
        w.open_table(key.clone(), "public", "orders", true, cx)
    });
    let first = tab_id(&ws, &key, "console_1", cx);
    ws.update(cx, |w, cx| w.activate(&key, &first, cx));
    let session = ws.read_with(cx, |w, cx| w.to_session(cx));
    let json: serde_json::Value = serde_json::from_str(&session.to_json()).unwrap();
    let db = core_session::db_key(FAKE_CONNECTION, "boa_dev");
    assert_eq!(json["active"]["connectionId"], FAKE_CONNECTION);
    assert_eq!(json["workspaces"][&db]["activeTab"], "q1");
    assert_eq!(
        json["workspaces"][&db]["tabs"][0],
        serde_json::json!({"id": "q1", "kind": "query", "title": "console_1", "sql": "select 1"})
    );
    assert_eq!(
        json["workspaces"][&db]["tabs"][1],
        serde_json::json!({"id": "t2", "kind": "table", "schema": "public", "table": "orders", "preview": true})
    );
    assert_eq!(
        json["mru"][0],
        core_session::table_key(FAKE_CONNECTION, "boa_dev", "public", "orders")
    );

    let restored = cx.new(|_| Workspaces::new());
    restored.update(cx, |w, cx| w.restore(&session, cx));
    assert_eq!(titles(&restored, &key, cx), ["console_1", "orders"]);
    assert_eq!(active_label(&restored, cx).as_deref(), Some("console_1"));
    let sql = restored.read_with(cx, |w, cx| {
        w.active_tab()
            .unwrap()
            .console()
            .unwrap()
            .read(cx)
            .sql
            .clone()
    });
    assert_eq!(sql, "select 1");
    // New ids continue after the restored ones.
    restored.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    assert_eq!(
        restored.read_with(cx, |w, _| w.active_tab().unwrap().id.to_string()),
        "q3"
    );
}

#[gpui::test]
fn destructive_statements_ask_and_read_only_escapes_are_refused(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    ws.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    let console = ws.read_with(cx, |w, _| {
        w.active_tab().unwrap().console().unwrap().clone()
    });
    let asked: Rc<RefCell<Vec<ConsoleEvent>>> = Rc::default();
    let _sub = cx.update(|cx| {
        let asked = asked.clone();
        cx.subscribe(&console, move |_, e: &ConsoleEvent, _| {
            asked.borrow_mut().push(e.clone())
        })
    });
    console.update(cx, |c, cx| c.run("delete from orders".into(), cx));
    cx.run_until_parked();
    match asked.borrow().as_slice() {
        [
            ConsoleEvent::ConfirmRun {
                sql,
                risks,
                production,
            },
        ] => {
            assert_eq!(sql, "delete from orders");
            assert!(!risks.is_empty());
            assert!(!production);
        }
        other => panic!("expected a confirmation, got {other:?}"),
    }
    assert!(
        console.read_with(cx, |c, _| c.running.is_none()),
        "not run before confirming"
    );

    // Read-only: a statement that turns read-only off is not run.
    cx.update(|cx| {
        let conns = AppState::global(cx).connections.clone();
        conns.update(cx, |c, _| c.saved[0].config.read_only = true);
    });
    console.update(cx, |c, cx| {
        c.run("set default_transaction_read_only = off".into(), cx)
    });
    cx.run_until_parked();
    let failed = console
        .read_with(cx, |c, _| c.failed.clone())
        .expect("refused");
    assert!(
        failed.message.starts_with("Not run: "),
        "{}",
        failed.message
    );
    let logged = ws.read_with(cx, |w, _| w.output(FAKE_CONNECTION)[0].entry.clone());
    assert_eq!(logged.error.as_deref(), Some(failed.message.as_str()));
    assert_eq!(logged.source.as_deref(), Some("console_1"));
}

#[gpui::test]
fn running_on_a_closed_connection_says_so(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    cx.update(|cx| {
        let conns = AppState::global(cx).connections.clone();
        conns.update(cx, |c, _| {
            c.state
                .insert(FAKE_CONNECTION.into(), ConnState::Disconnected);
            c.set_api(std::rc::Rc::new(RefusingApi));
        });
    });
    ws.update(cx, |w, cx| w.new_query_tab(key.clone(), cx));
    let console = ws.read_with(cx, |w, _| {
        w.active_tab().unwrap().console().unwrap().clone()
    });
    console.update(cx, |c, cx| c.run("select 1".into(), cx));
    cx.run_until_parked();
    let failed = console
        .read_with(cx, |c, _| c.failed.clone())
        .expect("failed");
    assert_eq!(
        failed.message,
        format!("{FAKE_CONNECTION} server is not connected")
    );
    let bottom = cx.update(|cx| AppState::global(cx).layout.read(cx).bottom_open());
    assert!(bottom, "failures show the Database Output panel");
    assert_eq!(
        ws.read_with(cx, |w, _| w.bottom_view.clone()),
        None,
        "on the Output log"
    );
}

/// A backend whose server refuses every connect ("running a query connects first").
struct RefusingApi;

impl crate::models::connections::ConnApi for RefusingApi {
    fn list(
        &self,
    ) -> crate::models::connections::ApiFuture<Vec<boa_core::connections::SavedConnection>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn save(
        &self,
        _: boa_core::connections::ConnConfig,
        _: Option<String>,
        _: bool,
    ) -> crate::models::connections::ApiFuture<()> {
        Box::pin(async { Ok(()) })
    }
    fn delete(&self, _: &str) -> crate::models::connections::ApiFuture<()> {
        Box::pin(async { Ok(()) })
    }
    fn test(
        &self,
        _: boa_core::connections::ConnConfig,
        _: Option<String>,
    ) -> crate::models::connections::ApiFuture<String> {
        Box::pin(async {
            Err(boa_core::error::CmdError::code(
                "unreachable",
                "connection refused",
            ))
        })
    }
    fn connect(
        &self,
        _: &str,
        _: Option<String>,
    ) -> crate::models::connections::ApiFuture<boa_core::pg::ConnectInfo> {
        Box::pin(async {
            Err(boa_core::error::CmdError::code(
                "unreachable",
                "connection refused",
            ))
        })
    }
    fn ping(&self, _: &str) -> crate::models::connections::ApiFuture<boa_core::pg::PingInfo> {
        Box::pin(async {
            Err(boa_core::error::CmdError::code(
                "unreachable",
                "connection refused",
            ))
        })
    }
    fn disconnect(&self, _: &str) -> crate::models::connections::ApiFuture<()> {
        Box::pin(async { Ok(()) })
    }
}

/// Deleting a data source dropped nothing but the saved entry: its tabs
/// stayed open and the title bar kept showing its database.
#[gpui::test]
fn deleting_a_connection_closes_its_tabs(cx: &mut TestAppContext) {
    let (_dir, ws, key) = setup(cx);
    cx.executor().allow_parking();
    let other = cx.update(|cx| fake_connection(cx, "other", &["db2"]));
    ws.update(cx, |w, cx| {
        w.new_query_tab(other.clone(), cx);
        w.new_query_tab(key.clone(), cx);
        w.new_query_tab(key.clone(), cx);
    });
    let id = tab_id(&ws, &key, "console_1", cx);
    ws.update(cx, |w, cx| w.set_bottom_view(Some(id), cx));
    let conns = cx.update(|cx| AppState::global(cx).connections.clone());
    let task = conns.update(cx, |c, cx| c.delete(FAKE_CONNECTION, cx));
    let done = Rc::new(RefCell::new(None));
    let slot = done.clone();
    cx.foreground_executor()
        .spawn(async move {
            *slot.borrow_mut() = Some(task.await);
        })
        .detach();
    // The real backend answers on its own thread.
    for _ in 0..500 {
        cx.run_until_parked();
        if done.borrow().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = done.borrow_mut().take().expect("the delete finished");
    assert!(result.is_ok(), "{result:?}");
    cx.run_until_parked();
    ws.read_with(cx, |w, _| {
        assert!(w.workspace(&key).is_none(), "its tabs are gone");
        assert_eq!(w.active, None, "nothing selected");
        assert_eq!(w.bottom_view, None);
        assert_eq!(w.workspace(&other).map(|w| w.tabs.len()), Some(1));
    });
}
