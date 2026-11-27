//! Phase 5 in the real main window: the filter bar, header sorts, staged
//! edits with their preview and submit, the discard prompts and the
//! read-only reasons. The live tests (`PGHOST`) build their own fixture
//! schema `boa_p5_edits` (dropped again) and run one at a time.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use boa_core::connections::{ConnConfig, SavedConnection};
use boa_core::error::CmdError;
use boa_core::grid::grid_edits::{Editability, Pending, RowRef};
use boa_core::grid::table_query::SortDir;
use boa_core::model::{Column, QueryResult, ResultColumn, Schema, Table, TableKind};
use boa_core::state::prefs::Scope;
use boa_core::tx::TxState;
use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_component::WindowExt as _;

use super::{active_preview, changes_label, discard_description, discard_title, preview_sql};
use crate::app_state::AppState;
use crate::grid::{GridEvent, ResultGrid};
use crate::models::catalogs::{Catalog, DbKey};
use crate::models::connections::ConnState;
use crate::models::layout::PanelKind;
use crate::models::table_tab::{TableNav, TableTab};
use crate::models::toasts::ToastKind;
use crate::tests::{FAKE_CONNECTION, TempDir, init, layout_open, open_main};
use crate::views::root::RootView;
use crate::views::table_filter_bar::{FilterField, TableFilterBar};

fn draw(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

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

fn has_dialog(vcx: &mut VisualTestContext) -> bool {
    vcx.update(|window, cx| window.has_active_dialog(cx))
}

fn active_table(vcx: &mut VisualTestContext) -> Option<Entity<TableTab>> {
    vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .read(cx)
            .active_tab()
            .and_then(|t| t.table().cloned())
    })
}

fn grid_of(
    vcx: &mut VisualTestContext,
    root: &Entity<RootView>,
    t: &Entity<TableTab>,
) -> Entity<ResultGrid> {
    vcx.update(|_, cx| root.read(cx).workspace().read(cx).grid_of(t))
        .expect("the table tab has its grid")
}

fn bar_of(
    vcx: &mut VisualTestContext,
    root: &Entity<RootView>,
    t: &Entity<TableTab>,
) -> Entity<TableFilterBar> {
    vcx.update(|_, cx| root.read(cx).workspace().read(cx).filter_bar_of(t))
        .expect("the table tab has its filter bar")
}

/// Stages `f`'s change in the grid as an edit would, announcing it.
fn stage(
    vcx: &mut VisualTestContext,
    grid: &Entity<ResultGrid>,
    f: impl FnOnce(&mut Pending, &[Vec<Option<String>>]),
) {
    vcx.update(|_, cx| {
        grid.update(cx, |g, cx| {
            let rows = g.page().expect("a page").result.rows.clone();
            let mut p = g.pending();
            f(&mut p, &rows);
            g.set_pending(p, cx);
            cx.emit(GridEvent::PendingChanged);
        })
    });
    draw(vcx);
}

fn focus_grid(vcx: &mut VisualTestContext, grid: &Entity<ResultGrid>) {
    vcx.update(|window, cx| {
        let handle = gpui::Focusable::focus_handle(grid.read(cx), cx);
        window.focus(&handle, cx);
    });
    draw(vcx);
}

#[test]
fn prompt_and_toolbar_texts_match_the_source() {
    assert_eq!(discard_title(1), "Discard 1 pending change?");
    assert_eq!(discard_title(3), "Discard 3 pending changes?");
    assert_eq!(
        discard_description("close the tab"),
        "They have not been submitted. To close the tab, they will be lost."
    );
    assert_eq!(changes_label(1), "1 pending change");
    assert_eq!(changes_label(2), "2 pending changes");
    assert!(super::preview_description(1).starts_with("1 statement, run in one transaction"));
    assert!(super::preview_description(2).starts_with("2 statements, run in one transaction"));
    assert_eq!(TableNav::Reload.action(), "reload");
    assert_eq!(TableNav::Page(0).action(), "change the page");
    assert_eq!(
        TableNav::Query(Default::default()).action(),
        "change the filter or sort"
    );
    assert_eq!(TableNav::Close.action(), "close the tab");
}

// ── Without a server: an injected page ────────────────────────────────────

fn fake_page() -> Arc<QueryResult> {
    Arc::new(QueryResult {
        columns: vec![
            ResultColumn::new("id", "int4"),
            ResultColumn::new("name", "text"),
        ],
        rows: (1..=3)
            .map(|i| vec![Some(i.to_string()), Some(format!("row {i}"))])
            .collect(),
        affected: None,
        truncated: false,
        pageable: true,
        ms: 1,
        tx: TxState::Idle,
    })
}

fn fake_table(kind: TableKind, pk: bool) -> Table {
    Table {
        name: "things".into(),
        kind,
        columns: vec![
            Column {
                name: "id".into(),
                ty: "integer".into(),
                pk,
                ..Column::default()
            },
            Column {
                name: "name".into(),
                ty: "text".into(),
                nullable: true,
                ..Column::default()
            },
        ],
        ..Table::default()
    }
}

fn set_catalog(vcx: &mut VisualTestContext, key: &DbKey, table: Option<Table>) {
    let key = key.clone();
    vcx.update(|_, cx| {
        AppState::global(cx).catalogs.clone().update(cx, |c, cx| {
            c.catalogs.insert(
                key,
                Catalog {
                    schemas: Arc::new(vec![Schema {
                        name: "public".into(),
                        tables: table.into_iter().collect(),
                    }]),
                    names_loaded: true,
                    loaded: true,
                    ..Catalog::default()
                },
            );
            cx.notify();
        })
    });
    draw(vcx);
}

