//! QueryHistory.tsx (the Query History view, ⌘⌥E) and the History tab of
//! RightPanel.tsx ([`HistoryPanel`]), on the [`History`] model.
//!
//! **Query History**: an overlay over the window (8 % from the top, up to
//! 1000 × 680 px): the queries run on the active connection (or on all of
//! them), newest first, filtered as you type (every word of the search, any
//! order) with an errors-only pill. The list is virtualized (64 px rows); the
//! detail pane shows the whole statement and its error. Keys (in the search
//! field): ↑ ↓ PgUp PgDn move, ⏎ inserts the statement at the caret of the
//! current console (or opens it in a new console when the active tab isn't
//! one), ⌘⏎ opens it in a new console on its database, ⌘C copies it (when
//! no text is selected in the field), ⌘⌫ deletes the entry, Esc closes.
//! "Clear history…" asks, then deletes the scope's saved statements.
//!
//! **History tab**: the active connection's list with a search field (Esc
//! clears it); a click opens the statement in a new console.
//!
//! [`History`]: crate::models::history::History

use std::rc::Rc;
use std::time::Duration;

use boa_core::grid::format::ago;
use boa_core::history::Entry;
use boa_core::sql::tokenize;
use boa_core::state::csv_export::grouped;
use boa_core::state::history::{
    HistoryFilter, HistoryItem, duration, filter_history, insertion_text, outcome_label,
    sql_preview,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, HighlightStyle, ListAlignment,
    ListState, MouseButton, ScrollStrategy, StyledText, Subscription, UniformListScrollHandle,
    Window, div, list, px, uniform_list,
};
use gpui_component::WindowExt as _;
use gpui_component::button::ButtonVariant;
use gpui_component::input::{Input, InputEvent, InputState};

use super::chrome::{BoaIcon, IconName, icon, tooltip};
use crate::actions::{
    ClearPanelSearch, Confirm, CopyHistoryEntry, DeleteHistoryEntry, Dismiss, OpenHistoryInConsole,
    SelectNext, SelectPageDown, SelectPageUp, SelectPrevious,
};
use crate::app_state::AppState;
use crate::editor::{SqlEditor, style_runs};
use crate::models::catalogs::DbKey;
use crate::models::toasts::NewToast;
use crate::models::workspaces::now_ms;
use crate::theme::{self, ActivePalette as _, Palette};

/// Height of a row of the Query History list (`ROW_H`).
pub const ROW_H: f32 = 64.;
/// "Statement copied" shows this long.
pub const COPIED_TOAST_FOR: Duration = Duration::from_millis(2400);

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// Rows a PgUp / PgDn moves in a list `view_h` px tall: a page less one,
/// at least one (`Math.max(1, Math.floor(viewH / ROW_H) - 1)`).
pub fn page_step(view_h: f32) -> usize {
    ((view_h / ROW_H).floor() as usize).saturating_sub(1).max(1)
}

/// The selection after moving `by` rows in a list of `len` (clamped, no wrap).
pub fn moved(sel: usize, len: usize, by: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let cur = sel.min(len - 1) as isize;
    (cur + by).clamp(0, len as isize - 1) as usize
}

/// The count in the header: "1,234", or "12 of 1,234" while filtered.
pub fn count_label(shown: usize, total: usize) -> String {
    if shown == total {
        grouped(total as u64)
    } else {
        format!("{} of {}", grouped(shown as u64), grouped(total as u64))
    }
}

/// What the empty list says.
pub fn empty_message(busy: bool, total: usize, recording: bool) -> &'static str {
    if busy {
        "Loading…"
    } else if total == 0 {
        if recording {
            "No statements yet. Statements you run are saved here."
        } else {
            "No statements saved. Recording is off (Settings → Queries)."
        }
    } else {
        "No statements match."
    }
}

/// `new Date(ms).toLocaleString()` (en-US): "11/26/2026, 6:25:04 AM".
pub fn local_time(ms: f64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.format("%-m/%-d/%Y, %-I:%M:%S %p").to_string())
        .unwrap_or_default()
}

/// The "db · 5m · 12 ms · 3 rows" line (`connection` first in the all-
/// connections view).
pub fn meta_line(e: &Entry, connection: Option<&str>, now: f64) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = connection {
        parts.push(c.to_owned());
    }
    parts.push(e.database.clone());
    parts.push(ago(e.at as i64, now as i64));
    parts.push(duration(e.ms));
    parts.push(outcome_label(e));
    parts.join(" · ")
}

