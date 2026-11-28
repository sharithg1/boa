//! The transactions model, the TxControls and the guard dialogs in the real
//! main window: outcomes tracked per session, the shortcuts, the
//! destructive-statement confirmation, the disconnect / quit / close guards,
//! and (with `PGHOST`) Manual mode, commit / rollback, aborted transactions,
//! typed BEGIN, read-only connections against the local server.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_core::connections::{ConnConfig, Environment, SavedConnection};
use boa_core::error::CmdError;
use boa_core::state::guards::CloseAction;
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::connections::{ConnState, ConnectionsEvent};
use crate::models::console::Console;
use crate::platform::terminate::TerminateReply;
use crate::tests::{FAKE_CONNECTION, TempDir, init, open_main};
use crate::views::guard_dialogs::{self, GuardAnswer, GuardDialog, GuardKind};
use crate::views::tx_controls;

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

fn fake_key() -> DbKey {
    DbKey {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev".into(),
    }
}

fn transactions(vcx: &mut VisualTestContext) -> Entity<Transactions> {
    vcx.update(|_, cx| AppState::global(cx).transactions.clone())
}

fn session(vcx: &mut VisualTestContext, key: &DbKey) -> TxSession {
    let t = transactions(vcx);
    vcx.update(|_, cx| t.read(cx).get(key))
}

fn note(vcx: &mut VisualTestContext, key: &DbKey, state: TxState) {
    let t = transactions(vcx);
    vcx.update(|_, cx| t.update(cx, |t, cx| t.note_outcome(key, Some(state), cx)));
}

fn guard(vcx: &mut VisualTestContext) -> Option<Entity<GuardDialog>> {
    vcx.update(|_, cx| guard_dialogs::active(cx))
}

fn guard_title(vcx: &mut VisualTestContext) -> Option<&'static str> {
    let g = guard(vcx)?;
    Some(vcx.update(|_, cx| g.read(cx).kind().title()))
}

fn answer(vcx: &mut VisualTestContext, answer: GuardAnswer) {
    let g = guard(vcx).expect("a guard dialog");
    vcx.update(|window, cx| g.update(cx, |d, cx| d.answer(answer, window, cx)));
    draw(vcx);
}

fn active_console(vcx: &mut VisualTestContext) -> Entity<Console> {
    vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .read(cx)
            .active_tab()
            .and_then(|t| t.console().cloned())
            .unwrap()
    })
}

fn set_confirm_destructive(vcx: &mut VisualTestContext, on: bool) {
    vcx.update(|_, cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| p.update(cx, |p| p.confirm_destructive = on));
    });
}

fn set_environment(vcx: &mut VisualTestContext, id: &str, env: Option<Environment>) {
    let id = id.to_owned();
    vcx.update(|_, cx| {
        let conns = AppState::global(cx).connections.clone();
        conns.update(cx, |c, cx| {
            if let Some(s) = c.saved.iter_mut().find(|s| s.config.id == id) {
                s.config.environment = env;
            }
            cx.notify();
        });
    });
}

fn is_quitting(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|_, cx| crate::app::is_quitting(cx))
}

// ── The model ─────────────────────────────────────────────────────────────

#[test]
fn outcomes_report_the_state_on_success_and_in_errors() {
    let mut err = CmdError::msg("current transaction is aborted");
    assert_eq!(outcome_tx::<()>(&Err(err.clone())), None);
    err.tx = Some(TxState::Failed);
    assert_eq!(outcome_tx::<()>(&Err(err)), Some(TxState::Failed));
    assert_eq!(outcome_tx(&Ok(())), None);
}