/// A table tab on the pretend connection showing `fake_page` (its own load
/// fails: no server), with `public.things` in the catalog.
fn fake_table_tab(
    cx: &mut TestAppContext,
    name: &str,
) -> (
    TempDir,
    Entity<RootView>,
    VisualTestContext,
    Entity<TableTab>,
    DbKey,
) {
    let dir = TempDir::new(name);
    init(cx, dir.path());
    cx.executor().allow_parking();
    let (root, mut vcx) = open_main(cx);
    let key = DbKey::new(FAKE_CONNECTION, "boa_dev");
    let k = key.clone();
    vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .clone()
            .update(cx, |w, cx| w.open_table(k, "public", "things", false, cx))
    });
    draw(&mut vcx);
    let table = active_table(&mut vcx).unwrap();
    wait_until(&mut vcx, "the failed load", |vcx| {
        vcx.update(|_, cx| {
            let t = table.read(cx);
            !t.loading && t.error.is_some()
        })
    });
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.result = Some(fake_page());
            t.error = None;
            t.loads += 1;
            cx.notify();
        })
    });
    set_catalog(&mut vcx, &key, Some(fake_table(TableKind::Table, true)));
    (dir, root, vcx, table, key)
}

#[gpui::test]
fn read_only_reasons_follow_the_catalog(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, table, key) = fake_table_tab(cx, "p5-read-only");
    let grid = grid_of(&mut vcx, &root, &table);
    let reason = |vcx: &mut VisualTestContext| match vcx
        .update(|_, cx| grid.read(cx).editability().clone())
    {
        Editability::ReadOnly { reason } => reason,
        Editability::Editable { pk } => {
            assert_eq!(pk, vec!["id".to_owned()]);
            ""
        }
    };
    assert_eq!(reason(&mut vcx), "", "a table with its key in the result");
    set_catalog(&mut vcx, &key, Some(fake_table(TableKind::View, false)));
    assert_eq!(reason(&mut vcx), "Read-only: views are not editable");
    set_catalog(&mut vcx, &key, Some(fake_table(TableKind::Table, false)));
    assert_eq!(reason(&mut vcx), "Read-only: the table has no primary key");
    let mut other_key = fake_table(TableKind::Table, true);
    other_key.columns[0].name = "uuid".into();
    set_catalog(&mut vcx, &key, Some(other_key));
    assert_eq!(
        reason(&mut vcx),
        "Read-only: the primary key is not in the result"
    );
    set_catalog(&mut vcx, &key, None);
    assert_eq!(reason(&mut vcx), "Read-only: table not introspected");
    // A read-only grid stages nothing (⌘N does nothing).
    focus_grid(&mut vcx, &grid);
    vcx.simulate_keystrokes("cmd-n");
    draw(&mut vcx);
    assert_eq!(vcx.update(|_, cx| table.read(cx).pending_count()), 0);
}

#[gpui::test]
fn pending_edits_ask_before_paging_and_closing(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, table, key) = fake_table_tab(cx, "p5-guards");
    let grid = grid_of(&mut vcx, &root, &table);
    // ⌘N in the grid adds a row: one change, kept by the tab.
    focus_grid(&mut vcx, &grid);
    vcx.simulate_keystrokes("cmd-n");
    draw(&mut vcx);
    stage(&mut vcx, &grid, |p, rows| {
        p.set_cell(rows, RowRef::Base(0), 1, Some("renamed".into()))
    });
    assert_eq!(vcx.update(|_, cx| table.read(cx).pending_count()), 2);

    // Paging asks first; Esc keeps editing.
    let went = vcx.update(|_, cx| table.update(cx, |t, cx| t.request(TableNav::Page(3), cx)));
    assert!(!went);
    draw(&mut vcx);
    let prompt = vcx
        .update(|_, cx| table.read(cx).discard_prompt().cloned())
        .unwrap();
    assert_eq!(
        (prompt.action.as_str(), prompt.count),
        ("change the page", 2)
    );
    assert!(has_dialog(&mut vcx), "the discard prompt shows");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(!has_dialog(&mut vcx));
    let (count, offset, prompt) = vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.pending_count(), t.offset, t.discard_prompt().cloned())
    });
    assert_eq!((count, offset, prompt), (2, 0, None), "kept, not paged");
    assert_eq!(
        vcx.update(|_, cx| grid.read(cx).pending().change_count()),
        2,
        "the grid keeps them too"
    );

    // Switching tabs keeps them (no prompt): the grid and the tab hold them.
    vcx.simulate_keystrokes("alt-1");
    draw(&mut vcx);
    assert!(active_table(&mut vcx).is_none(), "the console is active");
    vcx.simulate_keystrokes("alt-2");
    draw(&mut vcx);
    assert!(!has_dialog(&mut vcx));
    assert_eq!(vcx.update(|_, cx| table.read(cx).pending_count()), 2);
    assert_eq!(
        vcx.update(|_, cx| grid.read(cx).pending().change_count()),
        2
    );

    // Closing the tab asks too; Discard closes it.
    let id = vcx.update(|_, cx| table.read(cx).tab_id.clone());
    let k = key.clone();
    vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .clone()
            .update(cx, |w, cx| w.close_tab(k, id.clone(), cx))
    });
    draw(&mut vcx);
    let tab_open = |vcx: &mut VisualTestContext| {
        let id = vcx.update(|_, cx| table.read(cx).tab_id.clone());
        vcx.update(|_, cx| {
            AppState::global(cx)
                .workspaces
                .read(cx)
                .find_tab(&id)
                .is_some()
        })
    };
    assert!(tab_open(&mut vcx), "kept open while asking");
    let prompt = vcx
        .update(|_, cx| table.read(cx).discard_prompt().cloned())
        .unwrap();
    assert_eq!(prompt.action, "close the tab");
    assert!(has_dialog(&mut vcx));
    vcx.simulate_keystrokes("enter");
    draw(&mut vcx);
    assert!(!tab_open(&mut vcx), "Discard closed the tab");
    assert!(!has_dialog(&mut vcx));
}

