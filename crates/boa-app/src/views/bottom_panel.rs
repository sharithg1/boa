//! BottomPanel.tsx: the "Database Output" tool window. A header with the
//! Output log tab and one tab per console result of the selected database
//! (a spinner while it runs), the selected result's source / timing, its
//! pager and a hide button (⌥J); below, the result grid or the Output log.
//! Resizable from its top edge (the root view applies the height, prefs
//! `bottomHeight`, and the handle).
//!
//! The grid is a pure view: it gets the shown console's page through
//! [`ResultGrid::set_content`] and asks for other pages with
//! [`GridEvent::PageRequested`], which the console fetches.

use boa_core::grid::grid_edits::Editability;
use boa_core::state::history::OutputEntry;
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EntityId, FontWeight, MouseButton, ScrollHandle, SharedString,
    Subscription, Window, div, px,
};
use gpui_component::Sizable as _;
use gpui_component::spinner::Spinner;

use super::ActiveDb;
use super::chrome::{IconName, icon_button};
use super::csv_export::{MenuSource, export_menu};
use super::pager::{PagerState, pager};
use super::run_progress::run_bar;
use crate::app_state::AppState;
use crate::grid::{GridContent, GridEvent, ResultGrid};
use crate::models::console::Console;
use crate::models::layout::PanelKind;
use crate::models::workspaces::{OutputLine, Workspaces};
use crate::theme::{self, ActivePalette as _};

/// Height of the header (`h-9`).
pub const HEADER_HEIGHT: gpui::Pixels = px(36.);

/// Why a console result can't be edited.
pub const CONSOLE_READ_ONLY: &str = "Read-only: console results are not editable";

/// What the panel shows (store.ts `bottomView`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BottomView {
    /// The Output log.
    Output,
    /// A console's result (its tab id).
    Result(SharedString),
}

/// The Database Output panel.
#[derive(Debug)]
pub struct BottomPanel {
    workspaces: Entity<Workspaces>,
    grid: Entity<ResultGrid>,
    /// (console, run, offset) the grid shows.
    shown: Option<(EntityId, u64, usize)>,
    log_scroll: ScrollHandle,
    log_len: usize,
    _subscriptions: Vec<Subscription>,
}

