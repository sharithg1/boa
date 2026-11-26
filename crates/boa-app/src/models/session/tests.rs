//! Session model tests: save / restore through the real app init on a temp
//! config dir, the debounce, pruning, reconnecting at launch (on a scripted
//! [`ConnApi`], time driven by the test executor; the store-level cases of
//! session.ts `connectionsToRestore` / `startSession`), the one-time
//! localStorage import, and Tauri compatibility of the written JSON.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use boa_core::connections::{self as store, ConnConfig, SavedConnection, SslMode};
use boa_core::error::CmdError;
use boa_core::pg::{ConnectInfo, PingInfo};
use boa_core::state::local_storage::IMPORT_MARKER;
use boa_core::state::reconnect::RETRY_DELAYS_MS;
use boa_core::state::session::{self as core, db_key, parse_session, table_key};
use gpui::{Entity, TestAppContext};

use super::*;
use crate::models::connections::{ApiFuture, ConnApi, ConnState};
use crate::models::layout::PanelKind;
use crate::models::workspaces::{TabKind, Workspaces};
use crate::tests::{TempDir, init};

// ── A scripted backend ────────────────────────────────────────────────────

#[derive(Default)]
struct Script {
    /// Connect outcomes per connection id (then `connect_default`).
    connects: HashMap<String, VecDeque<Result<ConnectInfo, CmdError>>>,
    /// None: connects never finish.
    connect_default: Option<Result<ConnectInfo, CmdError>>,
    /// Connect calls, in order.
    connect_calls: Vec<String>,
}

#[derive(Default)]
struct FakeApi(RefCell<Script>);

fn info() -> ConnectInfo {
    ConnectInfo {
        server_version: "PostgreSQL 17".into(),
        databases: vec!["app".into()],
    }
}

fn unreachable() -> CmdError {
    CmdError::code("unreachable", "Connection refused")
}

fn wrong_password() -> CmdError {
    CmdError::code("28P01", "password authentication failed")
}

impl FakeApi {
    fn script(&self) -> std::cell::RefMut<'_, Script> {
        self.0.borrow_mut()
    }
    fn calls(&self) -> Vec<String> {
        self.0.borrow().connect_calls.clone()
    }
    fn calls_for(&self, id: &str) -> usize {
        self.0
            .borrow()
            .connect_calls
            .iter()
            .filter(|c| *c == id)
            .count()
    }
}

impl ConnApi for FakeApi {
    fn list(&self) -> ApiFuture<Vec<SavedConnection>> {
        Box::pin(std::future::pending())
    }
    fn save(&self, _: ConnConfig, _: Option<String>, _: bool) -> ApiFuture<()> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn delete(&self, _: &str) -> ApiFuture<()> {
        Box::pin(std::future::ready(Ok(())))
    }
    fn test(&self, _: ConnConfig, _: Option<String>) -> ApiFuture<String> {
        Box::pin(std::future::ready(Ok("PostgreSQL 17".to_owned())))
    }
    fn connect(&self, id: &str, _: Option<String>) -> ApiFuture<ConnectInfo> {
        let mut s = self.script();
        s.connect_calls.push(id.to_owned());
        let next = s.connects.get_mut(id).and_then(VecDeque::pop_front);
        match next.or_else(|| s.connect_default.clone()) {
            Some(r) => Box::pin(std::future::ready(r)),
            None => Box::pin(std::future::pending()),
        }
    }
    fn ping(&self, _: &str) -> ApiFuture<PingInfo> {
        // A new launch: the backend has nothing open.
        Box::pin(std::future::ready(Err(CmdError::code(
            "not_connected",
            "Not connected",
        ))))
    }
    fn disconnect(&self, _: &str) -> ApiFuture<()> {
        Box::pin(std::future::ready(Ok(())))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Saves connections `ids` in `dir`'s connections.json (database "app").
fn save_connections(dir: &Path, ids: &[&str]) {
    for id in ids {
        let config = ConnConfig {
            id: (*id).into(),
            name: format!("{id} server"),
            host: "localhost".into(),
            port: 5432,
            user: "me".into(),
            database: "app".into(),
            ssl_mode: SslMode::Prefer,
            read_only: false,
            environment: None,
            color: None,
            ssh: None,
        };
        store::upsert(dir, config, Some("pw".into()), true).unwrap();
    }
}

/// Puts the scripted backend under Connections, and keeps Catalogs from
/// introspecting (no server behind these connections).
fn script_backend(cx: &mut TestAppContext) -> Rc<FakeApi> {
    let api = Rc::new(FakeApi::default());
    cx.update(|cx| {
        let state = AppState::global(cx).clone();
        let ids: Vec<String> = state
            .connections
            .read(cx)
            .saved
            .iter()
            .map(|c| c.config.id.clone())
            .collect();
        state.catalogs.update(cx, |c, _| c.offline.extend(ids));
        let api: Rc<dyn ConnApi> = api.clone();
        state.connections.update(cx, |c, _| c.set_api(api));
    });
    api
}

fn state(cx: &mut TestAppContext) -> AppState {
    cx.update(|cx| AppState::global(cx).clone())
}

fn session_json(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(SESSION_FILE)).ok()
}

fn advance(cx: &mut TestAppContext, d: Duration) {
    cx.executor().advance_clock(d);
    cx.run_until_parked();
}

fn conn_state(cx: &mut TestAppContext, id: &str) -> &'static str {
    let s = state(cx);
    cx.read(|cx| s.connections.read(cx).state_of(id).as_str())
}