#[gpui::test]
fn revert_all_and_the_preview_follow_the_staged_edits(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, table, _key) = fake_table_tab(cx, "p5-preview");
    let grid = grid_of(&mut vcx, &root, &table);
    stage(&mut vcx, &grid, |p, rows| {
        p.set_cell(rows, RowRef::Base(1), 1, None);
        p.delete_row(RowRef::Base(2));
    });
    // ⌘↵ in the grid opens the preview with the exact statements.
    focus_grid(&mut vcx, &grid);
    vcx.simulate_keystrokes("cmd-enter");
    draw(&mut vcx);
    let preview = vcx.update(|_, cx| active_preview(cx)).expect("the preview");
    assert_eq!(
        vcx.update(|_, cx| preview_sql(&preview, cx)),
        vec![
            "DELETE FROM public.things WHERE id = '3'",
            "UPDATE public.things SET name = NULL WHERE id = '2'",
        ]
    );
    assert!(vcx.update(|_, cx| table.read(cx).preview));
    // Esc closes it; the edits stay.
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| active_preview(cx)).is_none());
    assert!(!vcx.update(|_, cx| table.read(cx).preview));
    assert_eq!(vcx.update(|_, cx| table.read(cx).pending_count()), 2);
    // Revert all: the tab and the grid drop them.
    vcx.update(|_, cx| table.update(cx, |t, cx| t.revert_all(cx)));
    draw(&mut vcx);
    assert_eq!(vcx.update(|_, cx| table.read(cx).pending_count()), 0);
    assert!(vcx.update(|_, cx| grid.read(cx).pending().is_empty()));
    // Nothing to preview now.
    focus_grid(&mut vcx, &grid);
    vcx.simulate_keystrokes("cmd-enter");
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| active_preview(cx)).is_none());
}

#[gpui::test]
fn clauses_that_escape_their_place_never_run(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, table, _key) = fake_table_tab(cx, "p5-clause");
    let bar = bar_of(&mut vcx, &root, &table);
    let input = vcx.update(|_, cx| bar.read(cx).input_of(FilterField::Where));
    vcx.update(|window, cx| {
        bar.update(cx, |b, cx| {
            b.set_draft(FilterField::Where, "id = 1; drop table things", window, cx)
        });
        let handle = gpui::Focusable::focus_handle(input.read(cx), cx);
        window.focus(&handle, cx);
    });
    draw(&mut vcx);
    // Enter in the field applies.
    vcx.simulate_keystrokes("enter");
    draw(&mut vcx);
    let (error, loading, filter) = vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.error.clone(), t.loading, t.where_clause.clone())
    });
    assert_eq!(
        error.as_deref(),
        Some("WHERE: Only one condition: remove the \";\"")
    );
    assert!(!loading, "refused before anything runs");
    assert_eq!(filter, "id = 1; drop table things");
    assert_eq!(
        vcx.update(|_, cx| bar.read(cx).draft_text(FilterField::Where, cx).to_string()),
        "id = 1; drop table things",
        "what was typed stays"
    );
    // Esc puts back the applied text; × clears and applies.
    vcx.update(|window, cx| {
        bar.update(cx, |b, cx| b.set_draft(FilterField::Where, "x", window, cx))
    });
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    assert_eq!(
        vcx.update(|_, cx| bar.read(cx).draft_text(FilterField::Where, cx).to_string()),
        "id = 1; drop table things"
    );
    vcx.update(|_, cx| bar.update(cx, |b, cx| b.clear(FilterField::Where, cx)));
    draw(&mut vcx);
    assert_eq!(vcx.update(|_, cx| table.read(cx).where_clause.clone()), "");
    assert_eq!(
        vcx.update(|_, cx| bar.read(cx).draft_text(FilterField::Where, cx).to_string()),
        ""
    );
}

// ── The table toolbar: Filter loaded rows, DDL, Export ────────────────────

fn click_at(vcx: &mut VisualTestContext, selector: &'static str) {
    draw(vcx);
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    vcx.simulate_click(at, gpui::Modifiers::none());
    draw(vcx);
}

fn drawn(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    draw(vcx);
    vcx.debug_bounds(selector).is_some()
}

#[gpui::test]
fn staged_edits_keep_browsing_controls_stable_at_narrow_and_wide_sizes(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, table, _) = fake_table_tab(cx, "ux-pending-bar");
    let grid = grid_of(&mut vcx, &root, &table);
    for (width, right_open) in [(720., false), (1280., true), (1600., false)] {
        vcx.update(|_, cx| {
            AppState::global(cx).layout.clone().update(cx, |l, cx| {
                l.set_open(PanelKind::RightPanel, right_open, cx)
            });
        });
        vcx.simulate_resize(gpui::size(gpui::px(width), gpui::px(820.)));
        draw(&mut vcx);
        assert!(!drawn(&mut vcx, "table-pending-bar"));
        let toolbar = vcx.debug_bounds("table-toolbar").unwrap();
        let filter = vcx.debug_bounds("table-row-filter").unwrap();
        assert!(filter.size.width >= gpui::px(200.));
        assert!(toolbar.contains(&filter.origin));
        assert!(filter.right() <= toolbar.right());

        stage(&mut vcx, &grid, |p, rows| {
            p.set_cell(rows, RowRef::Base(0), 1, Some("renamed".into()))
        });
        assert_eq!(vcx.debug_bounds("table-toolbar"), Some(toolbar));
        assert_eq!(vcx.debug_bounds("table-row-filter"), Some(filter));
        let pending = vcx.debug_bounds("table-pending-bar").unwrap();
        assert!(pending.top() >= toolbar.bottom());
        assert!(pending.right() <= toolbar.right());

        vcx.update(|_, cx| table.update(cx, |t, cx| t.revert_all(cx)));
        draw(&mut vcx);
        assert!(!drawn(&mut vcx, "table-pending-bar"));
        assert_eq!(vcx.debug_bounds("table-row-filter"), Some(filter));
        assert_eq!(
            vcx.update(|_, cx| grid.read(cx).pending().change_count()),
            0
        );
    }
}

