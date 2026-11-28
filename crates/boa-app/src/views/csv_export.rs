//! CsvExport.tsx: the Export menu of the result and table toolbars (Export
//! to CSV…, Copy page as CSV, delimiter and header options) and the status
//! bar's running exports with their progress and Cancel ([`export_status`]).
//! The state is [`Exports`](crate::models::exports::Exports).
//!
//! The menu is a popover with menu-like rows, so choosing a delimiter or
//! toggling the header row keeps it open (Radix `preventDefault` in the TS);
//! the two actions close it.

use std::sync::Arc;

use boa_core::grid::csv::CsvDelimiter;
use boa_core::grid::format::bytes;
use boa_core::model::{QueryResult, Value};
use boa_core::state::csv_export::{ExportSource, grouped};
use gpui::Anchor;
use gpui::prelude::*;
use gpui::{AnyElement, App, SharedString, WeakEntity, div, px};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::popover::Popover;
use gpui_component::spinner::Spinner;
use gpui_component::{Disableable as _, Icon, Sizable as _};

use super::chrome::{IconName, icon_button, tooltip};
use crate::app_state::AppState;
use crate::grid::ResultGrid;
use crate::grid::page::Page;
use crate::models::console::Console;
use crate::models::exports::{copy_csv, with_rows};
use crate::theme::{self, ActivePalette as _};

/// What the menu exports: the statement and, when shown, its loaded page.
#[derive(Debug, Clone)]
pub struct MenuSource {
    /// Without rows (they are added from `page` when the menu is used).
    pub source: ExportSource,
    pub page: Option<Arc<QueryResult>>,
    /// A table tab's grid: its rows as shown (row filter, staged edits,
    /// new rows) stand in for `page`'s (Workspace.tsx `rows: edits.rows`).
    pub grid: Option<WeakEntity<ResultGrid>>,
}

impl MenuSource {
    /// The console's result (BottomPanel: `resultSource(current)`).
    pub fn of_console(console: &Console) -> Option<Self> {
        let r = console.result.as_ref()?;
        Some(Self {
            source: ExportSource {
                connection_id: console.key.connection_id.clone(),
                database: console.key.database.clone(),
                sql: r.sql.clone(),
                label: console.title.to_string(),
                pageable: r.result.pageable,
                truncated: r.result.truncated,
                ..ExportSource::default()
            },
            page: Some(r.result.clone()),
            grid: None,
        })
    }

    /// The grid's page, when it shows this source's page.
    fn grid_page<'a>(&self, cx: &'a App) -> Option<&'a Page> {
        let page = self.page.as_ref()?;
        let grid = self.grid.as_ref()?.upgrade()?;
        grid.read(cx)
            .page()
            .filter(|shown| Arc::ptr_eq(&shown.result, page))
    }

    /// The rows the menu copies and a statement that changes data writes:
    /// the grid's as shown, else the loaded page's.
    pub fn rows(&self, cx: &App) -> Option<Vec<Vec<Value>>> {
        match self.grid_page(cx) {
            Some(page) => Some(page.shown_rows()),
            None => self.page.as_ref().map(|p| p.rows.clone()),
        }
    }

    /// The source with the page's columns and rows (as [`Self::rows`]).
    pub fn full(&self, cx: &App) -> ExportSource {
        let mut src = match &self.page {
            Some(page) => with_rows(self.source.clone(), page),
            None => self.source.clone(),
        };
        if let Some(page) = self.grid_page(cx) {
            src.rows = Some(page.shown_rows());
        }
        src
    }

    /// Rows loaded, or shown by the grid (the count next to "Copy page as CSV").
    pub fn loaded(&self, cx: &App) -> usize {
        match self.grid_page(cx) {
            Some(page) => page.row_count(),
            None => self.page.as_ref().map_or(0, |p| p.rows.len()),
        }
    }
}