#[gpui::test]
fn sessions_follow_outcomes_and_connection_events(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-model");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let key = fake_key();
    let other = DbKey {
        connection_id: FAKE_CONNECTION.into(),
        database: "boa_dev2".into(),
    };
    let t = transactions(&mut vcx);
    let events: Rc<RefCell<Vec<TransactionsEvent>>> = Rc::default();
    let seen = events.clone();
    let _sub = vcx.update(|_, cx| {
        cx.subscribe(&t, move |_, e: &TransactionsEvent, _| {
            seen.borrow_mut().push(e.clone())
        })
    });
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);
    note(&mut vcx, &key, TxState::Active);
    vcx.update(|_, cx| {
        t.update(cx, |t, cx| {
            t.note_mode_set(&other, TxMode::Manual, TxState::Idle, cx)
        })
    });
    assert_eq!(session(&mut vcx, &key).state, TxState::Active);
    assert_eq!(session(&mut vcx, &other).mode, TxMode::Manual);
    assert_eq!(
        vcx.update(|_, cx| t.read(cx).open_sessions()),
        std::slice::from_ref(&key)
    );
    assert_eq!(
        events.borrow().first(),
        Some(&TransactionsEvent::Changed { key: key.clone() })
    );

    // A lost transaction: idle, reported.
    let conns = vcx.update(|_, cx| AppState::global(cx).connections.clone());
    vcx.update(|_, cx| {
        conns.update(cx, |_, cx| {
            cx.emit(ConnectionsEvent::TxLost {
                id: FAKE_CONNECTION.into(),
                database: "boa_dev".into(),
            })
        })
    });
    vcx.run_until_parked();
    assert_eq!(session(&mut vcx, &key).state, TxState::Idle);
    assert!(
        events
            .borrow()
            .contains(&TransactionsEvent::TxLost { key: key.clone() })
    );

    // Connecting (or disconnecting) drops the connection's sessions.
    note(&mut vcx, &key, TxState::Failed);
    vcx.update(|_, cx| {
        conns.update(cx, |_, cx| {
            cx.emit(ConnectionsEvent::Connected {
                id: FAKE_CONNECTION.into(),
            })
        })
    });
    vcx.run_until_parked();
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);
    assert_eq!(session(&mut vcx, &other), IDLE_SESSION);
    note(&mut vcx, &key, TxState::Active);
    vcx.update(|_, cx| {
        conns.update(cx, |_, cx| {
            cx.emit(ConnectionsEvent::Deleted {
                id: FAKE_CONNECTION.into(),
            })
        })
    });
    vcx.run_until_parked();
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);
}

#[gpui::test]
fn the_status_bar_shows_manual_mode_and_open_transactions(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-status");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let key = fake_key();
    let shown =
        |vcx: &mut VisualTestContext| vcx.update(|_, cx| tx_controls::tx_status(cx).is_some());
    assert!(!shown(&mut vcx), "Auto and idle: nothing");
    note(&mut vcx, &key, TxState::Active);
    draw(&mut vcx);
    assert!(shown(&mut vcx), "a typed BEGIN in Auto shows");
    let t = transactions(&mut vcx);
    vcx.update(|_, cx| {
        t.update(cx, |t, cx| {
            t.note_mode_set(&key, TxMode::Manual, TxState::Idle, cx)
        })
    });
    draw(&mut vcx);
    assert!(shown(&mut vcx), "Manual shows");
    assert_eq!(tx_controls::mode_label(TxMode::Manual), "Manual");
}

#[gpui::test]
fn shortcuts_apply_only_with_a_transaction_open(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-shortcuts");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let key = fake_key();
    let t = transactions(&mut vcx);
    let busy = |vcx: &mut VisualTestContext| vcx.update(|_, cx| t.read(cx).is_busy(&fake_key()));
    // Idle: neither does anything (the key goes on to the editor).
    vcx.simulate_keystrokes("cmd-shift-enter");
    vcx.simulate_keystrokes("cmd-shift-backspace");
    assert!(!busy(&mut vcx));
    // Aborted: no commit.
    note(&mut vcx, &key, TxState::Failed);
    vcx.simulate_keystrokes("cmd-shift-enter");
    assert!(!busy(&mut vcx));
    assert!(!vcx.update(|_, cx| tx_controls::shortcut(true, cx)));
}

// ── Guard dialogs ─────────────────────────────────────────────────────────

