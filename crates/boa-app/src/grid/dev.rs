//! Development hooks, driven by environment variables (no UI; `dev-hooks`
//! feature only). The grid is a pure view, so `BOA_GRID_SQL` fetches the
//! result here and feeds it with `set_content`, as an owner would:
//!
//! - `BOA_GRID_SQL=<sql>`: run this statement at startup on a saved
//!   connection of `BOA_CONFIG_DIR` (`BOA_GRID_CONNECTION=<id>`, default the
//!   first; `BOA_GRID_DATABASE`, default the connection's) and show it.
//!   `BOA_GRID_PAGE_SIZE` (default 100,000, the backend maximum, for the
//!   scroll benchmark). `BOA_GRID_EDITABLE=pk1,pk2` makes it editable with
//!   that primary key (table-tab context `BOA_GRID_TABLE`, default
//!   `public.t`), `BOA_GRID_SORT=col[:desc],…` its ORDER BY arrows
//!   and ranks.
//! - `BOA_GRID_SELECT=r0,c0:r1,c1`: once a result is loaded, select that
//!   range (for screenshots).
//! - `BOA_GRID_STAGE=r,c=text;r,c=NULL`: stage edits once a result is loaded;
//!   `BOA_GRID_ADD_ROWS=n` adds new rows, `BOA_GRID_DELETE=r,r` deletes rows.
//! - `BOA_GRID_EDIT=r,c`: open the inline input on that cell once painted.
//! - `BOA_GRID_JSON=r,c` / `BOA_GRID_VALUE=r,c`: open the JSON viewer / the
//!   value editor on that cell once painted.
//! - `BOA_GRID_MENU=r,c`: right-click that cell once painted (the context
//!   menu); `BOA_GRID_MENU_KEYS="down right"` then sends those keys to it.
//! - `BOA_GRID_SCROLL=x,y`: scroll to that pixel offset once loaded.
//! - `BOA_GRID_BENCH=1`: once a result is painted, scroll it programmatically
//!   (several phases) and log frame statistics (`BOA_LOG=info`);
//!   `BOA_GRID_BENCH_QUIT=1` quits afterwards, `BOA_GRID_BENCH_WINDOW=WxH`
//!   resizes the window first.
//! - `BOA_GRID_QUIT_AFTER_MS=n`: log the pid and quit after n ms (memory
//!   measurements with `footprint <pid>`).
//!
//! The benchmark logs, per phase: the interval between frames (what the
//! display link delivered), the time spent building the grid's elements
//! (`render` + the visible rows), and the CPU time from the grid's `render`
//! to the end of its paint (build + layout + prepaint + paint; Metal's GPU
//! work and presentation are not included).

use std::cell::Cell as StdCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use std::sync::Arc;

use boa_core::grid::grid_edits::Editability;
use gpui::{
    Context, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput,
    Window, point, px,
};

use super::selection::Cell;
use super::{GridContent, ResultGrid, RunError, TableContext};
use crate::app_state::AppState;

/// One scripted scroll phase.
#[derive(Debug, Clone, Copy)]
struct Phase {
    name: &'static str,
    frames: usize,
    /// Pixels per frame (x, y); both 0: jump to pseudo-random rows.
    dx: f32,
    dy: f32,
}

const PHASES: [Phase; 4] = [
    Phase {
        name: "scroll down 40 px/frame",
        frames: 600,
        dx: 0.,
        dy: 40.,
    },
    Phase {
        name: "fling down 400 px/frame",
        frames: 300,
        dx: 0.,
        dy: 400.,
    },
    Phase {
        name: "scroll right 30 px/frame",
        frames: 300,
        dx: 30.,
        dy: 0.,
    },
    Phase {
        name: "jump to random rows",
        frames: 120,
        dx: 0.,
        dy: 0.,
    },
];

#[derive(Debug, Default)]
struct Sample {
    interval: Duration,
    build: Duration,
    cpu: Option<Duration>,
}

#[derive(Debug)]
struct BenchRun {
    phase: usize,
    frame: usize,
    last: Instant,
    samples: Vec<Sample>,
    seed: u64,
}

