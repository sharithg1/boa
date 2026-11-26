//! RightPanel.tsx: its own header row of tab pills (Structure / Info /
//! History, the choice persisted as prefs `rightTab`) with the panel toggle
//! at the far right, then hairline-ruled sections for the selected database
//! (Workspaces `active`):
//!
//! - **Structure**: the active table tab's table: kind, qualified name, row
//!   estimate and size, then its columns (primary-key icon, type, `?` when
//!   nullable, the default as a tooltip). From the explorer's catalog.
//! - **Info**: the connection (engine, name, state, server version) and the
//!   database's loaded schemas with their table counts.
//! - **History**: the connection's saved query history
//!   ([`super::query_history::HistoryPanel`]).
//!
//! The width (prefs `rightPanelWidth`, 280–800) and the resize handle on the
//! left edge are applied by the root view.

use boa_core::grid::format::bytes;
use boa_core::model::{Table, TableKind};
use boa_core::state::csv_export::grouped;
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FontWeight, MouseButton, SharedString, Subscription, Window, div, px,
};

use super::chrome::{BoaIcon, DotKind, IconName, WindowDrag, icon, icon_button, status_dot};
use super::query_history::HistoryPanel;
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;
use crate::models::layout::PanelKind;
use crate::models::prefs::RightTab;
use crate::models::workspaces::TabKind;
use crate::theme::{self, ActivePalette as _};

fn tab_icon(tab: RightTab) -> gpui_component::Icon {
    match tab {
        RightTab::Structure => gpui_component::Icon::new(IconName::Columns3),
        RightTab::Info => gpui_component::Icon::new(IconName::Info),
        RightTab::History => BoaIcon::History.icon(),
    }
}

/// StatusDot.tsx `connDot`.
pub fn conn_dot(state: &ConnState) -> DotKind {
    match state {
        ConnState::Connected { .. } => DotKind::Ok,
        ConnState::Connecting | ConnState::Reconnecting { .. } => DotKind::Wait,
        ConnState::Failed { .. } => DotKind::Err,
        ConnState::Disconnected => DotKind::Disc,
    }
}

/// StatusDot.tsx `DOT_LABEL`.
pub fn dot_label(kind: DotKind) -> &'static str {
    match kind {
        DotKind::Run => "Busy",
        DotKind::Wait => "Connecting",
        DotKind::Err => "Failed",
        DotKind::Ok => "Connected",
        DotKind::Idle => "Idle",
        DotKind::Disc => "Disconnected",
    }
}

/// The Structure tab's summary line: "~182,000 rows · 8.0 KB".
pub fn table_summary(t: &Table) -> String {
    format!(
        "~{} rows · {}",
        grouped(t.rows.max(0) as u64),
        bytes(t.size_bytes.max(0) as u64)
    )
}

/// What the Structure tab shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Structure {
    /// No table tab is active.
    NoTable,
    /// The tab's table isn't in the catalog.
    NotFound,
    Table {
        schema: String,
        table: Box<Table>,
    },
}

/// The right panel view.
#[derive(Debug)]
pub struct RightPanel {
    drag: WindowDrag,
    /// The History tab, created the first time it shows.
    history: Option<Entity<HistoryPanel>>,
    _subscriptions: Vec<Subscription>,
}

