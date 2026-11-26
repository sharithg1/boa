//! CommandPalette.tsx: the ⌘K / ⌘P palette ("Open table"), also opened from
//! the title bar's database chip and the sidebar's Search. With an empty
//! query: the recently opened tables first ("Recent", prefs `recentCount`),
//! then every table and view of the connected databases ("All tables");
//! otherwise fuzzy matches on the table name (highlighted, ranked first) or
//! on "schema.table database connection". Commands ("Actions": Query
//! History) follow the tables when every typed word starts one of their
//! words. ↑ ↓ / Tab ⇧Tab move (wrapping), ⏎ opens the table (pinned) or runs
//! the action, Esc or a click outside closes.
//!
//! The root view owns the open palette (see [`crate::views::root`]); this
//! view emits [`PaletteEvent::Dismissed`] when it should close.

use boa_core::grid::format::{FuzzyHit, ago, compact_count, fuzzy, highlight_runs, tile_hue};
use boa_core::model::TableKind;
use boa_core::state::prefs::apply_order;
use boa_core::state::session::table_key;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    HighlightStyle, Hsla, MouseButton, Rgba, ScrollHandle, StyledText, Subscription, Window, div,
    px,
};
use gpui_component::input::{Input, InputEvent, InputState};

use super::chrome::{BoaIcon, IconName, icon};
use crate::actions::{Confirm, Dismiss, SelectNext, SelectPrevious};
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::workspaces::now_ms;
use crate::theme::{self, ActivePalette as _, Palette};

/// The input's placeholder.
pub const PLACEHOLDER: &str = "Open table…  (table, schema, database or connection)";

// ── Pure part ────────────────────────────────────────────────────────────────

/// A table or view of a connected database (CommandPalette.tsx `Item`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    /// `tableKey` (the MRU / `seen` key).
    pub key: String,
    pub connection_id: String,
    /// The connection's name.
    pub connection: String,
    pub database: String,
    pub schema: String,
    pub table: String,
    pub view: bool,
    /// Planner estimate.
    pub rows: i64,
}

/// An item as listed, with the matched characters of its name (`Row`).
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteRow {
    pub item: PaletteItem,
    /// `char` indices of the table name to highlight.
    pub idxs: Vec<usize>,
}

/// A titled run of rows ("Recent", "All tables", or untitled).
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteGroup {
    pub label: Option<&'static str>,
    pub rows: Vec<PaletteRow>,
}

/// What a palette command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Opens Query History (`openHistory`).
    History,
}

/// A command listed after the tables (CommandPalette.tsx `ACTIONS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteAction {
    pub id: &'static str,
    pub label: &'static str,
    pub keys: &'static str,
    pub words: &'static [&'static str],
    pub kind: ActionKind,
}

/// The palette's commands.
pub const ACTIONS: [PaletteAction; 1] = [PaletteAction {
    id: "history",
    label: "Query History",
    keys: "⌘⌥E",
    words: &["query", "history", "recent", "statements", "log", "sql"],
    kind: ActionKind::History,
}];

/// The commands whose words every typed word starts (`matchActions`); none
/// for an empty query.
pub fn match_actions(q: &str) -> Vec<PaletteAction> {
    let q = q.to_lowercase();
    let terms: Vec<&str> = q.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }
    ACTIONS
        .iter()
        .filter(|a| {
            terms
                .iter()
                .all(|t| a.words.iter().any(|w| w.starts_with(t)))
        })
        .copied()
        .collect()
}

/// The listed groups (CommandPalette.tsx `groups`). Empty query: up to
/// `recent_count` items of `mru` ("Recent"), then the rest ("All tables",
/// untitled when nothing is recent). Otherwise every item whose name or
/// "schema.table database connection" matches, best first (a name match
/// scores 5 more and is highlighted; ties keep the items' order).
pub fn palette_groups(
    items: &[PaletteItem],
    q: &str,
    mru: &[String],
    recent_count: usize,
) -> Vec<PaletteGroup> {
    let query = q.trim();
    let plain = |i: &PaletteItem| PaletteRow {
        item: i.clone(),
        idxs: Vec::new(),
    };
    if query.is_empty() {
        let recent: Vec<&PaletteItem> = mru
            .iter()
            .filter_map(|k| items.iter().find(|i| &i.key == k))
            .take(recent_count)
            .collect();
        let rest: Vec<PaletteRow> = items
            .iter()
            .filter(|i| !recent.iter().any(|r| std::ptr::eq(*r, *i)))
            .map(plain)
            .collect();
        let mut groups = Vec::new();
        if !recent.is_empty() {
            groups.push(PaletteGroup {
                label: Some("Recent"),
                rows: recent.into_iter().map(plain).collect(),
            });
        }
        let label = (!groups.is_empty()).then_some("All tables");
        groups.push(PaletteGroup { label, rows: rest });
        groups.retain(|g| !g.rows.is_empty());
        return groups;
    }
    let mut hits: Vec<(f64, PaletteRow)> = items
        .iter()
        .filter_map(|it| {
            let name = fuzzy(query, &it.table);
            let full = fuzzy(
                query,
                &format!(
                    "{}.{} {} {}",
                    it.schema, it.table, it.database, it.connection
                ),
            );
            let best: &FuzzyHit = name.as_ref().or(full.as_ref())?;
            let score = if name.is_some() { 5. } else { 0. } + best.score;
            let idxs = name.as_ref().map(|n| n.idxs.clone()).unwrap_or_default();
            Some((
                score,
                PaletteRow {
                    item: it.clone(),
                    idxs,
                },
            ))
        })
        .collect();
    // Stable, as Array.prototype.sort.
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    vec![PaletteGroup {
        label: None,
        rows: hits.into_iter().map(|(_, r)| r).collect(),
    }]
}