/// SqlHighlight.tsx `SqlText`: `sql` highlighted, plus a faint `suffix`.
pub fn sql_text(sql: &str, suffix: &str, p: &Palette) -> StyledText {
    let tokens = tokenize(sql, None);
    let mut runs = style_runs(&tokens, &(0..sql.len()), p);
    let mut text = sql.to_owned();
    if !suffix.is_empty() {
        let start = text.len();
        text.push_str(suffix);
        runs.push((
            start..text.len(),
            HighlightStyle {
                color: Some(p.faint),
                ..HighlightStyle::default()
            },
        ));
    }
    StyledText::new(text).with_highlights(runs)
}

/// The name of a connection (its id when it is gone).
fn connection_name(id: &str, cx: &App) -> String {
    AppState::try_global(cx)
        .and_then(|s| {
            s.connections
                .read(cx)
                .get(id)
                .map(|c| c.config.name.clone())
        })
        .unwrap_or_else(|| id.to_owned())
}

/// The selected database.
fn active_db(cx: &App) -> Option<DbKey> {
    AppState::try_global(cx)?.workspaces.read(cx).active.clone()
}

/// The active editor tab is a console (`activeConsole`).
pub fn has_console(cx: &App) -> bool {
    AppState::try_global(cx).is_some_and(|s| {
        s.workspaces
            .read(cx)
            .active_tab()
            .is_some_and(|t| t.is_query())
    })
}

fn recording(cx: &App) -> bool {
    AppState::try_global(cx).is_none_or(|s| s.prefs.read(cx).get().record_history)
}

/// Copies a statement ("Statement copied").
pub fn copy_sql(sql: &str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(sql.to_owned()));
    if let Some(state) = AppState::try_global(cx).cloned() {
        state.toasts.update(cx, |t, cx| {
            t.push_for(
                NewToast::info("Statement copied"),
                Some(COPIED_TOAST_FOR),
                cx,
            );
        });
    }
}

/// Opens `sql` in a new console on its connection and database.
pub fn open_in_new_console(connection_id: &str, database: &str, sql: &str, cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let key = DbKey {
        connection_id: connection_id.to_owned(),
        database: database.to_owned(),
    };
    let sql = sql.to_owned();
    state
        .workspaces
        .update(cx, |w, cx| w.new_query_tab_with(key, sql, cx));
}

/// Inserts `sql` at the caret of a console's editor (replacing the
/// selection, as one undoable edit; `insertionText` puts it on its own line
/// next to other text), focusing the editor.
pub fn insert_into_editor(
    editor: &Entity<SqlEditor>,
    sql: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let state = editor.read(cx).state().clone();
    state.update(cx, |state, cx| {
        state.focus(window, cx);
        let value = state.value();
        let range = state.selected_range();
        let (a, b) = (range.start.min(value.len()), range.end.min(value.len()));
        let text = insertion_text(&value[..a], &value[b..], sql);
        state.replace(text, window, cx);
    });
}

/// Inserts `sql` into the active console (`insertIntoConsole`): at the
/// caret of its editor when it is shown, else appended to its SQL. False
/// when the active tab isn't a console.
pub fn insert_into_console(
    editor: Option<Entity<SqlEditor>>,
    sql: &str,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return false;
    };
    let Some(console) = state
        .workspaces
        .read(cx)
        .active_tab()
        .and_then(|t| t.console().cloned())
    else {
        return false;
    };
    match editor {
        Some(editor) => insert_into_editor(&editor, sql, window, cx),
        None => console.update(cx, |c, cx| {
            let sep = if !c.sql.is_empty() && !c.sql.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            let next = format!("{}{sep}{sql}", c.sql);
            c.set_sql(next);
            cx.notify();
        }),
    }
    true
}

/// A small outline button (`Button size="xs" variant="outline"`).
fn small_button(
    id: &'static str,
    name: IconName,
    label: &'static str,
    hint: &'static str,
    ghost: bool,
    p: &Palette,
) -> gpui::Stateful<gpui::Div> {
    let (hover, fg) = (p.hover, if ghost { p.quiet } else { p.body });
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(24.))
        .px(px(8.))
        .rounded(theme::radius::MD)
        .when(!ghost, |d| d.border_1().border_color(p.border))
        .text_size(theme::text::XS)
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .tooltip(tooltip(hint))
        .child(icon(name, 12., fg))
        .child(label)
}

// ── The Query History view ───────────────────────────────────────────────────

/// Which connections the view lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryScope {
    /// The active connection.
    Connection,
    /// Every saved connection, in sidebar order.
    All,
}

/// What the view asks of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryHistoryEvent {
    /// Close the view and insert this at the current console's caret.
    Insert(String),
}