#[gpui::test]
fn filter_loaded_rows_filters_the_grid_and_starts_empty_when_shown_again(cx: &mut TestAppContext) {
    let (_dir, root, mut vcx, table, key) = fake_table_tab(cx, "p9-row-filter");
    let grid = grid_of(&mut vcx, &root, &table);
    let input = vcx
        .update(|_, cx| root.read(cx).workspace().read(cx).row_filter_of(&table))
        .expect("the table tab has its row filter");
    vcx.update(|window, cx| {
        let handle = gpui::Focusable::focus_handle(input.read(cx), cx);
        window.focus(&handle, cx);
    });
    draw(&mut vcx);
    // Typed, any case: only "row 2" stays, numbered from 1.
    vcx.simulate_input("ROW 2");
    draw(&mut vcx);
    let (rows, label, filter) = vcx.update(|_, cx| {
        let g = grid.read(cx);
        let p = g.page().expect("a page");
        (p.row_count(), p.row_label(0), g.row_filter().to_owned())
    });
    assert_eq!((rows, label.as_str(), filter.as_str()), (1, "1", "ROW 2"));
    // Copy page as CSV copies what the grid shows.
    click_at(&mut vcx, "table-export-trigger");
    click_at(&mut vcx, "export-copy");
    assert_eq!(
        vcx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("id,name\n2,row 2\n")
    );
    // Another tab, then back: the filter starts empty (Workspace.tsx mounts
    // the TableView afresh).
    let ws = vcx.update(|_, cx| AppState::global(cx).workspaces.clone());
    let k = key.clone();
    vcx.update(|_, cx| ws.update(cx, |w, cx| w.new_query_tab(k, cx)));
    draw(&mut vcx);
    let id = vcx.update(|_, cx| table.read(cx).tab_id.clone());
    vcx.update(|_, cx| ws.update(cx, |w, cx| w.activate(&key, &id, cx)));
    draw(&mut vcx);
    assert_eq!(vcx.update(|_, cx| input.read(cx).value().to_string()), "");
    assert_eq!(
        vcx.update(|_, cx| grid.read(cx).page().map(|p| p.row_count())),
        Some(3)
    );
}

#[gpui::test]
fn the_ddl_button_shows_for_known_tables_and_opens_their_ddl(cx: &mut TestAppContext) {
    let (_dir, _root, mut vcx, _table, key) = fake_table_tab(cx, "p9-ddl");
    assert!(drawn(&mut vcx, "table-ddl"));
    // The pretend server has no DDL to give: the source's error toast.
    click_at(&mut vcx, "table-ddl");
    wait_until(&mut vcx, "the DDL toast", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .toasts
                .read(cx)
                .toasts()
                .iter()
                .any(|t| t.title.as_ref() == "No DDL for public.things")
        })
    });
    // A table the catalog doesn't know has no DDL button (`{table && …}`).
    set_catalog(&mut vcx, &key, None);
    assert!(!drawn(&mut vcx, "table-ddl"));
}

#[test]
fn the_export_menu_is_disabled_only_offline_or_for_a_refused_clause() {
    use crate::views::workspace::export_disabled;
    assert!(!export_disabled(true, false));
    assert!(export_disabled(false, false));
    assert!(export_disabled(true, true));
}

#[gpui::test]
fn a_server_error_keeps_the_export_menu_a_refused_clause_disables_it(cx: &mut TestAppContext) {
    let (_dir, _root, mut vcx, table, _key) = fake_table_tab(cx, "p9-export-error");
    // The load failed on the server: the menu still exports the table.
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.result = None;
            t.error = Some("relation does not exist".into());
            cx.notify();
        })
    });
    click_at(&mut vcx, "table-export-trigger");
    assert!(drawn(&mut vcx, "export-csv"), "the menu opens");
    vcx.simulate_keystrokes("escape");
    draw(&mut vcx);
    // A clause refused before anything runs (`clauseError`): disabled.
    vcx.update(|_, cx| {
        table.update(cx, |t, cx| {
            t.set_query(
                &boa_core::grid::table_query::TableQuery {
                    filter: "id = 1; drop table things".into(),
                    order_by: String::new(),
                },
                cx,
            )
        })
    });
    draw(&mut vcx);
    assert!(vcx.update(|_, cx| table.read(cx).error.is_some()));
    click_at(&mut vcx, "table-export-trigger");
    assert!(!drawn(&mut vcx, "export-csv"), "disabled");
}

// ── Live (PGHOST) ─────────────────────────────────────────────────────────

const LIVE: &str = "live";
const SCHEMA: &str = "boa_p5_edits";

/// The live tests share the fixture schema: one at a time.
static LIVE_LOCK: Mutex<()> = Mutex::new(());

