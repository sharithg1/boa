//! The right-click menus of the explorer and the title bar's "+" / "…"
//! menus in the main window: each item, picked with the keyboard as a user
//! would (the menu's ↓ skips disabled items, so the position an item is
//! reached at also checks the ones before it), does what it says. The grid's
//! menu is covered in `grid::tests`.

use std::sync::Arc;

use boa_core::model::{Column, Constraint, ConstraintKind, Schema, Table};
use gpui::{
    Keystroke, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point, px,
};
use gpui_component::WindowExt as _;

use crate::app_state::AppState;
use crate::models::catalogs::{Catalog, DbKey};
use crate::models::prefs::RightTab;
use crate::tests::{FAKE_CONNECTION, TempDir, init, open_main};
use crate::views::root::RootView;
use crate::views::title_bar::TITLE_BAR_HEIGHT;

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

fn key() -> DbKey {
    DbKey::new(FAKE_CONNECTION, "boa_dev")
}

fn tab_count(vcx: &mut VisualTestContext) -> usize {
    vcx.update(|_, cx| {
        let ws = AppState::global(cx).workspaces.read(cx);
        ws.active_workspace().map_or(0, |w| w.tabs.len())
    })
}

fn clipboard(vcx: &mut VisualTestContext) -> String {
    vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
        .unwrap_or_default()
}

fn has_dialog(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, cx| window.has_active_dialog(cx))
}

/// A popup menu (context or dropdown) has the focus.
fn menu_open(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, _| {
        window
            .context_stack()
            .iter()
            .any(|c| c.contains("PopupMenu"))
    })
}

fn click(vcx: &mut VisualTestContext, at: Point<Pixels>, button: MouseButton) {
    vcx.simulate_mouse_down(at, button, Modifiers::none());
    vcx.simulate_mouse_up(at, button, Modifiers::none());
    draw(vcx);
}

/// Opens the menu at `at` (`button`), moves to its `n`-th enabled item
/// and presses Enter; `then` reads the app right after that Enter, before
/// any backend answer can arrive.
fn pick<R>(
    vcx: &mut VisualTestContext,
    at: Point<Pixels>,
    button: MouseButton,
    n: usize,
    then: impl FnOnce(&mut gpui::App) -> R,
) -> R {
    click(vcx, at, button);
    assert!(menu_open(vcx), "a menu opens at {at:?}");
    choose(vcx, n, then)
}

/// In the open menu: the `n`-th enabled item, Enter (see [`pick`]).
fn choose<R>(vcx: &mut VisualTestContext, n: usize, then: impl FnOnce(&mut gpui::App) -> R) -> R {
    for _ in 0..n {
        vcx.simulate_keystrokes("down");
    }
    let enter = Keystroke::parse("enter").unwrap();
    let r = vcx.update(|window, cx| {
        window.dispatch_keystroke(enter, cx);
        then(cx)
    });
    draw(vcx);
    assert!(!menu_open(vcx), "the menu closes");
    r
}

fn pick_item(vcx: &mut VisualTestContext, at: Point<Pixels>, n: usize) {
    pick(vcx, at, MouseButton::Right, n, |_| ());
}

fn orders() -> Table {
    let col = |name: &str, pk: bool| Column {
        name: name.into(),
        ty: "integer".into(),
        nullable: !pk,
        pk,
        default: None,
    };
    Table {
        name: "orders".into(),
        columns: vec![col("id", true), col("note", false)],
        constraints: vec![Constraint {
            name: "orders_pkey".into(),
            kind: ConstraintKind::PrimaryKey,
            definition: "PRIMARY KEY (id)".into(),
            columns: vec!["id".into()],
            references: None,
        }],
        ..Table::default()
    }
}

/// The pretend connection's tree, expanded down to `orders` and its keys.
fn expanded_tree(vcx: &mut VisualTestContext) {
    vcx.update(|_, cx| {
        let state = AppState::global(cx).clone();
        state.catalogs.update(cx, |c, cx| {
            c.catalogs.insert(
                key(),
                Catalog {
                    schema_names: vec!["public".into()],
                    schemas: Arc::new(vec![Schema {
                        name: "public".into(),
                        tables: vec![orders()],
                    }]),
                    names_loaded: true,
                    loaded: true,
                    ..Catalog::default()
                },
            );
            cx.notify();
        });
        state.prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                for id in [
                    "fake",
                    "fake/boa_dev",
                    "fake/boa_dev/public",
                    "fake/boa_dev/public/#table",
                    "fake/boa_dev/public/#table/orders",
                    "fake/boa_dev/public/#table/orders/#keys",
                ] {
                    p.set_expanded(id, true);
                }
            })
        });
    });
    draw(vcx);
}