/// The Query History overlay. Created when it opens, dropped when it closes.
pub struct QueryHistoryView {
    input: Entity<InputState>,
    scope: HistoryScope,
    query: String,
    errors_only: bool,
    sel: usize,
    scroll: UniformListScrollHandle,
    /// The scope's entries (all of them), newest first.
    items: Vec<HistoryItem>,
    /// The ones the search and the errors pill keep.
    shown: Vec<HistoryItem>,
    /// Rows a PgUp / PgDn moves (from the list's height at the last render).
    page: usize,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for QueryHistoryView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryHistoryView")
            .field("scope", &self.scope)
            .field("query", &self.query)
            .field("errors_only", &self.errors_only)
            .field("sel", &self.sel)
            .field("shown", &self.shown.len())
            .finish_non_exhaustive()
    }
}

impl EventEmitter<QueryHistoryEvent> for QueryHistoryView {}

impl QueryHistoryView {
    /// Needs [`AppState`].
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search history…  (words in the SQL, any order)")
        });
        let mut subscriptions = vec![cx.subscribe(&input, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                this.reset_sel();
                this.refresh(cx);
            }
        })];
        if let Some(state) = AppState::try_global(cx).cloned() {
            subscriptions.push(cx.observe(&state.history, |this, _, cx| this.refresh(cx)));
            subscriptions.push(cx.observe(&state.workspaces, |this, _, cx| this.reload(cx)));
            subscriptions.push(cx.observe(&state.connections, |this, _, cx| this.reload(cx)));
            subscriptions.push(cx.observe(&state.prefs, |_, _, cx| cx.notify()));
        }
        let mut this = Self {
            input,
            scope: if active_db(cx).is_some() {
                HistoryScope::Connection
            } else {
                HistoryScope::All
            },
            query: String::new(),
            errors_only: false,
            sel: 0,
            scroll: UniformListScrollHandle::new(),
            items: Vec::new(),
            shown: Vec::new(),
            page: page_step(480.),
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    /// The search field (focused when the view opens).
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    pub fn scope(&self) -> HistoryScope {
        self.scope
    }

    pub fn errors_only(&self) -> bool {
        self.errors_only
    }

    /// The entries listed (after search and filter), newest first.
    pub fn shown(&self) -> &[HistoryItem] {
        &self.shown
    }

    /// Every entry of the scope.
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// The selected row's index (clamped to the list).
    pub fn selected_index(&self) -> usize {
        self.sel.min(self.shown.len().saturating_sub(1))
    }

    /// The selected entry.
    pub fn current(&self) -> Option<&HistoryItem> {
        self.shown.get(self.selected_index())
    }

    /// The connections of the scope (`ids`).
    fn ids(&self, cx: &App) -> Vec<String> {
        let active = active_db(cx);
        match (self.scope, active) {
            (HistoryScope::Connection, Some(key)) => vec![key.connection_id],
            _ => AppState::try_global(cx).map_or_else(Vec::new, |s| {
                s.connections
                    .read(cx)
                    .saved
                    .iter()
                    .map(|c| c.config.id.clone())
                    .collect()
            }),
        }
    }

    /// Loads the scope's connections (once each), then re-lists.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        if let Some(history) = AppState::try_global(cx).map(|s| s.history.clone()) {
            history.update(cx, |h, cx| {
                for id in &ids {
                    h.load(id, false, cx);
                }
            });
        }
        self.refresh(cx);
    }

    /// Re-lists from the model.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        self.items = AppState::try_global(cx)
            .map(|s| s.history.read(cx).items(&ids))
            .unwrap_or_default();
        let filter = HistoryFilter {
            query: self.query.clone(),
            errors_only: self.errors_only,
        };
        self.shown = filter_history(&self.items, &filter)
            .into_iter()
            .cloned()
            .collect();
        cx.notify();
    }

    fn busy(&self, cx: &App) -> bool {
        let ids = self.ids(cx);
        AppState::try_global(cx).is_some_and(|s| {
            let h = s.history.read(cx);
            ids.iter().any(|id| h.is_loading(id))
        })
    }

    fn reset_sel(&mut self) {
        self.sel = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// Moves the selection `by` rows.
    pub fn move_by(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        self.sel = moved(self.sel, self.shown.len(), by);
        self.scroll
            .scroll_to_item(self.sel, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Selects row `i` (a click).
    pub fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        self.sel = i.min(self.shown.len().saturating_sub(1));
        cx.notify();
    }

    /// The scope pills.
    pub fn set_scope(&mut self, scope: HistoryScope, cx: &mut Context<Self>) {
        self.scope = scope;
        self.reset_sel();
        self.reload(cx);
    }

    /// The "Errors only" pill.
    pub fn toggle_errors_only(&mut self, cx: &mut Context<Self>) {
        self.errors_only = !self.errors_only;
        self.reset_sel();
        self.refresh(cx);
    }

    fn close(cx: &mut App) {
        if let Some(history) = AppState::try_global(cx).map(|s| s.history.clone()) {
            history.update(cx, |h, cx| h.set_open(false, cx));
        }
    }

    /// ⏎ / double click: insert at the caret of the current console, or
    /// open in a new console when the active tab isn't one.
    pub fn insert(&mut self, item: Option<HistoryItem>, cx: &mut Context<Self>) {
        let Some(item) = item else { return };
        if has_console(cx) {
            cx.emit(QueryHistoryEvent::Insert(item.entry.sql));
        } else {
            Self::open_in_console(Some(item), cx);
        }
    }

    /// ⌘⏎ / "New console": the statement in a new console on its database.
    pub fn open_in_console(item: Option<HistoryItem>, cx: &mut App) {
        let Some(item) = item else { return };
        Self::close(cx);
        open_in_new_console(
            &item.connection_id,
            &item.entry.database,
            &item.entry.sql,
            cx,
        );
    }

    /// ⌘⌫ / "Delete": drops the entry.
    pub fn remove(&mut self, item: Option<HistoryItem>, cx: &mut Context<Self>) {
        let Some(item) = item else { return };
        if let Some(history) = AppState::try_global(cx).map(|s| s.history.clone()) {
            history.update(cx, |h, cx| {
                h.delete(
                    &item.connection_id,
                    std::slice::from_ref(&item.entry.id),
                    cx,
                )
            });
        }
    }

    /// ⌘C: copies the selected statement, or the field's selected text.
    fn copy(&mut self, cx: &mut Context<Self>) {
        let selected = {
            let input = self.input.read(cx);
            let r = input.selected_range();
            (!r.is_empty()).then(|| input.selected_text().to_string())
        };
        match selected {
            Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            None => {
                if let Some(item) = self.current() {
                    let sql = item.entry.sql.clone();
                    copy_sql(&sql, cx);
                }
            }
        }
    }

    /// "Clear history…": asks, then deletes the scope's saved statements.
    fn confirm_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let total = self.items.len();
        if total == 0 {
            return;
        }
        let ids = self.ids(cx);
        let label = match (self.scope, active_db(cx)) {
            (HistoryScope::Connection, Some(key)) => connection_name(&key.connection_id, cx),
            _ => "all connections".to_owned(),
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let ids = ids.clone();
            let this = this.clone();
            alert
                .title("Clear query history?")
                .description(format!(
                    "Deletes the {} saved statement{} of {label}. This cannot be undone.",
                    grouped(total as u64),
                    if total == 1 { "" } else { "s" }
                ))
                .show_cancel(true)
                .ok_text("Clear")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    if let Some(history) = AppState::try_global(cx).map(|s| s.history.clone()) {
                        history.update(cx, |h, cx| {
                            for id in &ids {
                                h.clear(id, cx);
                            }
                        });
                    }
                    this.update(cx, |this, cx| {
                        this.reset_sel();
                        cx.notify();
                    })
                    .ok();
                    true
                })
        });
    }

    fn pill(id: &'static str, selected: bool, p: &Palette) -> gpui::Stateful<gpui::Div> {
        let (hover, soft) = (p.hover, p.soft);
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(px(24.))
            .max_w(px(160.))
            .px(px(8.))
            .rounded(theme::radius::MD)
            .text_size(theme::text::XS)
            .cursor_pointer()
            .when(selected, |d| d.bg(p.card).text_color(p.body))
            .when(!selected, |d| {
                d.text_color(p.quiet)
                    .hover(move |s| s.bg(hover).text_color(soft))
            })
    }

    fn render_header(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let active = active_db(cx);
        let scope = self.scope;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(46.))
            .px(px(14.))
            .border_b_1()
            .border_color(p.border)
            .child(icon(IconName::Search, 14., p.quiet))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.input)
                        .appearance(false)
                        .px_0()
                        .py_0()
                        .h(px(28.))
                        .text_size(theme::text::BASE),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .when_some(active, |d, key| {
                        d.child(
                            Self::pill(
                                "history-scope-connection",
                                scope == HistoryScope::Connection,
                                p,
                            )
                            .child(
                                div()
                                    .truncate()
                                    .child(connection_name(&key.connection_id, cx)),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_scope(HistoryScope::Connection, cx)
                            })),
                        )
                    })
                    .child(
                        Self::pill("history-scope-all", scope == HistoryScope::All, p)
                            .child("All connections")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.set_scope(HistoryScope::All, cx)),
                            ),
                    )
                    .child(div().mx(px(4.)).h(px(16.)).w(px(1.)).bg(p.border))
                    .child(
                        Self::pill("history-errors-only", self.errors_only, p)
                            .child(icon(
                                IconName::CircleAlert,
                                12.,
                                if self.errors_only { p.body } else { p.quiet },
                            ))
                            .child("Errors only")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_errors_only(cx))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::XXS)
                    .text_color(p.faint)
                    .child(count_label(self.shown.len(), self.items.len())),
            )
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = cx.palette().clone();
        let selected = self.selected_index();
        let all = self.scope == HistoryScope::All;
        let now = now_ms();
        range
            .filter_map(|i| {
                let r = self.shown.get(i)?;
                let pv = sql_preview(&r.entry.sql, 2);
                let failed = r.entry.error.is_some();
                let connection = all.then(|| connection_name(&r.connection_id, cx));
                let mut meta = Vec::new();
                if let Some(c) = &connection {
                    meta.push(div().flex_none().text_color(p.quiet).child(c.clone()));
                    meta.push(div().text_color(p.dim).child("·"));
                }
                meta.extend([
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO_FONT)
                        .child(r.entry.database.clone()),
                    div().text_color(p.dim).child("·"),
                    div().flex_none().child(ago(r.entry.at as i64, now as i64)),
                    div().text_color(p.dim).child("·"),
                    div().flex_none().child(duration(r.entry.ms)),
                    div().text_color(p.dim).child("·"),
                    div()
                        .flex_none()
                        .when(failed, |d| d.text_color(p.status_err_text))
                        .child(outcome_label(&r.entry)),
                ]);
                let (hover, muted) = (p.hover, p.muted);
                Some(
                    div()
                        .id(("history-row", i))
                        .h(px(ROW_H))
                        .px(px(4.))
                        .child(
                            div()
                                .size_full()
                                .flex()
                                .gap(px(8.))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(theme::radius::MD)
                                .when(i == selected, |d| d.bg(muted))
                                .when(i != selected, |d| d.hover(move |s| s.bg(hover)))
                                .child(div().mt(px(2.)).flex_none().size(px(14.)).when(
                                    failed,
                                    |d| {
                                        d.child(icon(IconName::CircleAlert, 14., p.status_err_text))
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .h(px(36.))
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .font_family(theme::MONO_FONT)
                                                .text_size(theme::text::XS)
                                                .line_height(px(18.))
                                                .text_color(p.body)
                                                .child(sql_text(
                                                    &pv.text,
                                                    if pv.more { " …" } else { "" },
                                                    &p,
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .min_w_0()
                                                .items_center()
                                                .gap(px(4.))
                                                .overflow_hidden()
                                                .text_size(theme::text::XXS)
                                                .line_height(px(16.))
                                                .text_color(p.faint)
                                                .children(meta),
                                        ),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                                // Keep the focus in the search field.
                                window.prevent_default();
                                if e.click_count >= 2 {
                                    let item = this.shown.get(i).cloned();
                                    this.insert(item, cx);
                                } else {
                                    this.select(i, cx);
                                }
                            }),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_detail(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = self.current().cloned() else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(BoaIcon::History.icon().size(px(20.)).text_color(p.dim))
                .into_any_element();
        };
        let failed = r.entry.error.is_some();
        let console = has_console(cx);
        let header = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(4.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.))
                    .text_size(theme::text::SM)
                    .child(
                        div()
                            .truncate()
                            .text_color(p.soft)
                            .child(connection_name(&r.connection_id, cx)),
                    )
                    .child(div().text_color(p.dim).child("/"))
                    .child(
                        div()
                            .truncate()
                            .font_family(theme::MONO_FONT)
                            .text_color(p.soft)
                            .child(r.entry.database.clone()),
                    )
                    .when_some(r.entry.source.clone(), |d, source| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(theme::text::XS)
                                .text_color(p.dim)
                                .child(format!("· {source}")),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .text_size(theme::text::XS)
                    .text_color(p.quiet)
                    .child(format!(
                        "{} · {} · ",
                        local_time(r.entry.at),
                        duration(r.entry.ms)
                    ))
                    .child(
                        div()
                            .when(failed, |d| d.text_color(p.status_err_text))
                            .child(outcome_label(&r.entry)),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(4.))
                    .when(console, |d| {
                        d.child(
                            small_button(
                                "history-insert",
                                IconName::CornerDownLeft,
                                "Insert",
                                "Insert at caret (⏎)",
                                false,
                                p,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let item = this.current().cloned();
                                this.insert(item, cx);
                            })),
                        )
                    })
                    .child(
                        small_button(
                            "history-new-console",
                            IconName::SquareTerminal,
                            "New console",
                            "Open in new console (⌘⏎)",
                            false,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            Self::open_in_console(this.current().cloned(), cx);
                        })),
                    )
                    .child(
                        small_button(
                            "history-copy",
                            IconName::Copy,
                            "Copy",
                            "Copy (⌘C)",
                            false,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(item) = this.current() {
                                let sql = item.entry.sql.clone();
                                copy_sql(&sql, cx);
                            }
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        small_button(
                            "history-delete",
                            IconName::Trash,
                            "Delete",
                            "Delete entry (⌘⌫)",
                            true,
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let item = this.current().cloned();
                            this.remove(item, cx);
                        })),
                    ),
            );
        let error = r.entry.error.clone().map(|error| {
            div()
                .id("history-error")
                .flex_none()
                .max_h(gpui::relative(0.4))
                .overflow_y_scroll()
                .border_b_1()
                .border_color(p.border)
                .bg(p.status_err_text.opacity(0.05))
                .px(px(12.))
                .py(px(8.))
                .font_family(theme::MONO_FONT)
                .text_size(theme::text::XS)
                .line_height(px(20.))
                .text_color(p.status_err_text)
                .child(error)
                .when_some(r.entry.detail.clone(), |d, detail| {
                    d.child(
                        div()
                            .text_color(p.status_err_text.opacity(0.8))
                            .child(detail),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .children(error)
            .child(
                div()
                    .id("history-sql")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .px(px(12.))
                    .py(px(10.))
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::XS)
                    .line_height(px(20.))
                    .text_color(p.body)
                    .whitespace_nowrap()
                    .child(sql_text(&r.entry.sql, "", p)),
            )
            .into_any_element()
    }

    fn render_footer(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let console = has_console(cx);
        let total = self.items.len();
        let hover = p.body;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(16.))
            .px(px(14.))
            .py(px(8.))
            .border_t_1()
            .border_color(p.border)
            .font_family(theme::MONO_FONT)
            .text_size(theme::text::XXS)
            .text_color(p.faint)
            .child("↑↓ move")
            .child(if console {
                "⏎ insert at caret"
            } else {
                "⏎ open in console"
            })
            .child("⌘⏎ new console")
            .child("⌘C copy")
            .child("⌘⌫ delete")
            .child(div().flex_1())
            .when(!recording(cx), |d| {
                d.child(
                    div()
                        .font_family(theme::UI_FONT)
                        .text_color(p.status_wait)
                        .child("Recording is off"),
                )
            })
            .child(
                div()
                    .id("history-clear")
                    .font_family(theme::UI_FONT)
                    .text_size(theme::text::XS)
                    .text_color(p.quiet)
                    .when(total == 0, |d| d.opacity(0.4))
                    .when(total > 0, |d| {
                        d.cursor_pointer()
                            .hover(move |s| s.text_color(hover))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_clear(window, cx)),
                            )
                    })
                    .child("Clear history…"),
            )
            .child("esc")
    }
}