fn live_lock() -> MutexGuard<'static, ()> {
    LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Saves and connects the local server (PGHOST …) as connection "live".
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
    let key = DbKey::new(LIVE, database);
    // The fixture schema is in the database's scope.
    let scope_key = key.scope_key();
    vcx.update(|_, cx| {
        let state = AppState::global(cx).clone();
        state.prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                p.scopes.insert(
                    scope_key.clone(),
                    Scope::Some(vec!["public".into(), SCHEMA.into()]),
                );
            })
        });
        state.connections.update(cx, |c, cx| {
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
    Some(key)
}

fn exec(vcx: &mut VisualTestContext, key: &DbKey, sql: &str) -> Result<QueryResult, CmdError> {
    let backend = vcx.update(|_, cx| AppState::global(cx).backend.clone());
    await_task(
        vcx,
        backend.run_query(&key.connection_id, &key.database, sql, 500, 0),
    )
}

/// `boa_p5_edits.items`: 30 rows (id 1–30, name "item 01"…, qty = id,
/// note "even" on even ids), `qty` defaults to 7, `name` is unique.
fn create_fixture(vcx: &mut VisualTestContext, key: &DbKey) {
    for sql in [
        format!("drop schema if exists {SCHEMA} cascade"),
        format!("create schema {SCHEMA}"),
        format!(
            "create table {SCHEMA}.items (id int generated by default as identity primary key, \
             name text not null unique, qty int not null default 7, note text)"
        ),
        format!(
            "insert into {SCHEMA}.items (name, qty, note) select 'item ' || lpad(i::text, 2, '0'), i, \
             case when i % 2 = 0 then 'even' end from generate_series(1, 30) i"
        ),
    ] {
        exec(vcx, key, &sql).unwrap_or_else(|e| panic!("{sql}: {}", e.message));
    }
    let k = key.clone();
    vcx.update(|_, cx| {
        AppState::global(cx)
            .catalogs
            .clone()
            .update(cx, |c, cx| c.load(k, true, cx))
    });
    wait_until(vcx, "the fixture in the catalog", |vcx| {
        vcx.update(|_, cx| {
            AppState::global(cx)
                .catalogs
                .read(cx)
                .find_table(key, SCHEMA, "items")
                .is_some()
        })
    });
}

fn drop_fixture(vcx: &mut VisualTestContext, key: &DbKey) {
    let _ = exec(vcx, key, &format!("drop schema if exists {SCHEMA} cascade"));
}

struct Live {
    _lock: MutexGuard<'static, ()>,
    _dir: TempDir,
    root: Entity<RootView>,
    vcx: VisualTestContext,
    key: DbKey,
}

fn live(cx: &mut TestAppContext, name: &str) -> Option<Live> {
    std::env::var("PGHOST").ok().filter(|h| !h.is_empty())?;
    let lock = live_lock();
    let dir = TempDir::new(name);
    init(cx, dir.path());
    cx.executor().allow_parking();
    let (root, mut vcx) = open_main(cx);
    let key = live_database(&mut vcx)?;
    create_fixture(&mut vcx, &key);
    Some(Live {
        _lock: lock,
        _dir: dir,
        root,
        vcx,
        key,
    })
}

/// Opens `boa_p5_edits.items` and waits for its editable first page.
fn open_items(l: &mut Live) -> (Entity<TableTab>, Entity<ResultGrid>) {
    let k = l.key.clone();
    l.vcx.update(|_, cx| {
        AppState::global(cx)
            .workspaces
            .clone()
            .update(cx, |w, cx| w.open_table(k, SCHEMA, "items", false, cx))
    });
    draw(&mut l.vcx);
    let table = active_table(&mut l.vcx).unwrap();
    wait_until(&mut l.vcx, "the first page", |vcx| {
        vcx.update(|_, cx| table.read(cx).result.is_some())
    });
    draw(&mut l.vcx);
    let grid = grid_of(&mut l.vcx, &l.root, &table);
    wait_until(&mut l.vcx, "editable rows", |vcx| {
        vcx.update(|_, cx| matches!(grid.read(cx).editability(), Editability::Editable { .. }))
    });
    (table, grid)
}

fn wait_loaded(vcx: &mut VisualTestContext, table: &Entity<TableTab>, after: u64) {
    wait_until(vcx, "the reloaded page", |vcx| {
        vcx.update(|_, cx| {
            let t = table.read(cx);
            !t.loading && (t.loads > after || t.error.is_some())
        })
    });
    draw(vcx);
}

fn loads(vcx: &mut VisualTestContext, table: &Entity<TableTab>) -> u64 {
    vcx.update(|_, cx| table.read(cx).loads)
}

fn column(vcx: &mut VisualTestContext, key: &DbKey, sql: &str) -> Vec<Option<String>> {
    exec(vcx, key, sql)
        .expect("query")
        .rows
        .into_iter()
        .map(|r| r.into_iter().next().flatten())
        .collect()
}

fn last_toast(vcx: &mut VisualTestContext) -> (ToastKind, String, Option<String>) {
    vcx.update(|_, cx| {
        let toasts = AppState::global(cx).toasts.read(cx);
        let t = toasts.toasts().last().expect("a toast");
        (
            t.kind,
            t.title.to_string(),
            t.body.as_ref().map(|b| b.to_string()),
        )
    })
}

#[gpui::test]
fn live_edit_preview_submit_changes_the_row(cx: &mut TestAppContext) {
    let Some(mut l) = live(cx, "p5-live-submit") else {
        return;
    };
    let (table, grid) = open_items(&mut l);
    stage(&mut l.vcx, &grid, |p, rows| {
        p.set_cell(rows, RowRef::Base(0), 1, Some("first, renamed".into()))
    });
    assert_eq!(l.vcx.update(|_, cx| table.read(cx).pending_count()), 1);
    // ⌘↵ in the grid: the preview with the exact statement.
    focus_grid(&mut l.vcx, &grid);
    l.vcx.simulate_keystrokes("cmd-enter");
    draw(&mut l.vcx);
    let preview = l
        .vcx
        .update(|_, cx| active_preview(cx))
        .expect("the preview");
    assert_eq!(
        l.vcx.update(|_, cx| preview_sql(&preview, cx)),
        vec!["UPDATE boa_p5_edits.items SET name = 'first, renamed' WHERE id = '1'"]
    );
    // ↵ submits; the page reloads without the edits.
    let before = loads(&mut l.vcx, &table);
    l.vcx.simulate_keystrokes("enter");
    wait_loaded(&mut l.vcx, &table, before);
    let (count, preview_open, error) = l.vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.pending_count(), t.preview, t.submit_error().cloned())
    });
    assert_eq!((count, preview_open, error), (0, false, None));
    assert!(l.vcx.update(|_, cx| active_preview(cx)).is_none(), "closed");
    assert!(l.vcx.update(|_, cx| grid.read(cx).pending().is_empty()));
    let key = l.key.clone();
    assert_eq!(
        column(
            &mut l.vcx,
            &key,
            "select name from boa_p5_edits.items where id = 1"
        ),
        vec![Some("first, renamed".into())]
    );
    // (Without an ORDER BY the updated row may come back anywhere.)
    let shown = l.vcx.update(|_, cx| {
        let rows = &table.read(cx).result.as_ref().unwrap().rows;
        rows.iter()
            .find(|r| r[0].as_deref() == Some("1"))
            .and_then(|r| r[1].clone())
    });
    assert_eq!(shown.as_deref(), Some("first, renamed"), "refreshed");
    assert_eq!(
        last_toast(&mut l.vcx),
        (ToastKind::Success, "Submitted 1 change".into(), None)
    );
    // The Output log (and history) has the statement, from the table.
    let line = l
        .vcx
        .update(|_, cx| AppState::global(cx).workspaces.read(cx).output(LIVE)[0].clone());
    assert_eq!(
        line.entry.sql,
        "UPDATE boa_p5_edits.items SET name = 'first, renamed' WHERE id = '1'"
    );
    assert_eq!(line.entry.source.as_deref(), Some("items"));
    assert_eq!(line.entry.affected, Some(1.));
    assert_eq!(line.entry.error, None);
    drop_fixture(&mut l.vcx, &key);
}