/// The Export menu (download icon) for `menu`, or, while an export started
/// by `owner` runs, "Exporting…" with its Cancel button.
pub fn export_menu(
    id: &str,
    owner: impl Into<SharedString>,
    menu: Option<MenuSource>,
    disabled: bool,
    cx: &App,
) -> AnyElement {
    let owner: SharedString = owner.into();
    let p = cx.palette().clone();
    let Some(state) = AppState::try_global(cx).cloned() else {
        return div().into_any_element();
    };
    if let Some(job) = state.exports.read(cx).job_of(&owner) {
        let job_id = job.id;
        let exports = state.exports.clone();
        return div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .pl(px(6.))
            .text_size(theme::text::XS)
            .text_color(p.dim)
            .child(Spinner::new().with_size(px(12.)).color(p.quiet))
            .child("Exporting…")
            .child(
                icon_button(
                    SharedString::from(format!("{id}-cancel")),
                    IconName::X,
                    14.,
                    "Cancel export",
                    cx,
                )
                .debug_selector({
                    let id = format!("{id}-cancel");
                    move || id.clone()
                })
                .on_click(move |_, _, cx| {
                    exports.update(cx, |e, cx| e.cancel(job_id, cx));
                }),
            )
            .into_any_element();
    }
    let trigger_id = format!("{id}-trigger");
    let trigger = Button::new(SharedString::from(trigger_id.clone()))
        .debug_selector(move || trigger_id.clone())
        .ghost()
        // `size-6` with a `size-3.5` icon in `text-muted-foreground`: a
        // button sized 14 / 0.75 gives the icon 14 px, then it grows to 24.
        .with_size(px(14. / 0.75))
        .size(px(24.))
        .text_color(p.quiet)
        .icon(Icon::new(IconName::Download))
        .tooltip("Export");
    let Some(menu) = menu.filter(|_| !disabled) else {
        return trigger.disabled(true).into_any_element();
    };
    Popover::new(SharedString::from(format!("{id}-menu")))
        .anchor(Anchor::TopRight)
        .default_open(dev::menu_open())
        .p(px(4.))
        .w(px(240.))
        .trigger(trigger)
        .content(move |_, _, cx| {
            let popover = cx.entity();
            render_menu(&menu, owner.clone(), popover, cx)
        })
        .into_any_element()
}