/// Dev-hook state kept in the grid.
#[derive(Debug, Default)]
pub(super) struct DevState {
    startup_sql: Option<String>,
    select: Option<(Cell, Cell)>,
    stage: Vec<(Cell, Option<String>)>,
    add_rows: usize,
    delete: Vec<usize>,
    edit: Option<Cell>,
    json: Option<Cell>,
    value: Option<Cell>,
    menu: Option<Cell>,
    scroll: Option<(f32, f32)>,
    bench: bool,
    quit_after_bench: bool,
    bench_run: Option<BenchRun>,
    /// Element-building time accumulated since the last bench step.
    build_time: Duration,
    /// When the grid's last `render` started and when its paint finished.
    render_started: Option<Instant>,
    paint_done: Rc<StdCell<Option<Instant>>>,
}

fn parse_cell(text: &str) -> Option<Cell> {
    let (r, c) = text.trim().split_once(',')?;
    Some(Cell::new(r.trim().parse().ok()?, c.trim().parse().ok()?))
}

/// `id,name:desc` → the header sorts by column name, ranked in order.
fn parse_sort(text: &str) -> Vec<(String, boa_core::grid::table_query::ColumnSort)> {
    use boa_core::grid::table_query::{ColumnSort, SortDir};
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, item)| {
            let (name, dir) = match item.split_once(':') {
                Some((name, dir)) if dir.eq_ignore_ascii_case("desc") => (name, SortDir::Desc),
                Some((name, _)) => (name, SortDir::Asc),
                None => (item, SortDir::Asc),
            };
            let sort = ColumnSort { dir, rank: i + 1 };
            (name.trim().to_owned(), sort)
        })
        .collect()
}

/// `r0,c0:r1,c1` → the two corners.
fn parse_range(text: &str) -> Option<(Cell, Cell)> {
    let (a, b) = text.split_once(':')?;
    Some((parse_cell(a)?, parse_cell(b)?))
}

