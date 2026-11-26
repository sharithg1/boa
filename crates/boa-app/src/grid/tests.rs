//! View-level tests of the grid: mouse and keyboard driven through
//! `VisualTestContext` (the real keymap and dispatch tree) against content
//! handed over with `set_content` (no database needed), in a window whose
//! root is gpui-component's `Root`, with `AppState` on a temp dir (toasts).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use boa_core::grid::copy_as::CopyFormat;
use boa_core::grid::grid_edits::{Editability, RowMark, RowRef};
use boa_core::grid::table_query::{ColumnSort, SortDir};
use boa_core::model::{QueryResult, ResultColumn, Value};
use boa_core::tx::TxState;
use gpui::{
    AppContext as _, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Point,
    TestAppContext, VisualTestContext, point, px, size,
};
use gpui_component::Root;

use super::columns::MIN_WIDTH;
use super::selection::{Cell, RegionRows};
use super::*;
use crate::app_state::AppState;
use crate::views::json_viewer::Mode;

/// A scratch config dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "boa-grid-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn s(v: &str) -> Value {
    Some(v.to_owned())
}

fn result(rows: usize) -> QueryResult {
    QueryResult {
        columns: vec![
            ResultColumn::new("id", "int4"),
            ResultColumn::new("name", "text"),
            ResultColumn::new("payload", "jsonb"),
            ResultColumn::new("note", "text"),
        ],
        rows: (0..rows)
            .map(|i| {
                vec![
                    s(&i.to_string()),
                    s(&format!("name {i}")),
                    s(&format!("{{\"n\": {i}, \"tags\": [\"a\", \"b\"]}}")),
                    match i % 3 {
                        0 => None,
                        1 => s("x"),
                        _ => s("line 1\nline 2"),
                    },
                ]
            })
            .collect(),
        affected: None,
        truncated: false,
        pageable: true,
        ms: 1,
        tx: TxState::default(),
    }
}

fn editable() -> Editability {
    Editability::Editable {
        pk: vec!["id".into()],
    }
}

fn read_only() -> Editability {
    Editability::ReadOnly {
        reason: "Read-only: console results",
    }
}

fn content(result: QueryResult, editability: Editability) -> GridContent {
    GridContent {
        sql: "select * from t".into(),
        result: Arc::new(result),
        offset: 0,
        page_size: 500,
        editability,
    }
}

struct Harness {
    grid: Entity<ResultGrid>,
    cx: VisualTestContext,
    events: Rc<RefCell<Vec<GridEvent>>>,
    _dir: TempDir,
}

impl Harness {
    fn open(cx: &mut TestAppContext, rows: usize, editability: Editability) -> Self {
        Self::open_with(cx, content(result(rows), editability))
    }

    fn open_with(cx: &mut TestAppContext, content: GridContent) -> Self {
        let dir = TempDir::new();
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::init(cx);
            crate::keymap::init(cx);
            AppState::init(&dir.0, 1280., cx).unwrap();
        });
        let slot = Rc::new(RefCell::new(None));
        let handle = cx.open_window(size(px(1200.), px(700.)), {
            let slot = slot.clone();
            move |window, cx| {
                let grid = cx.new(|cx| ResultGrid::new(window, cx));
                window.focus(&grid.focus_handle(cx), cx);
                *slot.borrow_mut() = Some(grid.clone());
                Root::new(grid, window, cx)
            }
        });
        let mut vcx = VisualTestContext::from_window(handle.into(), cx);
        let grid: Entity<ResultGrid> = slot.borrow_mut().take().unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        vcx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&grid, move |_, event: &GridEvent, _| {
                events.borrow_mut().push(event.clone())
            })
            .detach();
        });
        grid.update(&mut vcx, |grid, cx| grid.set_content(content, cx));
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        Self {
            grid,
            cx: vcx,
            events,
            _dir: dir,
        }
    }

    /// Window position of a cell's centre (the list must have been drawn).
    fn cell_at(&mut self, cell: Cell) -> Point<gpui::Pixels> {
        self.grid.update(&mut self.cx, |grid, _| {
            let (bounds, offset) = grid.scroll_state();
            let page = grid.page.as_ref().unwrap();
            let x = f32::from(bounds.origin.x + offset.x)
                + page.layout.numbers_width()
                + page.layout.start(cell.col)
                + page.layout.width(cell.col) / 3.;
            let y = f32::from(bounds.origin.y + offset.y) + (cell.row as f32 + 0.5) * ROW_HEIGHT;
            point(px(x), px(y))
        })
    }

    fn row_number_at(&mut self, row: usize) -> Point<gpui::Pixels> {
        let p = self.cell_at(Cell::new(row, 0));
        self.grid.update(&mut self.cx, |grid, _| {
            let (bounds, _) = grid.scroll_state();
            point(bounds.origin.x + px(10.), p.y)
        })
    }

    fn header_at(&mut self, col: usize) -> Point<gpui::Pixels> {
        let p = self.cell_at(Cell::new(0, col));
        self.grid.update(&mut self.cx, |grid, _| {
            let (bounds, _) = grid.scroll_state();
            point(p.x, bounds.origin.y - px(HEADER_HEIGHT / 2.))
        })
    }

    fn click(&mut self, cell: Cell, modifiers: Modifiers) {
        let at = self.cell_at(cell);
        self.cx.simulate_click(at, modifiers);
    }

    fn double_click(&mut self, cell: Cell) {
        let at = self.cell_at(cell);
        self.cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        });
        self.cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 2,
        });
        self.cx.run_until_parked();
    }

    fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.cx.run_until_parked();
    }

    fn region(&mut self) -> (Vec<usize>, std::ops::Range<usize>) {
        self.grid.read_with(&self.cx, |grid, _| {
            let page = grid.page.as_ref().unwrap();
            let r = page
                .selection
                .region(page.row_count(), page.col_count())
                .expect("a selection");
            (r.rows.iter().collect(), r.cols)
        })
    }

    fn clipboard(&mut self) -> String {
        self.cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default()
    }

    fn last_toast(&mut self) -> Option<String> {
        self.cx.update(|_, cx| {
            AppState::global(cx)
                .toasts
                .read(cx)
                .toasts()
                .last()
                .map(|t| t.title.to_string())
        })
    }

    fn page<R>(&mut self, f: impl FnOnce(&page::Page) -> R) -> R {
        self.grid
            .read_with(&self.cx, |grid, _| f(grid.page.as_ref().unwrap()))
    }

    fn editing(&mut self) -> Option<Cell> {
        self.grid.read_with(&self.cx, |grid, _| grid.editing_cell())
    }

    fn take_events(&mut self) -> Vec<GridEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::default()
    }
}