#[gpui::test]
fn destructive_statements_ask_first_unless_turned_off_production_always(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-confirm");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    draw(&mut vcx);
    let console = active_console(&mut vcx);

    // On (the default): asks; Enter is Cancel and nothing runs.
    vcx.update(|_, cx| console.update(cx, |c, cx| c.run("delete from orders".into(), cx)));
    draw(&mut vcx);
    assert_eq!(guard_title(&mut vcx), Some("Run a destructive statement?"));
    let g = guard(&mut vcx).unwrap();
    vcx.update(|_, cx| {
        let GuardKind::Run {
            risks, database, ..
        } = g.read(cx).kind()
        else {
            panic!("a run confirmation");
        };
        assert_eq!(database, "boa_dev");
        assert!(
            risks[0].message.to_lowercase().contains("where"),
            "{risks:?}"
        );
    });
    vcx.simulate_keystrokes("enter");
    draw(&mut vcx);
    assert!(guard(&mut vcx).is_none(), "Enter cancels");
    assert!(vcx.update(|_, cx| console.read(cx).running.is_none()));

    // Production asks even with the setting off, with its own title; Esc cancels.
    set_confirm_destructive(&mut vcx, false);
    set_environment(&mut vcx, FAKE_CONNECTION, Some(Environment::Production));
    vcx.update(|_, cx| console.update(cx, |c, cx| c.run("truncate orders".into(), cx)));
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Run a destructive statement on production?")
    );
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(guard(&mut vcx).is_none(), "Esc cancels");
    assert!(vcx.update(|_, cx| console.read(cx).running.is_none()));

    // Off and not production: runs without asking (the pretend server fails it).
    set_environment(&mut vcx, FAKE_CONNECTION, None);
    vcx.executor().allow_parking();
    vcx.update(|_, cx| console.update(cx, |c, cx| c.run("delete from orders".into(), cx)));
    draw(&mut vcx);
    assert!(guard(&mut vcx).is_none(), "no question");
    wait_until(&mut vcx, "the run", |vcx| {
        vcx.update(|_, cx| console.read(cx).failed.is_some())
    });
}

#[gpui::test]
fn a_new_prompt_answers_the_earlier_one_no(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-one-at-a-time");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let key = fake_key();
    note(&mut vcx, &key, TxState::Active);
    let first =
        vcx.update(|_, cx| guard_dialogs::confirm_close_tx(CloseAction::Disconnect, None, cx));
    draw(&mut vcx);
    let second = vcx.update(|_, cx| guard_dialogs::choose_end_tx("boa_dev", TxState::Active, cx));
    draw(&mut vcx);
    assert_eq!(guard_title(&mut vcx), Some("Switch to auto-commit?"));
    let first = resolve(&mut vcx, first);
    assert!(!first, "replaced: no");
    // ⌘⇧↵ / ⌘⇧⌫ answer nothing in a guard dialog (GuardDialogs.tsx
    // `useTxShortcuts` returns while a dialog is open); its Commit button does.
    vcx.simulate_keystrokes("cmd-shift-enter");
    vcx.simulate_keystrokes("cmd-shift-backspace");
    draw(&mut vcx);
    assert_eq!(guard_title(&mut vcx), Some("Switch to auto-commit?"));
    assert!(
        !vcx.update(|_, cx| transactions_busy(cx, &key)),
        "nothing committed or rolled back"
    );
    answer(&mut vcx, GuardAnswer::Commit);
    assert_eq!(
        resolve(&mut vcx, second),
        Some(boa_core::state::guards::EndTxChoice::Commit)
    );

    // An aborted transaction can only be rolled back.
    let third = vcx.update(|_, cx| guard_dialogs::choose_end_tx("boa_dev", TxState::Failed, cx));
    draw(&mut vcx);
    assert!(guard(&mut vcx).is_some());
    answer(&mut vcx, GuardAnswer::Rollback);
    assert_eq!(
        resolve(&mut vcx, third),
        Some(boa_core::state::guards::EndTxChoice::Rollback)
    );
}

