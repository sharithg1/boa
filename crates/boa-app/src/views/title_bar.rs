//! TitleBar.tsx: the header of the main column, under the transparent macOS
//! title bar (it doubles as the tab row). It holds the database chip (opens
//! the ⌘K palette), the selected database's tabs as pills (consoles and
//! tables; a spinner while running, italic while a preview), a "+" menu
//! (new query, open table), a "…" menu of connection actions and, when the
//! right panel is closed, its toggle. With the sidebar closed it also takes
//! the traffic-light inset and the sidebar toggle. Its empty parts drag the
//! window; a double click zooms.
//!
//! Tabs activate on click, close on × or middle click, pin on double click
//! and reorder by dragging one onto another; the strip scrolls horizontally
//! when they overflow.

use gpui::prelude::*;
use gpui::{
    App, ClipboardItem, Context, Entity, FontWeight, MouseButton, ScrollHandle, SharedString,
    Subscription, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{Disableable as _, Sizable as _};

use super::ActiveDb;
use super::chrome::{self, IconName, WindowDrag, icon, icon_button};
use crate::actions::TogglePalette;
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::layout::{Layout, PanelKind};
use crate::models::prefs::RightTab;
use crate::models::toasts::NewToast;
use crate::models::workspaces::{TabKind, Workspaces};
use crate::theme::{self, ActivePalette as _};

/// Height of the row (`h-10`).
pub const TITLE_BAR_HEIGHT: gpui::Pixels = px(40.);

/// store.ts `connectionString`: `postgresql://user@host:port/database`.
pub fn connection_string(user: &str, host: &str, port: u16, database: &str) -> String {
    let auth = if user.is_empty() {
        String::new()
    } else {
        format!("{}@", encode_uri_component(user))
    };
    let port = if port == 0 {
        String::new()
    } else {
        format!(":{port}")
    };
    format!("postgresql://{auth}{host}{port}/{database}")
}

/// JS `encodeURIComponent`.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A tab being dragged (its drag preview).
#[derive(Debug, Clone)]
pub struct DraggedTab {
    pub key: DbKey,
    pub id: SharedString,
    pub label: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        div()
            .h(px(28.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(theme::radius::MD)
            .bg(p.hover_strong)
            .text_color(p.body)
            .text_size(theme::text::BASE)
            .child(self.label.clone())
    }
}

/// The title bar view.
#[derive(Debug)]
pub struct TitleBar {
    workspaces: Option<Entity<Workspaces>>,
    scroll: ScrollHandle,
    drag: WindowDrag,
    _subscriptions: Vec<Subscription>,
}

impl TitleBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let state = AppState::try_global(cx).cloned();
        if let Some(state) = &state {
            subscriptions.push(cx.observe(&state.layout, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.workspaces, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.connections, |_, _, cx| cx.notify()));
        }
        Self {
            workspaces: state.map(|s| s.workspaces),
            scroll: ScrollHandle::new(),
            drag: WindowDrag::new(),
            _subscriptions: subscriptions,
        }
    }

    fn render_database_title(&self, active: &ActiveDb, cx: &App) -> impl IntoElement {
        let p = cx.palette();
        let (quiet, body, hover, dim) = (p.quiet, p.body, p.hover, p.dim);
        let url = active.saved.as_ref().map(|s| {
            connection_string(
                &s.config.user,
                &s.config.host,
                s.config.port,
                &active.database,
            )
        });
        let tip = format!(
            "{}\n{}{}\nSearch (⌘K)",
            active.database,
            active.connection,
            url.map(|u| format!(" · {u}")).unwrap_or_default()
        );
        div()
            .id("database-title")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .max_w(px(200.))
            .min_w_0()
            .ml(px(-6.))
            .px(px(6.))
            .rounded(theme::radius::MD)
            .cursor_pointer()
            .text_color(quiet)
            .hover(move |s| s.bg(hover).text_color(body))
            .tooltip(chrome::tooltip(tip))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, window, cx| window.dispatch_action(Box::new(TogglePalette), cx))
            .child(icon(IconName::Database, 14., active.color().unwrap_or(dim)))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::SM)
                    .child(active.database.clone()),
            )
            .child(icon(IconName::ChevronDown, 14., dim))
    }

    fn render_tabs(&self, active: &ActiveDb, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let key = active.key();
        let tabs = self
            .workspaces
            .as_ref()
            .and_then(|w| w.read(cx).workspace(&key).cloned())
            .unwrap_or_default();
        let catalog_views: Vec<(String, String)> = AppState::try_global(cx)
            .and_then(|s| {
                s.catalogs.read(cx).get(&key).map(|c| {
                    c.schemas
                        .iter()
                        .flat_map(|s| {
                            s.tables
                                .iter()
                                .filter(|t| t.kind == boa_core::model::TableKind::View)
                                .map(|t| (s.name.clone(), t.name.clone()))
                                .collect::<Vec<_>>()
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        let workspaces = self.workspaces.clone();
        let strip = tabs.tabs.iter().enumerate().map(|(i, tab)| {
            let is_active = tabs.active_tab.as_ref() == Some(&tab.id);
            let id = tab.id.clone();
            let label = tab.label();
            let group: SharedString = format!("title-tab-{id}").into();
            let (preview, is_table, title) = match &tab.kind {
                TabKind::Query { title } => (false, false, title.to_string()),
                TabKind::Table {
                    schema,
                    table,
                    preview,
                } => (*preview, true, format!("{schema}.{table}")),
            };
            let is_view = matches!(&tab.kind, TabKind::Table { schema, table, .. }
                if catalog_views.iter().any(|(s, t)| s == schema.as_ref() && t == table.as_ref()));
            let kind_icon = if !is_table {
                IconName::FileCode
            } else if is_view {
                IconName::Eye
            } else {
                IconName::Table2
            };
            let running = tab.is_running(cx);
            let (hover_bg, soft) = (p.hover, p.soft);
            let (close_hover_bg, body) = (p.hover_strong, p.body);
            let (on_ws, on_key, on_id) = (workspaces.clone(), key.clone(), id.clone());
            let on =
                move |f: fn(&mut Workspaces, &DbKey, &SharedString, &mut Context<Workspaces>)| {
                    let (ws, key, id) = (on_ws.clone(), on_key.clone(), on_id.clone());
                    move |cx: &mut App| {
                        if let Some(ws) = &ws {
                            ws.update(cx, |w, cx| f(w, &key, &id, cx));
                        }
                    }
                };
            let activate = on(|w, k, id, cx| w.activate(k, id, cx));
            let pin = on(|w, k, id, cx| w.pin_tab(k, id, cx));
            let close = on(|w, k, id, cx| w.close_tab(k.clone(), id.clone(), cx));
            let close_middle = on(|w, k, id, cx| w.close_tab(k.clone(), id.clone(), cx));
            let dragged = DraggedTab {
                key: key.clone(),
                id: id.clone(),
                label: label.clone(),
            };
            let drop_ws = workspaces.clone();
            let drop_key = key.clone();
            div()
                .id(SharedString::from(format!("title-tab-{id}")))
                .group(group.clone())
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .pl(px(10.))
                .pr_1()
                .rounded(theme::radius::MD)
                .text_size(theme::text::BASE)
                .when(is_active, |d| d.bg(p.card).text_color(p.body))
                .when(!is_active, |d| {
                    d.text_color(p.quiet)
                        .hover(move |s| s.bg(hover_bg).text_color(soft))
                })
                .tooltip(chrome::tooltip(title))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_click(move |event, _, cx| {
                    if event.click_count() >= 2 {
                        pin(cx);
                    } else {
                        activate(cx);
                    }
                })
                .on_mouse_up(MouseButton::Middle, move |_, _, cx| close_middle(cx))
                .on_drag(dragged, |tab, _, _, cx| cx.new(|_| tab.clone()))
                .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(hover_bg))
                .on_drop(move |dragged: &DraggedTab, _, cx| {
                    if dragged.key == drop_key
                        && let Some(ws) = &drop_ws
                    {
                        ws.update(cx, |w, cx| w.move_tab(&drop_key, &dragged.id, i, cx));
                    }
                })
                .child(if running {
                    Spinner::new()
                        .xsmall()
                        .color(p.status_run)
                        .into_any_element()
                } else {
                    icon(kind_icon, 14., p.quiet).into_any_element()
                })
                .child(
                    div()
                        .whitespace_nowrap()
                        .when(is_table, |d| {
                            d.font_family(theme::MONO_FONT).text_size(theme::text::SM)
                        })
                        .when(preview, |d| d.italic())
                        .child(label.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("title-tab-close-{}", tab.id)))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(p.quiet)
                        .hover(move |s| s.bg(close_hover_bg).text_color(body))
                        .when(!is_active, |d| {
                            d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.))
                        })
                        .tooltip(chrome::tooltip(format!("Close tab {label}")))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            close(cx);
                        })
                        // No colour of its own: the button's text colour (brighter on hover).
                        .child(gpui_component::Icon::new(IconName::X).size(px(12.))),
                )
        });
        if let Some(i) = tabs
            .active_tab
            .as_ref()
            .and_then(|a| tabs.tabs.iter().position(|t| &t.id == a))
        {
            self.scroll.scroll_to_item(i);
        }
        div()
            .id("title-tabs")
            .ml_2()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(2.))
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .children(strip)
    }

    fn render_new_tab(&self, active: &ActiveDb) -> impl IntoElement {
        let key = active.key();
        // 24 px with an 18 px glyph, like TitleBar.tsx's IconButton: the
        // rem-based `.xsmall()` came out 20 px with a 10 px glyph (rem is 13 px).
        Button::new("title-new-tab")
            .ghost()
            .with_size(px(24.))
            .icon(IconName::Plus)
            .tooltip("New tab")
            .disabled(!active.is_connected())
            .dropdown_menu(move |menu, _, _| {
                let key = key.clone();
                menu.min_w(px(220.))
                    .item(
                        chrome::menu_row(IconName::FileCode, "New query", "⌘T").on_click(
                            move |_, _, cx| {
                                if let Some(state) = AppState::try_global(cx).cloned() {
                                    let key = key.clone();
                                    state
                                        .workspaces
                                        .update(cx, |w, cx| w.new_query_tab(key, cx));
                                }
                            },
                        ),
                    )
                    .item(
                        chrome::menu_row(IconName::Search, "Open table…", "⌘K").on_click(
                            |_, window, cx| window.dispatch_action(Box::new(TogglePalette), cx),
                        ),
                    )
            })
    }

    fn render_connection_actions(&self, active: &ActiveDb) -> impl IntoElement {
        let url = active
            .saved
            .as_ref()
            .map(|s| {
                connection_string(
                    &s.config.user,
                    &s.config.host,
                    s.config.port,
                    &active.database,
                )
            })
            .unwrap_or_default();
        let id = active.connection_id.to_string();
        let connected = active.is_connected();
        Button::new("title-connection-actions")
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .tooltip("Connection actions")
            .dropdown_menu(move |menu, _, _| {
                let (url, id) = (url.clone(), id.clone());
                menu.min_w(px(224.))
                    .item(
                        PopupMenuItem::new("Copy connection URL")
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                if let Some(state) = AppState::try_global(cx).cloned() {
                                    let title = format!("Copied {url}");
                                    state.toasts.update(cx, |t, cx| {
                                        t.push_for(
                                            NewToast::info(title.clone()),
                                            Some(std::time::Duration::from_millis(3200)),
                                            cx,
                                        );
                                    });
                                }
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Connection details")
                            .icon(IconName::Info)
                            .on_click(|_, _, cx| {
                                if let Some(state) = AppState::try_global(cx).cloned() {
                                    state.prefs.update(cx, |p, cx| {
                                        p.update(cx, |p| p.right_tab = RightTab::Info)
                                    });
                                    state.layout.update(cx, |l, cx| {
                                        l.set_open(PanelKind::RightPanel, true, cx)
                                    });
                                }
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Disconnect")
                            .icon(IconName::Unplug)
                            .disabled(!connected)
                            .on_click(move |_, _, cx| {
                                if let Some(state) = AppState::try_global(cx).cloned() {
                                    let id = id.clone();
                                    state.connections.update(cx, |c, cx| c.disconnect(&id, cx));
                                }
                            }),
                    )
            })
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let layout = AppState::try_global(cx).map(|s| s.layout.clone());
        let (sidebar_open, right_open) = layout.as_ref().map_or((true, true), |l| {
            let l = l.read(cx);
            (l.sidebar_open(), l.right_panel_open())
        });
        let active = ActiveDb::get(cx);
        let toggle = |panel: PanelKind, layout: Option<Entity<Layout>>| {
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                if let Some(layout) = &layout {
                    layout.update(cx, |l, cx| l.toggle(panel, cx));
                }
            }
        };
        let row = div()
            .id("title-bar")
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(TITLE_BAR_HEIGHT)
            .pr_2()
            .pl_3()
            .bg(p.background)
            .when(!sidebar_open, |d| {
                d.pl(chrome::traffic_lights_inset(window))
            })
            .when(!sidebar_open, |d| {
                d.child(
                    div().mr_1().flex_none().child(
                        icon_button(
                            "title-sidebar-toggle",
                            IconName::PanelLeft,
                            16.,
                            "Show sidebar (⌥B)",
                            cx,
                        )
                        .on_click(toggle(PanelKind::Sidebar, layout.clone())),
                    ),
                )
            })
            .when_some(active.as_ref(), |d, active| {
                d.child(self.render_database_title(active, cx))
                    .child(self.render_tabs(active, cx))
                    .child(
                        div()
                            .ml(px(2.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(self.render_new_tab(active)),
                    )
            })
            // The spacer: drags the window.
            .child(div().flex_1().min_w(px(16.)).h_full())
            .when_some(active.as_ref(), |d, active| {
                d.child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.render_connection_actions(active)),
                )
            })
            .when(!right_open, |d| {
                d.child(
                    icon_button(
                        "title-right-panel-toggle",
                        IconName::PanelRight,
                        16.,
                        "Show panel (⌥E)",
                        cx,
                    )
                    .on_click(toggle(PanelKind::RightPanel, layout.clone())),
                )
            })
            .font_weight(FontWeight::NORMAL);
        self.drag.apply(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_urls_like_store_ts() {
        assert_eq!(
            connection_string("boa", "localhost", 5432, "boa_dev"),
            "postgresql://boa@localhost:5432/boa_dev"
        );
        assert_eq!(
            connection_string("a b@c", "db", 0, "x"),
            "postgresql://a%20b%40c@db/x"
        );
        assert_eq!(
            connection_string("", "/tmp", 5432, "d"),
            "postgresql:///tmp:5432/d"
        );
    }
}