/// The new selection after moving `by` in a list of `total` (wraps; `move`).
pub fn wrap_move(sel: usize, by: isize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let t = total as isize;
    (((sel.min(total - 1) as isize + by) % t + t) % t) as usize
}

/// "1 table" / "12 tables".
pub fn count_label(n: usize) -> String {
    format!("{n} table{}", if n == 1 { "" } else { "s" })
}

/// The tables and views of every connected connection's loaded databases,
/// in sidebar order, then database, schema and table order (the TS
/// `items`: `order.flatMap(… c.databases … d.schemas … s.tables)`).
pub fn palette_items(cx: &App) -> Vec<PaletteItem> {
    let Some(state) = AppState::try_global(cx) else {
        return Vec::new();
    };
    let connections = state.connections.read(cx);
    let catalogs = state.catalogs.read(cx);
    let prefs = state.prefs.read(cx).get();
    let ids: Vec<&str> = connections
        .saved
        .iter()
        .map(|c| c.config.id.as_str())
        .collect();
    let mut out = Vec::new();
    for id in apply_order(&ids, &prefs.connection_order) {
        let Some(saved) = connections.get(&id) else {
            continue;
        };
        if !connections.is_connected(&id) {
            continue;
        }
        let mut databases = connections.databases(&id);
        // Loaded databases the server list doesn't name (yet) go last.
        let mut extra: Vec<&DbKey> = catalogs
            .catalogs
            .keys()
            .filter(|k| k.connection_id == id && !databases.contains(&k.database))
            .collect();
        extra.sort();
        databases.extend(extra.into_iter().map(|k| k.database.clone()));
        for database in databases {
            let key = DbKey::new(id.clone(), database.clone());
            let Some(catalog) = catalogs.get(&key) else {
                continue;
            };
            for schema in catalog.schemas.iter() {
                for t in &schema.tables {
                    out.push(PaletteItem {
                        key: table_key(&id, &database, &schema.name, &t.name),
                        connection_id: id.clone(),
                        connection: saved.config.name.clone(),
                        database: database.clone(),
                        schema: schema.name.clone(),
                        table: t.name.clone(),
                        view: t.kind == TableKind::View,
                        rows: t.rows,
                    });
                }
            }
        }
    }
    out
}

/// `oklch(l c h)` as a GPUI colour (CSS Color 4: OKLab → linear sRGB → sRGB).
pub fn oklch(l: f32, c: f32, hue_deg: f32) -> Hsla {
    let h = hue_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = (l + 0.396_337_8 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let r = 4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_;
    let g = -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_4 * s_;
    let bl = -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_;
    let enc = |x: f32| {
        let x = if x <= 0.003_130_8 {
            12.92 * x
        } else {
            1.055 * x.powf(1. / 2.4) - 0.055
        };
        x.clamp(0., 1.)
    };
    Rgba {
        r: enc(r),
        g: enc(g),
        b: enc(bl),
        a: 1.,
    }
    .into()
}

/// main.css `--tile-l` / `--tile-c`.
const TILE_L: f32 = 0.76;
const TILE_C: f32 = 0.09;

/// chrome.tsx `tileColor`: a name's tile colour.
pub fn tile_color(name: &str) -> Hsla {
    oklch(TILE_L, TILE_C, tile_hue(name) as f32)
}

/// chrome.tsx `Tile`: a coloured square, with the name's initial from 14 px.
pub fn tile(name: &str, size: f32, p: &Palette) -> gpui::Div {
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string());
    div()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(if size >= 14. { px(4.) } else { px(3.) })
        .bg(tile_color(name))
        .text_color(p.background)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(if size >= 16. { 10. } else { 9. }))
        .when(size >= 14., |d| d.children(initial))
}