#[gpui::test]
fn disconnecting_with_an_open_transaction_asks(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-disconnect");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let key = fake_key();
    let conns = vcx.update(|_, cx| AppState::global(cx).connections.clone());

    note(&mut vcx, &key, TxState::Active);
    let task = vcx.update(|_, cx| conns.update(cx, |c, cx| c.disconnect_task(FAKE_CONNECTION, cx)));
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Disconnect with an open transaction?")
    );
    let g = guard(&mut vcx).unwrap();
    vcx.update(|_, cx| {
        let GuardKind::CloseTx { open, .. } = g.read(cx).kind() else {
            panic!("a close prompt");
        };
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].connection, "fake server");
        assert_eq!(open[0].database, "boa_dev");
    });
    answer(&mut vcx, GuardAnswer::Cancel);
    assert!(!resolve(&mut vcx, task), "declined");
    assert!(vcx.update(|_, cx| conns.read(cx).is_connected(FAKE_CONNECTION)));
    assert_eq!(session(&mut vcx, &key).state, TxState::Active);

    // "Roll back and disconnect": disconnected, the session forgotten. (⌘⇧⌫
    // doesn't answer it: the source ignores the tx keys over a dialog.)
    vcx.executor().allow_parking();
    let task = vcx.update(|_, cx| conns.update(cx, |c, cx| c.disconnect_task(FAKE_CONNECTION, cx)));
    draw(&mut vcx);
    vcx.simulate_keystrokes("cmd-shift-backspace");
    draw(&mut vcx);
    assert!(guard(&mut vcx).is_some(), "⌘⇧⌫ answers nothing");
    answer(&mut vcx, GuardAnswer::Confirm);
    assert!(resolve(&mut vcx, task));
    assert!(!vcx.update(|_, cx| conns.read(cx).is_connected(FAKE_CONNECTION)));
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);

    // Nothing open: no question.
    let task =
        vcx.update(|_, cx| guard_dialogs::confirm_close_tx(CloseAction::Reconnect, None, cx));
    assert!(resolve(&mut vcx, task));
    assert!(guard(&mut vcx).is_none());
}

#[gpui::test]
fn quitting_with_an_open_transaction_asks_and_a_second_cmd_q_quits(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-quit");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    note(&mut vcx, &fake_key(), TxState::Active);
    vcx.simulate_keystrokes("cmd-q");
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Quit with an open transaction?")
    );
    assert!(!is_quitting(&mut vcx));
    // Again within 3 s: quits anyway.
    vcx.simulate_keystrokes("cmd-q");
    draw(&mut vcx);
    assert!(is_quitting(&mut vcx));
}

#[gpui::test]
fn quit_cancel_stays_roll_back_and_quit_quits(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-quit-answer");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    note(&mut vcx, &fake_key(), TxState::Failed);
    vcx.simulate_keystrokes("cmd-q");
    draw(&mut vcx);
    let g = guard(&mut vcx).expect("asks");
    vcx.update(|_, cx| {
        let GuardKind::CloseTx { open, action } = g.read(cx).kind() else {
            panic!("a close prompt");
        };
        assert_eq!(*action, CloseAction::Quit);
        assert_eq!(open[0].state, TxState::Failed);
    });
    answer(&mut vcx, GuardAnswer::Cancel);
    assert!(!is_quitting(&mut vcx));

    // The main window's close button asks the same (it is the last window).
    assert!(!vcx.simulate_close(), "held");
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Quit with an open transaction?")
    );
    vcx.executor().allow_parking();
    answer(&mut vcx, GuardAnswer::Confirm);
    wait_until(&mut vcx, "the quit", is_quitting);
}

#[gpui::test]
fn quitting_without_transactions_does_not_ask(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-quit-free");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    assert!(vcx.simulate_close(), "nothing open: closes");
    let dir = TempDir::new("tx-quit-free-2");
    let mut cx2 = cx.new_app();
    init(&mut cx2, dir.path());
    let (_root, mut vcx) = open_main(&mut cx2);
    vcx.simulate_keystrokes("cmd-q");
    draw(&mut vcx);
    assert!(guard(&mut vcx).is_none());
    assert!(is_quitting(&mut vcx));
}

// Dock → Quit / logout / restart: AppKit's `applicationShouldTerminate:`
// reaches `app::terminate_handler`'s function (platform/terminate.rs); here
// it is called directly, between updates, as AppKit would.