/// `r,c=text;r,c=NULL` → cells and values.
fn parse_stage(text: &str) -> Vec<(Cell, Option<String>)> {
    text.split(';')
        .filter_map(|item| {
            let (cell, value) = item.split_once('=')?;
            let value = (value != "NULL").then(|| value.to_owned());
            Some((parse_cell(cell)?, value))
        })
        .collect()
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl DevState {
    pub fn from_env() -> Self {
        Self {
            startup_sql: env("BOA_GRID_SQL"),
            select: env("BOA_GRID_SELECT").and_then(|s| parse_range(&s)),
            stage: env("BOA_GRID_STAGE").map_or_else(Vec::new, |s| parse_stage(&s)),
            add_rows: env("BOA_GRID_ADD_ROWS")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            delete: env("BOA_GRID_DELETE").map_or_else(Vec::new, |s| {
                s.split(',').filter_map(|r| r.trim().parse().ok()).collect()
            }),
            edit: env("BOA_GRID_EDIT").and_then(|s| parse_cell(&s)),
            json: env("BOA_GRID_JSON").and_then(|s| parse_cell(&s)),
            value: env("BOA_GRID_VALUE").and_then(|s| parse_cell(&s)),
            menu: env("BOA_GRID_MENU").and_then(|s| parse_cell(&s)),
            scroll: env("BOA_GRID_SCROLL").and_then(|s| {
                let (x, y) = s.split_once(',')?;
                Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
            }),
            bench: env("BOA_GRID_BENCH").is_some(),
            quit_after_bench: env("BOA_GRID_BENCH_QUIT").is_some(),
            ..Self::default()
        }
    }

    pub fn add_build_time(&mut self, time: Duration) {
        self.build_time += time;
    }

    /// Called at the start of the grid's render.
    pub fn render_started(&mut self) {
        if self.bench_run.is_some() {
            self.render_started = Some(Instant::now());
        }
    }

    /// A handle the grid's paint sets when it is done (only while benchmarking).
    pub fn paint_marker(&self) -> Option<Rc<StdCell<Option<Instant>>>> {
        self.bench_run.as_ref().map(|_| self.paint_done.clone())
    }
}

/// `BOA_GRID_QUIT_AFTER_MS`: quit after a while (called once, from `ResultGrid::new`).
pub(super) fn schedule_quit(cx: &mut Context<ResultGrid>) {
    let Some(ms) = env("BOA_GRID_QUIT_AFTER_MS").and_then(|v| v.trim().parse::<u64>().ok()) else {
        return;
    };
    tracing::info!("grid dev: pid {}, quitting in {ms} ms", std::process::id());
    cx.spawn(async move |_, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// `BOA_GRID_SQL`: fetch the result on a saved connection and feed it to
/// the grid (called once, from `ResultGrid::new`).
pub(super) fn start(grid: &mut ResultGrid, _: &mut Window, cx: &mut Context<ResultGrid>) {
    let Some(sql) = grid.dev.startup_sql.take() else {
        return;
    };
    grid.last_sql = Some(sql.clone());
    grid.running = true;
    let Some(state) = AppState::try_global(cx) else {
        grid.set_run_error(Some(RunError::plain("BOA_GRID_SQL: no AppState")), cx);
        return;
    };
    let backend = state.backend.clone();
    let wanted = env("BOA_GRID_CONNECTION");
    let database = env("BOA_GRID_DATABASE");
    let page_size = env("BOA_GRID_PAGE_SIZE")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(boa_core::backend::MAX_PAGE_ROWS);
    let pk: Option<Vec<String>> =
        env("BOA_GRID_EDITABLE").map(|s| s.split(',').map(|k| k.trim().to_owned()).collect());
    let table = env("BOA_GRID_TABLE").unwrap_or_else(|| "public.t".to_owned());
    let sort = env("BOA_GRID_SORT").map_or_else(Vec::new, |s| parse_sort(&s));
    cx.spawn(async move |this, cx| {
        let fetched: Result<_, boa_core::error::CmdError> = async {
            let saved = backend.list_connections().await?;
            let conn = saved
                .into_iter()
                .find(|c| wanted.as_ref().is_none_or(|w| &c.config.id == w))
                .ok_or_else(|| {
                    boa_core::error::CmdError::code(
                        "not_found",
                        "BOA_GRID_SQL: no saved connection in BOA_CONFIG_DIR",
                    )
                })?;
            let id = conn.config.id.clone();
            let db = database.unwrap_or(conn.config.database);
            backend.connect(&id, None).await?;
            backend.run_query(&id, &db, &sql, page_size, 0).await
        }
        .await;
        this.update(cx, |grid, cx| match fetched {
            Ok(result) => {
                let editability = match &pk {
                    Some(pk) => Editability::Editable { pk: pk.clone() },
                    None => Editability::ReadOnly {
                        reason: "Read-only: console results",
                    },
                };
                if let Some(pk) = pk {
                    grid.set_table_context(
                        Some(TableContext {
                            table,
                            pk,
                            sort,
                            ..TableContext::default()
                        }),
                        cx,
                    );
                }
                grid.set_content(
                    GridContent {
                        sql: sql.clone(),
                        result: Arc::new(result),
                        offset: 0,
                        page_size,
                        editability,
                    },
                    cx,
                );
            }
            Err(err) => grid.set_run_error(Some(RunError::from_cmd(&err, &sql)), cx),
        })
        .ok();
    })
    .detach();
}

/// A result was loaded: apply `BOA_GRID_STAGE` / `_ADD_ROWS` / `_DELETE`,
/// `BOA_GRID_SCROLL`, `BOA_GRID_SELECT`.
pub(super) fn after_load(grid: &mut ResultGrid) {
    let Some(page) = &mut grid.page else { return };
    for _ in 0..std::mem::take(&mut grid.dev.add_rows) {
        page.add_row();
    }
    for (cell, value) in std::mem::take(&mut grid.dev.stage) {
        if cell.row < page.row_count() && cell.col < page.col_count() {
            page.stage(cell, value);
        }
    }
    for row in std::mem::take(&mut grid.dev.delete) {
        page.delete_row(row);
    }
    if let Some((x, y)) = grid.dev.scroll.take() {
        grid.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(-x), px(-y)));
    }
    let Some((a, b)) = grid.dev.select.take() else {
        return;
    };
    if a.row < page.row_count()
        && b.row < page.row_count()
        && a.col < page.col_count()
        && b.col < page.col_count()
    {
        page.selection.press_cell(a, false);
        page.selection.press_cell(b, true);
    }
}

/// A result is about to be painted for the first time: start the benchmark.
pub(super) fn on_first_paint(
    grid: &mut ResultGrid,
    window: &mut Window,
    cx: &mut Context<ResultGrid>,
) {
    if let Some(cell) = grid.dev.edit.take() {
        cx.defer_in(window, move |grid, window, cx| {
            grid.start_edit(cell, None, window, cx);
        });
    }
    if let Some(cell) = grid.dev.json.take() {
        cx.defer_in(window, move |grid, window, cx| {
            if let Some(page) = &mut grid.page {
                page.selection.focus_cell(cell, false);
            }
            grid.open_json(cell, window, cx);
        });
    }
    if let Some(cell) = grid.dev.value.take() {
        cx.defer_in(window, move |grid, window, cx| {
            grid.open_value_editor(cell, window, cx);
        });
    }
    if let Some(cell) = grid.dev.menu.take() {
        // After a frame (the list knows its bounds), outside the grid's update.
        let entity = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let Some(position) = entity.upgrade().and_then(|grid| {
                let grid = grid.read(cx);
                let (bounds, offset) = grid.scroll_state();
                let page = grid.page.as_ref()?;
                let x = f32::from(bounds.origin.x + offset.x)
                    + page.layout.numbers_width()
                    + page.layout.start(cell.col)
                    + 20.;
                let y = f32::from(bounds.origin.y + offset.y)
                    + (cell.row as f32 + 0.5) * super::ROW_HEIGHT;
                Some(point(px(x), px(y)))
            }) else {
                return;
            };
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Right,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Right,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
            if let Some(keys) = env("BOA_GRID_MENU_KEYS") {
                let keystrokes: Vec<Keystroke> = keys
                    .split_whitespace()
                    .filter_map(|k| Keystroke::parse_with_os(k, gpui::OperatingSystem::Mac).ok())
                    .collect();
                window
                    .spawn(cx, async move |cx| {
                        for keystroke in keystrokes {
                            cx.background_executor()
                                .timer(Duration::from_millis(150))
                                .await;
                            cx.update(|window, cx| window.dispatch_keystroke(keystroke, cx))
                                .ok();
                        }
                    })
                    .detach();
            }
        });
    }
    if !grid.dev.bench || grid.dev.bench_run.is_some() {
        return;
    }
    grid.dev.bench = false;
    if let Some((w, h)) = env("BOA_GRID_BENCH_WINDOW").and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
    }) {
        window.resize(gpui::size(px(w), px(h)));
    }
    tracing::info!("grid bench: starting");
    grid.dev.bench_run = Some(BenchRun {
        phase: 0,
        frame: 0,
        last: Instant::now(),
        samples: Vec::new(),
        seed: 0x9e37_79b9_7f4a_7c15,
    });
    cx.on_next_frame(window, step);
    // Ask for that next frame: nothing else may redraw the window now.
    cx.defer_in(window, |_, window, _| window.refresh());
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}

