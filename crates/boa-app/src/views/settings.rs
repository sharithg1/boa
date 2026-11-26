//! SettingsView.tsx: Settings (linked from the sidebar / ⌘,), laid out like
//! Cursor's: a full-height nav column (Back, search, pages) beside a centred
//! column of titled cards whose rows carry a label, a description and a
//! right-aligned control. Covers the workspace until Back / Esc / ⌘,.
//!
//! Every pref of General is wired to [`crate::models::prefs::Prefs`] (switches
//! and choice menus), with search (⌘F), "Changes save automatically" /
//! "Saved", and "Reset to defaults" (keeps the layout). Connections lists
//! the saved connections once Phase 2 has them; Keyboard lists the shortcuts.
//!
//! Esc closes, also from the search field unless it has text (then the field
//! keeps the key), as in the source.

use std::time::Duration;

use boa_core::state::prefs::Prefs as CorePrefs;
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton, SharedString,
    Subscription, Task, Window, div, px,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};

use super::chrome::{IconName, WindowDrag, icon};
use super::command_palette::tile;
use crate::actions::{CloseSettings, FocusSettingsSearch};
use crate::app_state::AppState;
use crate::models::connections::ConnState;
use crate::models::layout::SettingsPage;
use crate::theme::{self, ActivePalette as _, Palette};

/// How long "Saved" shows after a change.
const SAVED_FOR: Duration = Duration::from_millis(1600);

/// A pref row's control.
#[derive(Debug, Clone, Copy)]
enum Control {
    Switch(fn(&CorePrefs) -> bool, fn(&mut CorePrefs, bool)),
    /// Choices (value, label).
    Select(
        &'static [(u32, &'static str)],
        fn(&CorePrefs) -> u32,
        fn(&mut CorePrefs, u32),
    ),
}

#[derive(Debug, Clone, Copy)]
struct Row {
    id: &'static str,
    label: &'static str,
    desc: Option<&'static str>,
    control: Control,
}

struct Section {
    title: &'static str,
    rows: &'static [Row],
}

const RECENT: [(u32, &str); 4] = [(3, "3"), (5, "5"), (8, "8"), (10, "10")];
const ROW_LIMIT: [(u32, &str); 4] = [(100, "100"), (500, "500"), (1000, "1,000"), (5000, "5,000")];
const HISTORY_DAYS: [(u32, &str); 5] = [
    (7, "7 days"),
    (30, "30 days"),
    (90, "90 days"),
    (365, "1 year"),
    (0, "Forever"),
];

const SECTIONS: [Section; 2] = [
    Section {
        title: "Sidebar and search",
        rows: &[
            Row {
                id: "sidebarRowCounts",
                label: "Row counts in the sidebar",
                desc: Some("Each table's approximate row count, from the planner's statistics."),
                control: Control::Switch(|p| p.sidebar_row_counts, |p, v| p.sidebar_row_counts = v),
            },
            Row {
                id: "sidebarViews",
                label: "Show views in the sidebar",
                desc: None,
                control: Control::Switch(|p| p.sidebar_views, |p, v| p.sidebar_views = v),
            },
            Row {
                id: "recentCount",
                label: "Recent tables in search",
                desc: None,
                control: Control::Select(&RECENT, |p| p.recent_count, |p, v| p.recent_count = v),
            },
        ],
    },
    Section {
        title: "Queries",
        rows: &[
            Row {
                id: "rowLimit",
                label: "Rows loaded when opening a table",
                desc: None,
                control: Control::Select(&ROW_LIMIT, |p| p.row_limit, |p, v| p.row_limit = v),
            },
            Row {
                id: "confirmDestructive",
                label: "Confirm destructive statements",
                desc: Some(
                    "Ask before running UPDATE or DELETE without a WHERE clause, DROP, TRUNCATE or DROP COLUMN. Connections tagged Production always ask.",
                ),
                control: Control::Switch(
                    |p| p.confirm_destructive,
                    |p, v| p.confirm_destructive = v,
                ),
            },
            Row {
                id: "recordHistory",
                label: "Save query history",
                desc: Some(
                    "Keep the queries you run, per connection, for Query History (⌘⌥E). A query run again, even with other values, replaces its earlier entry.",
                ),
                control: Control::Switch(|p| p.record_history, |p, v| p.record_history = v),
            },
            Row {
                id: "historyDays",
                label: "Keep query history for",
                desc: Some(
                    "Older runs are hidden, and removed the next time a statement is saved.",
                ),
                control: Control::Select(
                    &HISTORY_DAYS,
                    |p| p.history_days,
                    |p, v| p.history_days = v,
                ),
            },
        ],
    },
];