#[gpui::test]
fn a_dock_quit_with_an_open_transaction_is_held_and_asks(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-dock-quit");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let terminate = vcx.update(|_, cx| crate::app::terminate_handler(cx));
    // Nothing open: through at once, no question.
    assert_eq!(terminate(), TerminateReply::Now);
    draw(&mut vcx);
    assert!(guard(&mut vcx).is_none());

    note(&mut vcx, &fake_key(), TxState::Active);
    draw(&mut vcx);
    assert_eq!(terminate(), TerminateReply::Cancel, "held");
    draw(&mut vcx);
    let g = guard(&mut vcx).expect("asks");
    vcx.update(|_, cx| {
        let GuardKind::CloseTx { open, action } = g.read(cx).kind() else {
            panic!("a close prompt");
        };
        assert_eq!(*action, CloseAction::Quit);
        assert_eq!(open.len(), 1);
    });
    assert_eq!(
        guard_title(&mut vcx),
        Some("Quit with an open transaction?")
    );
    answer(&mut vcx, GuardAnswer::Cancel);
    assert!(!is_quitting(&mut vcx));
    assert_eq!(session(&mut vcx, &fake_key()).state, TxState::Active);
    // Again within 3 s (the Dock or ⌘Q, one gate): through, as in Tauri.
    assert_eq!(terminate(), TerminateReply::Now);
}

#[gpui::test]
fn a_held_dock_quit_rolls_back_and_quits_when_confirmed(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-dock-quit-yes");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let terminate = vcx.update(|_, cx| crate::app::terminate_handler(cx));
    note(&mut vcx, &fake_key(), TxState::Failed);
    draw(&mut vcx);
    assert_eq!(terminate(), TerminateReply::Cancel);
    draw(&mut vcx);
    vcx.executor().allow_parking();
    answer(&mut vcx, GuardAnswer::Confirm);
    wait_until(&mut vcx, "the quit", is_quitting);
    // The confirmed quit's own terminate isn't held again.
    assert_eq!(terminate(), TerminateReply::Now);
}

#[gpui::test]
fn after_cmd_q_asked_a_dock_quit_goes_through(cx: &mut TestAppContext) {
    let dir = TempDir::new("tx-dock-quit-gate");
    init(cx, dir.path());
    let (_root, mut vcx) = open_main(cx);
    let terminate = vcx.update(|_, cx| crate::app::terminate_handler(cx));
    note(&mut vcx, &fake_key(), TxState::Active);
    vcx.simulate_keystrokes("cmd-q");
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Quit with an open transaction?")
    );
    assert_eq!(terminate(), TerminateReply::Now, "asked within 3 s");
}

// ── Helpers ───────────────────────────────────────────────────────────────

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

/// Runs `task` to its end.
fn resolve<T: 'static>(
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
    wait_until(vcx, "a task", |_| slot.borrow().is_some());
    slot.borrow_mut().take().unwrap()
}

// ── Live (PGHOST) ─────────────────────────────────────────────────────────

fn env(k: &str, d: &str) -> String {
    std::env::var(k)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| d.to_owned())
}

/// Saves and connects the local server as connection `id` (as `user`,
/// read-only or not) and tells the Connections model.
fn live_connection(vcx: &mut VisualTestContext, id: &str, user: &str, read_only: bool) -> DbKey {
    let database = env("PGDATABASE", "boa_dev");
    let config = ConnConfig {
        kind: Default::default(),
        id: id.into(),
        name: format!("Local {id}"),
        host: env("PGHOST", "localhost"),
        port: env("PGPORT", "5432").parse().unwrap_or(5432),
        user: user.into(),
        database: database.clone(),
        ssl_mode: Default::default(),
        read_only,
        environment: None,
        color: None,
        ssh: None,
    };
    let backend = vcx.update(|_, cx| AppState::global(cx).backend.clone());
    let password = if user == "boa_ro" {
        "boa_ro".to_owned()
    } else {
        env("PGPASSWORD", "boa")
    };
    resolve(
        vcx,
        backend.save_connection(config.clone(), Some(password), true),
    )
    .expect("save");
    let info = resolve(vcx, backend.connect(id, None)).expect("connect");
    let id = id.to_owned();
    vcx.update(|_, cx| {
        let conns = AppState::global(cx).connections.clone();
        conns.update(cx, |c, cx| {
            c.saved.push(SavedConnection {
                config,
                has_password: true,
            });
            c.state.insert(
                id.clone(),
                ConnState::Connected {
                    server_version: info.server_version,
                    databases: info.databases,
                },
            );
            cx.notify();
        });
    });
    DbKey {
        connection_id: id,
        database,
    }
}