fn report(phase: &Phase, samples: &[Sample]) -> String {
    // The first interval of a phase includes the switch: skip it.
    let samples = samples.get(1..).unwrap_or_default();
    let mut intervals: Vec<f64> = samples.iter().map(|s| ms(s.interval)).collect();
    intervals.sort_by(f64::total_cmp);
    let mut builds: Vec<f64> = samples.iter().map(|s| ms(s.build)).collect();
    builds.sort_by(f64::total_cmp);
    let mut cpus: Vec<f64> = samples.iter().filter_map(|s| s.cpu.map(ms)).collect();
    cpus.sort_by(f64::total_cmp);
    let median = percentile(&intervals, 0.5);
    let long = intervals.iter().filter(|&&i| i > median * 1.5).count();
    format!(
        "{}: {} frames; interval ms p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} ({} over 1.5× median); \
         build ms p50 {:.2} p95 {:.2} max {:.2}; render→paint ms p50 {:.2} p95 {:.2} max {:.2}",
        phase.name,
        samples.len(),
        median,
        percentile(&intervals, 0.95),
        percentile(&intervals, 0.99),
        intervals.last().copied().unwrap_or(0.),
        long,
        percentile(&builds, 0.5),
        percentile(&builds, 0.95),
        builds.last().copied().unwrap_or(0.),
        percentile(&cpus, 0.5),
        percentile(&cpus, 0.95),
        cpus.last().copied().unwrap_or(0.),
    )
}

