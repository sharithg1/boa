//! Explorer tests: the Catalogs model live against the dev database (skipped
//! without `PGHOST`), and the tree view's keyboard, mouse, expansion and scope
//! picker through the real keymap (#[gpui::test]).
//!
//! Until the Connections model lands (p2/connections) these tests put the
//! saved connections and their live state into it directly.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_core::connections::{ConnConfig, Environment, SslMode};
use boa_core::state::prefs::Scope;
use gpui::{
    AppContext as _, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
    size,
};
use gpui_component::Root;

use super::Sidebar;
use super::tree::{Meta, NodeKind};
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;
use crate::models::prefs::SAVE_DELAY;

/// A scratch config dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "boa-explorer-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn init(cx: &mut TestAppContext, dir: &Path) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::init(cx);
        crate::keymap::init(cx);
        AppState::init(dir, 1280., cx).unwrap();
    });
}

/// The libpq environment of the live tests, or None (skip).
struct Pg {
    host: String,
    port: u16,
    user: String,
    password: String,
    database: String,
}

fn pg() -> Option<Pg> {
    let host = std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
    let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_owned());
    Some(Pg {
        host,
        port: var("PGPORT", "5432").parse().unwrap_or(5432),
        user: var("PGUSER", "boa"),
        password: var("PGPASSWORD", "boa"),
        database: var("PGDATABASE", "boa_dev"),
    })
}

fn config(id: &str, pg: &Pg) -> ConnConfig {
    ConnConfig {
        kind: Default::default(),
        id: id.into(),
        name: format!("{id} (dev)"),
        host: pg.host.clone(),
        port: pg.port,
        user: pg.user.clone(),
        database: pg.database.clone(),
        ssl_mode: SslMode::Disable,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    }
}

/// Saves a connection into the temp config dir (connections.json).
fn save(dir: &Path, config: ConnConfig, password: Option<String>) {
    boa_core::connections::upsert(dir, config, password, true).unwrap();
}

/// Puts the saved list into the Connections model (its reload is p2's).
fn load_saved(cx: &mut TestAppContext, dir: &Path) {
    let saved = boa_core::connections::list(dir).unwrap();
    cx.update(|cx| {
        let connections = AppState::global(cx).connections.clone();
        connections.update(cx, |c, cx| {
            c.saved = saved;
            cx.notify();
        });
    });
}

fn set_state(cx: &mut TestAppContext, id: &str, state: ConnState) {
    cx.update(|cx| {
        let connections = AppState::global(cx).connections.clone();
        connections.update(cx, |c, cx| {
            c.state.insert(id.to_owned(), state);
            cx.notify();
        });
    });
}

/// Runs the app until `done` holds (the backend answers on its own runtime).
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
            start.elapsed() < Duration::from_secs(60),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Connects through the backend, then marks the connection connected (what
/// Connections::connect will do).
fn connect_live(cx: &mut TestAppContext, id: &str) {
    let backend = cx.update(|cx| AppState::global(cx).backend.clone());
    let result = Rc::new(RefCell::new(None));
    {
        let (result, id) = (result.clone(), id.to_owned());
        cx.update(|cx| {
            cx.spawn(async move |_| {
                let r = backend.connect(&id, None).await;
                *result.borrow_mut() = Some(r);
            })
            .detach()
        });
    }
    wait_until(cx, "connect", |_| result.borrow().is_some());
    let info = result.borrow_mut().take().unwrap().expect("connects");
    set_state(
        cx,
        id,
        ConnState::Connected {
            server_version: info.server_version,
            databases: info.databases,
        },
    );
}

/// Runs one statement on the dev database (fixtures).
fn exec(cx: &mut TestAppContext, id: &str, database: &str, sql: &str) {
    let backend = cx.update(|cx| AppState::global(cx).backend.clone());
    let done = Rc::new(RefCell::new(None));
    {
        let (done, id, database, sql) = (
            done.clone(),
            id.to_owned(),
            database.to_owned(),
            sql.to_owned(),
        );
        cx.update(|cx| {
            cx.spawn(async move |_| {
                let r = backend.run_query(&id, &database, &sql, 1, 0).await;
                *done.borrow_mut() = Some(r.map(|_| ()).map_err(|e| e.message));
            })
            .detach()
        });
    }
    wait_until(cx, sql, |_| done.borrow().is_some());
    let r = done.borrow_mut().take().unwrap();
    r.unwrap_or_else(|e| panic!("{sql}: {e}"));
}

fn catalog_loaded(cx: &mut TestAppContext, key: &DbKey) -> bool {
    cx.read(|cx| {
        AppState::global(cx)
            .catalogs
            .read(cx)
            .get(key)
            .is_some_and(|c| c.loaded && !c.loading)
    })
}

fn schema_names_loaded(cx: &mut TestAppContext, key: &DbKey) -> Vec<String> {
    cx.read(|cx| {
        AppState::global(cx)
            .catalogs
            .read(cx)
            .get(key)
            .map(|c| c.schemas.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default()
    })
}

fn set_scope(cx: &mut TestAppContext, key: &str, scope: Scope) {
    cx.update(|cx| {
        let prefs = AppState::global(cx).prefs.clone();
        let key = key.to_owned();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                p.scopes.insert(key, scope);
            })
        });
    });
}