// ── Selection and copying ─────────────────────────────────────────────────

#[gpui::test]
fn click_shift_click_and_copy_tsv(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 20, read_only());
    h.click(Cell::new(1, 0), Modifiers::none());
    assert_eq!(h.region(), (vec![1], 0..1));
    h.click(Cell::new(3, 1), shift());
    assert_eq!(h.region(), (vec![1, 2, 3], 0..2));
    h.keys("cmd-c");
    assert_eq!(h.clipboard(), "1\tname 1\n2\tname 2\n3\tname 3\n");
    assert_eq!(h.last_toast().as_deref(), Some("Copied 3 rows"));
    // One cell: its text, "Copied value"; a NULL cell copies as nothing.
    h.click(Cell::new(3, 3), Modifiers::none());
    h.keys("cmd-c");
    assert_eq!(h.clipboard(), "");
    assert_eq!(h.last_toast().as_deref(), Some("Copied value"));
}

#[gpui::test]
fn drag_selects_a_range(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 20, read_only());
    let from = h.cell_at(Cell::new(2, 1));
    let to = h.cell_at(Cell::new(5, 3));
    h.cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    h.cx.run_until_parked();
    h.cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    h.cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    assert_eq!(h.region(), (vec![2, 3, 4, 5], 1..4));
    h.grid
        .read_with(&h.cx, |grid, _| assert!(grid.drag.is_none()));
}

#[gpui::test]
fn keyboard_selection_cmd_a_and_esc(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 20, read_only());
    // No active cell: arrows do nothing (as in the React grid).
    h.keys("down");
    assert_eq!(h.page(|p| p.selection.active()), None);
    h.click(Cell::new(0, 0), Modifiers::none());
    h.keys("down right");
    assert_eq!(h.region(), (vec![1], 1..2));
    h.keys("shift-down shift-down shift-right");
    assert_eq!(h.region(), (vec![1, 2, 3], 1..3));
    h.keys("escape");
    assert_eq!(h.region(), (vec![3], 2..3));
    h.keys("cmd-a");
    h.page(|page| {
        let r = page.selection.region(20, 4).unwrap();
        assert_eq!(r.rows, RegionRows::Span(0..20));
        assert_eq!(r.cols, 0..4);
    });
    h.keys("cmd-c");
    let text = h.clipboard();
    assert_eq!(text.lines().count(), 20 + 6, "multi-line notes are quoted");
    assert!(text.starts_with("0\tname 0\t\"{\"\"n\"\": 0,"), "{text:?}");
    assert_eq!(h.last_toast().as_deref(), Some("Copied 20 rows"));
    h.keys("escape");
    assert_eq!(h.region(), (vec![3], 2..3));
}

#[gpui::test]
fn row_numbers_pick_rows(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 20, read_only());
    let r2 = h.row_number_at(2);
    h.cx.simulate_click(r2, Modifiers::none());
    assert_eq!(h.region(), (vec![2], 0..4));
    let r4 = h.row_number_at(4);
    h.cx.simulate_click(r4, shift());
    assert_eq!(h.region(), (vec![2, 3, 4], 0..4));
    let r3 = h.row_number_at(3);
    h.cx.simulate_click(r3, Modifiers::command());
    assert_eq!(h.region(), (vec![2, 4], 0..4));
    // Dragging over the numbers picks a run.
    let r6 = h.row_number_at(6);
    let r8 = h.row_number_at(8);
    h.cx.simulate_mouse_down(r6, MouseButton::Left, Modifiers::none());
    h.cx.run_until_parked();
    h.cx.simulate_mouse_move(r8, Some(MouseButton::Left), Modifiers::none());
    h.cx.simulate_mouse_up(r8, MouseButton::Left, Modifiers::none());
    assert_eq!(h.region(), (vec![6, 7, 8], 0..4));
}

#[gpui::test]
fn paging_down_scrolls_100k_rows(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 100_000, read_only());
    h.click(Cell::new(0, 0), Modifiers::none());
    for _ in 0..50 {
        h.cx.simulate_keystrokes("pagedown");
    }
    h.cx.run_until_parked();
    let (active, offset_y, visible, viewport) = h.grid.read_with(&h.cx, |grid, _| {
        let (_, offset) = grid.scroll_state();
        (
            grid.page.as_ref().unwrap().selection.active().unwrap(),
            f32::from(offset.y),
            grid.visible_rows(),
            grid.frame.viewport_height,
        )
    });
    assert_eq!(active.row, 50 * visible);
    // The active row is in view: scrolled down to it.
    assert!((active.row as f32) * ROW_HEIGHT >= -offset_y, "{offset_y}");
    assert!(
        (active.row as f32 + 1.) * ROW_HEIGHT <= -offset_y + viewport + 0.5,
        "{offset_y}"
    );
}