/// The main window with a live connection and an empty table
/// `boa_txg.<schema> (id int)`; None without PGHOST.
fn live_window(
    cx: &mut TestAppContext,
    schema: &str,
) -> Option<(TempDir, VisualTestContext, DbKey)> {
    std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
    let dir = TempDir::new(schema);
    init(cx, dir.path());
    cx.executor().allow_parking();
    let (_root, mut vcx) = open_main(cx);
    let key = live_connection(&mut vcx, "live", "boa", false);
    let backend = vcx.update(|_, cx| AppState::global(cx).backend.clone());
    // One schema for all (created once: concurrent CREATE SCHEMA IF NOT EXISTS
    // can collide) and a table per test, never dropped: other tests
    // introspect the database meanwhile, and a relation dropped under
    // boa-core's introspection query makes it fail (pg.rs reads its NULL size).
    static SCHEMA: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    {
        let mut created = SCHEMA
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*created {
            resolve(
                &mut vcx,
                backend.run_query(
                    "live",
                    &key.database,
                    "create schema if not exists boa_txg",
                    10,
                    0,
                ),
            )
            .expect("schema");
            *created = true;
        }
    }
    let setup =
        format!("create table if not exists boa_txg.{schema} (id int); truncate boa_txg.{schema}");
    resolve(
        &mut vcx,
        backend.run_query("live", &key.database, &setup, 10, 0),
    )
    .expect("fixture");
    Some((dir, vcx, key))
}

fn empty_table(vcx: &mut VisualTestContext, key: &DbKey, schema: &str) {
    let backend = vcx.update(|_, cx| AppState::global(cx).backend.clone());
    let sql = format!("truncate boa_txg.{schema}");
    let _ = resolve(
        vcx,
        backend.run_query(&key.connection_id, &key.database, &sql, 10, 0),
    );
}

/// A console on `key`, shown and focused.
fn live_console(vcx: &mut VisualTestContext, key: &DbKey) -> Entity<Console> {
    let key = key.clone();
    vcx.update(|_, cx| {
        let ws = AppState::global(cx).workspaces.clone();
        ws.update(cx, |w, cx| w.new_query_tab_with(key, String::new(), cx));
    });
    draw(vcx);
    active_console(vcx)
}

/// Runs `sql` in `console` (guard rails included) and waits for it.
fn run(vcx: &mut VisualTestContext, console: &Entity<Console>, sql: &str) {
    let sql = sql.to_owned();
    vcx.update(|_, cx| console.update(cx, |c, cx| c.run(sql, cx)));
    draw(vcx);
    wait_until(vcx, "the statement", |vcx| {
        vcx.update(|_, cx| console.read(cx).running.is_none())
    });
    draw(vcx);
}

/// Rows in `schema.t`, from the live session (so its open transaction counts).
fn count(vcx: &mut VisualTestContext, key: &DbKey, schema: &str) -> String {
    let backend = vcx.update(|_, cx| AppState::global(cx).backend.clone());
    let sql = format!("select count(*)::text from boa_txg.{schema}");
    let r = resolve(
        vcx,
        backend.run_query(&key.connection_id, &key.database, &sql, 10, 0),
    )
    .expect("count");
    format!("{:?}", r.rows[0][0])
}

fn wait_state(vcx: &mut VisualTestContext, key: &DbKey, state: TxState) {
    wait_until(vcx, "the tx state", |vcx| session(vcx, key).state == state);
}