impl RightPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(state) = AppState::try_global(cx).cloned() {
            subscriptions.push(cx.observe(&state.prefs, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.workspaces, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.connections, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.catalogs, |_, _, cx| cx.notify()));
        }
        Self {
            drag: WindowDrag::new(),
            history: None,
            _subscriptions: subscriptions,
        }
    }

    /// The History tab's view, once it has shown.
    pub fn history_panel(&self) -> Option<&Entity<HistoryPanel>> {
        self.history.as_ref()
    }

    /// The selected tab (prefs `rightTab`).
    pub fn tab(cx: &App) -> RightTab {
        AppState::try_global(cx).map_or(RightTab::Structure, |s| s.prefs.read(cx).get().right_tab)
    }

    /// The selected database (Workspaces `active`).
    pub fn active(cx: &App) -> Option<DbKey> {
        AppState::try_global(cx)?.workspaces.read(cx).active.clone()
    }

    /// What the Structure tab shows for the selected database.
    pub fn structure(cx: &App) -> Structure {
        let Some(state) = AppState::try_global(cx) else {
            return Structure::NoTable;
        };
        let workspaces = state.workspaces.read(cx);
        let Some(key) = workspaces.active.as_ref() else {
            return Structure::NoTable;
        };
        let tab = workspaces.active_workspace().and_then(|ws| {
            let id = ws.active_tab.as_ref()?;
            ws.tabs.iter().find(|t| &t.id == id)
        });
        let Some(TabKind::Table { schema, table, .. }) = tab.map(|t| &t.kind) else {
            return Structure::NoTable;
        };
        match state.catalogs.read(cx).find_table(key, schema, table) {
            Some(t) => Structure::Table {
                schema: schema.to_string(),
                table: Box::new(t.clone()),
            },
            None => Structure::NotFound,
        }
    }

    fn empty(
        empty_icon: gpui_component::Icon,
        text: impl Into<SharedString>,
        cx: &App,
    ) -> gpui::Div {
        let p = cx.palette();
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
            .child(empty_icon.size(px(20.)).text_color(p.dim))
            .child(
                div()
                    .max_w(px(260.))
                    .text_center()
                    // leading-relaxed: 1.625 × 13 px.
                    .line_height(px(21.125))
                    .child(text.into()),
            )
    }

    /// Cursor's "1 File Changed" strip: a hairline-ruled header above a list.
    fn section(title: &str, count: Option<usize>, cx: &App) -> gpui::Div {
        let p = cx.palette();
        div().flex().flex_col().pb(px(4.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(36.))
                .px(px(12.))
                .border_y_1()
                .border_color(p.border)
                .text_size(theme::text::SM)
                .text_color(p.quiet)
                .child(title.to_owned())
                .children(
                    count
                        .filter(|c| *c > 0)
                        .map(|c| div().text_color(p.dim).child(c.to_string())),
                ),
        )
    }

    /// The pill + mono name, the heading and the line under it.
    fn header(
        pill: &str,
        pill_is_view: bool,
        name: String,
        heading: String,
        line: gpui::Div,
        cx: &App,
    ) -> gpui::Div {
        let p = cx.palette();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(12.))
            .pt(px(6.))
            .pb(px(12.))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(8.))
                    .text_size(theme::text::SM)
                    .child(
                        div()
                            .flex_none()
                            .rounded(px(5.))
                            .px(px(6.))
                            .py(px(1.))
                            .when(pill_is_view, |d| d.bg(p.muted).text_color(p.soft))
                            .when(!pill_is_view, |d| {
                                d.bg(p.status_run.opacity(0.15)).text_color(p.link)
                            })
                            .child(pill.to_owned()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(theme::MONO_FONT)
                            .text_size(theme::text::XS)
                            .text_color(p.soft)
                            .child(name),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .text_size(theme::text::LG)
                    // leading-snug: 1.375 × 16 px.
                    .line_height(px(22.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.body)
                    .child(heading),
            )
            .child(line)
    }

    fn render_structure(cx: &App) -> gpui::Div {
        let p = cx.palette().clone();
        let (schema, table) = match Self::structure(cx) {
            Structure::NoTable => {
                return Self::empty(
                    gpui_component::Icon::new(IconName::Columns3),
                    "Open a table to see its columns.",
                    cx,
                );
            }
            Structure::NotFound => {
                return Self::empty(
                    gpui_component::Icon::new(IconName::Columns3),
                    "Table not found.",
                    cx,
                );
            }
            Structure::Table { schema, table } => (schema, table),
        };
        let is_view = table.kind == TableKind::View;
        let columns = table.columns.iter().enumerate().map(|(i, c)| {
            div()
                .id(("structure-column", i))
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(32.))
                .px(px(12.))
                .text_size(theme::text::SM)
                .when_some(c.default.clone(), |d, default| {
                    d.tooltip(super::chrome::tooltip(format!("default {default}")))
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(c.pk, |d| {
                            d.child(icon(IconName::KeyRound, 14., p.status_wait))
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO_FONT)
                        .text_color(p.soft)
                        .child(c.name.clone()),
                )
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .flex_none()
                        .font_family(theme::MONO_FONT)
                        .text_size(theme::text::XS)
                        .text_color(p.quiet)
                        .child(div().text_color(p.sql_type).child(c.ty.clone()))
                        .when(c.nullable, |d| d.child("?")),
                )
        });
        div().flex().flex_col().flex_1().min_h_0().child(
            div()
                .id("structure-tab")
                .size_full()
                .overflow_y_scroll()
                .child(Self::header(
                    if is_view { "View" } else { "Table" },
                    is_view,
                    format!("{schema}.{}", table.name),
                    table.name.clone(),
                    div()
                        .text_size(theme::text::SM)
                        .text_color(p.quiet)
                        .child(table_summary(&table)),
                    cx,
                ))
                .child(
                    Self::section("Columns", Some(table.columns.len()), cx)
                        .child(div().flex().flex_col().py(px(4.)).children(columns)),
                ),
        )
    }

    fn render_info(key: &DbKey, cx: &App) -> gpui::Div {
        let p = cx.palette().clone();
        let Some(state) = AppState::try_global(cx) else {
            return div();
        };
        let connections = state.connections.read(cx);
        let Some(conn) = connections.get(&key.connection_id) else {
            return div().flex_1();
        };
        let conn_state = connections.state_of(&key.connection_id);
        let dot = conn_dot(&conn_state);
        let mut status = dot_label(dot).to_owned();
        if let ConnState::Connected { server_version, .. } = &conn_state
            && !server_version.is_empty()
        {
            status.push_str(&format!(" · {server_version}"));
        }
        let catalogs = state.catalogs.read(cx);
        let catalog = catalogs.get(key);
        let schemas: Vec<(String, usize)> = catalog
            .map(|c| {
                c.schemas
                    .iter()
                    .map(|s| (s.name.clone(), s.tables.len()))
                    .collect()
            })
            .unwrap_or_default();
        let tables = catalog.map_or(0, |c| c.table_count());
        let rows = schemas.iter().map(|(name, n)| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(32.))
                .px(px(12.))
                .text_size(theme::text::SM)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO_FONT)
                        .text_color(p.soft)
                        .child(name.clone()),
                )
                .child(
                    div()
                        .ml_auto()
                        .flex_none()
                        .text_size(theme::text::XS)
                        .text_color(p.quiet)
                        .child(format!("{n} table{}", if *n == 1 { "" } else { "s" })),
                )
        });
        div().flex().flex_col().flex_1().min_h_0().child(
            div()
                .id("info-tab")
                .size_full()
                .overflow_y_scroll()
                .child(Self::header(
                    "PostgreSQL",
                    false,
                    conn.config.name.clone(),
                    key.database.clone(),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(theme::text::SM)
                        .text_color(p.quiet)
                        .child(status_dot(dot, 6., cx))
                        .child(status),
                    cx,
                ))
                .child(
                    Self::section("Schemas", Some(schemas.len()), cx).child(
                        div()
                            .flex()
                            .flex_col()
                            .py(px(4.))
                            .children(rows)
                            .when(tables == 0, |d| {
                                d.child(
                                    div()
                                        .px(px(12.))
                                        .py(px(8.))
                                        .text_size(theme::text::SM)
                                        .text_color(p.quiet)
                                        .child("No tables"),
                                )
                            }),
                    ),
                ),
        )
    }
}