#[gpui::test]
fn column_resize_by_dragging_the_edge_clamps(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 5, read_only());
    let (edge, width) = h.grid.update(&mut h.cx, |grid, _| {
        let (bounds, _) = grid.scroll_state();
        let page = grid.page.as_ref().unwrap();
        let x = bounds.origin.x + px(page.layout.numbers_width() + page.layout.start(2));
        let y = bounds.origin.y - px(HEADER_HEIGHT / 2.);
        (point(x, y), page.layout.width(1))
    });
    h.cx.simulate_mouse_down(edge, MouseButton::Left, Modifiers::none());
    h.cx.run_until_parked();
    h.cx.simulate_mouse_move(
        edge + point(px(40.), px(0.)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    assert_eq!(h.page(|p| p.layout.width(1)), width + 40.);
    let far = edge - point(px(500.), px(0.));
    h.cx.simulate_mouse_move(far, Some(MouseButton::Left), Modifiers::none());
    h.cx.simulate_mouse_up(far, MouseButton::Left, Modifiers::none());
    assert_eq!(h.page(|p| p.layout.width(1)), MIN_WIDTH);
    h.grid.read_with(&h.cx, |grid, _| {
        assert!(grid.resize.is_none());
        // Resizing neither selected nor sorted anything.
        let page = grid.page.as_ref().unwrap();
        assert_eq!(page.selection.active(), None);
        assert_eq!(page.sort(), None);
    });
}

// ── Context menu and Copy as ──────────────────────────────────────────────

#[gpui::test]
fn every_copy_as_format_via_its_menu_action(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 5, read_only());
    h.click(Cell::new(0, 0), Modifiers::none());
    h.click(Cell::new(1, 1), shift());
    let expect = [
        (
            CopyFormat::Tsv,
            "0\tname 0\n1\tname 1\n",
            "Copied 2 rows as TSV",
        ),
        (
            CopyFormat::TsvHeader,
            "id\tname\n0\tname 0\n1\tname 1\n",
            "Copied 2 rows as TSV",
        ),
        (
            CopyFormat::Csv,
            "id,name\n0,name 0\n1,name 1\n",
            "Copied 2 rows as CSV",
        ),
        (
            CopyFormat::Markdown,
            "| id | name |\n| ---: | --- |\n| 0 | name 0 |\n| 1 | name 1 |\n",
            "Copied 2 rows as Markdown",
        ),
        (
            CopyFormat::Json,
            "[\n  {\n    \"id\": 0,\n    \"name\": \"name 0\"\n  },\n  {\n    \"id\": 1,\n    \"name\": \"name 1\"\n  }\n]\n",
            "Copied 2 rows as JSON",
        ),
        (
            CopyFormat::SqlInsert,
            "INSERT INTO MY_TABLE (id, name) VALUES (0, 'name 0');\nINSERT INTO MY_TABLE (id, name) VALUES (1, 'name 1');\n",
            "Copied 2 rows as INSERT statements",
        ),
    ];
    for (format, text, title) in expect {
        h.cx.dispatch_action(CopyAs(format));
        h.cx.run_until_parked();
        assert_eq!(h.clipboard(), text, "{format:?}");
        assert_eq!(h.last_toast().as_deref(), Some(title), "{format:?}");
    }
    // IN list: one column only; two columns fail with an error toast.
    h.cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    h.cx.dispatch_action(CopyAs(CopyFormat::SqlIn));
    h.cx.run_until_parked();
    assert_eq!(h.clipboard(), "before");
    assert_eq!(h.last_toast().as_deref(), Some("Copy failed"));
    h.click(Cell::new(0, 1), Modifiers::none());
    h.click(Cell::new(2, 1), shift());
    h.cx.dispatch_action(CopyAs(CopyFormat::SqlIn));
    h.cx.run_until_parked();
    assert_eq!(h.clipboard(), "'name 0', 'name 1', 'name 2'\n");
    assert_eq!(
        h.last_toast().as_deref(),
        Some("Copied 3 values as an SQL IN list")
    );
    // UPDATE: a table tab's table and key.
    h.grid.update(&mut h.cx, |grid, cx| {
        grid.set_table_context(
            Some(TableContext {
                table: "public.t".into(),
                pk: vec!["id".into()],
                sort: Vec::new(),
                defaults: Vec::new(),
            }),
            cx,
        )
    });
    h.cx.dispatch_action(CopyAs(CopyFormat::SqlUpdate));
    h.cx.run_until_parked();
    assert_eq!(
        h.clipboard(),
        "UPDATE public.t SET name = 'name 0' WHERE id = 0;\n\
         UPDATE public.t SET name = 'name 1' WHERE id = 1;\n\
         UPDATE public.t SET name = 'name 2' WHERE id = 2;\n"
    );
    assert_eq!(
        h.last_toast().as_deref(),
        Some("Copied 3 rows as UPDATE statements")
    );
    // INSERT now targets the table.
    h.cx.dispatch_action(CopyAs(CopyFormat::SqlInsert));
    h.cx.run_until_parked();
    assert!(
        h.clipboard()
            .starts_with("INSERT INTO public.t (name) VALUES ('name 0');")
    );
}

#[gpui::test]
fn right_click_opens_the_menu_and_copy_runs_from_it(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, read_only());
    // Right-click outside the selection selects the cell under the mouse.
    let at = h.cell_at(Cell::new(4, 1));
    h.cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    h.cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    h.cx.run_until_parked();
    h.cx.update(|window, _| window.refresh());
    h.cx.run_until_parked();
    assert_eq!(h.region(), (vec![4], 1..2));
    let model = h
        .grid
        .read_with(&h.cx, |grid, _| grid.menu_model().unwrap());
    assert!(model.has_shape);
    let disabled: Vec<CopyFormat> = model
        .formats
        .iter()
        .filter(|(_, d, _)| *d)
        .map(|(f, _, _)| *f)
        .collect();
    assert_eq!(disabled, [CopyFormat::SqlUpdate], "no table: no UPDATE");
    // The first item is Copy.
    h.keys("down enter");
    assert_eq!(h.clipboard(), "name 4");
    assert_eq!(h.last_toast().as_deref(), Some("Copied value"));
    // Inside a range, a right-click keeps the range.
    h.click(Cell::new(1, 0), Modifiers::none());
    h.click(Cell::new(3, 2), shift());
    let inside = h.cell_at(Cell::new(2, 1));
    h.cx.simulate_mouse_down(inside, MouseButton::Right, Modifiers::none());
    h.cx.simulate_mouse_up(inside, MouseButton::Right, Modifiers::none());
    h.cx.run_until_parked();
    assert_eq!(h.region(), (vec![1, 2, 3], 0..3));
    h.keys("escape");
}

// ── Sorting ───────────────────────────────────────────────────────────────

#[gpui::test]
fn header_clicks_sort_a_console_result_locally(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 12, read_only());
    let header = h.header_at(0);
    h.cx.simulate_click(header, Modifiers::none());
    assert_eq!(
        h.page(|p| p.sort()),
        Some(sort::LocalSort {
            col: 0,
            descending: false
        })
    );
    assert_eq!(h.page(|p| p.value(11, 0).map(str::to_owned)), s("11"));
    h.cx.simulate_click(header, Modifiers::none());
    assert_eq!(h.page(|p| p.value(0, 0).map(str::to_owned)), s("11"));
    // Row numbers stay 1..n in view order.
    assert_eq!(h.page(|p| p.row_label(0)), "1");
    h.cx.simulate_click(header, Modifiers::none());
    assert_eq!(h.page(|p| p.sort()), None);
    assert_eq!(h.page(|p| p.value(0, 0).map(str::to_owned)), s("0"));
    assert!(h.take_events().is_empty(), "consoles never ask the server");
}