impl BottomPanel {
    /// Needs [`AppState`] (panics without it).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspaces = AppState::global(cx).workspaces.clone();
        let grid = cx.new(|cx| {
            let mut grid = ResultGrid::new(window, cx);
            // The Database Output header is the toolbar here.
            grid.set_toolbar_visible(false, cx);
            grid
        });
        let exports = AppState::global(cx).exports.clone();
        let subscriptions = vec![
            cx.observe(&workspaces, |this, _, cx| this.sync(cx)),
            cx.observe(&exports, |_, _, cx| cx.notify()),
            cx.subscribe(&grid, |this, _, event: &GridEvent, cx| {
                if let GridEvent::PageRequested { offset } = event
                    && let Some(console) = this.shown_console(cx)
                {
                    let offset = *offset;
                    console.update(cx, |c, cx| c.page(offset, cx));
                }
            }),
        ];
        let mut this = Self {
            workspaces,
            grid,
            shown: None,
            log_scroll: ScrollHandle::new(),
            log_len: 0,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    pub fn grid(&self) -> &Entity<ResultGrid> {
        &self.grid
    }

    /// The (console, run, offset) whose page the grid shows.
    pub fn shown(&self) -> Option<(EntityId, u64, usize)> {
        self.shown
    }

    /// What the panel shows.
    pub fn view(&self, cx: &App) -> BottomView {
        match self.shown_console(cx) {
            Some(c) => BottomView::Result(c.read(cx).tab_id.clone()),
            None => BottomView::Output,
        }
    }

    fn shown_console(&self, cx: &App) -> Option<Entity<Console>> {
        self.workspaces.read(cx).shown_result(cx)
    }

    /// Hands the shown console's page to the grid when it changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        match self.shown_console(cx) {
            Some(console) => {
                let c = console.read(cx);
                let running = c.running.is_some();
                if let Some(r) = c.result.clone() {
                    let key = (console.entity_id(), r.run, r.offset);
                    if self.shown != Some(key) {
                        self.shown = Some(key);
                        let content = GridContent {
                            sql: r.sql,
                            result: r.result,
                            offset: r.offset,
                            page_size: r.page_size,
                            editability: Editability::ReadOnly {
                                reason: CONSOLE_READ_ONLY,
                            },
                        };
                        self.grid.update(cx, |g, cx| g.set_content(content, cx));
                    }
                }
                self.grid.update(cx, |g, cx| g.set_running(running, cx));
            }
            None => self.shown = None,
        }
        // New log lines: keep the newest in view.
        let len = ActiveDb::get(cx).map_or(0, |a| {
            self.workspaces.read(cx).output(&a.connection_id).len()
        });
        if len != self.log_len {
            self.log_len = len;
            self.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn select(&mut self, view: Option<SharedString>, cx: &mut Context<Self>) {
        self.workspaces
            .update(cx, |w, cx| w.set_bottom_view(view, cx));
    }

    fn panel_tab(
        id: impl Into<gpui::ElementId>,
        selected: bool,
        icon: impl IntoElement,
        label: impl IntoElement,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        let p = cx.palette();
        let (hover, soft) = (p.hover, p.soft);
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .pl(px(8.))
            .pr(px(8.))
            .rounded(theme::radius::MD)
            .whitespace_nowrap()
            .text_size(theme::text::SM)
            .when(selected, |d| d.bg(p.card).text_color(p.body))
            .when(!selected, |d| {
                d.text_color(p.quiet)
                    .hover(move |s| s.bg(hover).text_color(soft))
            })
            .child(div().text_color(p.quiet).child(icon))
            .child(label)
    }

    fn render_log(&self, lines: &[OutputLine], cx: &App) -> gpui::AnyElement {
        let p = cx.palette();
        if lines.is_empty() {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_size(theme::text::SM)
                .text_color(p.quiet)
                .child("Run a statement (⌘↵) to see its output here")
                .into_any_element();
        }
        let rows = lines.iter().rev().map(|line| {
            let h = &line.entry;
            let time = time_of_day(h.at);
            let pad = || div().flex_none().opacity(0.).child(format!("[{time}] "));
            let source = h.source.clone().unwrap_or_else(|| h.database.clone());
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .text_color(p.soft)
                        .child(
                            div()
                                .flex_none()
                                .text_color(p.dim)
                                .child(format!("[{time}] ")),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(p.quiet)
                                .child(format!("{source}> ")),
                        )
                        .child(div().min_w_0().child(h.sql.clone())),
                )
                .child(
                    div()
                        .flex()
                        .text_color(if h.error.is_some() {
                            p.status_err_text
                        } else {
                            p.dim
                        })
                        .child(pad())
                        .child(div().min_w_0().child(output_outcome(h))),
                )
                .when_some(h.detail.clone(), |d, detail| {
                    d.child(
                        div()
                            .flex()
                            .text_color(p.status_err_text.opacity(0.8))
                            .child(pad())
                            .child(div().min_w_0().child(detail)),
                    )
                })
                .when_some(line.excerpt.clone(), |d, (text, caret)| {
                    d.child(
                        div()
                            .flex()
                            .text_color(p.status_err_text.opacity(0.8))
                            .child(pad())
                            .child(div().whitespace_nowrap().child(text)),
                    )
                    .child(
                        div()
                            .flex()
                            .text_color(p.status_err_text)
                            .child(pad())
                            .child(div().whitespace_nowrap().child(caret)),
                    )
                })
        });
        div()
            .id("output-log")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.log_scroll)
            .px(px(12.))
            .py(px(8.))
            .font_family(theme::MONO_FONT)
            .text_size(theme::text::SM)
            .line_height(px(24.))
            .children(rows)
            .into_any_element()
    }
}

/// `[hh:mm:ss]` of an Output log line, local time.
pub fn time_of_day(at_ms: f64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(at_ms as i64)
        .single()
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

fn plural_s(n: f64) -> &'static str {
    if n == 1. { "" } else { "s" }
}

