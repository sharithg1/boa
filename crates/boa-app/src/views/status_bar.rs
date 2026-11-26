//! StatusBar.tsx: the full-width footer. The Database Output toggle (⌥J),
//! breadcrumbs of the active tab (connection › database › console, or ›
//! schema › table), and on the right the caret (consoles) or row estimate
//! (tables), the encoding, and the active connection (else the first one
//! the backend lists, `Connections::first_listed`) with its environment tag and `connMeta` (latency or state); a click
//! opens the connection popover (state, server version, latency, URL,
//! Connect / Disconnect).
//!
//! The transaction indicator (TxControls.tsx `TxStatus`) sits before the
//! connection; running CSV exports (file, rows, size, Cancel) show left of
//! the caret ([`super::csv_export::export_status`]); "Restart to update"
//! shows once an update is downloaded ([`super::about::update_pill`]).

use gpui::prelude::*;
use gpui::{Anchor, App, Context, Div, SharedString, Stateful, Subscription, Window, div, px};
use gpui_component::Selectable;
use gpui_component::popover::Popover;

use super::ActiveDb;
use super::chrome::{IconName, icon};
use super::sidebar::{row, tree::ConnInfo};
use crate::app_state::AppState;
use crate::editor::CaretPos;
use crate::models::connections::ConnState;
use crate::models::layout::PanelKind;
use crate::models::workspaces::TabKind;
use crate::theme::{self, ActivePalette as _};

/// Height of the footer (`h-8`).
pub const STATUS_BAR_HEIGHT: gpui::Pixels = px(32.);

/// The status bar view.
#[derive(Debug)]
pub struct StatusBar {
    /// The focused console's caret (the root view sets it from the workspace).
    pub caret: Option<CaretPos>,
    _subscriptions: Vec<Subscription>,
}

impl StatusBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(state) = AppState::try_global(cx).cloned() {
            subscriptions.push(cx.observe(&state.layout, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.workspaces, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.connections, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.catalogs, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.transactions, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.exports, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.updates, |_, _, cx| cx.notify()));
        }
        Self {
            caret: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_caret(&mut self, caret: Option<CaretPos>, cx: &mut Context<Self>) {
        if self.caret != caret {
            self.caret = caret;
            cx.notify();
        }
    }

    /// Breadcrumbs: connection › database › (schema › table | console).
    pub fn crumbs(cx: &gpui::App) -> Vec<SharedString> {
        let Some(active) = ActiveDb::get(cx) else {
            return Vec::new();
        };
        let mut crumbs = vec![active.connection.clone(), active.database.clone()];
        let tab =
            AppState::try_global(cx).and_then(|s| s.workspaces.read(cx).active_tab().cloned());
        match tab.map(|t| t.kind) {
            Some(TabKind::Table { schema, table, .. }) => {
                crumbs.push(schema);
                crumbs.push(table);
            }
            Some(TabKind::Query { title }) => crumbs.push(title),
            None => {}
        }
        crumbs
    }
}