#[gpui::test]
fn table_tab_header_clicks_ask_for_a_server_sort(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 5, editable());
    h.grid.update(&mut h.cx, |grid, cx| {
        grid.set_table_context(
            Some(TableContext {
                table: "public.t".into(),
                pk: vec!["id".into()],
                sort: vec![(
                    "name".into(),
                    ColumnSort {
                        dir: SortDir::Desc,
                        rank: 1,
                    },
                )],
                defaults: Vec::new(),
            }),
            cx,
        )
    });
    h.cx.run_until_parked();
    let header = h.header_at(1);
    h.cx.simulate_click(header, Modifiers::none());
    let header = h.header_at(3);
    h.cx.simulate_click(header, shift());
    assert_eq!(
        h.take_events(),
        [
            GridEvent::ServerSortRequested {
                column: 1,
                additive: false
            },
            GridEvent::ServerSortRequested {
                column: 3,
                additive: true
            },
        ]
    );
    // Rows stay in the query's order.
    assert_eq!(h.page(|p| p.sort()), None);
}

// ── JSON viewer ───────────────────────────────────────────────────────────

#[gpui::test]
fn json_viewer_opens_on_enter_and_double_click_and_closes(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 5, read_only());
    // Enter on a JSON cell of a read-only grid opens the viewer.
    h.click(Cell::new(2, 2), Modifiers::none());
    h.keys("enter");
    let viewer = h
        .grid
        .read_with(&h.cx, |g, _| g.json_viewer().cloned())
        .expect("viewer open");
    viewer.read_with(&h.cx, |v, _| {
        assert_eq!(v.target().column, "payload");
        assert_eq!(v.target().row, 3);
        assert_eq!(v.mode(), Mode::Tree);
        let tree = v.tree().unwrap();
        assert_eq!(tree.lines().len(), 7, "{{ n, tags [ a, b ] }}");
    });
    // Collapse all, then Text mode, Copy.
    viewer.update(&mut h.cx, |v, cx| {
        v.set_all(false, cx);
        v.set_mode(Mode::Text, cx);
        v.copy(cx);
    });
    h.cx.run_until_parked();
    viewer.read_with(&h.cx, |v, _| assert_eq!(v.tree().unwrap().lines().len(), 1));
    assert!(h.clipboard().starts_with("{\n  \"n\": 2,"));
    assert_eq!(h.last_toast().as_deref(), Some("Copied JSON"));
    // Esc closes it and the grid has the keyboard again.
    h.keys("escape");
    h.grid
        .read_with(&h.cx, |g, _| assert!(g.json_viewer().is_none()));
    h.keys("down");
    assert_eq!(h.region(), (vec![3], 2..3));
    // Double-click opens it too; Enter on a non-JSON cell does nothing.
    h.double_click(Cell::new(1, 2));
    assert!(h.grid.read_with(&h.cx, |g, _| g.json_viewer().is_some()));
    h.keys("escape");
    h.click(Cell::new(1, 1), Modifiers::none());
    h.keys("enter");
    assert!(h.grid.read_with(&h.cx, |g, _| g.json_viewer().is_none()));
    // The braces button's action.
    h.click(Cell::new(0, 2), Modifiers::none());
    h.cx.dispatch_action(ViewJson);
    h.cx.run_until_parked();
    assert!(h.grid.read_with(&h.cx, |g, _| g.json_viewer().is_some()));
}

// ── Value editor ──────────────────────────────────────────────────────────

#[gpui::test]
fn value_editor_saves_with_cmd_enter(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 5, editable());
    h.click(Cell::new(1, 1), Modifiers::none());
    h.keys("shift-enter");
    let editor = h
        .grid
        .read_with(&h.cx, |g, _| g.value_editor().cloned())
        .expect("value editor open");
    editor.read_with(&h.cx, |e, cx| {
        assert_eq!(e.target().column, "name");
        assert_eq!(e.text(cx), "name 1");
    });
    h.keys("cmd-a");
    h.cx.simulate_input("renamed\nover two lines");
    h.keys("cmd-enter");
    assert!(h.grid.read_with(&h.cx, |g, _| g.value_editor().is_none()));
    assert_eq!(
        h.page(|p| p.value(1, 1).map(str::to_owned)),
        s("renamed\nover two lines")
    );
    assert!(h.take_events().contains(&GridEvent::PendingChanged));
    // Multi-line, JSON and long values open it on Enter; Esc cancels.
    h.click(Cell::new(2, 3), Modifiers::none());
    h.keys("enter");
    assert!(h.grid.read_with(&h.cx, |g, _| g.value_editor().is_some()));
    assert_eq!(h.editing(), None);
    h.keys("escape");
    assert!(h.grid.read_with(&h.cx, |g, _| g.value_editor().is_none()));
    h.click(Cell::new(0, 2), Modifiers::none());
    h.keys("enter");
    let editor = h
        .grid
        .read_with(&h.cx, |g, _| g.value_editor().cloned())
        .expect("JSON opens the value editor in the data editor");
    assert!(editor.read_with(&h.cx, |e, _| e.target().json));
    // Set NULL, then apply: NULL, not ''.
    editor.update(&mut h.cx, |e, cx| {
        e.set_null(cx);
        e.apply(cx);
    });
    h.cx.run_until_parked();
    assert_eq!(h.page(|p| p.value(0, 2).map(str::to_owned)), None);
    assert!(h.page(|p| p.cell_edited(Cell::new(0, 2))));
    // Applying an untouched value stages nothing.
    let before = h.page(|p| p.pending.clone());
    h.click(Cell::new(4, 1), Modifiers::none());
    h.keys("shift-enter");
    h.keys("cmd-enter");
    assert_eq!(h.page(|p| p.pending.clone()), before);
}

// ── Editing shortcuts ─────────────────────────────────────────────────────

