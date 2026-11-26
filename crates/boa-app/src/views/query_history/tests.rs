//! Query History view and the right panel's History tab, in the main window
//! through the real keymap (#[gpui::test]), on a temp config dir with
//! seeded `history-<id>.jsonl` files.

use std::path::Path;
use std::time::{Duration, Instant};

use boa_core::history::Entry;
use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};

use super::*;
use crate::models::prefs::RightTab;
use crate::tests::{FAKE_CONNECTION, TempDir, fake_connection, init, open_main};
use crate::views::root::RootView;

#[test]
fn page_step_moves_a_page_less_one() {
    assert_eq!(page_step(480.), 6);
    assert_eq!(page_step(600.), 8);
    assert_eq!(page_step(64.), 1);
    assert_eq!(page_step(10.), 1);
    assert_eq!(moved(0, 10, -1), 0);
    assert_eq!(moved(3, 10, 6), 9);
    assert_eq!(moved(9, 10, 1), 9);
    assert_eq!(moved(42, 10, -1), 8, "a stale selection is clamped first");
    assert_eq!(moved(0, 0, 1), 0);
}

#[test]
fn header_and_empty_texts_match_the_source() {
    assert_eq!(count_label(1234, 1234), "1,234");
    assert_eq!(count_label(12, 1234), "12 of 1,234");
    assert_eq!(empty_message(true, 0, true), "Loading…");
    assert_eq!(
        empty_message(false, 0, true),
        "No statements yet. Statements you run are saved here."
    );
    assert_eq!(
        empty_message(false, 0, false),
        "No statements saved. Recording is off (Settings → Queries)."
    );
    assert_eq!(empty_message(false, 3, true), "No statements match.");
    let e = Entry {
        id: "1".into(),
        sql: "select 1".into(),
        key: None,
        database: "app".into(),
        at: 0.,
        ms: 1500.,
        rows: Some(12.),
        truncated: Some(true),
        affected: None,
        source: None,
        error: None,
        detail: None,
    };
    assert_eq!(
        meta_line(&e, Some("local"), 120_000.),
        "local · app · 2m · 1.5 s · 12+ rows"
    );
    assert_eq!(meta_line(&e, None, 1000.), "app · now · 1.5 s · 12+ rows");
}

const DAY: f64 = 86_400_000.;

fn stored(i: usize, sql: &str, error: Option<&str>) -> Entry {
    Entry {
        id: format!("e{i}"),
        sql: sql.into(),
        key: Some(boa_core::state::history::query_key(sql)),
        database: "boa_dev".into(),
        // Oldest first on disk: a larger i is newer.
        at: now_ms() - DAY + i as f64 * 1000.,
        ms: 3.,
        rows: error.is_none().then_some(1.),
        truncated: None,
        affected: None,
        source: Some("console_1".into()),
        error: error.map(str::to_owned),
        detail: None,
    }
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

/// 30 statements on the fake connection: `select * from t29` newest; every
/// fifth failed.
fn thirty() -> Vec<Entry> {
    (0..30)
        .map(|i| {
            let error = (i % 5 == 0).then_some("[42P01] relation does not exist");
            stored(i, &format!("select * from t{i}"), error)
        })
        .collect()
}

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
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn overlay(
    root: &Entity<RootView>,
    vcx: &mut VisualTestContext,
) -> Option<Entity<QueryHistoryView>> {
    vcx.update(|_, cx| root.read(cx).query_history().cloned())
}

/// Opens the window with a seeded history and ⌘⌥E, waits for the list.
fn open(
    cx: &mut TestAppContext,
    name: &str,
    entries: &[Entry],
) -> (
    TempDir,
    Entity<RootView>,
    VisualTestContext,
    Entity<QueryHistoryView>,
) {
    let dir = TempDir::new(name);
    seed(dir.path(), FAKE_CONNECTION, entries);
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    vcx.simulate_keystrokes("cmd-alt-e");
    let view = overlay(&root, &mut vcx).expect("⌘⌥E opens query history");
    let n = entries.len();
    wait_until(&mut vcx, "the list", |vcx| {
        vcx.update(|_, cx| view.read(cx).shown().len() == n)
    });
    (dir, root, vcx, view)
}

fn selected_sql(view: &Entity<QueryHistoryView>, vcx: &mut VisualTestContext) -> String {
    vcx.update(|_, cx| view.read(cx).current().unwrap().entry.sql.clone())
}

fn editor_text(root: &Entity<RootView>, vcx: &mut VisualTestContext) -> String {
    vcx.update(|_, cx| root.read(cx).editor_text(cx))
}

#[gpui::test]
fn cmd_alt_e_toggles_it_and_escape_closes_it(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, view) = open(cx, "qh-toggle", &thirty());
    let focused = vcx.update(|window, cx| {
        view.read(cx)
            .input()
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    });
    assert!(focused, "the search field takes the focus");
    // The active connection's history, newest first.
    assert_eq!(selected_sql(&view, &mut vcx), "select * from t29");
    assert_eq!(
        vcx.update(|_, cx| view.read(cx).scope()),
        HistoryScope::Connection
    );
    vcx.simulate_keystrokes("cmd-alt-e");
    assert!(overlay(&root, &mut vcx).is_none(), "⌘⌥E again closes it");
    vcx.simulate_keystrokes("cmd-alt-e");
    assert!(overlay(&root, &mut vcx).is_some());
    vcx.simulate_keystrokes("escape");
    assert!(overlay(&root, &mut vcx).is_none(), "Esc closes it");
    let editor_focused = vcx.update(|window, cx| {
        root.read(cx)
            .workspace()
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    });
    assert!(editor_focused, "the editor gets the focus back");
    // ⌘⌥E over Settings closes Settings first.
    vcx.simulate_keystrokes("cmd-,");
    vcx.simulate_keystrokes("cmd-alt-e");
    assert!(overlay(&root, &mut vcx).is_some());
    assert!(vcx.update(|_, cx| AppState::global(cx).layout.read(cx).settings().is_none()));
}