/// The Keyboard page (SettingsView.tsx `KEYS`).
pub const KEYS: [(&str, &str); 10] = [
    ("⌘K", "Search tables"),
    ("⌘T / ⌥N", "New query"),
    ("⌘↵", "Run query"),
    ("⌘⌥E", "Query history"),
    ("⌘W", "Close tab"),
    ("⌥1 … ⌥9", "Go to tab"),
    ("⌥↑ / ⌥↓", "Previous / next database"),
    ("⌥B", "Toggle sidebar"),
    ("⌥E", "Toggle right panel"),
    ("⌘,", "Settings"),
];

/// The Connections page's section title.
const CONNECTIONS_TITLE: &str =
    "Saved connections. Credentials are stored in the local config file";

/// Sidebar.tsx `connMeta`: "12 ms" / "connected", "connecting", "reconnecting",
/// "failed", "offline".
pub fn conn_meta(state: &ConnState, latency_ms: Option<u64>) -> String {
    match state {
        ConnState::Connected { .. } => {
            latency_ms.map_or_else(|| "connected".to_owned(), |ms| format!("{ms} ms"))
        }
        ConnState::Connecting => "connecting".into(),
        ConnState::Reconnecting { .. } => "reconnecting".into(),
        ConnState::Failed { .. } => "failed".into(),
        ConnState::Disconnected => "offline".into(),
    }
}

/// Sidebar.tsx `ConnStateDot`: a 6 px dot, dashed ring when offline.
fn state_dot(state: &ConnState, p: &Palette) -> gpui::Div {
    let dot = div().flex_none().size(px(6.)).rounded_full();
    match state {
        ConnState::Connected { .. } => dot.bg(p.status_ok),
        ConnState::Connecting | ConnState::Reconnecting { .. } => dot.bg(p.status_wait),
        ConnState::Failed { .. } => dot.bg(p.destructive),
        ConnState::Disconnected => dot.border(px(1.5)).border_dashed().border_color(p.dim),
    }
}

/// SettingsView.tsx `btn`: a small outlined button.
fn settings_button(
    id: SharedString,
    label: &'static str,
    disabled: bool,
    p: &Palette,
) -> gpui::Stateful<gpui::Div> {
    let (hover, body) = (p.hover, p.body);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(28.))
        .px(px(10.))
        .rounded(theme::radius::MD)
        .border_1()
        .border_color(p.border)
        .text_size(theme::text::SM)
        .text_color(p.soft)
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover).text_color(body))
        })
        .child(label)
}

/// A choice's trigger (SettingsView.tsx's `SelectTrigger`: h-7 gap-1 px-2,
/// a faint `border-border`, no fill, the value in text-sm, a 14 px chevron
/// in text-muted-foreground at 50 %).
fn select_trigger(id: SharedString, label: String, p: &Palette) -> Button {
    Button::new(id)
        .outline()
        .h(px(28.))
        .px(px(8.))
        .rounded(theme::radius::MD)
        .border_color(p.border)
        .bg(gpui::transparent_black())
        .text_color(p.body)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(theme::text::SM)
                .line_height(theme::text::SM_LINE)
                .font_weight(FontWeight::NORMAL)
                .child(label)
                .child(icon(IconName::ChevronDown, 14., p.quiet.opacity(0.5))),
        )
}