#[gpui::test]
fn enter_edits_and_stages_the_change(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, editable());
    let cell = Cell::new(2, 1);
    h.click(cell, Modifiers::none());
    h.keys("enter");
    assert_eq!(h.editing(), Some(cell));
    // The value is selected: typing replaces it.
    h.cx.simulate_input("bob");
    h.keys("enter");
    assert_eq!(h.editing(), None);
    h.page(|page| {
        assert_eq!(page.value(2, 1), Some("bob"));
        assert!(page.cell_edited(cell));
        assert_eq!(page.pending.updates[&2][&1], s("bob"));
        assert_eq!(page.row_mark(2), Some(RowMark::Updated));
    });
    assert_eq!(h.take_events(), [GridEvent::PendingChanged]);
    // Escape cancels.
    h.keys("enter");
    h.cx.simulate_input("zzz");
    h.keys("escape");
    assert_eq!(h.editing(), None);
    assert_eq!(h.page(|p| p.value(2, 1).map(str::to_owned)), s("bob"));
    // F2 edits too; closing without typing stages nothing.
    h.click(Cell::new(5, 1), Modifiers::none());
    h.keys("f2");
    assert_eq!(h.editing(), Some(Cell::new(5, 1)));
    h.keys("enter");
    assert_eq!(h.page(|p| p.pending.change_count()), 1);
    assert!(h.take_events().is_empty());
}

#[gpui::test]
fn double_click_typing_and_tab(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, editable());
    let cell = Cell::new(1, 1);
    h.double_click(cell);
    assert_eq!(h.editing(), Some(cell));
    // Tab commits and moves right; ⇧Tab moves left.
    h.cx.simulate_input("y");
    h.keys("tab");
    assert_eq!(h.editing(), None);
    assert_eq!(h.page(|p| p.value(1, 1).map(str::to_owned)), s("y"));
    assert_eq!(h.page(|p| p.selection.active()), Some(Cell::new(1, 2)));
    h.keys("left");
    // Typing on a selected cell starts editing with that character.
    h.keys("q");
    assert_eq!(h.editing(), Some(Cell::new(1, 1)));
    h.cx.simulate_input("rs");
    h.keys("shift-tab");
    assert_eq!(h.page(|p| p.value(1, 1).map(str::to_owned)), s("qrs"));
    assert_eq!(h.page(|p| p.selection.active()), Some(Cell::new(1, 0)));
}

#[gpui::test]
fn each_editing_shortcut_stages_its_change(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, editable());
    h.click(Cell::new(3, 1), Modifiers::none());
    // ⌘⌥N: NULL.
    h.keys("cmd-alt-n");
    h.page(|p| {
        assert_eq!(p.pending.updates[&3][&1], None);
        assert!(p.cell_edited(Cell::new(3, 1)));
    });
    // ⌘⌥Z: revert the cell.
    h.keys("cmd-alt-z");
    assert!(h.page(|p| p.pending.is_empty()));
    // ⌘⌫ and Delete: delete the row.
    h.keys("cmd-backspace");
    assert_eq!(h.page(|p| p.row_mark(3)), Some(RowMark::Deleted));
    assert!(h.page(|p| p.pending.deletes.contains(&3)));
    // A deleted row can't be edited; ⌘⌥Z on it brings it back.
    h.keys("enter");
    assert_eq!(h.editing(), None);
    h.keys("cmd-alt-z");
    assert_eq!(h.page(|p| p.row_mark(3)), None);
    h.keys("down delete");
    assert!(h.page(|p| p.pending.deletes.contains(&4)));
    // ⌘N: a new row at the end, selected; untouched columns are NULL (no
    // table context: no column has a default).
    h.keys("cmd-n");
    h.page(|p| {
        assert_eq!(p.pending.inserts.len(), 1);
        assert_eq!(p.row_count(), 11);
        assert_eq!(p.selection.active(), Some(Cell::new(10, 0)));
        assert_eq!(p.row_ref(10), Some(RowRef::New(1)));
        assert!(!p.is_default(10, 1));
        assert_eq!(p.shown(10, 1), page::Shown::Null);
        assert_eq!(p.row_label(10), "+");
    });
    // Alt+Insert adds another.
    h.keys("alt-insert");
    assert_eq!(h.page(|p| p.pending.inserts.len()), 2);
    // Typing into a new row sets the column.
    h.keys("right x enter");
    assert_eq!(h.page(|p| p.value(11, 1).map(str::to_owned)), s("x"));
    // Deleting a new row drops it.
    h.keys("cmd-backspace");
    assert_eq!(h.page(|p| p.pending.inserts.len()), 1);
    let changes = h
        .take_events()
        .iter()
        .filter(|e| **e == GridEvent::PendingChanged)
        .count();
    assert_eq!(changes, 9);
    // ⌘↵: submit.
    h.keys("cmd-enter");
    assert_eq!(h.take_events(), [GridEvent::SubmitRequested]);
    // The owner takes the edits.
    let taken = h.grid.update(&mut h.cx, |g, cx| g.take_pending(cx));
    assert_eq!(taken.inserts.len(), 1);
    assert!(taken.deletes.contains(&4));
    assert!(h.page(|p| p.pending.is_empty()));
    assert_eq!(h.page(|p| p.row_count()), 10);
}

#[gpui::test]
fn read_only_content_blocks_every_edit(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, read_only());
    h.click(Cell::new(3, 1), Modifiers::none());
    h.keys("enter");
    assert_eq!(h.editing(), None);
    h.keys("f2 q shift-enter");
    assert_eq!(h.editing(), None);
    assert!(h.grid.read_with(&h.cx, |g, _| g.value_editor().is_none()));
    h.keys("cmd-alt-n cmd-backspace delete cmd-n alt-insert");
    h.double_click(Cell::new(3, 1));
    assert_eq!(h.editing(), None);
    assert!(h.page(|p| p.pending.is_empty()));
    assert_eq!(h.page(|p| p.row_count()), 10);
    h.keys("cmd-enter");
    assert!(h.take_events().is_empty());
    // The menu has no edit items.
    let model = h.grid.read_with(&h.cx, |g, _| g.menu_model().unwrap());
    assert!(model.editing_is_none());
    assert_eq!(
        h.grid.read_with(&h.cx, |g, _| g.read_only_reason()),
        Some("Read-only: console results")
    );
    // …which the status strip doesn't show: neither app says why console
    // results are read-only (USAGE.md "Editing table data").
    assert_eq!(
        h.grid.read_with(&h.cx, |g, _| g.shown_read_only_reason()),
        None
    );
}

// ── The contract ──────────────────────────────────────────────────────────