#[gpui::test]
fn arrows_and_pages_move_the_selection(cx: &mut TestAppContext) {
    let (_dir, _root, mut vcx, view) = open(cx, "qh-keys", &thirty());
    let sel = |vcx: &mut VisualTestContext| vcx.update(|_, cx| view.read(cx).selected_index());
    vcx.simulate_keystrokes("down down");
    assert_eq!(sel(&mut vcx), 2);
    assert_eq!(selected_sql(&view, &mut vcx), "select * from t27");
    vcx.simulate_keystrokes("up");
    assert_eq!(sel(&mut vcx), 1);
    vcx.simulate_keystrokes("up up up");
    assert_eq!(sel(&mut vcx), 0, "stops at the top");
    // A page: the list's height in rows, less one (680 px dialog at 820 px
    // high: min(680, 689) − 79 = 601 px → 9 rows → 8).
    vcx.simulate_keystrokes("pagedown");
    assert_eq!(sel(&mut vcx), 8);
    vcx.simulate_keystrokes("pagedown pagedown pagedown pagedown");
    assert_eq!(sel(&mut vcx), 29, "stops at the bottom");
    vcx.simulate_keystrokes("pageup");
    assert_eq!(sel(&mut vcx), 21);
    // Typing filters (every word, any order) and goes back to the top.
    vcx.simulate_input("t1");
    vcx.run_until_parked();
    let shown: Vec<String> = vcx.update(|_, cx| {
        view.read(cx)
            .shown()
            .iter()
            .map(|i| i.entry.sql.clone())
            .collect()
    });
    assert_eq!(shown.len(), 11, "t1 and t10–t19: {shown:?}");
    assert_eq!(sel(&mut vcx), 0);
    assert_eq!(vcx.update(|_, cx| view.read(cx).total()), 30);
    // Errors only.
    view.update(&mut vcx, |v, cx| v.toggle_errors_only(cx));
    let shown: Vec<String> = vcx.update(|_, cx| {
        view.read(cx)
            .shown()
            .iter()
            .map(|i| i.entry.sql.clone())
            .collect()
    });
    assert_eq!(shown, ["select * from t15", "select * from t10"]);
}

#[gpui::test]
fn enter_inserts_at_the_consoles_caret(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, _view) = open(cx, "qh-insert", &thirty());
    let before = editor_text(&root, &mut vcx);
    vcx.simulate_keystrokes("down");
    vcx.simulate_keystrokes("enter");
    assert!(overlay(&root, &mut vcx).is_none(), "⏎ closes it");
    let after = editor_text(&root, &mut vcx);
    // The caret was at the start of a line holding text: the statement goes
    // on its own line above.
    assert_eq!(after, format!("select * from t28\n{before}"));
    // The console's SQL follows the editor.
    let sql = vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .read(cx)
            .active_tab()
            .unwrap()
            .console()
            .unwrap()
            .read(cx)
            .sql
            .clone()
    });
    assert_eq!(sql, after);
}

