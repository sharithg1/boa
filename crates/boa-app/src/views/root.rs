//! App.tsx: the main window's root view. DataGrip-style tool windows: the
//! Database Explorer on the left, the work area (title bar with the tabs,
//! then the editor) in the middle and the right panel, above a full-width
//! Database Output panel and the status bar; Settings covers it all when
//! open, toasts float over everything.
//!
//! It also handles the global shortcuts (App.tsx `useShortcuts`; bound in key
//! context `Workspace`, see [`crate::keymap`]) and the panel resizing
//! (PanelResizeHandle.tsx, see [`super::panel_resize`]).
//!
//! Wrap it in gpui-component's `Root` (see `app::open_main_window`).

use gpui::prelude::*;
use gpui::{
    App, Context, DispatchPhase, Entity, FocusHandle, Focusable, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Subscription, Window, canvas, div, px,
};

use super::bottom_panel::BottomPanel;
use super::command_palette::{CommandPalette, PaletteEvent};
use super::panel_resize::{self, HANDLE, PanelDrag};
use super::query_history::{QueryHistoryEvent, QueryHistoryView, insert_into_console};
use super::right_panel::RightPanel;
use super::settings::SettingsView;
use super::sidebar::Sidebar;
use super::status_bar::{STATUS_BAR_HEIGHT, StatusBar};
use super::title_bar::TitleBar;
use super::toasts::ToastsView;
use super::workspace::{Workspace, WorkspaceEvent};
use crate::actions::*;
use crate::app_state::AppState;
use crate::grid::ResultGrid;
use crate::keymap;
use crate::models::history::{History, HistoryEvent};
use crate::models::layout::{Layout, LayoutEvent, PanelKind};
use crate::models::prefs::Prefs;
use crate::models::workspaces::Workspaces;
use crate::theme::{self, ActivePalette as _};

/// Key contexts of the overlays whose open state another view holds and
/// that have the focus while open: a grid's value editor and JSON viewer
/// (also checked directly, see `RootView::grid_overlay_open`) and the
/// explorer's scope picker (a Radix popover, so `role=dialog`, in the source).
const OVERLAY_CONTEXTS: [&str; 3] = [
    keymap::VALUE_EDITOR,
    keymap::JSON_VIEWER,
    keymap::SCOPE_PICKER,
];