fn render_menu(
    menu: &MenuSource,
    owner: SharedString,
    popover: gpui::Entity<gpui_component::popover::PopoverState>,
    cx: &App,
) -> gpui::Div {
    let p = cx.palette().clone();
    let Some(state) = AppState::try_global(cx).cloned() else {
        return div();
    };
    let options = state.exports.read(cx).options();
    let row = |id: &'static str, enabled: bool| {
        let hover = p.hover_strong;
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(8.))
            .rounded(theme::radius::SM)
            .text_size(theme::text::SM)
            .text_color(p.body)
            .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
            .when(!enabled, |d| d.opacity(0.5))
    };
    let slot = |child: Option<AnyElement>| {
        div()
            .flex_none()
            .w(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.quiet)
            .children(child)
    };
    let separator = || div().my(px(4.)).mx(px(-4.)).h(px(1.)).bg(p.border);
    let label = |text: &'static str| {
        div()
            .px(px(8.))
            .py(px(6.))
            .text_size(theme::text::XS)
            .text_color(p.dim)
            .child(text)
    };

    let export = {
        let (exports, popover, owner, menu) = (
            state.exports.clone(),
            popover.clone(),
            owner.clone(),
            menu.clone(),
        );
        row("export-csv", true)
            .child(slot(Some(
                Icon::new(IconName::FileDown)
                    .size(px(14.))
                    .into_any_element(),
            )))
            .child("Export to CSV…")
            .on_click(move |_, window, cx| {
                popover.update(cx, |s, cx| s.dismiss(window, cx));
                let src = menu.full(cx);
                let owner = owner.to_string();
                exports.update(cx, |e, cx| e.export(src, owner, cx));
            })
    };
    let loaded = menu.loaded(cx);
    let can_copy = menu.page.is_some();
    let copy = {
        let (popover, menu) = (popover.clone(), menu.clone());
        row("export-copy", can_copy)
            .child(slot(Some(
                Icon::new(IconName::Copy).size(px(14.)).into_any_element(),
            )))
            .child(if menu.source.pageable {
                "Copy page as CSV"
            } else {
                "Copy rows as CSV"
            })
            .child(
                div()
                    .ml_auto()
                    .text_size(theme::text::XS)
                    .text_color(p.dim)
                    .child(grouped(loaded as u64)),
            )
            .when(can_copy, |d| {
                d.on_click(move |_, window, cx| {
                    popover.update(cx, |s, cx| s.dismiss(window, cx));
                    if let (Some(page), Some(rows)) = (&menu.page, menu.rows(cx)) {
                        let columns: Vec<String> =
                            page.columns.iter().map(|c| c.name.clone()).collect();
                        copy_csv(&columns, &rows, cx);
                    }
                })
            })
    };
    let delimiters = CsvDelimiter::ALL.into_iter().map(|d| {
        let exports = state.exports.clone();
        let id: &'static str = match d {
            CsvDelimiter::Comma => "export-delimiter-comma",
            CsvDelimiter::Tab => "export-delimiter-tab",
            CsvDelimiter::Semicolon => "export-delimiter-semicolon",
        };
        let dot = (options.delimiter == d).then(|| {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(p.body)
                .into_any_element()
        });
        row(id, true)
            .child(slot(dot))
            .child(d.label())
            .on_click(move |_, _, cx| exports.update(cx, |e, cx| e.set_delimiter(d, cx)))
    });
    let header = {
        let exports = state.exports.clone();
        let on = options.header;
        let check = on.then(|| Icon::new(IconName::Check).size(px(14.)).into_any_element());
        row("export-header", true)
            .child(slot(check))
            .child("Header row")
            .on_click(move |_, _, cx| exports.update(cx, |e, cx| e.set_header(!on, cx)))
    };
    div()
        .flex()
        .flex_col()
        .child(export)
        .child(copy)
        .when(!menu.source.pageable, |d| {
            d.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(theme::text::XS)
                    .line_height(px(16.))
                    .text_color(p.dim)
                    .child("The statement changes data: only the loaded rows are exported."),
            )
        })
        .child(separator())
        .child(label("Delimiter"))
        .children(delimiters)
        .child(separator())
        .child(header)
}