#[gpui::test]
fn live_manual_mode_commit_and_rollback(cx: &mut TestAppContext) {
    let schema = "manual";
    let Some((_dir, mut vcx, key)) = live_window(cx, schema) else {
        return;
    };
    let console = live_console(&mut vcx, &key);
    let switch = vcx.update(|_, cx| tx_controls::switch_mode(key.clone(), TxMode::Manual, cx));
    resolve(&mut vcx, switch);
    assert_eq!(
        session(&mut vcx, &key),
        TxSession {
            mode: TxMode::Manual,
            state: TxState::Idle
        }
    );

    // Manual: the insert opens a transaction (toolbar + status bar), ⌘⇧↵ commits it.
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (1)"),
    );
    assert_eq!(session(&mut vcx, &key).state, TxState::Active);
    assert!(vcx.update(|_, cx| tx_controls::tx_status(cx).is_some()));
    vcx.simulate_keystrokes("cmd-shift-enter");
    draw(&mut vcx);
    wait_state(&mut vcx, &key, TxState::Idle);
    let logged = vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .read(cx)
            .output(&key.connection_id)[0]
            .entry
            .sql
            .clone()
    });
    assert_eq!(logged, "COMMIT");
    assert_eq!(session(&mut vcx, &key).mode, TxMode::Manual, "Manual stays");

    // ⌘⇧⌫ rolls the next one back.
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (2)"),
    );
    assert_eq!(session(&mut vcx, &key).state, TxState::Active);
    vcx.simulate_keystrokes("cmd-shift-backspace");
    draw(&mut vcx);
    wait_state(&mut vcx, &key, TxState::Idle);
    // Counting opens another transaction in Manual: switch back first.
    let switch = vcx.update(|_, cx| tx_controls::switch_mode(key.clone(), TxMode::Auto, cx));
    resolve(&mut vcx, switch);
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("1")"#);
    empty_table(&mut vcx, &key, schema);
}

#[gpui::test]
fn live_aborted_transaction_and_leaving_manual_mode(cx: &mut TestAppContext) {
    let schema = "aborted";
    let Some((_dir, mut vcx, key)) = live_window(cx, schema) else {
        return;
    };
    let console = live_console(&mut vcx, &key);
    let switch = vcx.update(|_, cx| tx_controls::switch_mode(key.clone(), TxMode::Manual, cx));
    resolve(&mut vcx, switch);
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (1)"),
    );
    run(&mut vcx, &console, "select 1/0");
    assert_eq!(
        session(&mut vcx, &key).state,
        TxState::Failed,
        "25P02 until rolled back"
    );
    // No commit of an aborted transaction.
    vcx.simulate_keystrokes("cmd-shift-enter");
    draw(&mut vcx);
    assert!(!vcx.update(|_, cx| transactions_busy(cx, &key)));
    assert_eq!(session(&mut vcx, &key).state, TxState::Failed);

    // Leaving Manual asks; an aborted one can only be rolled back.
    let switch = vcx.update(|_, cx| tx_controls::switch_mode(key.clone(), TxMode::Auto, cx));
    draw(&mut vcx);
    assert_eq!(guard_title(&mut vcx), Some("Switch to auto-commit?"));
    answer(&mut vcx, GuardAnswer::Rollback);
    resolve(&mut vcx, switch);
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("0")"#);

    // Leaving Manual with an open one: Commit keeps the row.
    let switch = vcx.update(|_, cx| tx_controls::switch_mode(key.clone(), TxMode::Manual, cx));
    resolve(&mut vcx, switch);
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (2)"),
    );
    let switch = vcx.update(|_, cx| tx_controls::switch_mode(key.clone(), TxMode::Auto, cx));
    draw(&mut vcx);
    answer(&mut vcx, GuardAnswer::Commit);
    resolve(&mut vcx, switch);
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("1")"#);
    empty_table(&mut vcx, &key, schema);
}

fn transactions_busy(cx: &gpui::App, key: &DbKey) -> bool {
    AppState::global(cx).transactions.read(cx).is_busy(key)
}

#[gpui::test]
fn live_typed_begin_commit_and_rollback(cx: &mut TestAppContext) {
    let schema = "typed";
    let Some((_dir, mut vcx, key)) = live_window(cx, schema) else {
        return;
    };
    let console = live_console(&mut vcx, &key);
    run(&mut vcx, &console, "begin");
    assert_eq!(
        session(&mut vcx, &key),
        TxSession {
            mode: TxMode::Auto,
            state: TxState::Active
        }
    );
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (1)"),
    );
    run(&mut vcx, &console, "rollback");
    assert_eq!(session(&mut vcx, &key).state, TxState::Idle);
    run(&mut vcx, &console, "BEGIN");
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (2)"),
    );
    run(&mut vcx, &console, "commit");
    assert_eq!(session(&mut vcx, &key).state, TxState::Idle);
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("1")"#);
    empty_table(&mut vcx, &key, schema);
}