/// Waits until the catalog holds exactly `want` (in order).
fn wait_for_schemas(cx: &mut TestAppContext, key: &DbKey, want: &[&str]) {
    wait_until(cx, &format!("schemas {want:?}"), |cx| {
        catalog_loaded(cx, key) && schema_names_loaded(cx, key) == want
    });
}

// ── Catalogs, live ───────────────────────────────────────────────────────────

#[gpui::test]
fn catalogs_load_the_default_pick_on_connect(cx: &mut TestAppContext) {
    let Some(pg) = pg() else { return };
    cx.executor().allow_parking();
    let dir = TempDir::new("default-pick");
    save(dir.path(), config("c1", &pg), Some(pg.password.clone()));
    init(cx, dir.path());
    load_saved(cx, dir.path());
    connect_live(cx, "c1");
    // Connecting loads the first shown database (the connection's own) with
    // the default schema pick: public.
    let key = DbKey::new("c1", pg.database.clone());
    wait_for_schemas(cx, &key, &["public"]);
    cx.read(|cx| {
        let catalogs = AppState::global(cx).catalogs.read(cx);
        let c = catalogs.get(&key).unwrap();
        assert!(c.names_loaded && c.error.is_none());
        for name in ["public", "boa_fx", "boa_fx_many", "boa_fx_ünïcødé"] {
            assert!(
                c.schema_names.iter().any(|n| n == name),
                "{name}: {:?}",
                c.schema_names
            );
        }
        let orders = c.find_table("public", "orders").expect("public.orders");
        assert!(orders.columns.iter().any(|col| col.pk));
        assert!(
            orders.constraints.iter().any(|k| k.references.is_some()),
            "orders has an FK"
        );
        assert!(catalogs.find_table(&key, "public", "customers").is_some());
    });
    // A database that isn't shown isn't loaded, and a disconnected
    // connection loads nothing.
    let other = DbKey::new("c1", "boa_dev2");
    assert!(cx.read(|cx| AppState::global(cx).catalogs.read(cx).get(&other).is_none()));
    set_state(cx, "c1", ConnState::Disconnected);
    cx.update(|cx| {
        let catalogs = AppState::global(cx).catalogs.clone();
        catalogs.update(cx, |c, cx| c.load(other.clone(), true, cx));
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| AppState::global(cx).catalogs.read(cx).get(&other).is_none()));
}