/// What an Output log line says happened (BottomPanel.tsx `outcome`).
pub fn output_outcome(h: &OutputEntry) -> String {
    let ms = h.ms.round() as i64;
    if let Some(error) = &h.error {
        return format!("ERROR: {error}");
    }
    if let Some(note) = &h.note {
        return note.clone();
    }
    if let Some(rows) = h.rows {
        return format!(
            "{} row{} retrieved starting from {} in {ms} ms{}",
            rows as i64,
            plural_s(rows),
            h.offset.unwrap_or(0.) as i64 + 1,
            if h.truncated { " (more available)" } else { "" }
        );
    }
    match h.affected {
        Some(n) if n != 0. => format!("{} row{} affected in {ms} ms", n as i64, plural_s(n)),
        _ => format!("completed in {ms} ms"),
    }
}

impl Render for BottomPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let active = ActiveDb::get(cx);
        let current = self.shown_console(cx);
        let ws = self.workspaces.read(cx);
        let result_tabs = ws.result_tabs(cx);
        let busy = ws
            .active_workspace()
            .is_some_and(|w| w.tabs.iter().any(|t| t.is_running(cx)));
        let lines: Vec<OutputLine> = active
            .as_ref()
            .map(|a| ws.output(&a.connection_id).to_vec())
            .unwrap_or_default();
        let last_failed = lines.first().is_some_and(|l| l.entry.error.is_some());
        let output_icon = if last_failed {
            gpui_component::Icon::new(IconName::CircleAlert)
                .size(px(14.))
                .text_color(p.destructive)
        } else {
            gpui_component::Icon::new(IconName::SquareTerminal).size(px(14.))
        };
        let output_tab = Self::panel_tab(
            "bottom-tab-output",
            current.is_none(),
            output_icon,
            div()
                .when(last_failed, |d| d.text_color(p.status_err_text))
                .child("Output"),
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| this.select(None, cx)));
        let shown_id = current.as_ref().map(|c| c.entity_id());
        let tabs = result_tabs.into_iter().filter_map(|tab| {
            let console = tab.console()?.clone();
            let c = console.read(cx);
            let selected = shown_id == Some(console.entity_id());
            let running = c.running.is_some();
            let sql = c.result.as_ref().map(|r| r.sql.clone()).unwrap_or_default();
            let group: SharedString = format!("bottom-tab-{}", tab.id).into();
            let (hover_bg, body) = (p.hover_strong, p.body);
            let icon = if running {
                Spinner::new()
                    .with_size(px(14.))
                    .color(p.status_run)
                    .into_any_element()
            } else {
                gpui_component::Icon::new(IconName::Table2)
                    .size(px(14.))
                    .into_any_element()
            };
            let (select_id, close_console, middle_console) =
                (tab.id.clone(), console.clone(), console.clone());
            Some(
                Self::panel_tab(
                    SharedString::from(format!("bottom-tab-result-{}", tab.id)),
                    selected,
                    icon,
                    div()
                        .font_family(theme::MONO_FONT)
                        .text_size(theme::text::SM)
                        .child(tab.label()),
                    cx,
                )
                .group(group.clone())
                .pr(px(4.))
                .tooltip(super::chrome::tooltip(sql))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select(Some(select_id.clone()), cx)),
                )
                .on_mouse_up(MouseButton::Middle, move |_, _, cx| {
                    middle_console.update(cx, |c, cx| c.close_result(cx))
                })
                .child(
                    div()
                        .id(SharedString::from(format!("bottom-tab-close-{}", tab.id)))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(p.quiet)
                        .hover(move |s| s.bg(hover_bg).text_color(body))
                        .when(!selected, |d| {
                            d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.))
                        })
                        .tooltip(super::chrome::tooltip("Close result"))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            close_console.update(cx, |c, cx| c.close_result(cx));
                        })
                        .child(gpui_component::Icon::new(IconName::X).size(px(12.))),
                ),
            )
        });
        let shown = current.as_ref().and_then(|console| {
            let c = console.read(cx);
            let r = c.result.as_ref()?;
            let connection = active
                .as_ref()
                .map(|a| format!(" [{}]", a.connection))
                .unwrap_or_default();
            let limit = if r.result.truncated && !r.result.pageable {
                " · row limit reached"
            } else {
                ""
            };
            let source = format!("{}{connection} · {} ms{limit}", c.title, r.ms);
            let state = r.result.pageable.then(|| PagerState {
                offset: r.offset,
                count: r.result.rows.len(),
                page_size: r.page_size,
                more: r.result.truncated,
                busy: c.running.is_some(),
            });
            Some((source, state, console.clone()))
        });
        let (source, pager_el) = match shown {
            Some((source, state, console)) => {
                let pager_el = state.and_then(|s| {
                    pager(
                        "bottom-pager",
                        s,
                        move |offset, _, cx| {
                            console.update(cx, |c, cx| c.page(offset, cx));
                        },
                        cx,
                    )
                });
                (Some(source), pager_el)
            }
            None => (None, None),
        };
        // CsvExport.tsx ExportMenu, tied to the result's tab.
        let export_el = current.as_ref().map(|console| {
            let c = console.read(cx);
            export_menu(
                "bottom-export",
                c.tab_id.clone(),
                MenuSource::of_console(c),
                false,
                cx,
            )
        });
        let clear = (current.is_none())
            .then(|| active.as_ref().map(|a| a.connection_id.to_string()))
            .flatten();
        let header = div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(HEADER_HEIGHT)
            .px(px(8.))
            .border_b_1()
            .border_color(p.border)
            .when(busy, |d| d.child(run_bar(cx)))
            .child(
                div()
                    .mr(px(8.))
                    .pl(px(4.))
                    .text_size(theme::text::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.body)
                    .child("Database Output"),
            )
            .child(
                div()
                    .id("bottom-tabs")
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(2.))
                    .overflow_x_scroll()
                    .child(output_tab)
                    .children(tabs),
            )
            .child(div().flex_1().min_w(px(16.)))
            .children(source.map(|s| {
                div()
                    .flex_none()
                    .truncate()
                    .px(px(6.))
                    .text_size(theme::text::XS)
                    .text_color(p.dim)
                    .child(s)
            }))
            .children(export_el)
            .children(pager_el)
            .children(clear.map(|id| {
                icon_button("bottom-clear", IconName::Trash, 14., "Clear output", cx).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.workspaces.update(cx, |w, cx| w.clear_output(&id, cx));
                    }),
                )
            }))
            .child(
                icon_button("bottom-hide", IconName::Minus, 16., "Hide (⌥J)", cx).on_click(
                    |_, _, cx| {
                        if let Some(state) = AppState::try_global(cx) {
                            let layout = state.layout.clone();
                            layout.update(cx, |l, cx| l.set_open(PanelKind::Bottom, false, cx));
                        }
                    },
                ),
            );
        let body = if current.is_some() {
            div()
                .flex_1()
                .min_h_0()
                .child(self.grid.clone())
                .into_any_element()
        } else {
            self.render_log(&lines, cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.background)
            .border_t_1()
            .border_color(p.border)
            .child(header)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> OutputEntry {
        OutputEntry {
            sql: "select 1".into(),
            database: "db".into(),
            ms: 12.,
            ..OutputEntry::default()
        }
    }

    #[test]
    fn outcomes_read_like_the_tauri_output_log() {
        let mut e = entry();
        e.rows = Some(500.);
        e.truncated = true;
        assert_eq!(
            output_outcome(&e),
            "500 rows retrieved starting from 1 in 12 ms (more available)"
        );
        e.rows = Some(1.);
        e.truncated = false;
        e.offset = Some(500.);
        assert_eq!(
            output_outcome(&e),
            "1 row retrieved starting from 501 in 12 ms"
        );
        let mut e = entry();
        e.affected = Some(3.);
        assert_eq!(output_outcome(&e), "3 rows affected in 12 ms");
        e.affected = Some(0.);
        assert_eq!(output_outcome(&e), "completed in 12 ms");
        e.error = Some("[42P01] relation \"x\" does not exist".into());
        assert_eq!(
            output_outcome(&e),
            "ERROR: [42P01] relation \"x\" does not exist"
        );
        let mut e = entry();
        e.note = Some("26 rows exported to /tmp/x.csv".into());
        assert_eq!(output_outcome(&e), "26 rows exported to /tmp/x.csv");
    }
}