/// `Hl`: `text` with the `idxs` characters bright and semibold.
pub fn highlighted(text: &str, idxs: &[usize], p: &Palette) -> StyledText {
    let mut ranges = Vec::new();
    let mut at = 0;
    for run in highlight_runs(text, idxs) {
        let end = at + run.text.len();
        if run.hit {
            ranges.push((
                at..end,
                HighlightStyle {
                    color: Some(p.body),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..HighlightStyle::default()
                },
            ));
        }
        at = end;
    }
    StyledText::new(text.to_owned()).with_highlights(ranges)
}

// ── The view ─────────────────────────────────────────────────────────────────

/// What the palette tells the root view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteEvent {
    /// Close it (Esc, a click outside, or after opening something).
    Dismissed,
}

/// A selectable entry, in list order.
#[derive(Debug, Clone)]
enum Choice {
    Table(PaletteItem),
    Action(PaletteAction),
}

/// The command palette.
pub struct CommandPalette {
    input: Entity<InputState>,
    query: String,
    idx: usize,
    scroll: ScrollHandle,
    /// For each selectable entry, its child index in the list (headers count).
    child_ix: Vec<usize>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for CommandPalette {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandPalette")
            .field("query", &self.query)
            .field("idx", &self.idx)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));
        let mut subscriptions = vec![cx.subscribe(&input, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                this.idx = 0;
                this.scroll.scroll_to_item(0);
                cx.notify();
            }
        })];
        if let Some(state) = AppState::try_global(cx).cloned() {
            subscriptions.push(cx.observe(&state.catalogs, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.connections, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.workspaces, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.prefs, |_, _, cx| cx.notify()));
        }
        Self {
            input,
            query: String::new(),
            idx: 0,
            scroll: ScrollHandle::new(),
            child_ix: Vec::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// Types `query` into the search field (dev hooks, tests).
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(query.to_owned(), window, cx));
        self.query = query.to_owned();
        self.idx = 0;
        cx.notify();
    }

    /// The search field (focus it when opening).
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// The groups and actions for the current query.
    pub fn groups(&self, cx: &App) -> (Vec<PaletteGroup>, Vec<PaletteAction>) {
        let (mru, recent) = AppState::try_global(cx).map_or((Vec::new(), 5), |s| {
            (
                s.workspaces.read(cx).mru.clone(),
                s.prefs.read(cx).get().recent_count as usize,
            )
        });
        let groups = palette_groups(&palette_items(cx), &self.query, &mru, recent);
        (groups, match_actions(&self.query))
    }

    fn choices(&self, cx: &App) -> Vec<Choice> {
        let (groups, actions) = self.groups(cx);
        groups
            .into_iter()
            .flat_map(|g| g.rows)
            .map(|r| Choice::Table(r.item))
            .chain(actions.into_iter().map(Choice::Action))
            .collect()
    }

    /// The selected entry's index (clamped to the list).
    pub fn selected(&self, cx: &App) -> usize {
        let total = self.choices(cx).len();
        self.idx.min(total.saturating_sub(1))
    }

    /// The listed entries, in order (tables as `schema.table`, actions by
    /// label), for tests.
    pub fn listed(&self, cx: &App) -> Vec<String> {
        self.choices(cx)
            .into_iter()
            .map(|c| match c {
                Choice::Table(i) => format!("{}.{}", i.schema, i.table),
                Choice::Action(a) => a.label.to_owned(),
            })
            .collect()
    }

    fn move_by(&mut self, by: isize, cx: &mut Context<Self>) {
        let total = self.choices(cx).len();
        if total == 0 {
            return;
        }
        self.idx = wrap_move(self.idx, by, total);
        if let Some(&ix) = self.child_ix.get(self.idx) {
            self.scroll.scroll_to_item(ix);
        }
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let choices = self.choices(cx);
        let sel = self.idx.min(choices.len().saturating_sub(1));
        if let Some(choice) = choices.into_iter().nth(sel) {
            self.choose(choice, cx);
        }
    }

    fn choose(&mut self, choice: Choice, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Dismissed);
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        match choice {
            Choice::Table(it) => {
                // `openTable(…, { pin: true })`
                let key = DbKey::new(it.connection_id, it.database);
                state.workspaces.update(cx, |w, cx| {
                    w.open_table(key, &it.schema, &it.table, false, cx)
                });
            }
            Choice::Action(a) => match a.kind {
                ActionKind::History => state.history.update(cx, |h, cx| h.set_open(true, cx)),
            },
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Dismissed);
    }

    fn row_shell(
        id: usize,
        selected: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(("palette-item", id))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(theme::radius::MD)
            .px(px(8.))
            .py(px(6.))
            .when(selected, |d| d.bg(p.muted))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.idx != id {
                    this.idx = id;
                    cx.notify();
                }
            }))
    }

    fn enter_mark(selected: bool, p: &Palette) -> gpui::Div {
        div()
            .flex_none()
            .w(px(14.))
            .text_right()
            .font_family(theme::MONO_FONT)
            .text_size(theme::text::XXS)
            .text_color(p.quiet)
            .when(selected, |d| d.child("⏎"))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table(
        &self,
        i: usize,
        selected: bool,
        row: PaletteRow,
        seen: Option<i64>,
        now: i64,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let it = row.item;
        let meta = format!(
            "{}{} rows",
            seen.map(|s| format!("{} · ", ago(s, now)))
                .unwrap_or_default(),
            compact_count(it.rows.max(0) as u64)
        );
        let choice = Choice::Table(it.clone());
        Self::row_shell(i, selected, p, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.choose(choice.clone(), cx)))
            .child(icon(
                if it.view {
                    IconName::Eye
                } else {
                    IconName::Table2
                },
                14.,
                p.quiet,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme::MONO_FONT)
                                    .text_size(theme::text::SM)
                                    .line_height(theme::text::SM_LINE)
                                    .text_color(p.body)
                                    .child(highlighted(&it.table, &row.idxs, p)),
                            )
                            .when(it.view, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .rounded(px(4.))
                                        .bg(p.muted)
                                        .px(px(5.))
                                        .text_size(theme::text::XXS)
                                        .line_height(px(16.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(p.body)
                                        .child("view"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(px(4.))
                            .overflow_hidden()
                            .text_size(theme::text::XS)
                            .line_height(px(16.))
                            .text_color(p.faint)
                            .child(tile(&it.connection, 10., p))
                            .child(div().flex_none().child(it.connection.clone()))
                            .child(div().text_color(p.dim).child("·"))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme::MONO_FONT)
                                    .child(format!("{}/{}", it.database, it.schema)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::XXS)
                    .text_color(p.faint)
                    .child(meta),
            )
            .child(Self::enter_mark(selected, p))
            .into_any_element()
    }

    fn render_action(
        &self,
        i: usize,
        selected: bool,
        a: PaletteAction,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon_el = match a.kind {
            ActionKind::History => BoaIcon::History.icon(),
        };
        Self::row_shell(i, selected, p, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.choose(Choice::Action(a), cx)))
            .child(icon_el.size(px(14.)).text_color(p.quiet))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::text::SM)
                    .text_color(p.body)
                    .child(a.label),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::XXS)
                    .text_color(p.faint)
                    .child(a.keys),
            )
            .child(Self::enter_mark(selected, p))
            .into_any_element()
    }

    fn header(label: &'static str, p: &Palette) -> AnyElement {
        div()
            .flex()
            .items_center()
            .px(px(8.))
            .pt(px(8.))
            .pb(px(4.))
            .text_size(theme::text::XS)
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.quiet)
            .child(label)
            .into_any_element()
    }
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let size = window.viewport_size();
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        let (groups, actions) = self.groups(cx);
        let table_count: usize = groups.iter().map(|g| g.rows.len()).sum();
        let total = table_count + actions.len();
        let sel = self.idx.min(total.saturating_sub(1));
        let (seen, now) = AppState::try_global(cx).map_or((Default::default(), 0), |s| {
            (s.workspaces.read(cx).seen.clone(), now_ms() as i64)
        });

        let mut children: Vec<AnyElement> = Vec::new();
        let mut child_ix = Vec::with_capacity(total);
        let mut n = 0;
        for group in groups {
            if let Some(label) = group.label {
                children.push(Self::header(label, &p));
            }
            for row in group.rows {
                let seen_at = seen.get(&row.item.key).copied();
                child_ix.push(children.len());
                children.push(self.render_table(n, n == sel, row, seen_at, now, &p, cx));
                n += 1;
            }
        }
        if !actions.is_empty() {
            children.push(Self::header("Actions", &p));
            for a in actions {
                child_ix.push(children.len());
                children.push(self.render_action(n, n == sel, a, &p, cx));
                n += 1;
            }
        }
        if total == 0 {
            children.push(
                div()
                    .p(px(22.))
                    .text_center()
                    .text_size(theme::text::SM)
                    .text_color(p.faint)
                    .child("No tables match.")
                    .into_any_element(),
            );
        }
        self.child_ix = child_ix;

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(46.))
            .px(px(14.))
            .border_b_1()
            .border_color(p.border)
            // RingGlyph size 11
            .child(
                div()
                    .flex_none()
                    .size(px(11.))
                    .rounded_full()
                    .border(px(1.5))
                    .border_color(p.faint),
            )
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
                    .flex_none()
                    .font_family(theme::MONO_FONT)
                    .text_size(theme::text::XXS)
                    .text_color(p.faint)
                    .child(count_label(table_count)),
            );
        let list = div()
            .id("palette-list")
            .flex()
            .flex_col()
            .max_h(px((vh * 0.58).min(440.)))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(4.))
            .children(children);
        let footer = div()
            .flex()
            .flex_none()
            .gap(px(16.))
            .border_t_1()
            .border_color(p.border)
            .px(px(14.))
            .py(px(8.))
            .font_family(theme::MONO_FONT)
            .text_size(theme::text::XXS)
            .text_color(p.faint)
            .child("↑↓ move")
            .child("⏎ open")
            .child(div().flex_1())
            .child("esc");
        let dialog = div()
            .id("command-palette")
            .occlude()
            .w(px((vw - 32.).min(580.)))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme::radius::XL)
            .border_1()
            .border_color(p.border)
            .bg(p.popover)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                // Clicks inside keep the field focused.
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(header)
            .child(list)
            .child(footer);
        div()
            .id("command-palette-scrim")
            .key_context(crate::keymap::PALETTE)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_by(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_by(1, cx)))
            .on_action(cx.listener(|this, _: &Confirm, _, cx| this.confirm(cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.dismiss(cx)))
            .absolute()
            .inset_0()
            .occlude()
            .bg(p.scrim)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(vh * 0.13))
            .font_family(theme::UI_FONT)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.prevent_default();
                    this.dismiss(cx);
                }),
            )
            .child(dialog)
    }
}