#[gpui::test]
fn scope_changes_load_and_drop_schemas(cx: &mut TestAppContext) {
    let Some(pg) = pg() else { return };
    cx.executor().allow_parking();
    let dir = TempDir::new("scope");
    save(dir.path(), config("c1", &pg), Some(pg.password.clone()));
    init(cx, dir.path());
    load_saved(cx, dir.path());
    connect_live(cx, "c1");
    let key = DbKey::new("c1", pg.database.clone());
    wait_for_schemas(cx, &key, &["public"]);

    // Pick the 50-table schema and the non-ASCII one: public drops, they load,
    // in server order whatever the pick's order.
    set_scope(
        cx,
        &key.scope_key(),
        Scope::Some(vec!["boa_fx_ünïcødé".into(), "boa_fx_many".into()]),
    );
    let names = cx.read(|cx| {
        AppState::global(cx)
            .catalogs
            .read(cx)
            .get(&key)
            .unwrap()
            .schema_names
            .clone()
    });
    let mut want: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| ["boa_fx_ünïcødé", "boa_fx_many"].contains(n))
        .collect();
    assert_eq!(want.len(), 2);
    wait_for_schemas(cx, &key, &want);
    cx.read(|cx| {
        let catalogs = AppState::global(cx).catalogs.read(cx);
        let c = catalogs.get(&key).unwrap();
        let many = c.schemas.iter().find(|s| s.name == "boa_fx_many").unwrap();
        let tables = many
            .tables
            .iter()
            .filter(|t| t.kind == boa_core::model::TableKind::Table)
            .count();
        assert!(tables >= 50, "boa_fx_many has {tables} tables");
        let uni = c
            .schemas
            .iter()
            .find(|s| s.name == "boa_fx_ünïcødé")
            .unwrap();
        assert!(!uni.tables.is_empty(), "the non-ASCII schema has tables");
        assert!(uni.tables.iter().all(|t| !t.columns.is_empty()));
    });

    // '*': everything, the connection's databases pick likewise. Other
    // agents' and tests' schemas come and go on the shared server: only the
    // fixtures this test relies on are compared (all of them, in server order).
    set_scope(cx, &key.scope_key(), Scope::All);
    let fixtures = ["public", "boa_fx_ünïcødé", "boa_fx_many"];
    want = names
        .iter()
        .map(String::as_str)
        .filter(|n| fixtures.contains(n))
        .collect();
    assert_eq!(
        want.len(),
        fixtures.len(),
        "fixtures missing from {names:?}"
    );
    wait_until(cx, &format!("every schema, with {want:?}"), |cx| {
        let loaded = schema_names_loaded(cx, &key);
        catalog_loaded(cx, &key)
            && loaded
                .iter()
                .map(String::as_str)
                .filter(|n| fixtures.contains(n))
                .eq(want.iter().copied())
    });

    // Showing another database (connection pick) loads it once it is expanded.
    let other = DbKey::new("c1", "boa_dev2");
    cx.update(|cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| p.set_expanded("c1/boa_dev2", true))
        });
    });
    set_scope(
        cx,
        "c1",
        Scope::Some(vec![pg.database.clone(), "boa_dev2".into()]),
    );
    wait_until(cx, "boa_dev2 loads", |cx| catalog_loaded(cx, &other));
    // An empty pick shows no schema at all.
    set_scope(cx, &key.scope_key(), Scope::Some(vec![]));
    wait_for_schemas(cx, &key, &[]);
}