/// One benchmark frame: record the last frame, scroll, ask for the next.
fn step(grid: &mut ResultGrid, window: &mut Window, cx: &mut Context<ResultGrid>) {
    let now = Instant::now();
    let build = std::mem::take(&mut grid.dev.build_time);
    let cpu = match (grid.dev.render_started, grid.dev.paint_done.get()) {
        (Some(start), Some(end)) if end > start => Some(end - start),
        _ => None,
    };
    grid.dev.paint_done.set(None);
    let Some(run) = &mut grid.dev.bench_run else {
        return;
    };
    run.samples.push(Sample {
        interval: now - run.last,
        build,
        cpu,
    });
    run.last = now;
    run.frame += 1;

    let phase = PHASES[run.phase];
    if run.frame > phase.frames {
        let line = report(&phase, &run.samples);
        let frame = grid.frame;
        let shown = grid.page.as_ref().map_or(0, |p| {
            p.layout
                .visible(
                    frame.scroll_x,
                    frame.scroll_x + frame.viewport_width - p.layout.numbers_width(),
                )
                .len()
        });
        tracing::info!(
            "grid bench: {line} [viewport {:.0}×{:.0} px: ~{:.0} rows × {shown} columns]",
            frame.viewport_width,
            frame.viewport_height,
            frame.viewport_height / super::ROW_HEIGHT
        );
        run.samples.clear();
        run.frame = 0;
        run.phase += 1;
        if run.phase == PHASES.len() {
            let rows = grid.page.as_ref().map_or(0, |p| p.row_count());
            let cols = grid.page.as_ref().map_or(0, |p| p.col_count());
            tracing::info!("grid bench: done ({rows} rows × {cols} columns)");
            grid.dev.bench_run = None;
            if grid.dev.quit_after_bench {
                cx.quit();
            }
            return;
        }
    }

    // Scroll.
    let phase = PHASES[run.phase];
    let state = grid.scroll.0.borrow();
    let handle = &state.base_handle;
    let offset = handle.offset();
    let max = handle.max_offset();
    let (max_x, max_y) = (f32::from(max.x), f32::from(max.y));
    let (mut x, mut y) = (-f32::from(offset.x), -f32::from(offset.y));
    if phase.dx == 0. && phase.dy == 0. {
        run.seed ^= run.seed << 13;
        run.seed ^= run.seed >> 7;
        run.seed ^= run.seed << 17;
        y = (run.seed % 10_000) as f32 / 10_000. * max_y;
    } else {
        x += phase.dx;
        y += phase.dy;
        if x > max_x {
            x = 0.;
        }
        if y > max_y {
            y = 0.;
        }
    }
    handle.set_offset(point(px(-x), px(-y)));
    drop(state);
    cx.notify();
    cx.on_next_frame(window, step);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        assert_eq!(
            parse_range("2,1:8, 4"),
            Some((Cell::new(2, 1), Cell::new(8, 4)))
        );
        assert_eq!(parse_range("2,1"), None);
        assert_eq!(parse_range("a,1:2,2"), None);
        assert_eq!(
            parse_stage("1,2=x;3,0=NULL;bad"),
            vec![(Cell::new(1, 2), Some("x".into())), (Cell::new(3, 0), None)]
        );
    }

    #[test]
    fn percentiles() {
        let v = [1., 2., 3., 4., 5.];
        assert_eq!(percentile(&v, 0.5), 3.);
        assert_eq!(percentile(&v, 1.), 5.);
        assert_eq!(percentile(&[], 0.5), 0.);
    }
}