/// ExportStatus: the first running export (file, rows, size, how many more)
/// with Cancel, for the status bar; None when nothing runs.
pub fn export_status(cx: &App) -> Option<AnyElement> {
    let state = AppState::try_global(cx)?;
    let exports = state.exports.read(cx);
    let job = exports.jobs().first()?.clone();
    let more = exports.jobs().len() - 1;
    let p = cx.palette().clone();
    let entity = state.exports.clone();
    let path = exports
        .path_of(job.id)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| job.file.clone());
    Some(
        div()
            .id("status-export")
            .flex()
            .flex_none()
            .min_w_0()
            .items_center()
            .gap(px(6.))
            .px(px(6.))
            .text_size(theme::text::XS)
            .text_color(p.soft)
            .tooltip(tooltip(path))
            .child(Spinner::new().with_size(px(12.)).color(p.quiet))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child("Exporting\u{a0}")
                    .child(div().font_family(theme::MONO_FONT).child(job.file.clone()))
                    .when(job.bytes > 0, |d| {
                        d.child(div().text_color(p.dim).child(format!(
                            "\u{a0}· {} rows · {}",
                            grouped(job.rows),
                            bytes(job.bytes)
                        )))
                    })
                    .when(more > 0, |d| {
                        d.child(div().text_color(p.dim).child(format!("\u{a0}(+{more})")))
                    }),
            )
            .child(
                icon_button(
                    "status-export-cancel",
                    IconName::X,
                    14.,
                    "Cancel export",
                    cx,
                )
                .debug_selector(|| "status-export-cancel".to_owned())
                .on_click(move |_, _, cx| {
                    entity.update(cx, |e, cx| e.cancel(job.id, cx));
                }),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests;

/// Dev hooks (`BOA_DEV_EXPORT`, feature `dev-hooks`): script an export
/// for screenshots.
///
/// - `menu`: run `select * from orders` in a new console and open the
///   Database Output's Export menu;
/// - `progress` / `done` (with `BOA_DEV_EXPORT_FILE=/abs/out.csv`): run it,
///   then export to that file, as the console's result, a slowed-down
///   `orders` (`progress`: rows trickle in) or `orders` itself (`done`:
///   the toast).
pub mod dev {
    #[cfg(feature = "dev-hooks")]
    use std::sync::atomic::{AtomicBool, Ordering};

    #[cfg(feature = "dev-hooks")]
    static MENU_OPEN: AtomicBool = AtomicBool::new(false);

    /// Whether the Export menu starts open (`BOA_DEV_EXPORT=menu`).
    pub fn menu_open() -> bool {
        #[cfg(feature = "dev-hooks")]
        {
            MENU_OPEN.load(Ordering::Relaxed)
        }
        #[cfg(not(feature = "dev-hooks"))]
        {
            false
        }
    }

    /// Applies `BOA_DEV_EXPORT` (call once the main window is open): waits
    /// (up to 10 s) for the first saved connection to connect.
    #[cfg(feature = "dev-hooks")]
    pub fn apply(cx: &mut gpui::App) {
        use crate::app_state::AppState;
        use crate::models::catalogs::DbKey;
        use boa_core::state::csv_export::ExportSource;

        let Some(what) = std::env::var("BOA_DEV_EXPORT")
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            return;
        };
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let what = what.trim().to_owned();
        if what == "menu" {
            MENU_OPEN.store(true, Ordering::Relaxed);
        }
        cx.spawn(async move |cx| {
            for _ in 0..100 {
                let ready = cx.update(|cx| {
                    let conns = state.connections.read(cx);
                    conns
                        .saved
                        .first()
                        .is_some_and(|c| conns.is_connected(&c.config.id))
                });
                if ready {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
            let started = cx.update(|cx| {
                let key = {
                    let conns = state.connections.read(cx);
                    conns.saved.first().map(|c| DbKey {
                        connection_id: c.config.id.clone(),
                        database: c.config.default_database(),
                    })
                }?;
                let sql = "select * from orders";
                state.workspaces.update(cx, |w, cx| {
                    w.new_query_tab_with(key.clone(), format!("{sql};\n"), cx)
                });
                let console = state
                    .workspaces
                    .read(cx)
                    .active_tab()
                    .and_then(|t| t.console().cloned())?;
                console.update(cx, |c, cx| c.run(sql.to_owned(), cx));
                Some((key, console))
            });
            let Some((key, console)) = started else {
                return;
            };
            let Some(file) = std::env::var_os("BOA_DEV_EXPORT_FILE") else {
                return;
            };
            let export_sql = match what.as_str() {
                // Slowed down so the progress shows (rows stream as they come).
                "progress" => "select o.*, pg_sleep(0.0003)::text as pause from orders o",
                "done" => "select * from orders",
                _ => return,
            };
            // Not while the console's statement runs on the same session.
            for _ in 0..100 {
                if cx.update(|cx| console.read(cx).result.is_some()) {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
            cx.update(|cx| {
                let owner = console.read(cx).tab_id.to_string();
                let src = ExportSource {
                    connection_id: key.connection_id.clone(),
                    database: key.database.clone(),
                    sql: export_sql.to_owned(),
                    label: "console_1".into(),
                    pageable: true,
                    ..ExportSource::default()
                };
                state
                    .exports
                    .update(cx, |e, cx| e.export_to(src, owner, file.into(), cx));
            });
        })
        .detach();
    }
}