#[gpui::test]
fn cmd_enter_opens_a_new_console_and_cmd_c_copies(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, _view) = open(cx, "qh-open", &thirty());
    vcx.simulate_keystrokes("cmd-c");
    assert_eq!(
        vcx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("select * from t29")
    );
    let toast = vcx.update(|_, cx| {
        AppState::global(cx)
            .toasts
            .read(cx)
            .toasts()
            .last()
            .map(|t| t.title.to_string())
    });
    assert_eq!(toast.as_deref(), Some("Statement copied"));
    vcx.simulate_keystrokes("down cmd-enter");
    assert!(overlay(&root, &mut vcx).is_none());
    let (title, sql) = vcx.update(|_, cx| {
        let ws = AppState::global(cx).workspaces.read(cx);
        let tab = ws.active_tab().unwrap();
        (
            tab.label().to_string(),
            tab.console().unwrap().read(cx).sql.clone(),
        )
    });
    assert_eq!(title, "console_2");
    assert_eq!(sql, "select * from t28");
}

#[gpui::test]
fn cmd_backspace_deletes_the_selected_entry(cx: &mut TestAppContext) {
    let (dir, _root, mut vcx, view) = open(cx, "qh-delete", &thirty());
    vcx.simulate_keystrokes("down cmd-backspace");
    let (len, sel) = vcx.update(|_, cx| {
        let v = view.read(cx);
        (v.shown().len(), v.selected_index())
    });
    assert_eq!(len, 29);
    assert_eq!(sel, 1, "the selection stays in place");
    assert_eq!(selected_sql(&view, &mut vcx), "select * from t27");
    let path = dir.path().join("history-fake.jsonl");
    wait_until(&mut vcx, "deleted on disk", |_| {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        text.lines().count() == 29 && !text.contains("from t28\"")
    });
}

#[gpui::test]
fn all_connections_merge_newest_first(cx: &mut TestAppContext) {
    let dir = TempDir::new("qh-all");
    seed(
        dir.path(),
        FAKE_CONNECTION,
        &[
            stored(1, "select * from a", None),
            stored(3, "select * from c", None),
        ],
    );
    seed(dir.path(), "other", &[stored(2, "select * from b", None)]);
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    vcx.update(|_, cx| {
        fake_connection(cx, "other", &["boa_dev"]);
    });
    vcx.simulate_keystrokes("cmd-alt-e");
    let view = overlay(&root, &mut vcx).unwrap();
    wait_until(&mut vcx, "the connection's list", |vcx| {
        vcx.update(|_, cx| view.read(cx).shown().len() == 2)
    });
    view.update(&mut vcx, |v, cx| v.set_scope(HistoryScope::All, cx));
    wait_until(&mut vcx, "both lists", |vcx| {
        vcx.update(|_, cx| view.read(cx).shown().len() == 3)
    });
    let shown: Vec<(String, String)> = vcx.update(|_, cx| {
        view.read(cx)
            .shown()
            .iter()
            .map(|i| (i.connection_id.clone(), i.entry.sql.clone()))
            .collect()
    });
    assert_eq!(
        shown,
        [
            (FAKE_CONNECTION.into(), "select * from c".into()),
            ("other".into(), "select * from b".into()),
            (FAKE_CONNECTION.into(), "select * from a".into()),
        ]
    );
}

#[gpui::test]
fn the_right_panels_history_tab_searches_and_escape_clears(cx: &mut TestAppContext) {
    let dir = TempDir::new("qh-panel");
    seed(dir.path(), FAKE_CONNECTION, &thirty());
    init(cx, dir.path());
    cx.update(|cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| p.right_tab = RightTab::History)
        });
    });
    let (root, mut vcx) = open_main(cx);
    let panel = vcx
        .update(|_, cx| {
            root.read(cx)
                .right_panel()
                .read(cx)
                .history_panel()
                .cloned()
        })
        .expect("the History tab shows");
    wait_until(&mut vcx, "the panel's list", |vcx| {
        vcx.update(|_, cx| panel.read(cx).shown().len() == 30)
    });
    let input = vcx.update(|_, cx| panel.read(cx).input().clone());
    vcx.update(|window, cx| input.update(cx, |i, cx| i.focus(window, cx)));
    vcx.simulate_input("t2");
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| panel.read(cx).shown().len()), 11);
    vcx.simulate_keystrokes("escape");
    assert_eq!(vcx.update(|_, cx| panel.read(cx).query().to_owned()), "");
    assert_eq!(vcx.update(|_, cx| panel.read(cx).shown().len()), 30);
    // New runs show up at the top.
    vcx.update(|_, cx| {
        let ws = AppState::global(cx).workspaces.clone();
        ws.update(cx, |w, cx| {
            w.log(
                FAKE_CONNECTION,
                boa_core::state::history::OutputEntry {
                    sql: "select 'fresh' from z".into(),
                    database: "boa_dev".into(),
                    at: now_ms(),
                    ..Default::default()
                },
                None,
                true,
                cx,
            )
        });
    });
    vcx.run_until_parked();
    assert_eq!(
        vcx.update(|_, cx| panel.read(cx).shown()[0].sql.clone()),
        "select 'fresh' from z"
    );
}