fn tab_labels(ws: &Entity<Workspaces>, key: &DbKey, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        ws.read(cx)
            .workspace(key)
            .map(|w| w.tabs.iter().map(|t| t.label().to_string()).collect())
            .unwrap_or_default()
    })
}

/// Starts the app on `dir` with `raw` as its session.json.
fn launch_with(cx: &mut TestAppContext, dir: &Path, raw: &str) -> Rc<FakeApi> {
    std::fs::write(dir.join(SESSION_FILE), raw).unwrap();
    init(cx, dir);
    script_backend(cx)
}

// ── Save and restore ──────────────────────────────────────────────────────

#[gpui::test]
fn the_session_is_saved_and_restored_through_the_app(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-roundtrip");
    save_connections(dir.path(), &["c1"]);
    init(cx, dir.path());
    let _api = script_backend(cx); // connects never finish: nothing is run
    let s = state(cx);
    let key = DbKey::new("c1", "app");
    cx.update(|cx| {
        s.workspaces.update(cx, |w, cx| {
            w.new_query_tab_with(key.clone(), "select 42".into(), cx);
            w.open_table(key.clone(), "public", "orders", false, cx);
            w.new_query_tab_with(key.clone(), "select 'second'".into(), cx);
        });
        let table = s
            .workspaces
            .read(cx)
            .workspace(&key)
            .unwrap()
            .tabs
            .iter()
            .find_map(|t| t.table().cloned())
            .unwrap();
        table.update(cx, |t, cx| {
            t.where_clause = "id > 5".into();
            t.order_by = "id desc".into();
            cx.notify();
        });
        let table_id = table.read(cx).tab_id.clone();
        s.workspaces
            .update(cx, |w, cx| w.activate(&key, &table_id, cx));
        s.layout
            .update(cx, |l, cx| l.set_open(PanelKind::Bottom, false, cx));
    });
    cx.run_until_parked();
    assert_eq!(session_json(dir.path()), None, "not before the delay");
    advance(cx, SAVE_DELAY);
    let raw = session_json(dir.path()).expect("saved");

    // The Tauri rules read it back as written.
    let parsed = parse_session(Some(&raw), ["c1"]).unwrap();
    let ws = &parsed.workspaces[&db_key("c1", "app")];
    assert_eq!(ws.active_tab.as_deref(), Some("t2"));
    assert_eq!(
        ws.tabs,
        vec![
            core::Tab::Query(core::QueryTab {
                id: "q1".into(),
                title: "console_1".into(),
                sql: "select 42".into()
            }),
            core::Tab::Table(core::TableTab {
                id: "t2".into(),
                schema: "public".into(),
                table: "orders".into(),
                preview: false,
                r#where: Some("id > 5".into()),
                order_by: Some("id desc".into()),
            }),
            core::Tab::Query(core::QueryTab {
                id: "q3".into(),
                title: "console_2".into(),
                sql: "select 'second'".into()
            }),
        ]
    );
    assert_eq!(parsed.mru, [table_key("c1", "app", "public", "orders")]);
    assert_eq!(parsed.bottom_open, Some(false));
    assert_eq!(parsed.sidebar_open, Some(true));
    assert!(
        raw.starts_with(r#"{"active":{"connectionId":"c1","database":"app"},"workspaces":{"c1\u0000app":{"tabs":[{"id":"q1","kind":"query","title":"console_1","sql":"select 42"},"#),
        "{raw}"
    );

    // A new launch on the same dir: the same tabs, SQL, filter, active tab
    // and panels; nothing runs; new ids continue after the restored ones.
    let mut cx2 = TestAppContext::single();
    init(&mut cx2, dir.path());
    let s2 = state(&mut cx2);
    assert_eq!(
        tab_labels(&s2.workspaces, &key, &mut cx2),
        ["console_1", "orders", "console_2"]
    );
    cx2.update(|cx| {
        let w = s2.workspaces.read(cx);
        assert_eq!(w.active.as_ref(), Some(&key));
        let tab = w.active_tab().unwrap();
        assert_eq!(tab.label().as_ref(), "orders");
        let t = tab.table().unwrap().read(cx);
        assert_eq!(
            (t.where_clause.as_str(), t.order_by.as_str()),
            ("id > 5", "id desc")
        );
        assert!(t.result.is_none() && !t.loading);
        let first = w.workspace(&key).unwrap().tabs[0]
            .console()
            .unwrap()
            .read(cx);
        assert_eq!(first.sql, "select 42");
        assert!(first.result.is_none() && first.running.is_none());
        assert!(!s2.layout.read(cx).bottom_open());
        assert!(s2.layout.read(cx).sidebar_open());
    });
    let _api2 = script_backend(&mut cx2);
    cx2.update(|cx| {
        s2.workspaces.update(cx, |w, cx| {
            w.open_table(key.clone(), "public", "x", true, cx)
        })
    });
    let id = cx2.read(|cx| s2.workspaces.read(cx).active_tab().unwrap().id.to_string());
    assert_eq!(id, "t4");
}

#[gpui::test]
fn saves_are_debounced_and_skip_unchanged_sessions(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-debounce");
    init(cx, dir.path());
    let s = state(cx);
    let toggle = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            s.layout
                .update(cx, |l, cx| l.toggle(PanelKind::Sidebar, cx))
        })
    };
    toggle(cx);
    advance(cx, Duration::from_millis(200));
    assert_eq!(session_json(dir.path()), None);
    // Another change restarts the delay (session.ts clearTimeout).
    toggle(cx);
    advance(cx, Duration::from_millis(200));
    assert_eq!(session_json(dir.path()), None);
    advance(cx, Duration::from_millis(60));
    let first = session_json(dir.path()).expect("written 250 ms after the last change");
    assert!(first.contains(r#""sidebarOpen":true"#), "{first}");

    // Nothing changed: not rewritten.
    std::fs::remove_file(dir.path().join(SESSION_FILE)).unwrap();
    cx.update(|cx| s.session.update(cx, |s, cx| s.touch(cx)));
    advance(cx, SAVE_DELAY * 2);
    assert_eq!(session_json(dir.path()), None);

    // Changes that never stop for 250 ms still get saved (≈ 2 s).
    toggle(cx);
    for _ in 0..11 {
        cx.update(|cx| s.session.update(cx, |s, cx| s.touch(cx)));
        advance(cx, Duration::from_millis(200));
    }
    let raw = session_json(dir.path()).expect("saved within MAX_SAVE_DELAY + SAVE_DELAY");
    assert!(raw.contains(r#""sidebarOpen":false"#), "{raw}");
}

#[gpui::test]
fn a_pending_save_is_written_on_quit(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-quit");
    init(cx, dir.path());
    let s = state(cx);
    cx.update(|cx| {
        s.layout
            .update(cx, |l, cx| l.set_open(PanelKind::RightPanel, false, cx))
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| s.session.read(cx).save_pending()));
    assert_eq!(session_json(dir.path()), None);
    cx.quit();
    let raw = session_json(dir.path()).expect("flushed on quit");
    assert!(raw.contains(r#""rightPanelOpen":false"#), "{raw}");
}

#[gpui::test]
fn restoring_prunes_what_belongs_to_deleted_connections(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-prune");
    save_connections(dir.path(), &["c1"]);
    let raw = serde_json::json!({
        "active": { "connectionId": "gone", "database": "app" },
        "workspaces": {
            db_key("c1", "app"): {
                "tabs": [{ "id": "q7", "kind": "query", "title": "console_1", "sql": "select 1" }],
                "activeTab": "q7"
            },
            db_key("gone", "app"): {
                "tabs": [{ "id": "q8", "kind": "query", "title": "console_1", "sql": "select 2" }],
                "activeTab": "q8"
            }
        },
        "mru": [table_key("gone", "app", "public", "t"), table_key("c1", "app", "public", "u")],
        "seen": { table_key("gone", "app", "public", "t"): 1 },
        "wanted": { "gone": true, "c1": true }
    })
    .to_string();
    let api = launch_with(cx, dir.path(), &raw);
    api.script().connect_default = Some(Ok(info()));
    let s = state(cx);
    cx.read(|cx| {
        let w = s.workspaces.read(cx);
        assert_eq!(w.active, None);
        assert!(w.workspace(&DbKey::new("gone", "app")).is_none());
        assert_eq!(w.workspace(&DbKey::new("c1", "app")).unwrap().tabs.len(), 1);
        assert_eq!(w.mru, [table_key("c1", "app", "public", "u")]);
        assert!(w.seen.is_empty());
    });
    // Only the connection that still exists is reopened.
    cx.update(|cx| s.session.update(cx, |s, cx| s.reconnect(cx)));
    cx.run_until_parked();
    assert_eq!(api.calls(), ["c1"]);
    // The next save no longer has the deleted connection.
    cx.update(|cx| s.session.update(cx, |s, cx| s.touch(cx)));
    advance(cx, SAVE_DELAY);
    let saved = session_json(dir.path()).unwrap();
    assert!(!saved.contains("gone"), "{saved}");
    assert!(saved.contains(r#""wanted":{"c1":true}"#), "{saved}");
}

// ── Reconnecting at launch (session.ts startSession / connectionsToRestore) ──

/// A saved session selecting `active`'s "app" database, with `wanted` in
/// the given order (None: a session saved before `wanted` was kept).
fn saved_session(active: Option<&str>, wanted: Option<&[&str]>) -> String {
    let active = active.map_or("null".to_owned(), |id| {
        format!(r#"{{"connectionId":"{id}","database":"app"}}"#)
    });
    let wanted = wanted.map_or(String::new(), |ids| {
        let body: Vec<String> = ids.iter().map(|id| format!(r#""{id}":true"#)).collect();
        format!(r#","wanted":{{{}}}"#, body.join(","))
    });
    format!(r#"{{"active":{active},"workspaces":{{}},"mru":[],"seen":{{}}{wanted}}}"#)
}

#[gpui::test]
fn reconnects_retrying_while_the_server_is_unreachable(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-retry");
    save_connections(dir.path(), &["c1"]);
    let api = launch_with(cx, dir.path(), &saved_session(Some("c1"), Some(&["c1"])));
    api.script().connects.insert(
        "c1".into(),
        VecDeque::from([
            Err(unreachable()),
            Err(unreachable()),
            Err(unreachable()),
            Err(unreachable()),
            Err(unreachable()),
            Ok(info()),
        ]),
    );
    let s = state(cx);
    cx.update(|cx| s.session.update(cx, |s, cx| s.reconnect(cx)));
    cx.run_until_parked();
    assert_eq!(api.calls_for("c1"), 1);
    assert_eq!(conn_state(cx, "c1"), "reconnecting");
    // Wanted at once: a quit now still reconnects it next time.
    assert!(cx.read(|cx| s.connections.read(cx).is_wanted("c1")));
    // Retries after 1 s, 2 s, 4 s, 8 s, 15 s: connected after ~30 s.
    for (i, delay) in RETRY_DELAYS_MS.iter().take(5).enumerate() {
        advance(cx, Duration::from_millis(delay - 1));
        assert_eq!(api.calls_for("c1"), i + 1, "not before {delay} ms");
        advance(cx, Duration::from_millis(1));
        assert_eq!(api.calls_for("c1"), i + 2, "after {delay} ms");
    }
    assert_eq!(RETRY_DELAYS_MS.iter().take(5).sum::<u64>(), 30_000);
    assert!(matches!(
        cx.read(|cx| s.connections.read(cx).state_of("c1")),
        ConnState::Connected { .. }
    ));
}

#[gpui::test]
fn gives_up_after_the_retries_and_keeps_the_connection_wanted(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-giveup");
    save_connections(dir.path(), &["c1"]);
    let api = launch_with(cx, dir.path(), &saved_session(Some("c1"), Some(&["c1"])));
    api.script().connect_default = Some(Err(unreachable()));
    let s = state(cx);
    cx.update(|cx| s.session.update(cx, |s, cx| s.reconnect(cx)));
    cx.run_until_parked();
    for delay in RETRY_DELAYS_MS {
        advance(cx, Duration::from_millis(delay));
    }
    assert_eq!(api.calls_for("c1"), RETRY_DELAYS_MS.len() + 1);
    assert_eq!(conn_state(cx, "c1"), "failed");
    // "Could not connect" (a launch, not "Lost connection"); still wanted.
    let titles: Vec<String> = cx.read(|cx| {
        s.toasts
            .read(cx)
            .toasts()
            .iter()
            .map(|t| t.title.to_string())
            .collect()
    });
    assert_eq!(titles, ["Could not connect to c1 server"]);
    advance(cx, SAVE_DELAY);
    let saved = session_json(dir.path()).unwrap();
    assert!(saved.contains(r#""wanted":{"c1":true}"#), "{saved}");
}

#[gpui::test]
fn reopens_the_selected_connection_first_then_the_others_quietly(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-order");
    save_connections(dir.path(), &["c1", "c2", "c3"]);
    let api = launch_with(
        cx,
        dir.path(),
        &saved_session(Some("c1"), Some(&["c3", "c2", "c1"])),
    );
    {
        let mut sc = api.script();
        // A stored password the server refuses (28P01) asks for another;
        // in the background it doesn't.
        sc.connects
            .insert("c1".into(), VecDeque::from([Err(wrong_password())]));
        sc.connects
            .insert("c2".into(), VecDeque::from([Err(wrong_password())]));
        sc.connect_default = Some(Ok(info()));
    }
    let s = state(cx);
    cx.update(|cx| s.session.update(cx, |s, cx| s.reconnect(cx)));
    cx.run_until_parked();
    assert_eq!(api.calls().first().map(String::as_str), Some("c1"));
    let mut calls = api.calls();
    calls.sort();
    assert_eq!(calls, ["c1", "c2", "c3"]);
    cx.read(|cx| {
        let c = s.connections.read(cx);
        // The selected one asks for its password; a background one doesn't
        // (and is no longer wanted).
        assert_eq!(
            c.password_prompt().map(|p| p.connection_id.as_str()),
            Some("c1")
        );
        assert!(c.is_wanted("c1"));
        assert!(!c.is_wanted("c2"));
        assert!(c.is_connected("c3"));
    });
}

#[gpui::test]
fn reopens_just_the_selected_one_for_sessions_saved_before_open_connections_were_kept(
    cx: &mut TestAppContext,
) {
    let dir = TempDir::new("session-old");
    save_connections(dir.path(), &["c1", "c2"]);
    let api = launch_with(cx, dir.path(), &saved_session(Some("c2"), None));
    api.script().connect_default = Some(Ok(info()));
    let s = state(cx);
    cx.update(|cx| s.session.update(cx, |s, cx| s.reconnect(cx)));
    cx.run_until_parked();
    assert_eq!(api.calls(), ["c2"]);
    assert_eq!(conn_state(cx, "c2"), "connected");
    assert_eq!(conn_state(cx, "c1"), "disconnected");
}

#[gpui::test]
fn reopens_the_open_ones_when_nothing_was_selected(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-noactive");
    save_connections(dir.path(), &["c1", "c2"]);
    let api = launch_with(cx, dir.path(), &saved_session(None, Some(&["c1"])));
    api.script().connect_default = Some(Ok(info()));
    let s = state(cx);
    cx.update(|cx| s.session.update(cx, |s, cx| s.reconnect(cx)));
    cx.run_until_parked();
    assert_eq!(api.calls(), ["c1"]);
    assert_eq!(conn_state(cx, "c1"), "connected");
}

#[gpui::test]
fn without_a_session_nothing_is_restored_or_reconnected(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-none");
    save_connections(dir.path(), &["c1"]);
    let api = launch_with(cx, dir.path(), "{nope");
    let s = state(cx);
    cx.update(|cx| s.session.update(cx, |s, cx| s.reconnect(cx)));
    cx.run_until_parked();
    assert!(api.calls().is_empty());
    assert!(cx.read(|cx| s.workspaces.read(cx).workspaces.is_empty()));
    // Defaults (1280 px: everything open).
    assert!(cx.read(|cx| s.layout.read(cx).right_panel_open()));
}

// ── Tauri compatibility ───────────────────────────────────────────────────

#[gpui::test]
fn the_file_is_what_the_tauri_app_writes(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-compat");
    save_connections(dir.path(), &["c1"]);
    // As the Tauri app saved it, plus a key it doesn't know.
    let raw = format!(
        r#"{{"active":{{"connectionId":"c1","database":"app"}},"workspaces":{{"{k}":{{"tabs":[{{"id":"t5","kind":"table","schema":"public","table":"users","preview":true,"where":"a = 1"}}],"activeTab":"t5"}}}},"mru":[],"seen":{{}},"wanted":{{}},"sidebarOpen":false,"rightPanelOpen":false,"bottomOpen":true,"extra":1}}"#,
        k = r"c1\u0000app"
    );
    let _api = launch_with(cx, dir.path(), &raw);
    let s = state(cx);
    let tab = cx.read(|cx| {
        let w = s.workspaces.read(cx);
        w.active_tab().map(|t| t.kind.clone())
    });
    assert_eq!(
        tab,
        Some(TabKind::Table {
            schema: "public".into(),
            table: "users".into(),
            preview: true
        })
    );
    assert!(!cx.read(|cx| s.layout.read(cx).sidebar_open()));
    cx.update(|cx| {
        s.layout
            .update(cx, |l, cx| l.set_open(PanelKind::Bottom, false, cx))
    });
    advance(cx, SAVE_DELAY);
    let saved = session_json(dir.path()).unwrap();
    // JSON.stringify(pick(state)): these keys in this order, nothing else.
    let keys = [
        r#""active":"#,
        r#""workspaces":"#,
        r#""mru":"#,
        r#""seen":"#,
        r#""wanted":"#,
        r#""sidebarOpen":"#,
        r#""rightPanelOpen":"#,
        r#""bottomOpen":"#,
    ];
    let at: Vec<usize> = keys.iter().map(|k| saved.find(k).expect(k)).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{saved}");
    assert!(!saved.contains("extra"), "{saved}");
    let v: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(v.as_object().unwrap().len(), keys.len());
    assert_eq!(v["wanted"], serde_json::json!({}));
    assert_eq!(v["bottomOpen"], serde_json::json!(false));
    // And the Tauri rules read it back to the same session.
    let parsed = parse_session(Some(&saved), ["c1"]).unwrap();
    let again = parse_session(Some(&raw), ["c1"]).unwrap();
    assert_eq!(parsed.workspaces, again.workspaces);
    assert_eq!(parsed.active, again.active);
    // Owner-only, like every config file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(SESSION_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[gpui::test]
fn a_scripted_layout_keeps_the_saved_panel_flags(cx: &mut TestAppContext) {
    let dir = TempDir::new("session-scripted");
    let raw = r#"{"active":null,"workspaces":{},"mru":[],"seen":{},"sidebarOpen":true}"#;
    let _api = launch_with(cx, dir.path(), raw);
    let s = state(cx);
    cx.update(|cx| {
        s.session.update(cx, |s, _| s.keep_saved_panels());
        s.layout.update(cx, |l, cx| {
            l.set_open(PanelKind::Sidebar, false, cx);
            l.set_open(PanelKind::Bottom, false, cx);
        });
    });
    advance(cx, SAVE_DELAY);
    let saved = session_json(dir.path()).unwrap();
    // The saved flag stays; one the file didn't have takes the layout's.
    assert!(saved.contains(r#""sidebarOpen":true"#), "{saved}");
    assert!(saved.contains(r#""bottomOpen":false"#), "{saved}");
}

// ── The one-time localStorage import ──────────────────────────────────────

#[gpui::test]
fn the_local_storage_import_runs_once(_cx: &mut TestAppContext) {
    let dir = TempDir::new("session-import");
    let home = TempDir::new("session-import-home");
    let report = import_local_storage(dir.path(), home.path()).expect("ran");
    assert!(report.imported.is_empty());
    assert!(report.source.is_none());
    assert!(dir.path().join(IMPORT_MARKER).exists());
    assert_eq!(import_local_storage(dir.path(), home.path()), None);
}

/// Migration check on a COPY of a real install (never the originals):
/// `BOA_MIGRATION_DIR=<dir>` holding `config/` (a copy of the config dir)
/// and `home/Library/WebKit/dev.boa.desktop` (a copy of WebKit's data).
/// Imports once, starts the app models on the copy (nothing connects) and
/// prints counts only (no SQL, no names).
/// `cargo test -p boa-app -- --ignored real_migration --nocapture`
#[gpui::test]
#[ignore]
fn real_migration_copy(cx: &mut TestAppContext) {
    let Some(root) = std::env::var_os("BOA_MIGRATION_DIR").map(std::path::PathBuf::from) else {
        println!("BOA_MIGRATION_DIR not set");
        return;
    };
    let (config, home) = (root.join("config"), root.join("home"));
    let report = import_local_storage(&config, &home);
    match &report {
        Some(r) => println!(
            "import: found localStorage: {}, imported {:?}, skipped {:?}",
            r.source.is_some(),
            r.imported,
            r.skipped
        ),
        None => println!("import: not run (marker, or prefs.json + session.json present)"),
    }
    init(cx, &config);
    let s = state(cx);
    cx.read(|cx| {
        let c = s.connections.read(cx);
        let w = s.workspaces.read(cx);
        let tabs: usize = w.workspaces.values().map(|ws| ws.tabs.len()).sum();
        let tables = w
            .workspaces
            .values()
            .flat_map(|ws| &ws.tabs)
            .filter(|t| t.table().is_some())
            .count();
        let filtered = w
            .workspaces
            .values()
            .flat_map(|ws| &ws.tabs)
            .filter_map(|t| t.table())
            .filter(|t| !t.read(cx).where_clause.is_empty() || !t.read(cx).order_by.is_empty())
            .count();
        let to_reconnect = s
            .session
            .read(cx)
            .restored()
            .map(|r| core::connections_to_restore(r).len());
        let l = s.layout.read(cx);
        println!(
            "connections: {} saved ({} with a password); restored: {} databases, {tabs} tabs ({tables} table tabs, {filtered} with filter/sort), active db: {}, mru {}, seen {}; panels sidebar/right/bottom: {}/{}/{}; to reconnect: {:?}",
            c.saved.len(),
            c.saved.iter().filter(|c| c.has_password).count(),
            w.workspaces.len(),
            w.active.is_some(),
            w.mru.len(),
            w.seen.len(),
            l.sidebar_open(),
            l.right_panel_open(),
            l.bottom_open(),
            to_reconnect
        );
        let prefs = s.prefs.read(cx).get();
        println!(
            "prefs: rowLimit {}, scopes {}, expanded {}",
            prefs.row_limit,
            prefs.scopes.len(),
            prefs.tree_expanded.len()
        );
    });
    // Saved back in the Tauri format, readable by the Tauri rules.
    cx.update(|cx| s.session.update(cx, |s, cx| s.write(cx)));
    let raw = session_json(&config).unwrap_or_default();
    let ids: Vec<String> = cx.read(|cx| {
        s.connections
            .read(cx)
            .saved
            .iter()
            .map(|c| c.config.id.clone())
            .collect()
    });
    let again = parse_session(Some(&raw), ids.iter().map(String::as_str));
    println!(
        "session.json rewritten: {} bytes, parses with the Tauri rules: {}",
        raw.len(),
        again.is_some()
    );
}