impl Render for StatusBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let state = AppState::try_global(cx).cloned();
        let bottom_open = state
            .as_ref()
            .is_some_and(|s| s.layout.read(cx).bottom_open());
        let active = ActiveDb::get(cx);
        // StatusBar.tsx: the active connection, else `connectionOrder[0]`
        // (the backend's order, not the sidebar's).
        let shown = active.clone().or_else(|| {
            let state = state.as_ref()?;
            let first = state.connections.read(cx).first_listed()?.config.clone();
            Some(ActiveDb::of(
                &crate::models::catalogs::DbKey {
                    connection_id: first.id,
                    database: first.database,
                },
                cx,
            ))
        });
        let tab = state
            .as_ref()
            .and_then(|s| s.workspaces.read(cx).active_tab().cloned());
        let crumbs = Self::crumbs(cx);
        let last = crumbs.len().saturating_sub(1);
        let nav = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(4.))
            .px(px(4.))
            .children(crumbs.into_iter().enumerate().map(|(i, c)| {
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(4.))
                    .when(i > 0, |d| d.child(icon(IconName::ChevronRight, 12., p.dim)))
                    .child(
                        div()
                            .truncate()
                            .text_color(if i == last { p.soft } else { p.quiet })
                            .when(i > 0, |d| {
                                d.font_family(theme::MONO_FONT).text_size(theme::text::XS)
                            })
                            .child(c),
                    )
            }))
            .when(active.is_none(), |d| {
                d.child(div().text_color(p.dim).child("No database selected"))
            });
        let is_query = tab.as_ref().is_some_and(|t| t.is_query());
        let right: Option<String> = match tab.as_ref() {
            Some(t) if !t.is_query() => t.table().and_then(|tt| tt.read(cx).meta(cx)).map(|m| {
                format!(
                    "~{} rows",
                    crate::grid::page::grouped(m.rows.max(0) as usize)
                )
            }),
            Some(_) => self.caret.map(|c| c.to_string()),
            None => None,
        };
        let body = p.body;
        let toggle = div()
            .id("status-bottom-toggle")
            .flex_none()
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::radius::MD)
            .cursor_pointer()
            .text_color(if bottom_open { p.body } else { p.quiet })
            .hover(move |s| s.bg(p.hover_strong).text_color(body))
            .tooltip(super::chrome::tooltip(if bottom_open {
                "Hide Database Output (⌥J)"
            } else {
                "Show Database Output (⌥J)"
            }))
            .on_click(|_, _, cx| {
                if let Some(state) = AppState::try_global(cx) {
                    let layout = state.layout.clone();
                    layout.update(cx, |l, cx| l.toggle(PanelKind::Bottom, cx));
                }
            })
            .child(gpui_component::Icon::new(IconName::PanelBottom).size(px(16.)));
        let connection = shown
            .as_ref()
            .map(|a| connection_popover(a, p.background, cx));
        let dim_text = |text: String, mono: bool| {
            div()
                .px(px(6.))
                .text_size(theme::text::XS)
                .text_color(p.dim)
                .when(mono, |d| d.font_family(theme::MONO_FONT))
                .child(text)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(STATUS_BAR_HEIGHT)
            .overflow_hidden()
            .border_t_1()
            .border_color(p.border)
            .bg(p.background)
            .px(px(8.))
            .text_size(theme::text::SM)
            .whitespace_nowrap()
            .text_color(p.quiet)
            .child(toggle)
            .child(nav)
            .child(div().flex_1())
            .children(super::csv_export::export_status(cx))
            .children(right.map(|r| dim_text(r, true)))
            .when(is_query, |d| d.child(dim_text("UTF-8".into(), false)))
            .children(super::tx_controls::tx_status(cx))
            .children(connection)
            .when(shown.is_none(), |d| {
                d.child(div().px(px(6.)).child("no connection"))
            })
            .children(super::about::update_pill(cx))
    }
}

/// Sidebar.tsx `connMeta`: the latency while connected (`"402 ms"`, else
/// `"connected"`), otherwise the state (`"offline"` when disconnected).
pub fn conn_meta(state: &ConnState, latency_ms: Option<u64>) -> String {
    match state {
        ConnState::Connected { .. } => {
            latency_ms.map_or_else(|| "connected".to_owned(), |ms| format!("{ms} ms"))
        }
        ConnState::Connecting => "connecting".to_owned(),
        ConnState::Reconnecting { .. } => "reconnecting".to_owned(),
        ConnState::Failed { .. } => "failed".to_owned(),
        ConnState::Disconnected => "offline".to_owned(),
    }
}

/// The label of the popover's button (StatusBar.tsx `ConnectionPopover`):
/// Disconnect while connected, else Connect (disabled while connecting).
pub fn popover_action(state: &ConnState) -> (&'static str, bool) {
    match state {
        ConnState::Connected { .. } => ("Disconnect", true),
        ConnState::Connecting => ("Connecting…", false),
        ConnState::Reconnecting { .. } => ("Reconnecting… Retry now", true),
        ConnState::Failed { .. } | ConnState::Disconnected => ("Connect", true),
    }
}

/// The popover's trigger: a row that keeps its hover look while open.
#[derive(IntoElement)]
struct Trigger {
    el: Stateful<Div>,
    open: bool,
}

impl Selectable for Trigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for Trigger {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let hover = cx.palette().hover;
        self.el.when(self.open, |d| d.bg(hover))
    }
}