#[gpui::test]
fn live_add_row_with_defaults_delete_and_set_null(cx: &mut TestAppContext) {
    let Some(mut l) = live(cx, "p5-live-rows") else {
        return;
    };
    let (table, grid) = open_items(&mut l);
    // ⌘N adds a row (untouched columns keep their defaults).
    focus_grid(&mut l.vcx, &grid);
    l.vcx.simulate_keystrokes("cmd-n");
    draw(&mut l.vcx);
    stage(&mut l.vcx, &grid, |p, rows| {
        let id = p.inserts.last().expect("the new row").id;
        p.set_cell(rows, RowRef::New(id), 1, Some("brand new".into()));
        p.delete_row(RowRef::Base(2));
        p.set_cell(rows, RowRef::Base(3), 3, None);
    });
    assert_eq!(l.vcx.update(|_, cx| table.read(cx).pending_count()), 3);
    let statements: Vec<String> = l.vcx.update(|_, cx| {
        table
            .read(cx)
            .statements(cx)
            .into_iter()
            .map(|s| s.sql)
            .collect()
    });
    assert_eq!(
        statements,
        vec![
            "DELETE FROM boa_p5_edits.items WHERE id = '3'",
            "UPDATE boa_p5_edits.items SET note = NULL WHERE id = '4'",
            "INSERT INTO boa_p5_edits.items (name) VALUES ('brand new')",
        ]
    );
    let before = loads(&mut l.vcx, &table);
    l.vcx
        .update(|_, cx| table.update(cx, |t, cx| t.submit_edits(cx)));
    wait_loaded(&mut l.vcx, &table, before);
    assert_eq!(l.vcx.update(|_, cx| table.read(cx).pending_count()), 0);
    let key = l.key.clone();
    assert_eq!(
        column(
            &mut l.vcx,
            &key,
            "select count(*)::text from boa_p5_edits.items where id = 3"
        ),
        vec![Some("0".into())]
    );
    assert_eq!(
        column(
            &mut l.vcx,
            &key,
            "select note from boa_p5_edits.items where id = 4"
        ),
        vec![None],
        "NULL, not an empty string"
    );
    assert_eq!(
        column(
            &mut l.vcx,
            &key,
            "select qty::text from boa_p5_edits.items where name = 'brand new'"
        ),
        vec![Some("7".into())],
        "the default applied"
    );
    assert_eq!(
        last_toast(&mut l.vcx),
        (ToastKind::Success, "Submitted 3 changes".into(), None)
    );
    let lines: Vec<String> = l.vcx.update(|_, cx| {
        AppState::global(cx).workspaces.read(cx).output(LIVE)[..3]
            .iter()
            .map(|l| l.entry.sql.clone())
            .collect()
    });
    assert_eq!(lines.len(), 3, "one Output line per statement");
    drop_fixture(&mut l.vcx, &key);
}