impl Render for QueryHistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let size = window.viewport_size();
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        // `top-[8vh] h-[min(680px,84vh)] w-[1000px] max-w-[calc(100vw-32px)]`
        let (w, h) = ((vw - 32.).min(1000.), (vh * 0.84).min(680.));
        // The list: the dialog less the header (46) and the footer (~33).
        self.page = page_step(h - 46. - 33.);
        let busy = self.busy(cx);
        let list_body = if self.shown.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px(px(32.))
                .text_center()
                .text_size(theme::text::SM)
                .text_color(p.quiet)
                .child(empty_message(busy, self.items.len(), recording(cx)))
                .into_any_element()
        } else {
            uniform_list(
                "history-list",
                self.shown.len(),
                cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        let dialog = div()
            .id("query-history")
            .occlude()
            .w(px(w))
            .h(px(h))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme::radius::XL)
            .border_1()
            .border_color(p.border)
            .bg(p.popover)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                // Clicks inside keep the search field focused.
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(self.render_header(&p, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex_none()
                            .w(gpui::relative(0.6))
                            .min_h_0()
                            .py(px(4.))
                            .child(list_body),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(gpui::relative(0.4))
                            .min_h_0()
                            .border_l_1()
                            .border_color(p.border)
                            .child(self.render_detail(&p, cx)),
                    ),
            )
            .child(self.render_footer(&p, cx));
        div()
            .id("query-history-scrim")
            .key_context(crate::keymap::QUERY_HISTORY)
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_by(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_by(1, cx)))
            .on_action(
                cx.listener(|this, _: &SelectPageUp, _, cx| {
                    this.move_by(-(this.page as isize), cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SelectPageDown, _, cx| this.move_by(this.page as isize, cx)),
            )
            .on_action(cx.listener(|this, _: &Confirm, _, cx| {
                let item = this.current().cloned();
                this.insert(item, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenHistoryInConsole, _, cx| {
                Self::open_in_console(this.current().cloned(), cx);
            }))
            .on_action(cx.listener(|this, _: &CopyHistoryEntry, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &DeleteHistoryEntry, _, cx| {
                let item = this.current().cloned();
                this.remove(item, cx);
            }))
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| Self::close(cx)))
            .absolute()
            .inset_0()
            .occlude()
            .bg(p.scrim)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(vh * 0.08))
            .font_family(theme::UI_FONT)
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                Self::close(cx);
            })
            .child(dialog)
    }
}