/// StatusBar.tsx `ConnectionPopover`: the connection icon (state badge),
/// name, tag and `connMeta`; a click shows the state, server version,
/// latency and URL with a Connect / Disconnect button.
fn connection_popover(active: &ActiveDb, cutout: gpui::Hsla, cx: &App) -> Popover {
    let p = cx.palette().clone();
    let id = active.connection_id.to_string();
    let conns = AppState::try_global(cx).map(|s| s.connections.read(cx));
    let latency = conns.and_then(|c| c.latency_ms(&id));
    let meta = conn_meta(&active.status, latency);
    let info = active
        .saved
        .as_ref()
        .map(|s| ConnInfo::new(s, active.status.clone()));
    let (hover, body) = (p.hover, p.body);
    let trigger = div()
        .id("status-connection")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(24.))
        .px(px(6.))
        .rounded(theme::radius::MD)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(body))
        .children(info.as_ref().map(|i| row::connection_icon(i, cutout, cx)))
        .child(active.connection.clone())
        .children(info.as_ref().and_then(|i| row::connection_tag(i, cx)))
        .child(
            div()
                .text_size(theme::text::XS)
                .text_color(p.dim)
                .child(meta),
        )
        .child(icon(IconName::ChevronDown, 12., p.quiet));
    let name = active.connection.clone();
    let state = active.status.clone();
    let version = active.server_version().map(str::to_owned);
    let url = conns
        .and_then(|c| c.connection_string(&id, None))
        .unwrap_or_default();
    Popover::new("status-connection-popover")
        .anchor(Anchor::BottomRight)
        .trigger(Trigger {
            el: trigger,
            open: false,
        })
        .content(move |_, _, cx| {
            let p = cx.palette().clone();
            let mut details = "PostgreSQL".to_owned();
            if let Some(v) = &version {
                details.push_str(&format!(" · {v}"));
            }
            if let Some(ms) = latency {
                details.push_str(&format!(" · {ms} ms"));
            }
            let (label, enabled) = popover_action(&state);
            let connected = matches!(state, ConnState::Connected { .. });
            let id = id.clone();
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .w(px(280. - 24.))
                // The status bar's nowrap would reach in here.
                .whitespace_normal()
                .text_size(theme::text::XS)
                .child(
                    div()
                        .font_family(theme::MONO_FONT)
                        .text_color(p.body)
                        .child(format!("{name} · {}", state.as_str())),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .text_color(p.quiet)
                        .child(div().w_full().child(details))
                        .child(div().w_full().child(url.clone())),
                )
                .child(
                    div()
                        .id("status-connection-action")
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(28.))
                        .rounded(theme::radius::MD)
                        .border_1()
                        .border_color(p.border_strong)
                        .bg(p.card)
                        .text_size(theme::text::SM)
                        .text_color(p.soft)
                        .when(enabled, |d| {
                            d.cursor_pointer()
                                .hover(move |s| s.bg(p.hover_strong).text_color(p.body))
                                .on_click(move |_, _, cx| {
                                    if let Some(state) = AppState::try_global(cx).cloned() {
                                        state.connections.update(cx, |c, cx| {
                                            if connected {
                                                c.disconnect(&id, cx);
                                            } else {
                                                c.connect(&id, cx);
                                            }
                                        });
                                    }
                                })
                        })
                        .when(!enabled, |d| d.opacity(0.5))
                        .child(label),
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conn_meta_is_the_latency_or_the_state() {
        let connected = ConnState::Connected {
            server_version: "17.0".into(),
            databases: Vec::new(),
        };
        assert_eq!(conn_meta(&connected, Some(402)), "402 ms");
        assert_eq!(conn_meta(&connected, None), "connected");
        assert_eq!(conn_meta(&ConnState::Connecting, Some(3)), "connecting");
        assert_eq!(
            conn_meta(&ConnState::Reconnecting { attempt: 1 }, None),
            "reconnecting"
        );
        assert_eq!(
            conn_meta(
                &ConnState::Failed {
                    message: String::new()
                },
                None
            ),
            "failed"
        );
        assert_eq!(conn_meta(&ConnState::Disconnected, None), "offline");
    }

    #[test]
    fn popover_button_follows_the_state() {
        let connected = ConnState::Connected {
            server_version: String::new(),
            databases: Vec::new(),
        };
        assert_eq!(popover_action(&connected), ("Disconnect", true));
        assert_eq!(
            popover_action(&ConnState::Connecting),
            ("Connecting…", false)
        );
        assert_eq!(
            popover_action(&ConnState::Reconnecting { attempt: 2 }),
            ("Reconnecting… Retry now", true)
        );
        assert_eq!(popover_action(&ConnState::Disconnected), ("Connect", true));
    }
}