impl Render for RightPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let selected = Self::tab(cx);
        let tabs = RightTab::ALL.map(|tab| {
            let is_selected = tab == selected;
            div()
                .id(SharedString::from(format!("right-tab-{}", tab.as_str())))
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(28.))
                .min_w_0()
                .px(px(6.))
                .rounded(theme::radius::MD)
                .cursor_pointer()
                .text_size(theme::text::BASE)
                .when(is_selected, |d| d.bg(p.card).text_color(p.body))
                .when(!is_selected, |d| {
                    d.text_color(p.quiet)
                        .hover(|s| s.bg(p.hover).text_color(p.soft))
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    if let Some(state) = AppState::try_global(cx) {
                        let prefs = state.prefs.clone();
                        prefs.update(cx, |prefs, cx| prefs.update(cx, |p| p.right_tab = tab));
                    }
                })
                .child(tab_icon(tab).size(px(14.)))
                .child(div().min_w_0().truncate().child(tab.label()))
        });
        let header = div()
            .id("right-panel-header")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(40.))
            .px(px(8.))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(2.))
                    .children(tabs),
            )
            .child(div().flex_1().h_full())
            .child(
                icon_button(
                    "right-panel-toggle",
                    IconName::PanelRight,
                    16.,
                    "Hide panel (⌥E)",
                    cx,
                )
                .on_click(|_, _, cx| {
                    if let Some(state) = AppState::try_global(cx) {
                        let layout = state.layout.clone();
                        layout.update(cx, |l, cx| l.toggle(PanelKind::RightPanel, cx));
                    }
                }),
            );
        let body = match (Self::active(cx), selected) {
            (None, _) => Self::empty(
                gpui_component::Icon::new(IconName::Table2),
                "Select a database.",
                cx,
            ),
            (Some(_), RightTab::Structure) => Self::render_structure(cx),
            (Some(key), RightTab::Info) => Self::render_info(&key, cx),
            (Some(_), RightTab::History) => {
                let panel = self
                    .history
                    .get_or_insert_with(|| cx.new(|cx| HistoryPanel::new(window, cx)))
                    .clone();
                div().flex().flex_col().flex_1().min_h_0().child(panel)
            }
        };
        div()
            .key_context(crate::keymap::RIGHT_PANEL)
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(p.background)
            .border_l_1()
            .border_color(p.border)
            .child(self.drag.apply(header))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_and_dots_match_the_source() {
        let t = Table {
            name: "orders".into(),
            rows: 100_000,
            size_bytes: 1536,
            ..Table::default()
        };
        assert_eq!(table_summary(&t), "~100,000 rows · 1.5 KB");
        assert_eq!(dot_label(conn_dot(&ConnState::Connecting)), "Connecting");
        assert_eq!(
            dot_label(conn_dot(&ConnState::Reconnecting { attempt: 1 })),
            "Connecting"
        );
        assert_eq!(
            dot_label(conn_dot(&ConnState::Disconnected)),
            "Disconnected"
        );
        assert_eq!(
            dot_label(conn_dot(&ConnState::Failed {
                message: String::new()
            })),
            "Failed"
        );
    }
}