#[gpui::test]
fn live_destructive_confirmation(cx: &mut TestAppContext) {
    let schema = "confirm";
    let Some((_dir, mut vcx, key)) = live_window(cx, schema) else {
        return;
    };
    let console = live_console(&mut vcx, &key);
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (1), (2), (3)"),
    );
    let delete = format!("delete from boa_txg.{schema}");

    // Setting on: asks; Run runs it.
    vcx.update(|_, cx| console.update(cx, |c, cx| c.run(delete.clone(), cx)));
    draw(&mut vcx);
    assert_eq!(guard_title(&mut vcx), Some("Run a destructive statement?"));
    answer(&mut vcx, GuardAnswer::Confirm);
    wait_until(&mut vcx, "the delete", |vcx| {
        vcx.update(|_, cx| console.read(cx).running.is_none())
    });
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("0")"#);

    // Off: runs without asking.
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (1)"),
    );
    set_confirm_destructive(&mut vcx, false);
    run(&mut vcx, &console, &delete);
    assert!(guard(&mut vcx).is_none());
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("0")"#);

    // Production: always asks; cancelled, nothing deleted.
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (1)"),
    );
    set_environment(&mut vcx, "live", Some(Environment::Production));
    vcx.update(|_, cx| console.update(cx, |c, cx| c.run(delete.clone(), cx)));
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Run a destructive statement on production?")
    );
    answer(&mut vcx, GuardAnswer::Cancel);
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("1")"#);
    empty_table(&mut vcx, &key, schema);
}

#[gpui::test]
fn live_read_only_connections_refuse_writes(cx: &mut TestAppContext) {
    let Some((_dir, mut vcx, _key)) = live_window(cx, "ro") else {
        return;
    };
    let ro = live_connection(&mut vcx, "ro", "boa_ro", true);
    let console = live_console(&mut vcx, &ro);
    // The server refuses the write.
    run(
        &mut vcx,
        &console,
        "create table public.boa_txg_ro_probe (a int)",
    );
    let failed = vcx
        .update(|_, cx| console.read(cx).failed.clone())
        .expect("refused");
    assert!(
        failed.message.contains("read-only transaction"),
        "{}",
        failed.message
    );
    // Talking the session out of read-only isn't even sent.
    run(
        &mut vcx,
        &console,
        "set default_transaction_read_only = off",
    );
    let failed = vcx
        .update(|_, cx| console.read(cx).failed.clone())
        .expect("not run");
    assert_eq!(
        failed.message,
        "Not run: The read-only setting cannot be changed on a read-only connection"
    );
    let key = DbKey {
        connection_id: "live".into(),
        database: ro.database.clone(),
    };
    empty_table(&mut vcx, &key, "ro");
}

#[gpui::test]
fn live_disconnect_with_an_open_transaction_asks_and_rolls_back(cx: &mut TestAppContext) {
    let schema = "disconnect";
    let Some((_dir, mut vcx, key)) = live_window(cx, schema) else {
        return;
    };
    let console = live_console(&mut vcx, &key);
    run(&mut vcx, &console, "begin");
    run(
        &mut vcx,
        &console,
        &format!("insert into boa_txg.{schema} values (1)"),
    );
    let conns = vcx.update(|_, cx| AppState::global(cx).connections.clone());
    let task = vcx.update(|_, cx| conns.update(cx, |c, cx| c.disconnect_task("live", cx)));
    draw(&mut vcx);
    assert_eq!(
        guard_title(&mut vcx),
        Some("Disconnect with an open transaction?")
    );
    answer(&mut vcx, GuardAnswer::Cancel);
    assert!(!resolve(&mut vcx, task));
    assert_eq!(session(&mut vcx, &key).state, TxState::Active);

    let task = vcx.update(|_, cx| conns.update(cx, |c, cx| c.disconnect_task("live", cx)));
    draw(&mut vcx);
    answer(&mut vcx, GuardAnswer::Confirm);
    assert!(resolve(&mut vcx, task));
    assert_eq!(session(&mut vcx, &key), IDLE_SESSION);
    // Reconnected: the insert was rolled back.
    let backend = vcx.update(|_, cx| AppState::global(cx).backend.clone());
    resolve(&mut vcx, backend.connect("live", None)).expect("reconnect");
    assert_eq!(count(&mut vcx, &key, schema), r#"Some("0")"#);
    empty_table(&mut vcx, &key, schema);
}