#[gpui::test]
fn refresh_rereads_and_a_plain_load_keeps_the_cache(cx: &mut TestAppContext) {
    let Some(pg) = pg() else { return };
    cx.executor().allow_parking();
    let dir = TempDir::new("refresh");
    save(dir.path(), config("c1", &pg), Some(pg.password.clone()));
    init(cx, dir.path());
    load_saved(cx, dir.path());
    connect_live(cx, "c1");
    let db = pg.database.clone();
    let key = DbKey::new("c1", db.clone());
    wait_for_schemas(cx, &key, &["public"]);
    exec(
        cx,
        "c1",
        &db,
        "drop schema if exists boa_p3_explorer cascade",
    );
    exec(cx, "c1", &db, "create schema boa_p3_explorer");
    exec(
        cx,
        "c1",
        &db,
        "create table boa_p3_explorer.t1 (id int primary key)",
    );

    // The new schema isn't known yet: refresh re-reads the names too.
    cx.update(|cx| {
        let catalogs = AppState::global(cx).catalogs.clone();
        catalogs.update(cx, |c, cx| c.refresh(key.clone(), cx));
    });
    wait_until(cx, "names re-read", |cx| {
        cx.read(|cx| {
            AppState::global(cx)
                .catalogs
                .read(cx)
                .get(&key)
                .is_some_and(|c| {
                    !c.loading && c.schema_names.iter().any(|n| n == "boa_p3_explorer")
                })
        })
    });
    set_scope(
        cx,
        &key.scope_key(),
        Scope::Some(vec!["boa_p3_explorer".into()]),
    );
    wait_for_schemas(cx, &key, &["boa_p3_explorer"]);
    let tables = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            AppState::global(cx)
                .catalogs
                .read(cx)
                .get(&key)
                .unwrap()
                .schemas[0]
                .tables
                .iter()
                .map(|t| t.name.clone())
                .collect()
        })
    };
    assert_eq!(tables(cx), ["t1"]);

    exec(cx, "c1", &db, "create table boa_p3_explorer.t2 (id int)");
    // A plain load keeps what is cached ...
    cx.update(|cx| {
        let catalogs = AppState::global(cx).catalogs.clone();
        catalogs.update(cx, |c, cx| c.load(key.clone(), false, cx));
    });
    wait_until(cx, "load", |cx| catalog_loaded(cx, &key));
    assert_eq!(tables(cx), ["t1"]);
    // ... a refresh re-reads it; a second refresh asked for while one runs
    // runs after it.
    cx.update(|cx| {
        let catalogs = AppState::global(cx).catalogs.clone();
        catalogs.update(cx, |c, cx| {
            c.refresh(key.clone(), cx);
            assert!(c.is_loading(&key));
            c.refresh(key.clone(), cx);
        });
    });
    wait_until(cx, "refresh", |cx| {
        catalog_loaded(cx, &key) && tables(cx).len() == 2
    });
    assert_eq!(tables(cx), ["t1", "t2"]);

    // Reconnecting re-reads the names on the next load, keeping the tables.
    exec(cx, "c1", &db, "create schema boa_p3_explorer_b");
    set_state(cx, "c1", ConnState::Disconnected);
    cx.run_until_parked();
    connect_live(cx, "c1");
    wait_until(cx, "names after reconnect", |cx| {
        cx.read(|cx| {
            AppState::global(cx)
                .catalogs
                .read(cx)
                .get(&key)
                .is_some_and(|c| {
                    !c.loading
                        && c.names_loaded
                        && c.schema_names.iter().any(|n| n == "boa_p3_explorer_b")
                })
        })
    });
    assert_eq!(tables(cx), ["t1", "t2"]);
    exec(
        cx,
        "c1",
        &db,
        "drop schema boa_p3_explorer, boa_p3_explorer_b cascade",
    );
}

// ── The tree view ─────────────────────────────────────────────────────────────