#[gpui::test]
fn live_failed_submit_keeps_the_edits_and_points_at_the_statement(cx: &mut TestAppContext) {
    let Some(mut l) = live(cx, "p5-live-fail") else {
        return;
    };
    l.vcx.update(|_, cx| {
        AppState::global(cx)
            .layout
            .clone()
            .update(cx, |l, cx| l.set_open(PanelKind::Bottom, false, cx))
    });
    let (table, grid) = open_items(&mut l);
    stage(&mut l.vcx, &grid, |p, rows| {
        p.set_cell(rows, RowRef::Base(0), 2, Some("100".into()));
        // name is unique: "item 01" exists.
        p.set_cell(rows, RowRef::Base(1), 1, Some("item 01".into()));
    });
    l.vcx
        .update(|_, cx| table.update(cx, |t, cx| t.submit_edits(cx)));
    wait_until(&mut l.vcx, "the failed submit", |vcx| {
        vcx.update(|_, cx| {
            let t = table.read(cx);
            !t.submitting() && t.submit_error().is_some()
        })
    });
    draw(&mut l.vcx);
    let (count, preview, error) = l.vcx.update(|_, cx| {
        let t = table.read(cx);
        (
            t.pending_count(),
            t.preview,
            t.submit_error().cloned().unwrap(),
        )
    });
    assert_eq!(count, 2, "the edits stay");
    assert!(preview, "the preview shows the failure");
    assert!(l.vcx.update(|_, cx| active_preview(cx)).is_some());
    assert_eq!(error.index, Some(1));
    assert!(error.message.starts_with("[23505] "), "{}", error.message);
    assert_eq!(
        l.vcx.update(|_, cx| grid.read(cx).pending().change_count()),
        2,
        "still staged in the grid"
    );
    let key = l.key.clone();
    assert_eq!(
        column(
            &mut l.vcx,
            &key,
            "select qty::text from boa_p5_edits.items where id = 1"
        ),
        vec![Some("1".into())],
        "statement 1 was rolled back"
    );
    let (kind, title, body) = last_toast(&mut l.vcx);
    assert_eq!(
        (kind, title.as_str()),
        (ToastKind::Error, "Changes not submitted")
    );
    assert!(
        body.unwrap()
            .starts_with("Statement 2 of 2 failed; all changes were rolled back.\n[23505]")
    );
    let line = l
        .vcx
        .update(|_, cx| AppState::global(cx).workspaces.read(cx).output(LIVE)[0].clone());
    assert_eq!(
        line.entry.sql,
        "UPDATE boa_p5_edits.items SET name = 'item 01' WHERE id = '2'"
    );
    assert_eq!(line.entry.error.as_deref(), Some(error.message.as_str()));
    assert!(
        line.entry
            .detail
            .as_deref()
            .unwrap()
            .starts_with("Statement 2 of 2 failed; all changes were rolled back.")
    );
    assert!(
        layout_open(&mut l.vcx, PanelKind::Bottom),
        "the Output log shows"
    );
    // Fixing the edit clears the error; the retry goes through.
    stage(&mut l.vcx, &grid, |p, rows| {
        p.set_cell(rows, RowRef::Base(1), 1, Some("item 02b".into()))
    });
    assert!(
        l.vcx
            .update(|_, cx| table.read(cx).submit_error().is_none())
    );
    let before = loads(&mut l.vcx, &table);
    l.vcx
        .update(|_, cx| table.update(cx, |t, cx| t.submit_edits(cx)));
    wait_loaded(&mut l.vcx, &table, before);
    assert!(l.vcx.update(|_, cx| active_preview(cx)).is_none());
    assert_eq!(
        column(
            &mut l.vcx,
            &key,
            "select name from boa_p5_edits.items where id = 2"
        ),
        vec![Some("item 02b".into())]
    );
    drop_fixture(&mut l.vcx, &key);
}

#[gpui::test]
fn live_filter_bar_applies_where_and_shows_server_errors_in_place(cx: &mut TestAppContext) {
    let Some(mut l) = live(cx, "p5-live-filter") else {
        return;
    };
    let (table, _grid) = open_items(&mut l);
    let bar = bar_of(&mut l.vcx, &l.root.clone(), &table);
    let input = l
        .vcx
        .update(|_, cx| bar.read(cx).input_of(FilterField::Where));
    let apply = |l: &mut Live, text: &str| {
        let before = loads(&mut l.vcx, &table);
        l.vcx.update(|window, cx| {
            bar.update(cx, |b, cx| {
                b.set_draft(FilterField::Where, text, window, cx)
            });
            let handle = gpui::Focusable::focus_handle(input.read(cx), cx);
            window.focus(&handle, cx);
        });
        draw(&mut l.vcx);
        l.vcx.simulate_keystrokes("enter");
        wait_loaded(&mut l.vcx, &table, before);
    };
    apply(&mut l, "where qty > 25 and note is not null");
    let (sql, rows, error) = l.vcx.update(|_, cx| {
        let t = table.read(cx);
        (
            t.sql.clone(),
            t.result.as_ref().map(|r| r.rows.len()),
            t.error.clone(),
        )
    });
    assert_eq!(
        sql,
        "select * from boa_p5_edits.items\nwhere qty > 25 and note is not null"
    );
    assert_eq!((rows, error), (Some(3), None), "26, 28, 30");
    // A bad condition: the server's error in place of the rows.
    apply(&mut l, "qtyy > 1");
    let (error, result) = l.vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.error.clone(), t.result.is_some())
    });
    let error = error.expect("the server's error");
    assert!(
        error.starts_with("[42703] column \"qtyy\" does not exist"),
        "{error}"
    );
    assert!(!result);
    assert_eq!(
        l.vcx
            .update(|_, cx| bar.read(cx).draft_text(FilterField::Where, cx).to_string()),
        "qtyy > 1",
        "what was typed stays"
    );
    // One clause only: refused before anything runs.
    l.vcx.update(|window, cx| {
        bar.update(cx, |b, cx| {
            b.set_draft(FilterField::Where, "qty > 1 /* open", window, cx);
            b.apply(cx);
        })
    });
    draw(&mut l.vcx);
    assert_eq!(
        l.vcx
            .update(|_, cx| table.read(cx).error.clone())
            .as_deref(),
        Some("WHERE: Unterminated /* comment")
    );
    // Emptied and applied: all rows again.
    apply(&mut l, "");
    assert_eq!(
        l.vcx
            .update(|_, cx| table.read(cx).result.as_ref().map(|r| r.rows.len())),
        Some(30)
    );
    let key = l.key.clone();
    drop_fixture(&mut l.vcx, &key);
}