/// Dev hook (feature `dev-hooks`): `BOA_DEV_PALETTE=open|<query>` opens the
/// palette (with `<query>` typed) once the first connection has tables;
/// `BOA_DEV_PALETTE_RECENT=public.orders,public.customers` opens those tables
/// first, so "Recent" shows; `BOA_DEV_PALETTE_ALL_SCHEMAS=1` picks every
/// schema of that database.
#[cfg(feature = "dev-hooks")]
pub mod dev {
    use std::time::Duration;

    use gpui::App;

    use super::*;
    use crate::views::root::RootView;

    pub const ENV: &str = "BOA_DEV_PALETTE";
    pub const RECENT_ENV: &str = "BOA_DEV_PALETTE_RECENT";
    /// Set: show every schema of the first connection's database.
    pub const ALL_SCHEMAS_ENV: &str = "BOA_DEV_PALETTE_ALL_SCHEMAS";

    pub fn apply(cx: &mut App) {
        let Some(what) = std::env::var(ENV).ok().filter(|v| !v.trim().is_empty()) else {
            return;
        };
        let recent = std::env::var(RECENT_ENV).unwrap_or_default();
        let all_schemas = std::env::var(ALL_SCHEMAS_ENV).is_ok();
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        cx.spawn(async move |cx| {
            // The first connection connects (BOA_DEV_CONNECTION), then its
            // database is selected, so its catalog loads.
            for _ in 0..100 {
                let key = cx.update(|cx| {
                    let conns = state.connections.read(cx);
                    let c = conns.saved.first()?;
                    conns
                        .is_connected(&c.config.id)
                        .then(|| DbKey::new(c.config.id.clone(), c.config.database.clone()))
                });
                if let Some(key) = key {
                    cx.update(|cx| {
                        if all_schemas {
                            state.prefs.update(cx, |p, cx| {
                                p.update(cx, |p| {
                                    p.scopes
                                        .insert(key.scope_key(), crate::models::prefs::Scope::All);
                                })
                            });
                        }
                        state
                            .workspaces
                            .update(cx, |w, cx| w.select_database(key, cx));
                    });
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            for _ in 0..150 {
                let settled = cx.update(|cx| {
                    let loading = state.catalogs.read(cx).catalogs.values().any(|c| c.loading);
                    !loading && !palette_items(cx).is_empty()
                });
                if settled {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            cx.update(|cx| open(&what, &recent, cx));
        })
        .detach();
    }

    fn open(what: &str, recent: &str, cx: &mut App) {
        let items = palette_items(cx);
        if let Some(state) = AppState::try_global(cx).cloned() {
            for name in recent.split(',').map(str::trim).rev() {
                if let Some(it) = items
                    .iter()
                    .find(|i| format!("{}.{}", i.schema, i.table) == name)
                {
                    let key = DbKey::new(it.connection_id.clone(), it.database.clone());
                    state.workspaces.update(cx, |w, cx| {
                        w.open_table(key, &it.schema, &it.table, false, cx)
                    });
                }
            }
        }
        let Some(main) = crate::app::main_window(cx) else {
            return;
        };
        let query = if what.trim() == "open" { "" } else { what };
        main.update(cx, |root, window, cx| {
            let Ok(view) = root.view().clone().downcast::<RootView>() else {
                return;
            };
            view.update(cx, |r, cx| r.toggle_palette(window, cx));
            if let Some(palette) = view.read(cx).palette().cloned() {
                palette.update(cx, |p, cx| p.set_query(query, window, cx));
            }
        })
        .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(conn: &str, db: &str, schema: &str, table: &str) -> PaletteItem {
        PaletteItem {
            key: table_key(conn, db, schema, table),
            connection_id: conn.into(),
            connection: format!("{conn} server"),
            database: db.into(),
            schema: schema.into(),
            table: table.into(),
            view: false,
            rows: 10,
        }
    }

    fn items() -> Vec<PaletteItem> {
        vec![
            item("c1", "shop", "public", "customers"),
            item("c1", "shop", "public", "orders"),
            item("c1", "shop", "billing", "order_items"),
            item("c1", "shop", "public", "products"),
            item("c2", "crm", "public", "accounts"),
        ]
    }

    fn names(groups: &[PaletteGroup]) -> Vec<(Option<&'static str>, Vec<&str>)> {
        groups
            .iter()
            .map(|g| {
                (
                    g.label,
                    g.rows.iter().map(|r| r.item.table.as_str()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn empty_query_lists_every_table_untitled_without_recents() {
        let g = palette_groups(&items(), "  ", &[], 5);
        assert_eq!(
            names(&g),
            vec![(
                None,
                vec!["customers", "orders", "order_items", "products", "accounts"]
            )]
        );
    }

    #[test]
    fn recents_come_first_newest_first_up_to_recent_count() {
        let it = items();
        let mru = vec![
            it[3].key.clone(),
            "gone/db/public/dropped".to_owned(),
            it[0].key.clone(),
            it[4].key.clone(),
        ];
        let g = palette_groups(&it, "", &mru, 2);
        assert_eq!(
            names(&g),
            vec![
                (Some("Recent"), vec!["products", "customers"]),
                (
                    Some("All tables"),
                    vec!["orders", "order_items", "accounts"]
                ),
            ]
        );
        // Every table recent: only the Recent group.
        let all: Vec<String> = it.iter().map(|i| i.key.clone()).collect();
        let g = palette_groups(&it, "", &all, 10);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].label, Some("Recent"));
    }

    #[test]
    fn name_matches_rank_first_and_are_highlighted() {
        let g = palette_groups(&items(), "ord", &[], 5);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].label, None);
        let tables: Vec<&str> = g[0].rows.iter().map(|r| r.item.table.as_str()).collect();
        // "orders" (shorter) before "order_items"; no other name has o-r-d,
        // but "products" / "customers" don't match the full text either.
        assert_eq!(tables, vec!["orders", "order_items"]);
        assert_eq!(g[0].rows[0].idxs, vec![0, 1, 2]);
    }

    #[test]
    fn schema_database_and_connection_match_without_highlight() {
        // "billing" only matches through the schema.
        let g = palette_groups(&items(), "billing", &[], 5);
        let rows = &g[0].rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].item.table, "order_items");
        assert!(rows[0].idxs.is_empty());
        // The connection name: c2's table.
        let g = palette_groups(&items(), "c2 server", &[], 5);
        assert_eq!(g[0].rows[0].item.table, "accounts");
        // A name hit beats a full-text hit.
        let g = palette_groups(&items(), "acc", &[], 5);
        assert_eq!(g[0].rows[0].item.table, "accounts");
        assert!(palette_groups(&items(), "zzz", &[], 5)[0].rows.is_empty());
    }

    #[test]
    fn actions_match_when_every_word_starts_one_of_theirs() {
        assert!(match_actions("").is_empty());
        assert_eq!(match_actions("hist")[0].label, "Query History");
        assert_eq!(match_actions("Query hist").len(), 1);
        assert_eq!(match_actions("sql log").len(), 1);
        assert!(match_actions("query xyz").is_empty());
        assert!(match_actions("istory").is_empty());
    }

    #[test]
    fn moving_wraps_around() {
        assert_eq!(wrap_move(0, 1, 3), 1);
        assert_eq!(wrap_move(2, 1, 3), 0);
        assert_eq!(wrap_move(0, -1, 3), 2);
        assert_eq!(wrap_move(7, 1, 3), 0, "a stale index is clamped first");
        assert_eq!(wrap_move(0, 1, 0), 0);
    }

    #[test]
    fn labels_and_tiles() {
        assert_eq!(count_label(1), "1 table");
        assert_eq!(count_label(0), "0 tables");
        assert_eq!(count_label(12), "12 tables");
        // oklch(0.76 0.09 h) is a light, muted colour; same name, same colour.
        let c = tile_color("Local dev");
        assert_eq!(c, tile_color("Local dev"));
        assert!(c.l > 0.55 && c.l < 0.85, "{c:?}");
        // oklch(1 0 0) is white, oklch(0 0 0) black.
        assert!((oklch(1., 0., 0.).l - 1.).abs() < 1e-3);
        assert!(oklch(0., 0., 0.).l < 1e-3);
    }
}

#[cfg(test)]
mod window_tests {
    use std::sync::Arc;

    use boa_core::model::{Schema, Table};
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::models::catalogs::Catalog;
    use crate::models::layout::SettingsPage;
    use crate::tests::{TempDir, init, open_main};
    use crate::views::root::RootView;

    fn table(name: &str, kind: TableKind, rows: i64) -> Table {
        Table {
            name: name.into(),
            kind,
            rows,
            ..Table::default()
        }
    }

    /// The pretend connection's boa_dev with a few tables, loaded.
    fn load_tables(vcx: &mut VisualTestContext) -> DbKey {
        vcx.update(|_, cx| {
            let key = DbKey::new(crate::tests::FAKE_CONNECTION, "boa_dev");
            let catalogs = AppState::global(cx).catalogs.clone();
            catalogs.update(cx, |c, cx| {
                c.catalogs.insert(
                    key.clone(),
                    Catalog {
                        schema_names: vec!["public".into()],
                        schemas: Arc::new(vec![Schema {
                            name: "public".into(),
                            tables: vec![
                                table("customers", TableKind::Table, 5_000),
                                table("orders", TableKind::Table, 100_000),
                                table("order_totals", TableKind::View, 0),
                                table("products", TableKind::Table, 42),
                            ],
                        }]),
                        names_loaded: true,
                        loaded: true,
                        ..Catalog::default()
                    },
                );
                cx.notify();
            });
            key
        })
    }

    fn palette(
        root: &Entity<RootView>,
        vcx: &mut VisualTestContext,
    ) -> Option<Entity<CommandPalette>> {
        vcx.update(|_, cx| root.read(cx).palette().cloned())
    }

    fn listed(root: &Entity<RootView>, vcx: &mut VisualTestContext) -> Vec<String> {
        let p = palette(root, vcx).expect("palette open");
        vcx.update(|_, cx| p.read(cx).listed(cx))
    }

    fn selected(root: &Entity<RootView>, vcx: &mut VisualTestContext) -> usize {
        let p = palette(root, vcx).expect("palette open");
        vcx.update(|_, cx| p.read(cx).selected(cx))
    }

    #[gpui::test]
    fn cmd_k_and_cmd_p_toggle_the_palette_and_esc_closes_it(cx: &mut TestAppContext) {
        let dir = TempDir::new("palette-toggle");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        vcx.simulate_keystrokes("cmd-k");
        let p = palette(&root, &mut vcx).expect("⌘K opens it");
        let focused = vcx.update(|window, cx| p.focus_handle(cx).contains_focused(window, cx));
        assert!(focused, "the search field has the focus");
        vcx.simulate_keystrokes("cmd-k");
        assert!(palette(&root, &mut vcx).is_none(), "⌘K closes it again");
        vcx.simulate_keystrokes("cmd-p");
        assert!(palette(&root, &mut vcx).is_some(), "⌘P too");
        vcx.simulate_keystrokes("escape");
        assert!(palette(&root, &mut vcx).is_none(), "Esc closes it");
        let editor_focused = vcx.update(|window, cx| {
            root.read(cx)
                .workspace()
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        });
        assert!(editor_focused, "the editor has the focus back");
    }

    #[gpui::test]
    fn palette_closes_settings_and_settings_closes_the_palette(cx: &mut TestAppContext) {
        let dir = TempDir::new("palette-settings");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        let page = |vcx: &mut VisualTestContext| {
            vcx.update(|_, cx| AppState::global(cx).layout.read(cx).settings())
        };
        vcx.simulate_keystrokes("cmd-,");
        assert_eq!(page(&mut vcx), Some(SettingsPage::General));
        vcx.simulate_keystrokes("cmd-k");
        assert_eq!(page(&mut vcx), None, "⌘K closes Settings");
        assert!(palette(&root, &mut vcx).is_some());
        vcx.simulate_keystrokes("cmd-,");
        assert_eq!(page(&mut vcx), Some(SettingsPage::General));
        assert!(palette(&root, &mut vcx).is_none(), "Settings closes it");
        // ⌘⌥E closes the palette and opens Query History.
        vcx.simulate_keystrokes("cmd-k");
        vcx.simulate_keystrokes("cmd-alt-e");
        assert!(palette(&root, &mut vcx).is_none());
        assert!(vcx.update(|_, cx| root.read(cx).query_history().is_some()));
    }

    #[gpui::test]
    fn lists_tables_fuzzy_matches_and_moves_with_arrows_and_tab(cx: &mut TestAppContext) {
        let dir = TempDir::new("palette-list");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        load_tables(&mut vcx);
        vcx.simulate_keystrokes("cmd-k");
        assert_eq!(
            listed(&root, &mut vcx),
            [
                "public.customers",
                "public.orders",
                "public.order_totals",
                "public.products"
            ]
        );
        vcx.simulate_input("ord");
        assert_eq!(
            listed(&root, &mut vcx),
            ["public.orders", "public.order_totals", "public.customers"],
            "name matches first; customers through \"…customers boa_dev …\""
        );
        assert_eq!(selected(&root, &mut vcx), 0);
        vcx.simulate_keystrokes("down");
        assert_eq!(selected(&root, &mut vcx), 1);
        vcx.simulate_keystrokes("down down");
        assert_eq!(selected(&root, &mut vcx), 0, "wraps");
        vcx.simulate_keystrokes("shift-tab");
        assert_eq!(selected(&root, &mut vcx), 2);
        vcx.simulate_keystrokes("tab");
        assert_eq!(selected(&root, &mut vcx), 0);
        vcx.simulate_keystrokes("up");
        assert_eq!(selected(&root, &mut vcx), 2);
        // The field kept the text (no tab / arrow went into it).
        let p = palette(&root, &mut vcx).unwrap();
        let text = vcx.update(|_, cx| p.read(cx).input().read(cx).value().to_string());
        assert_eq!(text, "ord");
        // "hist": no table, the Query History action.
        vcx.update(|window, cx| {
            p.read(cx)
                .input()
                .clone()
                .update(cx, |s, cx| s.set_value("", window, cx))
        });
        vcx.simulate_input("hist");
        assert_eq!(listed(&root, &mut vcx), ["Query History"]);
        vcx.simulate_keystrokes("enter");
        assert!(palette(&root, &mut vcx).is_none());
        assert!(
            vcx.update(|_, cx| root.read(cx).query_history().is_some()),
            "⏎ runs the action"
        );
    }

    #[gpui::test]
    fn enter_opens_the_table_pinned_and_it_becomes_recent(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let dir = TempDir::new("palette-open");
        init(cx, dir.path());
        let (root, mut vcx) = open_main(cx);
        let key = load_tables(&mut vcx);
        vcx.simulate_keystrokes("cmd-k");
        vcx.simulate_input("prod");
        vcx.simulate_keystrokes("enter");
        assert!(palette(&root, &mut vcx).is_none(), "⏎ closes the palette");
        let (active, mru) = vcx.update(|_, cx| {
            let w = AppState::global(cx).workspaces.read(cx);
            (w.active_tab().map(|t| t.kind.clone()), w.mru.clone())
        });
        assert!(
            matches!(
                active,
                Some(crate::models::workspaces::TabKind::Table { ref table, preview: false, .. })
                    if table.as_ref() == "products"
            ),
            "{active:?}"
        );
        assert_eq!(
            mru.first().map(String::as_str),
            Some(table_key(&key.connection_id, &key.database, "public", "products").as_str())
        );
        // Empty query: Recent first, then the others.
        // Draw the table tab, so its grid (focused now) is in the dispatch tree.
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        // (Without a server the table tab draws no grid, so the focus
        // moved there is outside the dispatch tree: put it on the window.)
        vcx.update(|window, cx| window.focus(&root.focus_handle(cx), cx));
        vcx.simulate_keystrokes("cmd-k");
        let p = palette(&root, &mut vcx).unwrap();
        let groups = vcx.update(|_, cx| p.read(cx).groups(cx).0);
        let labels: Vec<_> = groups.iter().map(|g| g.label).collect();
        assert_eq!(labels, [Some("Recent"), Some("All tables")]);
        assert_eq!(groups[0].rows[0].item.table, "products");
        assert_eq!(groups[1].rows.len(), 3);
        // recentCount caps the Recent group.
        vcx.update(|_, cx| {
            let workspaces = AppState::global(cx).workspaces.clone();
            workspaces.update(cx, |w, _| {
                let keys: Vec<String> = ["customers", "orders", "order_totals"]
                    .iter()
                    .map(|t| table_key(&key.connection_id, &key.database, "public", t))
                    .collect();
                w.mru.splice(0..0, keys);
            });
            let prefs = AppState::global(cx).prefs.clone();
            prefs.update(cx, |p, cx| p.update(cx, |p| p.recent_count = 3));
        });
        vcx.run_until_parked();
        let groups = vcx.update(|_, cx| p.read(cx).groups(cx).0);
        assert_eq!(groups[0].rows.len(), 3);
        assert_eq!(groups[1].rows.len(), 1);
    }
}