/// Opens a window holding only the explorer (Root around it), drawn once.
fn open_sidebar(cx: &mut TestAppContext) -> (Entity<Sidebar>, VisualTestContext) {
    let slot = Rc::new(RefCell::new(None));
    let handle = cx.open_window(size(px(320.), px(700.)), {
        let slot = slot.clone();
        move |window, cx| {
            let sidebar = cx.new(Sidebar::new);
            *slot.borrow_mut() = Some(sidebar.clone());
            Root::new(sidebar, window, cx)
        }
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let sidebar = slot.borrow_mut().take().unwrap();
    (sidebar, vcx)
}

/// A saved connection marked connected with these databases (no server).
fn fake_connected(cx: &mut TestAppContext, dir: &Path, databases: &[&str]) {
    let pg = Pg {
        host: "localhost".into(),
        port: 5432,
        user: "boa".into(),
        password: String::new(),
        database: databases[0].into(),
    };
    let mut c = config("c1", &pg);
    c.environment = Some(Environment::Production);
    save(dir, c, None);
    load_saved(cx, dir);
    set_state(
        cx,
        "c1",
        ConnState::Connected {
            server_version: "PostgreSQL 17".into(),
            databases: databases.iter().map(|d| (*d).to_owned()).collect(),
        },
    );
}

fn expanded(vcx: &mut VisualTestContext, id: &str) -> bool {
    vcx.read(|cx| AppState::global(cx).prefs.read(cx).get().is_expanded(id))
}

fn focused(vcx: &mut VisualTestContext, sidebar: &Entity<Sidebar>) -> Option<String> {
    vcx.read(|cx| sidebar.read(cx).focused().map(str::to_owned))
}

fn redraw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
}

#[gpui::test]
fn arrow_keys_move_expand_and_collapse_as_the_source(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = TempDir::new("keys");
    init(cx, dir.path());
    fake_connected(cx, dir.path(), &["app", "analytics", "postgres"]);
    set_scope(cx, "c1", Scope::All);
    let (sidebar, mut vcx) = open_sidebar(&mut *cx);
    vcx.update(|window, cx| {
        let handle = sidebar.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
    });
    redraw(&mut vcx);
    let ids = |vcx: &mut VisualTestContext| -> Vec<String> {
        vcx.read(|cx| {
            sidebar
                .read(cx)
                .nodes()
                .iter()
                .map(|n| n.id.clone())
                .collect()
        })
    };
    assert_eq!(ids(&mut vcx), ["c1"]);

    vcx.simulate_keystrokes("down");
    assert_eq!(focused(&mut vcx, &sidebar).as_deref(), Some("c1"));
    // → expands (saved under the node id), → again enters.
    vcx.simulate_keystrokes("right");
    redraw(&mut vcx);
    assert!(expanded(&mut vcx, "c1"));
    // Server order.
    assert_eq!(
        ids(&mut vcx),
        ["c1", "c1/app", "c1/analytics", "c1/postgres"]
    );
    vcx.simulate_keystrokes("right");
    assert_eq!(focused(&mut vcx, &sidebar).as_deref(), Some("c1/app"));
    vcx.simulate_keystrokes("down down down");
    assert_eq!(focused(&mut vcx, &sidebar).as_deref(), Some("c1/postgres"));
    vcx.simulate_keystrokes("up");
    assert_eq!(focused(&mut vcx, &sidebar).as_deref(), Some("c1/analytics"));
    // → on a database expands it (saved as conn/db).
    vcx.simulate_keystrokes("right");
    redraw(&mut vcx);
    assert!(expanded(&mut vcx, "c1/analytics"));
    // ← collapses it, ← again goes to the parent, ← there collapses it.
    vcx.simulate_keystrokes("left");
    redraw(&mut vcx);
    assert!(!expanded(&mut vcx, "c1/analytics"));
    vcx.simulate_keystrokes("left");
    assert_eq!(focused(&mut vcx, &sidebar).as_deref(), Some("c1"));
    vcx.simulate_keystrokes("left");
    redraw(&mut vcx);
    assert!(!expanded(&mut vcx, "c1"));
    assert_eq!(ids(&mut vcx), ["c1"]);
    // ↵ on a connection toggles it.
    vcx.simulate_keystrokes("enter");
    redraw(&mut vcx);
    assert!(expanded(&mut vcx, "c1"));

    // The expansion reaches prefs.json with the TS keys.
    vcx.executor().advance_clock(SAVE_DELAY * 2);
    vcx.run_until_parked();
    let saved = std::fs::read_to_string(dir.path().join("prefs.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(v["treeExpanded"], serde_json::json!({ "c1": true }));
}

#[gpui::test]
fn clicks_focus_rows_and_the_chevron_only_toggles(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = TempDir::new("mouse");
    init(cx, dir.path());
    fake_connected(cx, dir.path(), &["app"]);
    let (sidebar, mut vcx) = open_sidebar(&mut *cx);
    // Header 40 + toolbar 32 + list padding 6; rows are 26 high.
    let row_y = |ix: usize| px(40. + 32. + 6. + 13. + ix as f32 * 26.);
    // The chevron (6 margin + 6 padding + 8) toggles without focusing the
    // row (Sidebar.tsx: `stopPropagation`, no `onFocus`).
    let chevron = point(px(20.), row_y(0));
    vcx.simulate_click(chevron, Modifiers::default());
    redraw(&mut vcx);
    assert!(expanded(&mut vcx, "c1"));
    assert_eq!(focused(&mut vcx, &sidebar), None);
    vcx.simulate_click(chevron, Modifiers::default());
    redraw(&mut vcx);
    assert!(!expanded(&mut vcx, "c1"));
    assert_eq!(focused(&mut vcx, &sidebar), None);
    // The connection row: a click on its name focuses it.
    vcx.simulate_click(point(px(80.), row_y(0)), Modifiers::default());
    redraw(&mut vcx);
    assert_eq!(focused(&mut vcx, &sidebar).as_deref(), Some("c1"));
    assert!(!expanded(&mut vcx, "c1"));
    // Its chevron expands it; the focus stays where it was.
    vcx.simulate_click(chevron, Modifiers::default());
    redraw(&mut vcx);
    assert!(expanded(&mut vcx, "c1"));
    assert_eq!(focused(&mut vcx, &sidebar).as_deref(), Some("c1"));
    let ids: Vec<String> = vcx.read(|cx| {
        sidebar
            .read(cx)
            .nodes()
            .iter()
            .map(|n| n.id.clone())
            .collect()
    });
    assert_eq!(ids, ["c1", "c1/app"]);
    // A double-click on a row toggles it too.
    let at = point(px(80.), row_y(0));
    vcx.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(gpui::MouseUpEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    vcx.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    vcx.simulate_event(gpui::MouseUpEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
    });
    redraw(&mut vcx);
    assert!(!expanded(&mut vcx, "c1"));
}

#[gpui::test]
fn the_production_tag_and_badges_render(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = TempDir::new("tag");
    init(cx, dir.path());
    fake_connected(cx, dir.path(), &["app", "other"]);
    let (sidebar, vcx) = open_sidebar(&mut *cx);
    let node = vcx.read(|cx| sidebar.read(cx).nodes()[0].clone());
    let NodeKind::Connection(conn) = &node.kind else {
        panic!("{node:?}")
    };
    assert_eq!(
        conn.environment,
        Some(boa_core::grid::conn_tag::Environment::Production)
    );
    assert_eq!(
        conn.color,
        Some(boa_core::grid::conn_tag::ConnectionColor::Red)
    );
    let Meta::Scope(badge) = &node.meta else {
        panic!("{:?}", node.meta)
    };
    assert_eq!((badge.selected.len(), badge.items.len()), (1, 2));
}

#[gpui::test]
fn scope_picker_enter_applies_and_escape_cancels(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = TempDir::new("picker");
    init(cx, dir.path());
    fake_connected(cx, dir.path(), &["app", "analytics", "postgres"]);
    let (sidebar, mut vcx) = open_sidebar(&mut *cx);
    let open = |vcx: &mut VisualTestContext| {
        let badge = vcx.read(|cx| match &sidebar.read(cx).nodes()[0].meta {
            Meta::Scope(b) => b.clone(),
            other => panic!("{other:?}"),
        });
        sidebar.update_in(vcx, |s, window, cx| {
            s.picker_closed = None;
            s.open_picker(&badge, point(px(200.), px(90.)), window, cx)
        });
        redraw(vcx);
        vcx.read(|cx| sidebar.read(cx).picker().cloned())
            .expect("picker open")
    };
    let picker = open(&mut vcx);
    assert_eq!(
        vcx.read(|cx| picker.read(cx).draft().selected.clone()),
        ["app"]
    );
    // Tick "postgres", Enter (from the focused filter box) applies.
    picker.update(&mut vcx, |p, cx| p.toggle("postgres", cx));
    vcx.simulate_keystrokes("enter");
    redraw(&mut vcx);
    assert!(vcx.read(|cx| sidebar.read(cx).picker().is_none()), "closed");
    let scope = vcx.read(|cx| {
        AppState::global(cx)
            .prefs
            .read(cx)
            .get()
            .scopes
            .get("c1")
            .cloned()
    });
    assert_eq!(
        scope,
        Some(Scope::Some(vec!["app".into(), "postgres".into()]))
    );

    // Esc cancels.
    let picker = open(&mut vcx);
    picker.update(&mut vcx, |p, cx| p.toggle_all(cx));
    vcx.simulate_keystrokes("escape");
    redraw(&mut vcx);
    assert!(vcx.read(|cx| sidebar.read(cx).picker().is_none()));
    let scope = vcx.read(|cx| {
        AppState::global(cx)
            .prefs
            .read(cx)
            .get()
            .scopes
            .get("c1")
            .cloned()
    });
    assert_eq!(
        scope,
        Some(Scope::Some(vec!["app".into(), "postgres".into()]))
    );

    // "All": '*', and typing in the filter narrows the list.
    let picker = open(&mut vcx);
    vcx.simulate_input("ana");
    redraw(&mut vcx);
    assert_eq!(vcx.read(|cx| picker.read(cx).draft().filter.clone()), "ana");
    picker.update(&mut vcx, |p, cx| p.toggle_all(cx));
    vcx.simulate_keystrokes("enter");
    redraw(&mut vcx);
    let scope = vcx.read(|cx| {
        AppState::global(cx)
            .prefs
            .read(cx)
            .get()
            .scopes
            .get("c1")
            .cloned()
    });
    assert_eq!(scope, Some(Scope::All));
    let ids: Vec<String> = vcx.read(|cx| {
        sidebar
            .read(cx)
            .nodes()
            .iter()
            .map(|n| n.id.clone())
            .collect()
    });
    assert_eq!(ids, ["c1"], "still collapsed");
}