#[gpui::test]
fn set_content_running_error_and_pending(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, editable());
    h.click(Cell::new(2, 1), Modifiers::none());
    h.keys("cmd-alt-n");
    h.take_events();
    // The same result again (e.g. new editability): edits and selection stay.
    let same = h.page(|p| p.result.clone());
    h.grid.update(&mut h.cx, |g, cx| {
        g.set_content(
            GridContent {
                sql: "select * from t".into(),
                result: same,
                offset: 0,
                page_size: 500,
                editability: editable(),
            },
            cx,
        )
    });
    assert_eq!(h.page(|p| p.pending.change_count()), 1);
    assert_eq!(h.page(|p| p.selection.active()), Some(Cell::new(2, 1)));
    // set_running: shown, edits ignored.
    h.grid.update(&mut h.cx, |g, cx| g.set_running(true, cx));
    h.keys("cmd-alt-z enter");
    assert_eq!(h.page(|p| p.pending.change_count()), 1);
    assert_eq!(h.editing(), None);
    assert!(h.grid.read_with(&h.cx, |g, _| g.is_running()));
    // A new page: edits start over (announced), offset in row numbers.
    let mut next = content(result(3), editable());
    next.offset = 500;
    h.grid.update(&mut h.cx, |g, cx| g.set_content(next, cx));
    assert!(!h.grid.read_with(&h.cx, |g, _| g.is_running()));
    assert!(h.page(|p| p.pending.is_empty()));
    assert_eq!(h.page(|p| p.row_label(0)), "501");
    assert_eq!(h.take_events(), [GridEvent::PendingChanged]);
    // Restoring edits for it (owner switching tabs back).
    let mut pending = boa_core::grid::grid_edits::Pending::default();
    pending.add_row();
    h.grid
        .update(&mut h.cx, |g, cx| g.set_pending(pending.clone(), cx));
    assert_eq!(h.grid.read_with(&h.cx, |g, _| g.pending()), pending);
    h.grid.update(&mut h.cx, |g, cx| g.clear_pending(cx));
    assert!(h.grid.read_with(&h.cx, |g, _| g.pending().is_empty()));
    // An error replaces the rows until cleared.
    h.grid.update(&mut h.cx, |g, cx| {
        g.set_run_error(Some(RunError::plain("boom")), cx)
    });
    assert_eq!(
        h.grid
            .read_with(&h.cx, |g, _| g.error().map(|e| e.message.clone())),
        Some("boom".into())
    );
    h.grid.update(&mut h.cx, |g, cx| g.set_run_error(None, cx));
    assert!(h.grid.read_with(&h.cx, |g, _| g.error().is_none()));
    // A statement without rows: its notice, no page.
    let mut dml = result(0);
    dml.columns.clear();
    dml.affected = Some(3);
    h.grid.update(&mut h.cx, |g, cx| {
        g.set_content(content(dml, read_only()), cx)
    });
    h.grid.read_with(&h.cx, |g, _| {
        assert!(g.page().is_none());
        assert_eq!(g.notice.as_deref(), Some("3 rows affected in 1 ms"));
    });
}

#[gpui::test]
fn the_pager_asks_for_pages(cx: &mut TestAppContext) {
    let mut r = result(5);
    r.truncated = true;
    let mut c = content(r, read_only());
    c.page_size = 5;
    c.offset = 5;
    let mut h = Harness::open_with(cx, c);
    let next =
        h.cx.debug_bounds("page-next")
            .expect("pager shown")
            .center();
    h.cx.simulate_click(next, Modifiers::none());
    let prev = h.cx.debug_bounds("page-prev").unwrap().center();
    h.cx.simulate_click(prev, Modifiers::none());
    assert_eq!(
        h.take_events(),
        [
            GridEvent::PageRequested { offset: 10 },
            GridEvent::PageRequested { offset: 0 },
        ]
    );
    // Not while running.
    h.grid.update(&mut h.cx, |g, cx| g.set_running(true, cx));
    h.cx.run_until_parked();
    let next = h.cx.debug_bounds("page-next").unwrap().center();
    h.cx.simulate_click(next, Modifiers::none());
    assert!(h.take_events().is_empty());
}

#[gpui::test]
fn new_rows_show_default_only_in_columns_with_a_default(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 3, editable());
    h.grid.update(&mut h.cx, |grid, cx| {
        grid.set_table_context(
            Some(TableContext {
                table: "public.t".into(),
                pk: vec!["id".into()],
                defaults: vec!["id".into(), "note".into()],
                ..TableContext::default()
            }),
            cx,
        )
    });
    h.cx.run_until_parked();
    h.click(Cell::new(0, 0), Modifiers::none());
    h.keys("cmd-n");
    h.page(|p| {
        // TableEdits.tsx `placeholder`: <default> where the column has one.
        assert_eq!(p.shown(3, 0), page::Shown::Default);
        assert_eq!(p.shown(3, 1), page::Shown::Null);
        assert_eq!(p.shown(3, 2), page::Shown::Null);
        assert_eq!(p.shown(3, 3), page::Shown::Default);
    });
    // A new page keeps the context's defaults.
    h.grid.update(&mut h.cx, |grid, cx| {
        grid.set_content(content(result(2), editable()), cx);
        grid.page.as_mut().unwrap().add_row();
    });
    h.page(|p| {
        assert_eq!(p.shown(2, 0), page::Shown::Default);
        assert_eq!(p.shown(2, 1), page::Shown::Null);
    });
}

#[gpui::test]
fn a_narrow_sorted_key_column_keeps_its_name_arrow_and_rank(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 3, editable());
    h.grid.update(&mut h.cx, |grid, cx| {
        grid.set_table_context(
            Some(TableContext {
                table: "public.t".into(),
                pk: vec!["id".into()],
                sort: vec![
                    (
                        "name".into(),
                        ColumnSort {
                            dir: SortDir::Asc,
                            rank: 1,
                        },
                    ),
                    (
                        "id".into(),
                        ColumnSort {
                            dir: SortDir::Desc,
                            rank: 2,
                        },
                    ),
                ],
                ..TableContext::default()
            }),
            cx,
        )
    });
    h.cx.run_until_parked();
    h.cx.update(|window, _| window.refresh());
    h.cx.run_until_parked();
    // "id int4": 102 px, too narrow for key + name + type + arrow + rank.
    assert_eq!(h.page(|p| p.layout.width(0)), 102.);
    let cell = h.cx.debug_bounds("grid-header-0").expect("the header cell");
    let name = h.cx.debug_bounds("grid-header-0-name").expect("the name");
    let sort = h.cx.debug_bounds("grid-header-0-sort").expect("the arrow");
    let rank = h.cx.debug_bounds("grid-header-0-rank").expect("the rank");
    // The name keeps two characters' room ("id", or "i…"), not a lone dot…
    assert!(
        f32::from(name.size.width) >= 15.5,
        "name squeezed to {:?}",
        name.size.width
    );
    // …and the arrow and rank stay inside the cell (the type gives way).
    assert!(sort.right() <= cell.right(), "{sort:?} outside {cell:?}");
    assert!(rank.right() <= cell.right(), "{rank:?} outside {cell:?}");
    assert!(sort.left() >= name.right());
    // A wide column keeps everything at its natural size.
    let ty = h.cx.debug_bounds("grid-header-1-type").expect("the type");
    assert!(f32::from(ty.size.width) >= 4. * 6.6 - 0.5, "{ty:?}");
}