// ── The right panel's History tab ────────────────────────────────────────────

/// RightPanel.tsx `HistoryTab`: the active connection's saved history,
/// searchable (Esc clears the search), a click opens a statement in a new
/// console.
pub struct HistoryPanel {
    input: Entity<InputState>,
    query: String,
    connection: Option<String>,
    shown: Rc<Vec<Entry>>,
    list: ListState,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for HistoryPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryPanel")
            .field("query", &self.query)
            .field("connection", &self.connection)
            .field("shown", &self.shown.len())
            .finish_non_exhaustive()
    }
}

impl HistoryPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search history"));
        let mut subscriptions = vec![cx.subscribe(&input, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                this.sync(cx);
            }
        })];
        if let Some(state) = AppState::try_global(cx).cloned() {
            subscriptions.push(cx.observe(&state.history, |this, _, cx| this.sync(cx)));
            subscriptions.push(cx.observe(&state.workspaces, |this, _, cx| this.sync(cx)));
            subscriptions.push(cx.observe(&state.prefs, |_, _, cx| cx.notify()));
        }
        let mut this = Self {
            input,
            query: String::new(),
            connection: None,
            shown: Rc::new(Vec::new()),
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    /// The search field.
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// The search text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The entries listed, newest first.
    pub fn shown(&self) -> &[Entry] {
        &self.shown
    }

    /// Follows the active connection (loading its history once) and the search.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let connection = active_db(cx).map(|k| k.connection_id);
        if connection != self.connection {
            self.connection = connection.clone();
            if let Some(id) = &connection {
                state.history.update(cx, |h, cx| h.load(id, false, cx));
            }
        }
        let shown: Vec<Entry> = connection
            .as_deref()
            .and_then(|id| state.history.read(cx).list(id))
            .map(|list| {
                let filter = HistoryFilter {
                    query: self.query.clone(),
                    errors_only: false,
                };
                filter_history(list, &filter).into_iter().cloned().collect()
            })
            .unwrap_or_default();
        if *self.shown != shown {
            self.list.reset(shown.len());
            self.shown = Rc::new(shown);
        }
        cx.notify();
    }

    /// Esc in the search field: clears it (else lets Esc through).
    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query.is_empty() {
            cx.propagate();
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.query.clear();
        self.sync(cx);
    }

    fn empty(text: &'static str, p: &Palette) -> gpui::Div {
        div()
            .flex()
            .flex_1()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .px(px(32.))
            .pb(px(64.))
            .text_size(theme::text::SM)
            .text_color(p.quiet)
            .child(BoaIcon::History.icon().size(px(20.)).text_color(p.dim))
            .child(
                div()
                    .max_w(px(260.))
                    .text_center()
                    .line_height(px(22.75))
                    .child(text),
            )
    }

    fn render_item(connection: &str, i: usize, h: &Entry, p: &Palette, now: f64) -> AnyElement {
        let pv = sql_preview(&h.sql, 3);
        let failed = h.error.is_some();
        let (conn, db, sql) = (connection.to_owned(), h.database.clone(), h.sql.clone());
        let hover = p.hover;
        div()
            .id(("history-panel-row", i))
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(12.))
            .py(px(6.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .tooltip(tooltip(format!(
                "{}\n\nClick to open in a new query tab",
                h.sql
            )))
            .on_click(move |_, _, cx| open_in_new_console(&conn, &db, &sql, cx))
            .child(
                div()
                    .max_h(px(72.))
                    .overflow_hidden()
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::XS)
                    .line_height(px(18.))
                    .text_color(p.body)
                    .child(sql_text(&pv.text, if pv.more { " …" } else { "" }, p)),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(4.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(theme::text::XS)
                    .text_color(p.dim)
                    .when(failed, |d| {
                        d.child(icon(IconName::CircleAlert, 12., p.status_err_text))
                    })
                    .child(div().flex_none().child(format!(
                        "{} · {} · {} · ",
                        h.database,
                        ago(h.at as i64, now as i64),
                        duration(h.ms)
                    )))
                    .child(
                        div()
                            .flex_none()
                            .when(failed, |d| d.text_color(p.status_err_text))
                            .child(outcome_label(h)),
                    ),
            )
            .into_any_element()
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let Some(state) = AppState::try_global(cx).cloned() else {
            return div().into_any_element();
        };
        let Some(connection) = self.connection.clone() else {
            return div().into_any_element();
        };
        let (loaded_len, loading) = {
            let h = state.history.read(cx);
            (
                h.list(&connection).map_or(0, <[Entry]>::len),
                h.is_loading(&connection),
            )
        };
        if loaded_len == 0 {
            return Self::empty(
                if loading {
                    "Loading history…"
                } else if recording(cx) {
                    "Queries you run on this connection show up here."
                } else {
                    "Query history is turned off in Settings → Queries."
                },
                &p,
            )
            .into_any_element();
        }
        let shown = self.shown.clone();
        let now = now_ms();
        let row_palette = p.clone();
        let rows = list(self.list.clone(), move |i, _, _| match shown.get(i) {
            Some(h) => Self::render_item(&connection, i, h, &row_palette, now),
            None => div().into_any_element(),
        })
        .size_full();
        let (hover, body) = (p.hover, p.body);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .on_action(
                cx.listener(|this, _: &ClearPanelSearch, window, cx| this.clear_search(window, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(p.border)
                    .child(icon(IconName::Search, 14., p.faint))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.input)
                                .appearance(false)
                                .px_0()
                                .py_0()
                                .h(px(28.))
                                .text_size(theme::text::SM),
                        ),
                    )
                    .when(!self.query.is_empty(), |d| {
                        d.child(
                            div()
                                .id("history-panel-clear")
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(px(20.))
                                .rounded(theme::radius::SM)
                                .cursor_pointer()
                                .text_color(p.faint)
                                .hover(move |s| s.bg(hover).text_color(body))
                                .tooltip(tooltip("Clear search"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.clear_search(window, cx)
                                    }),
                                )
                                .child(gpui_component::Icon::new(IconName::X).size(px(14.))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(36.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(p.border)
                    .text_size(theme::text::SM)
                    .text_color(p.quiet)
                    .child(if self.query.is_empty() {
                        "Recent"
                    } else {
                        "Matches"
                    })
                    .when(!self.shown.is_empty(), |d| {
                        d.child(div().text_color(p.dim).child(self.shown.len().to_string()))
                    }),
            )
            .when(self.shown.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .text_size(theme::text::SM)
                        .text_color(p.quiet)
                        .child(format!("No queries match “{}”.", self.query)),
                )
            })
            .child(div().flex_1().min_h_0().py(px(4.)).child(rows))
            .into_any_element()
    }
}

/// Dev hook (feature `dev-hooks`): `BOA_DEV_HISTORY=overlay|panel` opens a
/// console on the selected (else the first) connection's own database, then
/// the Query History overlay, or the right panel on its History tab, for
/// screenshots.
#[cfg(feature = "dev-hooks")]
pub mod dev {
    use gpui::App;

    use crate::app_state::AppState;
    use crate::models::catalogs::DbKey;
    use crate::models::layout::PanelKind;
    use crate::models::prefs::RightTab;

    pub const ENV: &str = "BOA_DEV_HISTORY";

    /// Applies `BOA_DEV_HISTORY` (call once the main window is open): waits
    /// (up to 10 s) for the first saved connection to connect, then opens.
    pub fn apply(cx: &mut App) {
        let Some(what) = std::env::var(ENV).ok().filter(|v| !v.trim().is_empty()) else {
            return;
        };
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
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
            cx.update(|cx| open(&what, &state, cx));
        })
        .detach();
    }

    fn open(what: &str, state: &AppState, cx: &mut App) {
        let key = state.workspaces.read(cx).active.clone().or_else(|| {
            let conns = state.connections.read(cx);
            conns.saved.first().map(|c| DbKey {
                connection_id: c.config.id.clone(),
                database: c.config.database.clone(),
            })
        });
        if let Some(key) = key {
            state.workspaces.update(cx, |w, cx| {
                w.new_query_tab_with(
                    key,
                    "select *\nfrom orders\nwhere status = 'paid';\n".into(),
                    cx,
                )
            });
        }
        match what.trim() {
            "panel" => {
                state.prefs.update(cx, |p, cx| {
                    p.update(cx, |p| p.right_tab = RightTab::History)
                });
                state
                    .layout
                    .update(cx, |l, cx| l.set_open(PanelKind::RightPanel, true, cx));
            }
            _ => state.history.update(cx, |h, cx| h.set_open(true, cx)),
        }
    }
}

#[cfg(test)]
mod tests;