/// Where the explorer draws the row of node `id` (header 40 + toolbar 32 +
/// padding 6, rows 26 high).
fn row(root: &gpui::Entity<RootView>, vcx: &mut VisualTestContext, id: &str) -> Point<Pixels> {
    let ix = vcx.update(|_, cx| {
        let sidebar = root.read(cx).sidebar().read(cx);
        sidebar.nodes().iter().position(|n| n.id == id)
    });
    let ix = ix.unwrap_or_else(|| panic!("no row {id}"));
    point(px(110.), px(40. + 32. + 6. + 13. + ix as f32 * 26.))
}

#[gpui::test]
fn explorer_menus_copy_open_and_show_what_they_say(cx: &mut TestAppContext) {
    let dir = TempDir::new("menus-explorer");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    expanded_tree(&mut vcx);
    let ids: Vec<String> = vcx.update(|_, cx| {
        let sidebar = root.read(cx).sidebar().read(cx);
        sidebar.nodes().iter().map(|n| n.id.clone()).collect()
    });
    assert_eq!(
        ids,
        [
            "fake",
            "fake/boa_dev",
            "fake/boa_dev/public",
            "fake/boa_dev/public/#table",
            "fake/boa_dev/public/#table/orders",
            "fake/boa_dev/public/#table/orders/id",
            "fake/boa_dev/public/#table/orders/note",
            "fake/boa_dev/public/#table/orders/#keys",
            "fake/boa_dev/public/#table/orders/#keys/orders_pkey",
        ]
    );
    let conn = row(&root, &mut vcx, "fake");
    let db = row(&root, &mut vcx, "fake/boa_dev");
    let table = row(&root, &mut vcx, "fake/boa_dev/public/#table/orders");
    let column = row(&root, &mut vcx, "fake/boa_dev/public/#table/orders/note");
    let pkey = row(
        &root,
        &mut vcx,
        "fake/boa_dev/public/#table/orders/#keys/orders_pkey",
    );

    // Columns: Copy name. Keys / indexes / checks: Copy name, Copy definition.
    pick_item(&mut vcx, column, 1);
    assert_eq!(clipboard(&mut vcx), "note");
    pick_item(&mut vcx, pkey, 1);
    assert_eq!(clipboard(&mut vcx), "orders_pkey");
    pick_item(&mut vcx, pkey, 2);
    assert_eq!(clipboard(&mut vcx), "PRIMARY KEY (id)");

    // Table: Open in new tab, New console: select *, Show DDL, Export to
    // CSV…, Copy name, Copy qualified name, Refresh.
    pick_item(&mut vcx, table, 5);
    assert_eq!(clipboard(&mut vcx), "orders");
    pick_item(&mut vcx, table, 6);
    assert_eq!(clipboard(&mut vcx), "public.orders");
    let tabs = tab_count(&mut vcx);
    pick_item(&mut vcx, table, 2);
    assert_eq!(tab_count(&mut vcx), tabs + 1);
    let sql = vcx.update(|_, cx| {
        let w = AppState::global(cx).workspaces.read(cx);
        w.active_tab()
            .and_then(|t| t.console().cloned())
            .map(|c| c.read(cx).sql.clone())
    });
    assert_eq!(sql.as_deref(), Some("select * from public.orders;"));
    pick_item(&mut vcx, table, 1);
    assert_eq!(tab_count(&mut vcx), tabs + 2);
    let opened = vcx.update(|_, cx| {
        let w = AppState::global(cx).workspaces.read(cx);
        w.active_tab()
            .and_then(|t| t.table().cloned())
            .map(|t| t.read(cx).table.clone())
    });
    assert_eq!(opened.as_deref(), Some("orders"), "a table tab");
    pick_item(&mut vcx, table, 4);
    assert!(cx.did_prompt_for_new_path(), "Export asks where to save");
    // Show DDL asks the backend (not connected here: an error toast naming it).
    vcx.executor().allow_parking();
    pick_item(&mut vcx, table, 3);
    let mut toast = None;
    for _ in 0..300 {
        vcx.run_until_parked();
        toast = vcx.update(|_, cx| {
            let toasts = AppState::global(cx).toasts.read(cx);
            toasts
                .toasts()
                .iter()
                .map(|t| t.title.to_string())
                .find(|t| t.contains("orders"))
        });
        if toast.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(toast.is_some(), "Show DDL reported back");

    // Database: New query console, Refresh.
    let tabs = tab_count(&mut vcx);
    pick_item(&mut vcx, db, 1);
    assert_eq!(tab_count(&mut vcx), tabs + 1);

    // Connection: Disconnect, New query console, Refresh, Copy connection
    // URL, Edit…, Delete….
    pick_item(&mut vcx, conn, 4);
    assert_eq!(
        clipboard(&mut vcx),
        "postgresql://boa@localhost:5432/boa_dev"
    );
    let tabs = tab_count(&mut vcx);
    pick_item(&mut vcx, conn, 2);
    assert_eq!(tab_count(&mut vcx), tabs + 1);
    pick_item(&mut vcx, conn, 5);
    assert!(has_dialog(&mut vcx), "Edit… opens the connection dialog");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(!has_dialog(&mut vcx));
    pick_item(&mut vcx, conn, 6);
    assert!(has_dialog(&mut vcx), "Delete… asks first");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(!has_dialog(&mut vcx));

    // Refresh reloads (once the catalog may load: not offline any more).
    vcx.update(|_, cx| {
        let catalogs = AppState::global(cx).catalogs.clone();
        catalogs.update(cx, |c, _| {
            c.offline.remove(FAKE_CONNECTION);
        });
    });
    let loading = |cx: &mut gpui::App| AppState::global(cx).catalogs.read(cx).is_loading(&key());
    assert!(pick(&mut vcx, table, MouseButton::Right, 7, loading));
    draw(&mut vcx);
    assert!(pick(&mut vcx, db, MouseButton::Right, 2, loading));
    draw(&mut vcx);
    assert!(pick(&mut vcx, conn, MouseButton::Right, 3, loading));
    draw(&mut vcx);

    // Disconnect.
    pick_item(&mut vcx, conn, 1);
    wait_disconnected(&mut vcx);
}

fn wait_disconnected(vcx: &mut VisualTestContext) {
    vcx.executor().allow_parking();
    for _ in 0..300 {
        vcx.run_until_parked();
        let connected = vcx.update(|_, cx| {
            AppState::global(cx)
                .connections
                .read(cx)
                .is_connected(FAKE_CONNECTION)
        });
        if !connected {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("still connected");
}

/// Clicks along the title bar from `from` towards `to` until a dropdown
/// menu opens; returns where (the menu stays open). The "+" moves with the
/// tabs, so it is looked up again after each change.
fn find_menu_button(vcx: &mut VisualTestContext, from: f32, to: f32) -> Point<Pixels> {
    let y = TITLE_BAR_HEIGHT / 2.;
    let step = if to < from { -4. } else { 4. };
    let mut x = from;
    while (to - x) * step > 0. {
        let at = point(px(x), y);
        click(vcx, at, MouseButton::Left);
        if menu_open(vcx) {
            return at;
        }
        x += step;
    }
    panic!("no menu button between x {from} and {to}");
}

#[gpui::test]
fn title_bar_menus_open_tabs_the_palette_and_the_connection_actions(cx: &mut TestAppContext) {
    let dir = TempDir::new("menus-title");
    init(cx, dir.path());
    let (root, mut vcx) = open_main(cx);
    // The title row spans the editor column: sidebar 260 to right panel 940.
    // "…" is its last button, "+" the first one left of the spacer.
    let more = find_menu_button(&mut vcx, 938., 860.);
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(!menu_open(&mut vcx), "Esc closed the menu at {more:?}");
    let plus = |vcx: &mut VisualTestContext| {
        find_menu_button(vcx, f32::from(more.x) - 20., 262.);
    };

    // "+": New query (⌘T), Open table… (⌘K).
    let tabs = tab_count(&mut vcx);
    plus(&mut vcx);
    choose(&mut vcx, 1, |_| ());
    assert_eq!(tab_count(&mut vcx), tabs + 1);
    plus(&mut vcx);
    choose(&mut vcx, 2, |_| ());
    assert!(vcx.update(|_, cx| root.read(cx).palette().is_some()));
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);

    // "…": Copy connection URL, Connection details, Disconnect.
    pick(&mut vcx, more, MouseButton::Left, 1, |_| ());
    assert_eq!(
        clipboard(&mut vcx),
        "postgresql://boa@localhost:5432/boa_dev"
    );
    vcx.update(|_, cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| {
            p.update(cx, |p| p.right_tab = RightTab::Structure)
        });
    });
    pick(&mut vcx, more, MouseButton::Left, 2, |_| ());
    let tab = vcx.update(|_, cx| AppState::global(cx).prefs.read(cx).get().right_tab);
    assert_eq!(tab, RightTab::Info, "Connection details");
    pick(&mut vcx, more, MouseButton::Left, 3, |_| ());
    wait_disconnected(&mut vcx);
}