/// The main window's root view.
#[derive(Debug)]
pub struct RootView {
    focus_handle: FocusHandle,
    layout: Entity<Layout>,
    prefs: Entity<Prefs>,
    workspaces: Entity<Workspaces>,
    title_bar: Entity<TitleBar>,
    sidebar: Entity<Sidebar>,
    workspace: Entity<Workspace>,
    right_panel: Entity<RightPanel>,
    bottom_panel: Entity<BottomPanel>,
    status_bar: Entity<StatusBar>,
    settings: Entity<SettingsView>,
    toasts: Entity<ToastsView>,
    history: Entity<History>,
    /// The Query History overlay while it is open (⌘⌥E), with its subscription.
    query_history: Option<(Entity<QueryHistoryView>, Subscription)>,
    /// The command palette while it is open (⌘K / ⌘P), with its subscription.
    palette: Option<(Entity<CommandPalette>, Subscription)>,
    /// Opened over a gpui-component dialog: the focus to give back on close.
    palette_over: Option<FocusHandle>,
    /// The same for Query History.
    history_over: Option<FocusHandle>,
    drag: Option<PanelDrag>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    /// Builds the window's views. Needs [`AppState`] (panics without it).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx).clone();
        let workspace = cx.new(|cx| Workspace::new(window, cx));
        let bottom_panel = cx.new(|cx| BottomPanel::new(window, cx));
        let settings = cx.new(|cx| SettingsView::new(window, cx));
        let toasts = cx.new(|cx| ToastsView::new(state.toasts.clone(), cx));
        let status_bar = cx.new(StatusBar::new);
        super::guard_dialogs::install(window, cx);
        let caret = cx.subscribe(&workspace, |this, _, event: &WorkspaceEvent, cx| {
            let WorkspaceEvent::Caret(caret) = *event;
            this.status_bar.update(cx, |s, cx| s.set_caret(caret, cx));
        });
        let layout_events = cx.subscribe_in(
            &state.layout,
            window,
            |this, _, event: &LayoutEvent, window, cx| {
                if let LayoutEvent::Settings(page) = event {
                    // Over a dialog (⌘, works there, App.tsx) Settings opens
                    // under it (`z-40` vs the dialogs' `z-50`) and the
                    // dialog keeps the focus.
                    let dialog = gpui_component::WindowExt::has_active_dialog(window, cx);
                    if page.is_some() {
                        // Opening Settings closes the palette.
                        this.palette = None;
                        this.palette_over = None;
                        // Not when it has it (a page change from the search field).
                        let settings = this.settings.focus_handle(cx);
                        if !dialog && !settings.contains_focused(window, cx) {
                            window.focus(&settings, cx);
                        }
                    } else if !this.overlay_open(window, cx) {
                        // Not when the palette or History replaced it (⌘K,
                        // ⌘⌥E: they have the focus) or a dialog is open.
                        this.focus_editor(window, cx);
                    }
                }
            },
        );
        let history_events = cx.subscribe_in(
            &state.history,
            window,
            |this, _, event: &HistoryEvent, window, cx| match event {
                HistoryEvent::Opened => this.open_history(window, cx),
                HistoryEvent::Closed => this.close_history(window, cx),
            },
        );
        // A closed dialog, menu or popover whose own focus handle is gone
        // leaves nothing focused, and then no key reaches the `Workspace`
        // context (⌘T, ⌘W, ⌥1… would do nothing): give the focus back to the
        // active tab (or Settings while it is open), as the Tauri app's
        // document-level shortcuts never lose it.
        let focus_lost = cx.on_focus_lost(window, |this: &mut Self, window, cx| {
            if this.settings_open(cx) {
                window.focus(&this.settings.focus_handle(cx), cx);
            } else {
                this.focus_editor(window, cx);
            }
        });
        // View › Enter / Exit Full Screen follows this window.
        let bounds = cx.observe_window_bounds(window, |_, window, cx| {
            crate::app::sync_full_screen_menu(window.is_fullscreen(), cx);
        });
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if window.is_window_active() {
                crate::app::sync_full_screen_menu(window.is_fullscreen(), cx);
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            title_bar: cx.new(TitleBar::new),
            sidebar: cx.new(Sidebar::new),
            right_panel: cx.new(RightPanel::new),
            status_bar,
            workspaces: state.workspaces.clone(),
            _subscriptions: vec![
                caret,
                focus_lost,
                bounds,
                activation,
                layout_events,
                history_events,
                cx.observe(&state.layout, |_, _, cx| cx.notify()),
                cx.observe(&state.prefs, |_, _, cx| cx.notify()),
            ]
            .into_iter()
            .chain(super::password_prompt::subscribe(window, cx))
            .collect(),
            layout: state.layout,
            prefs: state.prefs,
            workspace,
            bottom_panel,
            settings,
            toasts,
            history: state.history,
            query_history: None,
            palette: None,
            palette_over: None,
            history_over: None,
            drag: None,
        }
    }

    /// The Query History overlay, while it is open.
    pub fn query_history(&self) -> Option<&Entity<QueryHistoryView>> {
        self.query_history.as_ref().map(|(view, _)| view)
    }

    fn open_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_history.is_some() {
            return;
        }
        // Handed over from the palette it replaced (⌘⌥E), else from here.
        let over = self.history_over.take();
        self.history_over = over.or_else(|| Self::over_dialog(window, cx));
        let view = cx.new(|cx| QueryHistoryView::new(window, cx));
        let events = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &QueryHistoryEvent, window, cx| {
                let QueryHistoryEvent::Insert(sql) = event;
                // Close first, so the editor has the focus back, then insert.
                this.query_history = None;
                this.history_over = None;
                this.history.update(cx, |h, cx| h.set_open(false, cx));
                this.focus_editor(window, cx);
                let editor = this.workspace.read(cx).editor(cx);
                insert_into_console(editor, sql, window, cx);
                cx.notify();
            },
        );
        let input = view.read(cx).input().clone();
        input.update(cx, |input, cx| input.focus(window, cx));
        self.query_history = Some((view, events));
        cx.notify();
    }

    /// The command palette, while it is open.
    pub fn palette(&self) -> Option<&Entity<CommandPalette>> {
        self.palette.as_ref().map(|(view, _)| view)
    }

    /// ⌘K / ⌘P (App.tsx: `paletteOpen: !paletteOpen, settings: null`).
    pub fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        self.close_settings(cx);
        self.palette_over = Self::over_dialog(window, cx);
        let view = cx.new(|cx| CommandPalette::new(window, cx));
        let events = cx.subscribe_in(&view, window, |this, _, event, window, cx| {
            let PaletteEvent::Dismissed = event;
            this.close_palette(window, cx);
        });
        let input = view.read(cx).input().clone();
        input.update(cx, |input, cx| input.focus(window, cx));
        self.palette = Some((view, events));
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let over = self.palette_over.take();
        if self.palette.take().is_some() {
            self.refocus(over, window, cx);
            cx.notify();
        }
    }

    fn close_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let over = self.history_over.take();
        if self.query_history.take().is_some() {
            self.refocus(over, window, cx);
            cx.notify();
        }
    }

    /// ⌘⌥E (App.tsx: `paletteOpen: false, settings: null`, then toggle).
    pub fn toggle_query_history(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            // History takes the focus; a dialog under the palette keeps
            // its place under History.
            let over = self.palette_over.take();
            if self.query_history.is_none() {
                self.history_over = over;
            }
            cx.notify();
        }
        self.close_settings(cx);
        self.history.update(cx, |h, cx| h.toggle(cx));
    }

    /// The palette and Query History are Radix dialogs in the source, so
    /// one opened over another dialog (⌘K in the connection dialog) shows on
    /// top of it, and closing it gives that dialog the focus back. Here:
    /// the focus to give back, when a gpui-component dialog is open.
    fn over_dialog(window: &mut Window, cx: &mut App) -> Option<FocusHandle> {
        if gpui_component::WindowExt::has_active_dialog(window, cx) {
            window.focused(cx)
        } else {
            None
        }
    }

    /// After the palette or History closed: back to the dialog it was
    /// opened over while that is still open, else the active tab.
    fn refocus(&self, over: Option<FocusHandle>, window: &mut Window, cx: &mut App) {
        match over {
            Some(handle) if gpui_component::WindowExt::has_active_dialog(window, cx) => {
                window.focus(&handle, cx);
            }
            _ => self.focus_editor(window, cx),
        }
    }

    /// While Settings is open with nothing over it, the focus belongs in it
    /// (its keys, Esc and ⌘F, are window-wide in the source). A dialog closed
    /// over Settings (⌘, opens it under a dialog) gives the focus back to
    /// the tab behind Settings; this takes it back, after the frame.
    fn keep_focus_in_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings_open(cx) || self.overlay_open(window, cx) {
            return;
        }
        let settings = self.settings.focus_handle(cx);
        if settings.contains_focused(window, cx) {
            return;
        }
        cx.defer_in(window, |this, window, cx| {
            if this.settings_open(cx) && !this.overlay_open(window, cx) {
                let settings = this.settings.focus_handle(cx);
                if !settings.contains_focused(window, cx) {
                    window.focus(&settings, cx);
                }
            }
        });
    }

    /// Whether the palette was opened over a gpui-component dialog (drawn above it).
    pub fn palette_over_dialog(&self) -> bool {
        self.palette_over.is_some()
    }

    pub fn workspace(&self) -> &Entity<Workspace> {
        &self.workspace
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub fn title_bar(&self) -> &Entity<TitleBar> {
        &self.title_bar
    }

    pub fn bottom_panel(&self) -> &Entity<BottomPanel> {
        &self.bottom_panel
    }

    pub fn settings(&self) -> &Entity<SettingsView> {
        &self.settings
    }

    pub fn status_bar(&self) -> &Entity<StatusBar> {
        &self.status_bar
    }

    pub fn right_panel(&self) -> &Entity<RightPanel> {
        &self.right_panel
    }

    /// The active console editor's text ("" when the active tab isn't a console).
    pub fn editor_text(&self, cx: &App) -> String {
        self.workspace
            .read(cx)
            .editor(cx)
            .map(|e| e.read(cx).text(cx).to_string())
            .unwrap_or_default()
    }

    /// The resize in progress, if any.
    pub fn drag(&self) -> Option<PanelDrag> {
        self.drag
    }

    /// Focuses the active tab (its editor or grid).
    pub fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        let handle = self.workspace.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// The size `panel` shows at (the drag's while dragging it, else the saved one, clamped).
    pub fn panel_size(&self, panel: PanelKind, window: &Window, cx: &App) -> u32 {
        if let Some(drag) = self.drag.filter(|d| d.panel == panel) {
            return drag.size;
        }
        let height = f64::from(f32::from(window.viewport_size().height));
        panel.saved_size(self.prefs.read(cx).get(), height)
    }

    fn settings_open(&self, cx: &App) -> bool {
        self.layout.read(cx).settings().is_some()
    }

    /// Whether something dialog-like is open over the main window: App.tsx
    /// and GuardDialogs.tsx check `[role=dialog],[role=alertdialog]` (Radix
    /// dialogs, alert dialogs and popovers). Here: the command palette, Query
    /// History, the gpui-component dialogs (connection dialog, password
    /// prompt, guard dialogs, edit preview and discard prompts), a grid's
    /// value editor or JSON viewer, and the scope picker. Settings is not one
    /// (App.tsx checks it separately).
    pub fn overlay_open(&self, window: &mut Window, cx: &mut App) -> bool {
        self.palette.is_some()
            || self.query_history.is_some()
            || gpui_component::WindowExt::has_active_dialog(window, cx)
            || self.grid_overlay_open(cx)
            || window
                .context_stack()
                .iter()
                .any(|c| OVERLAY_CONTEXTS.iter().any(|name| c.contains(name)))
    }

    /// The Database Output grid's or the active table tab's value editor /
    /// JSON viewer is open (they draw over the whole window).
    fn grid_overlay_open(&self, cx: &App) -> bool {
        let open = |grid: &Entity<ResultGrid>| {
            let grid = grid.read(cx);
            grid.value_editor().is_some() || grid.json_viewer().is_some()
        };
        if open(self.bottom_panel.read(cx).grid()) {
            return true;
        }
        let table = self
            .workspaces
            .read(cx)
            .active_tab()
            .and_then(|t| t.table().cloned());
        table
            .and_then(|t| self.workspace.read(cx).grid_of(&t))
            .is_some_and(|grid| open(&grid))
    }

    // ── Panel resizing ────────────────────────────────────────────────────

    fn begin_drag(
        &mut self,
        panel: PanelKind,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            // Double click: back to the default size.
            self.drag = None;
            self.prefs.update(cx, |prefs, cx| {
                prefs.update(cx, |p| panel.store(p, panel.default_size()))
            });
            cx.notify();
            return;
        }
        let size = self.panel_size(panel, window, cx);
        self.drag = Some(PanelDrag::begin(panel, event.position, size));
        cx.notify();
    }

    fn drag_moved(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        let height = f32::from(window.viewport_size().height);
        let before = drag.size;
        if drag.update(event.position, height) != before {
            cx.notify();
        }
    }

    fn drag_ended(&mut self, event: &MouseUpEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        let height = f32::from(window.viewport_size().height);
        let size = drag.update(event.position, height);
        self.prefs.update(cx, |prefs, cx| {
            prefs.update(cx, |p| drag.panel.store(p, size))
        });
        cx.notify();
    }

    fn resize_handle(&self, panel: PanelKind, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let this = cx.entity().downgrade();
        let active = self.drag.is_some_and(|d| d.panel == panel);
        panel_resize::handle(
            panel,
            active,
            cx.palette().border_strong,
            move |event, window, cx| {
                this.update(cx, |this, cx| this.begin_drag(panel, event, window, cx))
                    .ok();
            },
        )
    }

    // ── Global shortcuts (App.tsx useShortcuts) ───────────────────────────

    /// For the shortcuts App.tsx ignores while Settings or a dialog is open
    /// (`if (overlay || state.settings) return`), and the ⌥ ones that
    /// composed a character: true when the handler should not run (it then
    /// lets the key through, so ⌥B typed in History's search types "∫").
    fn pass_through(&self, option_key: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let skip = self.settings_open(cx)
            || self.overlay_open(window, cx)
            || (option_key && !keymap::option_shortcut_applies(cx));
        if skip {
            cx.propagate();
        }
        skip
    }

    fn toggle_panel(&mut self, panel: PanelKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.pass_through(true, window, cx) {
            return;
        }
        self.layout.update(cx, |l, cx| l.toggle(panel, cx));
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.layout.update(cx, |l, cx| l.set_settings(None, cx));
    }

    fn register_actions(
        &self,
        el: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        el.on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| {
            this.toggle_panel(PanelKind::Sidebar, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleRightPanel, window, cx| {
            this.toggle_panel(PanelKind::RightPanel, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleBottomPanel, window, cx| {
            this.toggle_panel(PanelKind::Bottom, window, cx)
        }))
        .on_action(cx.listener(|this, _: &TogglePalette, window, cx| {
            this.toggle_palette(window, cx);
        }))
        .on_action(cx.listener(|this, _: &ToggleQueryHistory, window, cx| {
            this.toggle_query_history(window, cx);
        }))
        .on_action(cx.listener(|this, _: &NewConnection, window, cx| {
            // ⌘O: not over another dialog (App.tsx); works over Settings.
            if !this.overlay_open(window, cx) {
                super::connection_dialog::open(None, window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &NewQueryTab, window, cx| {
            // App.tsx: ⌘T opens a console on the selected database.
            if this.pass_through(false, window, cx) {
                return;
            }
            let active = this.workspaces.read(cx).active.clone();
            if let Some(key) = active {
                this.workspaces.update(cx, |w, cx| w.new_query_tab(key, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &NewQueryConsole, window, cx| {
            // ⌥N: the selected database, else the first connected one.
            if this.pass_through(true, window, cx) {
                return;
            }
            let target = this.workspaces.read(cx).new_query_target(cx);
            if let Some(key) = target {
                this.workspaces.update(cx, |w, cx| w.new_query_tab(key, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
            // Over a dialog or Settings App.tsx returns early. ⌘W then does
            // nothing here, where in the Tauri app the key went on to the
            // menu's Close Window and closed the main window (and so quit):
            // it would throw away what the dialog holds (the value editor's
            // text…): a deliberate deviation.
            if this.settings_open(cx) || this.overlay_open(window, cx) {
                return;
            }
            // No tab to close: App.tsx doesn't `preventDefault`, so the
            // menu's Close Window (⌘W) closes the window (asking first with a
            // transaction open). After this dispatch: the window is in use.
            if !this.workspaces.update(cx, |w, cx| w.close_active(cx)) {
                let handle = window.window_handle();
                cx.defer(move |cx| crate::app::close_window(handle, cx));
            }
        }))
        .on_action(cx.listener(|this, action: &ActivateTab, window, cx| {
            if !this.pass_through(true, window, cx) {
                let index = action.0;
                this.workspaces
                    .update(cx, |w, cx| w.activate_index(index, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &PreviousDatabase, window, cx| {
            if !this.pass_through(true, window, cx) {
                this.workspaces.update(cx, |w, cx| w.cycle_database(-1, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &NextDatabase, window, cx| {
            if !this.pass_through(true, window, cx) {
                this.workspaces.update(cx, |w, cx| w.cycle_database(1, cx));
            }
        }))
        // GuardDialogs.tsx useTxShortcuts: only with a transaction open, and
        // not while a dialog is open (Settings doesn't count).
        .on_action(cx.listener(|this, _: &Commit, window, cx| {
            if this.overlay_open(window, cx) || !super::tx_controls::shortcut(true, cx) {
                cx.propagate();
            }
        }))
        .on_action(cx.listener(|this, _: &Rollback, window, cx| {
            if this.overlay_open(window, cx) || !super::tx_controls::shortcut(false, cx) {
                cx.propagate();
            }
        }))
    }
}

#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod shortcut_tests;
#[cfg(test)]
mod tests;

impl Focusable for RootView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.keep_focus_in_settings(window, cx);
        let p = cx.palette().clone();
        let (sidebar_open, right_open, bottom_open, settings_open) = {
            let l = self.layout.read(cx);
            (
                l.sidebar_open(),
                l.right_panel_open(),
                l.bottom_open(),
                l.settings().is_some(),
            )
        };
        let sidebar_w = self.panel_size(PanelKind::Sidebar, window, cx) as f32;
        let right_w = self.panel_size(PanelKind::RightPanel, window, cx) as f32;
        let bottom_h = self.panel_size(PanelKind::Bottom, window, cx) as f32;
        let below = f32::from(STATUS_BAR_HEIGHT) + if bottom_open { bottom_h } else { 0. };
        let half = f32::from(HANDLE) / 2.;

        let main_row = div()
            .flex()
            .flex_1()
            .min_h_0()
            .when(sidebar_open, |d| {
                d.child(
                    div()
                        .flex_none()
                        .h_full()
                        .w(px(sidebar_w))
                        .child(self.sidebar.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .child(self.title_bar.clone())
                    .child(div().flex_1().min_h_0().child(self.workspace.clone())),
            )
            .when(right_open, |d| {
                d.child(
                    div()
                        .flex_none()
                        .h_full()
                        .w(px(right_w))
                        .child(self.right_panel.clone()),
                )
            });

        // The handles go after the panels, so they win the hit test on the
        // 3 px that overlap the neighbour.
        let mut handles = Vec::new();
        if sidebar_open {
            handles.push(
                self.resize_handle(PanelKind::Sidebar, cx)
                    .top_0()
                    .left(px(sidebar_w - half))
                    .bottom(px(below)),
            );
        }
        if right_open {
            handles.push(
                self.resize_handle(PanelKind::RightPanel, cx)
                    .top_0()
                    .right(px(right_w - half))
                    .bottom(px(below)),
            );
        }
        if bottom_open {
            handles.push(
                self.resize_handle(PanelKind::Bottom, cx)
                    .left_0()
                    .right_0()
                    .bottom(px(below - half)),
            );
        }

        // While dragging: follow the pointer anywhere in the window.
        let follow = self.drag.map(|_| {
            let entity = cx.entity().downgrade();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let moved = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            moved
                                .update(cx, |this, cx| this.drag_moved(event, window, cx))
                                .ok();
                        }
                    });
                    let released = entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            released
                                .update(cx, |this, cx| this.drag_ended(event, window, cx))
                                .ok();
                        }
                    });
                },
            )
            .absolute()
            .size_full()
            .into_any_element()
        });

        let root = div()
            .id("root")
            .key_context(keymap::WORKSPACE)
            .track_focus(&self.focus_handle);
        self.register_actions(root, cx)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(p.background)
            .text_color(p.body)
            .font_family(theme::UI_FONT)
            .text_size(theme::text::SM)
            .line_height(theme::text::SM_LINE)
            .when_some(self.drag, |d, drag| {
                if drag.panel.vertical_edge() {
                    d.cursor_col_resize()
                } else {
                    d.cursor_row_resize()
                }
            })
            .child(main_row)
            .when(bottom_open, |d| {
                d.child(
                    div()
                        .flex_none()
                        .w_full()
                        .h(px(bottom_h))
                        .child(self.bottom_panel.clone()),
                )
            })
            .child(self.status_bar.clone())
            .children(handles)
            .when(settings_open, |d| d.child(self.settings.clone()))
            .children(
                self.query_history()
                    .map(|v| over_dialogs(v.clone(), self.history_over.is_some())),
            )
            .children(
                self.palette()
                    .map(|v| over_dialogs(v.clone(), self.palette_over.is_some())),
            )
            .child(self.toasts.clone())
            .children(follow)
    }
}

/// gpui-component draws dialog `n` (from 0) deferred at priority 10 + n,
/// above everything in this view. An overlay opened over one is drawn at
/// 11: above the first dialog, and under a dialog opened after it (the next
/// layer shares 11 and is drawn later), as Radix stacks them by opening order.
const OVER_DIALOG_PRIORITY: usize = 11;

/// `view` as is, or over the dialogs when it was opened over one.
fn over_dialogs(view: impl IntoElement, over: bool) -> gpui::AnyElement {
    if over {
        gpui::deferred(view)
            .with_priority(OVER_DIALOG_PRIORITY)
            .into_any_element()
    } else {
        view.into_any_element()
    }
}