#[gpui::test]
fn the_row_filter_hides_rows_and_survives_new_pages(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 12, editable());
    h.grid
        .update(&mut h.cx, |grid, cx| grid.set_row_filter("NAME 1", cx));
    h.cx.run_until_parked();
    // name 1, name 10, name 11, numbered from 1.
    h.page(|p| {
        assert_eq!(p.row_count(), 3);
        assert_eq!(p.row_ref(1), Some(RowRef::Base(10)));
        assert_eq!(p.row_label(1), "2");
    });
    // Copy works on what is shown.
    h.click(Cell::new(2, 1), Modifiers::none());
    h.keys("cmd-c");
    assert_eq!(h.clipboard(), "name 11");
    // Another page (a reload): still filtered.
    h.grid.update(&mut h.cx, |grid, cx| {
        let mut c = content(result(20), editable());
        c.offset = 500;
        grid.set_content(c, cx)
    });
    h.page(|p| {
        assert_eq!(p.base_count(), 11);
        assert_eq!(p.row_label(0), "1");
    });
    assert_eq!(
        h.grid.read_with(&h.cx, |g, _| g.row_filter().to_owned()),
        "NAME 1"
    );
    h.grid
        .update(&mut h.cx, |grid, cx| grid.set_row_filter("", cx));
    h.page(|p| {
        assert_eq!(p.row_count(), 20);
        assert_eq!(p.row_label(0), "501");
    });
}

/// DataGrid.tsx takes ⌘ or Ctrl for the edit keys (`e.metaKey || e.ctrlKey`)
/// and for ⌘A / ⌘C.
#[gpui::test]
fn ctrl_works_like_cmd_for_the_grid_keys(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, editable());
    h.click(Cell::new(3, 1), Modifiers::none());
    h.keys("ctrl-alt-n");
    assert_eq!(h.page(|p| p.pending.updates[&3][&1].clone()), None);
    h.keys("ctrl-alt-z");
    assert!(h.page(|p| p.pending.is_empty()));
    h.keys("ctrl-backspace");
    assert_eq!(h.page(|p| p.row_mark(3)), Some(RowMark::Deleted));
    h.keys("cmd-alt-z");
    h.keys("ctrl-n");
    assert_eq!(h.page(|p| p.pending.inserts.len()), 1);
    h.take_events();
    h.keys("ctrl-enter");
    assert_eq!(h.take_events(), [GridEvent::SubmitRequested]);
    h.click(Cell::new(2, 1), Modifiers::none());
    h.keys("ctrl-c");
    assert_eq!(h.clipboard(), "name 2");
    h.keys("ctrl-a");
    assert_eq!(h.region().0.len(), 11, "every row, the new one too");
}

/// In the cell input: ⇧↵ swaps it for the value editor, ⌘⌥N sets NULL
/// (DataGrid.tsx cell editor `onKeyDown`).
#[gpui::test]
fn shift_enter_and_cmd_alt_n_inside_the_cell_input(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, editable());
    let cell = Cell::new(2, 1);
    h.click(cell, Modifiers::none());
    h.keys("enter");
    assert_eq!(h.editing(), Some(cell));
    h.keys("shift-enter");
    assert_eq!(h.editing(), None);
    let editor = h
        .grid
        .read_with(&h.cx, |g, _| g.value_editor().cloned())
        .expect("the value editor took over");
    editor.read_with(&h.cx, |e, _| assert_eq!(e.target().column, "name"));
    h.keys("escape");
    assert!(h.grid.read_with(&h.cx, |g, _| g.value_editor().is_none()));
    h.keys("enter");
    assert_eq!(h.editing(), Some(cell));
    h.keys("cmd-alt-n");
    assert_eq!(h.editing(), None);
    h.page(|p| {
        assert_eq!(p.value(2, 1), None);
        assert!(p.cell_edited(cell));
    });
}

/// The right-click menu's edit section on a table tab's grid (DataGrid.tsx
/// `editMenu`): each item does what its shortcut does. Picked with ↓, which
/// skips disabled items, so the positions also check the enable rules
/// (Revert cell / Revert row only with something to revert).
#[gpui::test]
fn the_menus_edit_items_each_do_their_edit(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 10, editable());
    let menu = |h: &mut Harness, cell: Cell, n: usize| {
        let at = h.cell_at(cell);
        h.cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
        h.cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
        h.cx.run_until_parked();
        h.cx.update(|window, _| window.refresh());
        h.cx.run_until_parked();
        let open = h.cx.update(|window, _| {
            window
                .context_stack()
                .iter()
                .any(|c| c.contains("PopupMenu"))
        });
        assert!(open, "the menu opens");
        for _ in 0..n {
            h.keys("down");
        }
        h.keys("enter");
    };
    let cell = Cell::new(1, 1);
    // Copy, Copy as ›, Edit, Open in editor, Set NULL, Add row, Delete row,
    // (Revert cell), (Revert row), Preview and submit….
    menu(&mut h, cell, 3);
    assert_eq!(h.editing(), Some(cell), "Edit");
    h.keys("escape");
    menu(&mut h, cell, 4);
    assert!(
        h.grid.read_with(&h.cx, |g, _| g.value_editor().is_some()),
        "Open in editor"
    );
    h.keys("escape");
    menu(&mut h, cell, 5);
    assert_eq!(
        h.page(|p| p.value(1, 1).map(str::to_owned)),
        None,
        "Set NULL"
    );
    // Now Revert cell and Revert row are there.
    menu(&mut h, cell, 8);
    assert!(h.page(|p| p.pending.is_empty()), "Revert cell");
    menu(&mut h, cell, 5);
    menu(&mut h, cell, 9);
    assert!(h.page(|p| p.pending.is_empty()), "Revert row");
    menu(&mut h, cell, 6);
    assert_eq!(h.page(|p| p.pending.inserts.len()), 1, "Add row");
    menu(&mut h, Cell::new(2, 1), 7);
    assert_eq!(
        h.page(|p| p.row_mark(2)),
        Some(RowMark::Deleted),
        "Delete row"
    );
    h.take_events();
    menu(&mut h, Cell::new(3, 1), 8);
    assert_eq!(
        h.take_events(),
        [GridEvent::SubmitRequested],
        "Preview and submit…"
    );
}