#[gpui::test]
fn live_header_clicks_sort_on_the_server_shift_adds_columns(cx: &mut TestAppContext) {
    let Some(mut l) = live(cx, "p5-live-sort") else {
        return;
    };
    let (table, grid) = open_items(&mut l);
    let bar = bar_of(&mut l.vcx, &l.root.clone(), &table);
    let click = |l: &mut Live, col: usize, shift: bool| {
        let before = loads(&mut l.vcx, &table);
        l.vcx
            .update(|_, cx| grid.update(cx, |g, cx| g.header_clicked(col, shift, cx)));
        wait_loaded(&mut l.vcx, &table, before);
    };
    let first_ids = |l: &mut Live| -> Vec<String> {
        l.vcx.update(|_, cx| {
            table.read(cx).result.as_ref().unwrap().rows[..3]
                .iter()
                .map(|r| r[0].clone().unwrap())
                .collect()
        })
    };
    // note (3) ascending, then ⇧ qty (2): two columns, ranked.
    click(&mut l, 3, false);
    assert_eq!(
        l.vcx.update(|_, cx| table.read(cx).order_by.clone()),
        "note"
    );
    click(&mut l, 2, true);
    click(&mut l, 2, true);
    let (order_by, sql) = l.vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.order_by.clone(), t.sql.clone())
    });
    assert_eq!(order_by, "note, qty desc");
    assert!(sql.ends_with("\norder by note, qty desc"), "{sql}");
    // "even" rows first (nulls last), highest qty first.
    assert_eq!(first_ids(&mut l), vec!["30", "28", "26"]);
    let sort = l
        .vcx
        .update(|_, cx| grid.read(cx).table_context().unwrap().sort.clone());
    assert_eq!(sort.len(), 2);
    assert_eq!(
        (sort[0].0.as_str(), sort[0].1.rank, sort[0].1.dir),
        ("note", 1, SortDir::Asc)
    );
    assert_eq!(
        (sort[1].0.as_str(), sort[1].1.rank, sort[1].1.dir),
        ("qty", 2, SortDir::Desc)
    );
    // The filter bar shows the rewritten ORDER BY.
    assert_eq!(
        l.vcx.update(|_, cx| bar
            .read(cx)
            .draft_text(FilterField::OrderBy, cx)
            .to_string()),
        "note, qty desc"
    );
    // A plain click on another column replaces the sort.
    click(&mut l, 1, false);
    assert_eq!(
        l.vcx.update(|_, cx| table.read(cx).order_by.clone()),
        "name"
    );
    // With pending edits a header click asks first.
    stage(&mut l.vcx, &grid, |p, _| p.delete_row(RowRef::Base(0)));
    l.vcx
        .update(|_, cx| grid.update(cx, |g, cx| g.header_clicked(1, false, cx)));
    draw(&mut l.vcx);
    let prompt = l
        .vcx
        .update(|_, cx| table.read(cx).discard_prompt().cloned())
        .unwrap();
    assert_eq!(prompt.action, "change the filter or sort");
    let before = loads(&mut l.vcx, &table);
    l.vcx.simulate_keystrokes("enter");
    wait_loaded(&mut l.vcx, &table, before);
    let (order_by, count) = l.vcx.update(|_, cx| {
        let t = table.read(cx);
        (t.order_by.clone(), t.pending_count())
    });
    assert_eq!(
        (order_by.as_str(), count),
        ("name desc", 0),
        "discarded, then sorted"
    );
    let key = l.key.clone();
    drop_fixture(&mut l.vcx, &key);
}

#[gpui::test]
fn live_paging_with_edits_asks_then_pages(cx: &mut TestAppContext) {
    let Some(mut l) = live(cx, "p5-live-paging") else {
        return;
    };
    l.vcx.update(|_, cx| {
        let prefs = AppState::global(cx).prefs.clone();
        prefs.update(cx, |p, cx| p.update(cx, |p| p.row_limit = 10));
    });
    let (table, grid) = open_items(&mut l);
    stage(&mut l.vcx, &grid, |p, rows| {
        p.set_cell(rows, RowRef::Base(0), 2, Some("5".into()))
    });
    // The grid's pager asks for the next page: the prompt first.
    l.vcx
        .update(|_, cx| grid.update(cx, |_, cx| cx.emit(GridEvent::PageRequested { offset: 10 })));
    draw(&mut l.vcx);
    assert!(has_dialog(&mut l.vcx));
    assert_eq!(l.vcx.update(|_, cx| table.read(cx).offset), 0);
    let before = loads(&mut l.vcx, &table);
    l.vcx.simulate_keystrokes("enter");
    wait_loaded(&mut l.vcx, &table, before);
    let (offset, first, count) = l.vcx.update(|_, cx| {
        let t = table.read(cx);
        (
            t.offset,
            t.result.as_ref().unwrap().rows[0][0].clone(),
            t.pending_count(),
        )
    });
    assert_eq!((offset, first.as_deref(), count), (10, Some("11"), 0));
    assert!(l.vcx.update(|_, cx| grid.read(cx).pending().is_empty()));
    let key = l.key.clone();
    drop_fixture(&mut l.vcx, &key);
}