/// The settings switch (SettingsView.tsx's `Switch` classes): green when on
/// (`bg-status-ok`), `bg-input` when off, with a white 16 px thumb 1 px /
/// 17 px in from its 1 px border. The track is 32 × 18.4: switch.tsx's
/// `data-[size=default]:w-8 h-[1.15rem]` win over the view's `w-9 h-5`, so
/// the thumb sticks out 2 px on the right when on, as in the Tauri app.
pub fn pref_switch(id: SharedString, checked: bool, p: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .w(px(32.))
        .h(px(18.4))
        .rounded_full()
        .border_1()
        .border_color(gpui::transparent_black())
        .pl(px(if checked { 17. } else { 1. }))
        .cursor_pointer()
        .bg(if checked { p.status_ok } else { p.input })
        .child(
            div()
                .flex_none()
                .size(px(16.))
                .rounded_full()
                .bg(gpui::white()),
        )
}

/// The Settings view.
pub struct SettingsView {
    focus_handle: FocusHandle,
    search: Entity<InputState>,
    query: String,
    saved: bool,
    saved_timer: Option<Task<()>>,
    drag: WindowDrag,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for SettingsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsView")
            .field("query", &self.query)
            .field("saved", &self.saved)
            .finish_non_exhaustive()
    }
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search Settings"));
        let mut subscriptions = vec![cx.subscribe(&search, |this, search, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = search.read(cx).value().to_string();
                if !this.query.is_empty() {
                    this.set_page(SettingsPage::General, cx);
                }
                cx.notify();
            }
        })];
        if let Some(state) = AppState::try_global(cx) {
            let prefs = state.prefs.clone();
            let layout = state.layout.clone();
            subscriptions.push(cx.observe(&prefs, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&layout, |_, _, cx| cx.notify()));
        }
        Self {
            focus_handle: cx.focus_handle(),
            search,
            query: String::new(),
            saved: false,
            saved_timer: None,
            drag: WindowDrag::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The search field's state.
    pub fn search(&self) -> &Entity<InputState> {
        &self.search
    }

    fn page(cx: &App) -> SettingsPage {
        AppState::try_global(cx)
            .and_then(|s| s.layout.read(cx).settings())
            .unwrap_or(SettingsPage::General)
    }

    fn set_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        if let Some(state) = AppState::try_global(cx) {
            let layout = state.layout.clone();
            layout.update(cx, |l, cx| l.set_settings(Some(page), cx));
        }
    }

    fn close(cx: &mut App) {
        if let Some(state) = AppState::try_global(cx) {
            let layout = state.layout.clone();
            layout.update(cx, |l, cx| l.set_settings(None, cx));
        }
    }

    /// Esc: close, unless the search field has the focus and text.
    fn on_close(&mut self, _: &CloseSettings, window: &mut Window, cx: &mut Context<Self>) {
        let in_search = self.search.focus_handle(cx).contains_focused(window, cx);
        if in_search && !self.search.read(cx).value().is_empty() {
            cx.propagate();
            return;
        }
        Self::close(cx);
    }

    fn on_focus_search(
        &mut self,
        _: &FocusSettingsSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    /// The row of pref `id` (its prefs.json field name).
    fn row(id: &str) -> Option<Row> {
        SECTIONS
            .iter()
            .flat_map(|s| s.rows.iter())
            .find(|r| r.id == id)
            .copied()
    }

    /// Every pref shown on the General page (prefs.json field names), in order.
    pub fn pref_ids() -> Vec<&'static str> {
        SECTIONS
            .iter()
            .flat_map(|s| s.rows.iter().map(|r| r.id))
            .collect()
    }

    /// The labels of the rows the search leaves on the General page.
    pub fn shown_rows(&self, cx: &App) -> Vec<&'static str> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        SECTIONS
            .iter()
            .flat_map(|s| s.rows.iter().map(move |r| (s.title, r)))
            .filter(|(title, r)| {
                query.is_empty()
                    || format!("{} {} {}", r.label, r.desc.unwrap_or(""), title)
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|(_, r)| r.label)
            .collect()
    }

    /// A switch row clicked: flips pref `id`.
    pub fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(Row {
            control: Control::Switch(get, set),
            ..
        }) = Self::row(id)
        {
            self.set_pref(cx, move |p| {
                let v = get(p);
                set(p, !v)
            });
        }
    }

    /// A choice picked from row `id`'s menu.
    pub fn select(&mut self, id: &str, value: u32, cx: &mut Context<Self>) {
        if let Some(Row {
            control: Control::Select(choices, _, set),
            ..
        }) = Self::row(id)
            && choices.iter().any(|(v, _)| *v == value)
        {
            self.set_pref(cx, move |p| set(p, value));
        }
    }

    /// "Saved" shows (after a change, for 1.6 s).
    pub fn saved(&self) -> bool {
        self.saved
    }

    /// Applies a change to the prefs and flashes "Saved".
    fn set_pref(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut CorePrefs)) {
        if let Some(state) = AppState::try_global(cx) {
            let prefs = state.prefs.clone();
            prefs.update(cx, |prefs, cx| prefs.update(cx, f));
        }
        self.flash_saved(cx);
    }

    fn flash_saved(&mut self, cx: &mut Context<Self>) {
        self.saved = true;
        self.saved_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVED_FOR).await;
            this.update(cx, |this, cx| {
                this.saved = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// "Reset to defaults" (keeps the layout).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = AppState::try_global(cx) {
            let prefs = state.prefs.clone();
            prefs.update(cx, |prefs, cx| prefs.reset(cx));
        }
        self.saved = true;
        self.saved_timer = None;
        cx.notify();
    }

    fn render_nav(&self, page: SettingsPage, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let searching = !self.query.trim().is_empty();
        let nav_item =
            |id: &'static str, icon_name: IconName, label: &'static str, current: bool| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(theme::radius::MD)
                    .cursor_pointer()
                    .text_size(theme::text::BASE)
                    .when(current, |d| d.bg(p.muted).text_color(p.body))
                    .when(!current, |d| {
                        d.text_color(p.soft)
                            .hover(|s| s.bg(p.hover).text_color(p.body))
                    })
                    .child(icon(icon_name, 16., p.quiet))
                    .child(label)
            };
        let pages = SettingsPage::ALL.map(|pg| {
            let (id, icon_name) = match pg {
                SettingsPage::General => ("settings-nav-general", IconName::Settings),
                SettingsPage::Connections => ("settings-nav-connections", IconName::Plug),
                SettingsPage::Keyboard => ("settings-nav-keyboard", IconName::Keyboard),
            };
            nav_item(id, icon_name, pg.label(), pg == page && !searching).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.search.update(cx, |s, cx| s.set_value("", window, cx));
                    this.query.clear();
                    this.set_page(pg, cx);
                },
            ))
        });
        div()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(260.))
            .border_r_1()
            .border_color(p.border)
            .bg(p.sidebar)
            .text_color(p.soft)
            .child(
                self.drag
                    .apply(div().id("settings-nav-top").flex_none().h(px(40.))),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .flex_col()
                    .gap(px(1.))
                    .px(px(8.))
                    .child(
                        nav_item("settings-back", IconName::ArrowLeft, "Back", false)
                            .text_color(p.quiet)
                            .tooltip(super::chrome::tooltip("Back (esc)"))
                            .on_click(|_, _, cx| Self::close(cx)),
                    )
                    .child(
                        div()
                            .my(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(8.))
                            .rounded(theme::radius::MD)
                            .border_1()
                            .border_color(p.border)
                            .bg(p.field)
                            .child(icon(IconName::Search, 16., p.faint))
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&self.search)
                                        .appearance(false)
                                        .px_0()
                                        .py_0()
                                        .h(px(28.))
                                        .text_size(theme::text::BASE),
                                ),
                            ),
                    )
                    .children(pages),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(48.))
                    .px(px(16.))
                    .text_size(theme::text::XS)
                    .text_color(p.faint)
                    .child(if self.saved {
                        "Saved"
                    } else {
                        "Changes save automatically"
                    }),
            )
    }

    fn section(title: impl Into<SharedString>, rows: Vec<gpui::AnyElement>, cx: &App) -> gpui::Div {
        let p = cx.palette();
        let divider = gpui::hsla(0., 0., 0.94, 0.07);
        let count = rows.len();
        div()
            .mt(px(24.))
            .child(
                div()
                    .mb(px(8.))
                    .px(px(8.))
                    .text_size(theme::text::SM)
                    .line_height(theme::text::SM_LINE)
                    .text_color(p.quiet)
                    .child(title.into()),
            )
            .child(
                div()
                    .rounded(theme::radius::LG)
                    .bg(p.card)
                    .px(px(12.))
                    .children(rows.into_iter().enumerate().map(move |(i, row)| {
                        div()
                            .when(i + 1 < count, |d| d.border_b_1().border_color(divider))
                            .child(row)
                    })),
            )
    }

    fn render_row(&self, row: Row, prefs: &CorePrefs, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = cx.palette().clone();
        let control = match row.control {
            Control::Switch(get, _) => {
                let checked = get(prefs);
                let id = row.id;
                pref_switch(SharedString::from(format!("pref-{id}")), checked, &p)
                    .debug_selector(move || format!("pref-{id}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, cx)))
                    .into_any_element()
            }
            Control::Select(choices, get, _) => {
                let id = row.id;
                let current = get(prefs);
                let label = choices
                    .iter()
                    .find(|(v, _)| *v == current)
                    .map_or_else(|| current.to_string(), |(_, l)| (*l).to_owned());
                let this = cx.entity().downgrade();
                select_trigger(SharedString::from(format!("pref-{}", row.id)), label, &p)
                    .dropdown_menu(move |mut menu, _, _| {
                        for &(value, text) in choices {
                            let this = this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(text).checked(value == current).on_click(
                                    move |_, _, cx| {
                                        this.update(cx, |this, cx| this.select(id, value, cx)).ok();
                                    },
                                ),
                            );
                        }
                        menu
                    })
                    .into_any_element()
            }
        };
        div()
            .flex()
            .items_center()
            .gap(px(24.))
            .min_h(px(60.))
            .px(px(4.))
            .py(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(theme::text::SM)
                            .line_height(theme::text::SM_LINE)
                            .text_color(p.body)
                            .child(row.label),
                    )
                    .children(row.desc.map(|desc| {
                        div()
                            .mt(px(2.))
                            .text_size(theme::text::SM)
                            .line_height(theme::text::SM_LINE)
                            .text_color(p.quiet)
                            .child(desc)
                    })),
            )
            .child(control)
            .into_any_element()
    }

    fn render_general(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let prefs = AppState::try_global(cx)
            .map(|s| s.prefs.read(cx).get().clone())
            .unwrap_or_default();
        let query = self.query.trim().to_lowercase();
        let mut out = Vec::new();
        for section in &SECTIONS {
            let rows: Vec<Row> = section
                .rows
                .iter()
                .copied()
                .filter(|r| {
                    query.is_empty()
                        || format!("{} {} {}", r.label, r.desc.unwrap_or(""), section.title)
                            .to_lowercase()
                            .contains(&query)
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            let rows = rows
                .into_iter()
                .map(|r| self.render_row(r, &prefs, cx))
                .collect();
            out.push(Self::section(section.title, rows, cx).into_any_element());
        }
        let p = cx.palette().clone();
        if out.is_empty() {
            out.push(
                div()
                    .mt(px(24.))
                    .px(px(8.))
                    .text_size(theme::text::SM)
                    .text_color(p.faint)
                    .child("No settings match.")
                    .into_any_element(),
            );
        }
        if query.is_empty() {
            out.push(
                div()
                    .mt(px(24.))
                    .flex()
                    .child(
                        div()
                            .id("settings-reset")
                            .debug_selector(|| "settings-reset".into())
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(px(28.))
                            .px(px(10.))
                            .rounded(theme::radius::MD)
                            .border_1()
                            .border_color(p.border)
                            .cursor_pointer()
                            .text_size(theme::text::SM)
                            .text_color(p.soft)
                            .hover(|s| s.bg(p.hover).text_color(p.body))
                            .on_click(cx.listener(|this, _, _, cx| this.reset(cx)))
                            .child("Reset to defaults"),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// SettingsView.tsx `ConnectionsPage`: every saved connection with its
    /// state, `PostgreSQL · postgresql://… · meta` and Connect / Disconnect /
    /// Retry now.
    fn render_connections(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = cx.palette().clone();
        let Some(state) = AppState::try_global(cx).cloned() else {
            return Self::section(CONNECTIONS_TITLE, vec![], cx).into_any_element();
        };
        let infos: Vec<(String, String, ConnState, String)> = {
            let conns = state.connections.read(cx);
            conns
                .saved
                .iter()
                .map(|c| {
                    let id = c.config.id.clone();
                    let st = conns.state_of(&id);
                    let meta = conn_meta(&st, conns.latency_ms(&id));
                    let line = format!(
                        "PostgreSQL · {} · {meta}",
                        conns.connection_string(&id, None).unwrap_or_default()
                    );
                    (id, c.config.name.clone(), st, line)
                })
                .collect()
        };
        let mut rows: Vec<gpui::AnyElement> = infos
            .into_iter()
            .map(|(id, name, st, line)| {
                let connected = matches!(st, ConnState::Connected { .. });
                let connecting = matches!(st, ConnState::Connecting);
                let label = if connected {
                    "Disconnect"
                } else if matches!(st, ConnState::Reconnecting { .. }) {
                    "Retry now"
                } else {
                    "Connect"
                };
                let connections = state.connections.clone();
                let button_id = SharedString::from(format!("settings-conn-{id}"));
                let selector = button_id.to_string();
                let button = settings_button(button_id, label, connecting, &p)
                    .debug_selector(move || selector)
                    .when(!connecting, |d| {
                        d.on_click(move |_, _, cx| {
                            connections.update(cx, |c, cx| match c.state_of(&id) {
                                ConnState::Connected { .. } => c.disconnect(&id, cx),
                                ConnState::Reconnecting { .. } => c.retry_now(&id, cx).detach(),
                                _ => c.connect(&id, cx),
                            });
                        })
                    });
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(4.))
                    .py(px(12.))
                    .child(tile(&name, 16., &p))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(theme::text::SM)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.body)
                                    .child(name)
                                    .child(state_dot(&st, &p)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(theme::MONO_FONT)
                                    .text_size(theme::text::XS)
                                    .text_color(p.faint)
                                    .child(line),
                            ),
                    )
                    .child(button)
                    .into_any_element()
            })
            .collect();
        if rows.is_empty() {
            rows.push(
                div()
                    .px(px(4.))
                    .py(px(12.))
                    .text_size(theme::text::SM)
                    .text_color(p.faint)
                    .child("No connections.")
                    .into_any_element(),
            );
        }
        Self::section(CONNECTIONS_TITLE, rows, cx).into_any_element()
    }

    fn render_keyboard(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = cx.palette().clone();
        let rows = KEYS
            .iter()
            .map(|(keys, label)| {
                div()
                    .flex()
                    .items_center()
                    .px(px(4.))
                    .py(px(10.))
                    .text_size(theme::text::SM)
                    .text_color(p.body)
                    .child(*label)
                    .child(div().flex_1())
                    .child(
                        div()
                            .font_family(theme::MONO_FONT)
                            .text_size(theme::text::XS)
                            .text_color(p.quiet)
                            .child(*keys),
                    )
                    .into_any_element()
            })
            .collect();
        Self::section(
            "Shortcuts work everywhere, including inside the editor",
            rows,
            cx,
        )
        .into_any_element()
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        // The field's text (also when set programmatically, which sends no Change).
        self.query = self.search.read(cx).value().to_string();
        let page = Self::page(cx);
        let searching = !self.query.trim().is_empty();
        let show_general = page == SettingsPage::General || searching;
        let title = if show_general {
            "General"
        } else {
            page.label()
        };
        let content: Vec<gpui::AnyElement> = if show_general {
            self.render_general(cx)
        } else if page == SettingsPage::Connections {
            vec![self.render_connections(cx)]
        } else {
            vec![self.render_keyboard(cx)]
        };
        div()
            .id("settings")
            .key_context(crate::keymap::SETTINGS)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_focus_search))
            // Clicks don't fall through to the workspace underneath.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .absolute()
            .inset_0()
            .flex()
            .bg(p.background)
            .text_color(p.body)
            .child(self.render_nav(page, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .child(
                        self.drag
                            .apply(div().id("settings-main-top").flex_none().h(px(40.))),
                    )
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px(px(32.))
                            .pb(px(64.))
                            .child(
                                div()
                                    .mx_auto()
                                    .max_w(px(780.))
                                    .pt(px(16.))
                                    .child(
                                        div()
                                            .px(px(8.))
                                            .text_size(theme::text::LG)
                                            .line_height(theme::text::LG_LINE)
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(p.body)
                                            .child(title),
                                    )
                                    .children(content),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod window_tests {
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};

    use super::*;
    use crate::tests::{TempDir, init, open_main, settle};
    use crate::views::root::RootView;

    fn open_settings(root: &Entity<RootView>, vcx: &mut VisualTestContext) -> Entity<SettingsView> {
        vcx.simulate_keystrokes("cmd-,");
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|_, cx| root.read(cx).settings().clone())
    }

    fn prefs_json(dir: &TempDir) -> serde_json::Value {
        let text = std::fs::read_to_string(dir.path().join("prefs.json")).expect("prefs.json");
        serde_json::from_str(&text).unwrap()
    }

    fn click(vcx: &mut VisualTestContext, selector: &'static str) {
        let bounds = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn"));
        vcx.simulate_click(bounds.center(), Modifiers::none());
        let _ = MouseButton::Left;
        vcx.run_until_parked();
    }

    #[gpui::test]
    fn search_filters_the_rows_and_shows_general(cx: &mut TestAppContext) {
        let dir = TempDir::new("settings-search");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        let view = open_settings(&root, &mut vcx);
        vcx.update(|_, cx| {
            let layout = AppState::global(cx).layout.clone();
            layout.update(cx, |l, cx| l.set_settings(Some(SettingsPage::Keyboard), cx));
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        let all = vcx.update(|_, cx| view.read(cx).shown_rows(cx));
        assert_eq!(all.len(), SettingsView::pref_ids().len());
        vcx.simulate_keystrokes("cmd-f");
        vcx.simulate_input("history");
        let shown = vcx.update(|_, cx| view.read(cx).shown_rows(cx));
        assert_eq!(shown, ["Save query history", "Keep query history for"]);
        let page = vcx.update(|_, cx| AppState::global(cx).layout.read(cx).settings());
        assert_eq!(page, Some(SettingsPage::General), "typing shows General");
        // Section titles and descriptions match too.
        vcx.update(|window, cx| {
            view.read(cx)
                .search()
                .clone()
                .update(cx, |s, cx| s.set_value("sidebar and", window, cx))
        });
        vcx.run_until_parked();
        let shown = vcx.update(|_, cx| view.read(cx).shown_rows(cx));
        assert_eq!(
            shown,
            [
                "Row counts in the sidebar",
                "Show views in the sidebar",
                "Recent tables in search"
            ]
        );
        vcx.update(|window, cx| {
            view.read(cx)
                .search()
                .clone()
                .update(cx, |s, cx| s.set_value("PRODUCTION", window, cx))
        });
        vcx.run_until_parked();
        let shown = vcx.update(|_, cx| view.read(cx).shown_rows(cx));
        assert_eq!(
            shown,
            ["Confirm destructive statements"],
            "case-insensitive"
        );
        vcx.update(|window, cx| {
            view.read(cx)
                .search()
                .clone()
                .update(cx, |s, cx| s.set_value("zzz", window, cx))
        });
        vcx.run_until_parked();
        assert!(vcx.update(|_, cx| view.read(cx).shown_rows(cx)).is_empty());
    }

    #[gpui::test]
    fn every_switch_and_choice_writes_prefs_json(cx: &mut TestAppContext) {
        let dir = TempDir::new("settings-controls");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        let view = open_settings(&root, &mut vcx);
        // The switches, clicked: all on by default.
        for (id, selector) in [
            ("sidebarRowCounts", "pref-sidebarRowCounts"),
            ("sidebarViews", "pref-sidebarViews"),
            ("confirmDestructive", "pref-confirmDestructive"),
            ("recordHistory", "pref-recordHistory"),
        ] {
            click(&mut vcx, selector);
            assert!(vcx.update(|_, cx| view.read(cx).saved()), "Saved shows");
            settle(&mut vcx);
            assert_eq!(prefs_json(&dir)[id], false, "{id} off");
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            click(&mut vcx, selector);
            settle(&mut vcx);
            assert_eq!(prefs_json(&dir)[id], true, "{id} on again");
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
        }
        // The choices: every value of each menu.
        for (id, choices) in [
            ("recentCount", &RECENT[..]),
            ("rowLimit", &ROW_LIMIT[..]),
            ("historyDays", &HISTORY_DAYS[..]),
        ] {
            for &(value, _) in choices {
                vcx.update(|_, cx| view.update(cx, |v, cx| v.select(id, value, cx)));
                settle(&mut vcx);
                assert_eq!(prefs_json(&dir)[id], value, "{id} = {value}");
            }
        }
        // Not a choice of the menu: ignored.
        vcx.update(|_, cx| view.update(cx, |v, cx| v.select("rowLimit", 123, cx)));
        settle(&mut vcx);
        assert_eq!(prefs_json(&dir)["rowLimit"], 5000);
        assert_eq!(
            SettingsView::pref_ids(),
            [
                "sidebarRowCounts",
                "sidebarViews",
                "recentCount",
                "rowLimit",
                "confirmDestructive",
                "recordHistory",
                "historyDays"
            ]
        );
    }

    #[gpui::test]
    fn reset_to_defaults_keeps_the_layout_in_prefs_json(cx: &mut TestAppContext) {
        let dir = TempDir::new("settings-reset");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        let view = open_settings(&root, &mut vcx);
        vcx.update(|_, cx| {
            let prefs = AppState::global(cx).prefs.clone();
            prefs.update(cx, |p, cx| {
                p.update(cx, |p| {
                    p.row_limit = 5000;
                    p.history_days = 0;
                    p.sidebar_views = false;
                    p.sidebar_width = 400;
                    p.bottom_height = 300;
                    p.connection_order = vec!["b".into(), "a".into()];
                })
            });
        });
        settle(&mut vcx);
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        click(&mut vcx, "settings-reset");
        settle(&mut vcx);
        let json = prefs_json(&dir);
        assert_eq!(json["rowLimit"], 500);
        assert_eq!(json["historyDays"], 30);
        assert_eq!(json["sidebarViews"], true);
        assert_eq!(json["sidebarWidth"], 400, "layout kept");
        assert_eq!(json["bottomHeight"], 300);
        assert_eq!(json["connectionOrder"], serde_json::json!(["b", "a"]));
        assert!(vcx.update(|_, cx| view.read(cx).saved()));
    }

    #[gpui::test]
    fn connections_page_lists_saved_connections_with_their_state(cx: &mut TestAppContext) {
        let dir = TempDir::new("settings-conns");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        let _view = open_settings(&root, &mut vcx);
        vcx.update(|_, cx| {
            let layout = AppState::global(cx).layout.clone();
            layout.update(cx, |l, cx| {
                l.set_settings(Some(SettingsPage::Connections), cx)
            });
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("settings-conn-fake").is_some(),
            "the pretend connection's button is drawn"
        );
        assert_eq!(conn_meta(&ConnState::Disconnected, None), "offline");
        assert_eq!(conn_meta(&ConnState::Connecting, Some(3)), "connecting");
        assert_eq!(
            conn_meta(
                &ConnState::Connected {
                    server_version: "17".into(),
                    databases: vec![]
                },
                Some(12)
            ),
            "12 ms"
        );
        assert_eq!(
            conn_meta(
                &ConnState::Connected {
                    server_version: "17".into(),
                    databases: vec![]
                },
                None
            ),
            "connected"
        );
    }
}

#[cfg(test)]
mod tests {
    use boa_core::state::prefs as core;

    use super::*;

    #[test]
    fn choices_match_the_core_lists() {
        assert_eq!(RECENT.map(|c| c.0), core::RECENT_COUNT_CHOICES);
        assert_eq!(ROW_LIMIT.map(|c| c.0), core::ROW_LIMIT_CHOICES);
        assert_eq!(HISTORY_DAYS.map(|c| c.0), core::HISTORY_DAYS_CHOICES);
    }
}