/// DataGrid.tsx `copyMenu`: with nothing selected the "Copy as" trigger is
/// disabled (so ↓ skips it, like Copy); with a selection a format that
/// can't copy it is disabled and says why (`title={why}`).
#[gpui::test]
fn copy_as_is_off_without_a_selection_and_formats_say_why(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 0, editable());
    let model = h.grid.read_with(&h.cx, |g, _| g.menu_model().unwrap());
    assert!(!model.has_shape);
    assert!(
        model.formats.iter().all(|&(_, d, why)| d && why.is_none()),
        "{:?}",
        model.formats
    );
    // Right-click the empty body: Copy, Copy as ›, Edit, Open in editor and
    // Set NULL are disabled, so the first ↓ lands on Add row.
    let at = h.cell_at(Cell::new(0, 1));
    h.cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    h.cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    h.cx.run_until_parked();
    h.cx.update(|window, _| window.refresh());
    h.cx.run_until_parked();
    h.keys("down enter");
    assert_eq!(h.page(|p| p.pending.inserts.len()), 1, "Add row");

    // Two columns of a console result (no table): no UPDATE, no IN list.
    let mut h = Harness::open(cx, 5, read_only());
    h.click(Cell::new(0, 0), Modifiers::none());
    h.click(Cell::new(1, 1), shift());
    let model = h.grid.read_with(&h.cx, |g, _| g.menu_model().unwrap());
    let off: Vec<_> = model
        .formats
        .iter()
        .filter(|(_, d, _)| *d)
        .map(|&(f, _, why)| (f, why))
        .collect();
    assert_eq!(
        off,
        [
            (
                CopyFormat::SqlUpdate,
                Some("Needs a table with a primary key")
            ),
            (CopyFormat::SqlIn, Some("Select a single column")),
        ]
    );
    // The menu (with the tooltip rows) opens and draws.
    let at = h.cell_at(Cell::new(1, 1));
    h.cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    h.cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    h.cx.run_until_parked();
    h.cx.update(|window, cx| window.draw(cx).clear(cx));
    // Copy as › opens its submenu (→), whose first item is TSV.
    h.keys("down down right enter");
    assert_eq!(h.clipboard(), "0\tname 0\n1\tname 1\n");
}

/// The right-click menu shows the keys DataGrid.tsx shows (an action's hint
/// is its last binding in the grid's context, so the Ctrl / ⌦ / F2 /
/// ⌥Insert variants are bound first), and "Revert row" shows ⌘⌥Z: the key
/// of `Revert` (cell, else row), as in the source.
#[gpui::test]
fn the_grid_menu_hints_are_the_sources_keys(cx: &mut TestAppContext) {
    use crate::actions::{AddRow, SubmitEdits};
    let mut h = Harness::open(cx, 3, editable());
    h.click(Cell::new(0, 1), Modifiers::none());
    let focus = h.grid.read_with(&h.cx, |g, cx| g.focus_handle(cx));
    let hint = |h: &mut Harness, action: &dyn gpui::Action| {
        h.cx.update(|window, _| {
            window
                .highest_precedence_binding_for_action_in(action, &focus)
                .map(|b| b.keystrokes()[0].inner().clone())
        })
    };
    let key = |s: &'static str| Some(gpui::Keystroke::parse(s).unwrap());
    assert_eq!(hint(&mut h, &Copy), key("cmd-c"));
    assert_eq!(hint(&mut h, &Edit), key("enter"));
    assert_eq!(hint(&mut h, &EditLarge), key("shift-enter"));
    assert_eq!(hint(&mut h, &SetNull), key("cmd-alt-n"));
    assert_eq!(hint(&mut h, &AddRow), key("cmd-n"));
    assert_eq!(hint(&mut h, &DeleteRow), key("cmd-backspace"));
    assert_eq!(hint(&mut h, &SubmitEdits), key("cmd-enter"));
    assert_eq!(hint(&mut h, &Revert), key(interact::REVERT_ROW_HINT));
    assert_eq!(hint(&mut h, &RevertRow), None, "shown, not bound");
    // ⌘⌥Z reverts the cell first, then the row (DataGrid.tsx `revert`).
    h.keys("cmd-alt-n");
    h.click(Cell::new(0, 2), Modifiers::none());
    h.keys("cmd-alt-n");
    h.keys("cmd-alt-z");
    h.page(|p| {
        assert!(!p.cell_edited(Cell::new(0, 2)));
        assert!(p.cell_edited(Cell::new(0, 1)), "only the cell");
    });
    h.keys("cmd-alt-z");
    assert!(h.page(|p| p.pending.is_empty()), "then the row");
}

/// PgUp / PgDn move by a screen of rows: a GPUI addition (DataGrid.tsx
/// leaves them to the browser, which scrolls the grid), kept as harmless
/// and listed in USAGE.md "Differences".
#[gpui::test]
fn page_down_and_up_move_by_a_screen_of_rows(cx: &mut TestAppContext) {
    let mut h = Harness::open(cx, 200, read_only());
    h.click(Cell::new(0, 1), Modifiers::none());
    let page = h.grid.read_with(&h.cx, |g, _| g.visible_rows());
    assert!(page > 1, "{page}");
    h.keys("pagedown");
    let active = |h: &mut Harness| h.page(|p| p.selection.active());
    assert_eq!(active(&mut h), Some(Cell::new(page, 1)));
    h.keys("pageup");
    assert_eq!(active(&mut h), Some(Cell::new(0, 1)));
}
