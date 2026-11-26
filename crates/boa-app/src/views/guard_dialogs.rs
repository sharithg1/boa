//! GuardDialogs.tsx: the confirmations of the guard rails
//! (`boa_core::state::guards` decides when, and holds the texts), one at a
//! time in the main window; a new one answers the earlier one "no"
//! (guards.ts `ask`):
//!
//! - "Run a destructive statement?" ([`ask_run`], [`confirm_run`]): the
//!   risks, the statements, the connection with its tag; Cancel is the
//!   default (Enter), Run is destructive.
//! - "Disconnect / Reconnect / Quit / Restart to update with an open
//!   transaction?" ([`confirm_close_tx`]): the open transactions, Cancel or
//!   "Roll back and …".
//! - "Switch to auto-commit?" ([`choose_end_tx`]): leaving Manual with a
//!   transaction open; Rollback or Commit (not for an aborted one).
//!
//! Enter (and Esc) is Cancel. ⌘⇧↵ / ⌘⇧⌫ answer nothing here: the source's
//! `useTxShortcuts` returns while any dialog is open.
//!
//! [`install`] (called once by the main window's root view) asks before the
//! Connections model disconnects or reconnects a connection with a
//! transaction open, and before the main window closes with one open (the
//! last window: closing it quits). ⌘Q / Quit go through `app::request_quit`.

use std::cell::Cell;

use boa_core::connections::SavedConnection;
use boa_core::grid::conn_tag::is_production;
use boa_core::sql::guard::Risk;
use boa_core::state::guards::{
    CLOSE_TX_DESCRIPTION, CloseAction, END_TX_TITLE, EndTxChoice, RUN_CONFIRM_LABEL,
    end_tx_choices, end_tx_description, open_tx_label, risk_statements, run_confirm_title,
};
use futures::channel::oneshot;
use gpui::prelude::*;
use gpui::{
    AnyWindowHandle, App, Context, Entity, FocusHandle, Focusable, Global, Task, WeakEntity,
    Window, div, px,
};
use gpui_component::WindowExt as _;
use gpui_component::button::{Button, ButtonVariants as _};

use super::chrome::{IconName, icon};
use super::connection_dialog::{dialog_surface, md_button};
use super::{connection_tag, connection_tint, env_tag};
use crate::actions::Dismiss;
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::console::Console;
use crate::models::transactions::TxState;
use crate::theme::{self, ActivePalette as _};

/// Key context of a guard dialog's content (keymap: Enter is Cancel).
pub const GUARD_DIALOG: &str = "GuardDialog";

/// How the user answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardAnswer {
    /// Cancel, Esc, Enter, or replaced by another prompt.
    Cancel,
    /// Run / "Roll back and …".
    Confirm,
    Commit,
    Rollback,
}

/// One open transaction in the close prompt's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTxRow {
    pub connection_id: String,
    /// The connection's name (its id when unknown; filled in when shown).
    pub connection: String,
    pub database: String,
    pub state: TxState,
}

/// What a guard dialog asks.
#[derive(Debug, Clone)]
pub enum GuardKind {
    /// Before a destructive statement.
    Run {
        connection: Option<Box<SavedConnection>>,
        connection_name: String,
        database: String,
        risks: Vec<Risk>,
        production: bool,
    },
    /// Before closing sessions with a transaction open.
    CloseTx {
        action: CloseAction,
        open: Vec<OpenTxRow>,
    },
    /// Leaving Manual mode with a transaction open.
    EndTx { database: String, state: TxState },
}

impl GuardKind {
    /// The dialog's title.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Run { production, .. } => run_confirm_title(*production),
            Self::CloseTx { action, .. } => action.title(),
            Self::EndTx { .. } => END_TX_TITLE,
        }
    }

    /// Its width (`sm:max-w-[…]`).
    fn width(&self) -> f32 {
        match self {
            Self::Run { .. } => 560.,
            Self::CloseTx { .. } => 460.,
            Self::EndTx { .. } => 440.,
        }
    }
}

/// A guard dialog's content.
pub struct GuardDialog {
    kind: GuardKind,
    focus_handle: FocusHandle,
    reply: Option<oneshot::Sender<GuardAnswer>>,
    closed: bool,
}

impl std::fmt::Debug for GuardDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardDialog")
            .field("kind", &self.kind)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

/// The guard dialog showing, if any (one at a time).
#[derive(Default)]
struct ActiveGuard(Option<WeakEntity<GuardDialog>>);

impl Global for ActiveGuard {}

impl GuardDialog {
    fn new(kind: GuardKind, reply: oneshot::Sender<GuardAnswer>, cx: &mut Context<Self>) -> Self {
        Self {
            kind,
            focus_handle: cx.focus_handle(),
            reply: Some(reply),
            closed: false,
        }
    }

    pub fn kind(&self) -> &GuardKind {
        &self.kind
    }

    /// Answered or dismissed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Answers and closes the dialog.
    pub fn answer(&mut self, answer: GuardAnswer, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(answer);
        }
        if !self.closed {
            self.closed = true;
            window.close_dialog(cx);
        }
        cx.notify();
    }

    /// Esc (or the dialog went away): Cancel.
    fn dismissed(&mut self, cx: &mut Context<Self>) {
        self.closed = true;
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(GuardAnswer::Cancel);
        }
        cx.notify();
    }

    /// A footer button: button.tsx `size="md"` (30 px, text-sm medium).
    fn button(
        button: Button,
        label: &'static str,
        answer: GuardAnswer,
        cx: &mut Context<Self>,
    ) -> Button {
        md_button(button, label)
            .on_click(cx.listener(move |this, _, window, cx| this.answer(answer, window, cx)))
    }

    fn render_run(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = cx.palette().clone();
        let GuardKind::Run {
            connection,
            connection_name,
            database,
            risks,
            ..
        } = &self.kind
        else {
            return Vec::new();
        };
        let color = connection_tint(connection.as_deref());
        let statements = risk_statements(risks);
        vec![
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children(color.map(|c| div().flex_none().size(px(8.)).rounded_full().bg(c)))
                .child(
                    div()
                        .font_family(theme::MONO_FONT)
                        .text_size(theme::text::XS)
                        .text_color(p.quiet)
                        .child(format!("{connection_name} · {database}")),
                )
                .children(connection_tag(connection.as_deref(), cx))
                .into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_size(theme::text::SM)
                .children(risks.iter().map(|r| {
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .text_color(p.status_err_text)
                        .child(div().mt(px(2.)).flex_none().child(icon(
                            IconName::TriangleAlert,
                            16.,
                            p.status_err_text,
                        )))
                        .child(div().min_w_0().flex_1().child(r.message.clone()))
                }))
                .into_any_element(),
            // The statements, with the connection colour as a left border.
            div()
                .flex()
                .rounded(theme::radius::MD)
                .border_1()
                .border_color(p.border)
                .overflow_hidden()
                .bg(p.editor)
                .children(color.map(|c| div().flex_none().w(px(3.)).bg(c)))
                .child(
                    div()
                        .id("guard-statements")
                        .flex_1()
                        .min_w_0()
                        .max_h(px(192.))
                        .overflow_y_scroll()
                        .px(px(12.))
                        .py(px(8.))
                        .font_family(theme::MONO_FONT)
                        .text_size(theme::text::XS)
                        .text_color(p.body)
                        .child(statements),
                )
                .into_any_element(),
        ]
    }

    fn render_close(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = cx.palette().clone();
        let GuardKind::CloseTx { open, .. } = &self.kind else {
            return Vec::new();
        };
        vec![
            div()
                .text_size(theme::text::SM)
                .text_color(p.quiet)
                .child(CLOSE_TX_DESCRIPTION)
                .into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(theme::text::SM)
                .children(open.iter().map(|o| {
                    let failed = o.state == TxState::Failed;
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().truncate().child(o.connection.clone()))
                        .child(
                            div()
                                .font_family(theme::MONO_FONT)
                                .text_size(theme::text::XS)
                                .text_color(p.quiet)
                                .child(o.database.clone()),
                        )
                        .child(
                            div()
                                .text_size(theme::text::XS)
                                .text_color(if failed {
                                    p.status_err_text
                                } else {
                                    p.status_wait
                                })
                                .child(open_tx_label(o.state)),
                        )
                }))
                .into_any_element(),
        ]
    }

    fn render_end(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = cx.palette().clone();
        let GuardKind::EndTx { database, state } = &self.kind else {
            return Vec::new();
        };
        vec![
            div()
                .text_size(theme::text::SM)
                .text_color(p.quiet)
                .child(end_tx_description(database, *state))
                .into_any_element(),
        ]
    }

    fn footer(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = cx.palette().clone();
        let cancel = Self::button(
            Button::new("guard-cancel").outline(),
            "Cancel",
            GuardAnswer::Cancel,
            cx,
        );
        let mut buttons = vec![cancel.into_any_element()];
        match &self.kind {
            GuardKind::Run { .. } => buttons.push(
                Self::button(
                    destructive(Button::new("guard-confirm"), &p),
                    RUN_CONFIRM_LABEL,
                    GuardAnswer::Confirm,
                    cx,
                )
                .into_any_element(),
            ),
            GuardKind::CloseTx { action, .. } => buttons.push(
                Self::button(
                    destructive(Button::new("guard-confirm"), &p),
                    action.confirm_label(),
                    GuardAnswer::Confirm,
                    cx,
                )
                .into_any_element(),
            ),
            GuardKind::EndTx { state, .. } => {
                let failed = *state == TxState::Failed;
                let rollback = Button::new("guard-rollback");
                let rollback = if failed {
                    rollback.primary()
                } else {
                    rollback.outline()
                };
                buttons.push(
                    Self::button(rollback, "Rollback", GuardAnswer::Rollback, cx)
                        .into_any_element(),
                );
                if end_tx_choices(*state).contains(&EndTxChoice::Commit) {
                    buttons.push(
                        Self::button(
                            Button::new("guard-commit").primary(),
                            "Commit",
                            GuardAnswer::Commit,
                            cx,
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        div().flex().justify_end().gap(px(8.)).children(buttons)
    }
}

impl Focusable for GuardDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GuardDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let body = match &self.kind {
            GuardKind::Run { .. } => self.render_run(cx),
            GuardKind::CloseTx { .. } => self.render_close(cx),
            GuardKind::EndTx { .. } => self.render_end(cx),
        };
        let footer = self.footer(cx);
        // AlertDialogHeader: the title and the description, 6 px apart
        // (`gap-1.5`); then the body and the footer, 12 px apart (`gap-3`).
        let mut body = body.into_iter();
        let description = body.next();
        div()
            .id("guard-dialog")
            .key_context(GUARD_DIALOG)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                this.answer(GuardAnswer::Cancel, window, cx)
            }))
            .flex()
            .flex_col()
            .gap(px(12.))
            .text_color(p.body)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(theme::text::BASE)
                            .line_height(theme::text::BASE_LINE)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.kind.title()),
                    )
                    .children(description),
            )
            .children(body)
            .child(footer)
    }
}

/// button.tsx `variant="destructive"`: `bg-destructive text-white` (the
/// theme's danger button is a translucent tint).
fn destructive(button: Button, p: &theme::Palette) -> Button {
    button.danger().bg(p.destructive).text_color(gpui::white())
}

/// The guard dialog showing, if any.
pub fn active(cx: &App) -> Option<Entity<GuardDialog>> {
    cx.try_global::<ActiveGuard>()
        .and_then(|a| a.0.as_ref())
        .and_then(WeakEntity::upgrade)
        .filter(|d| !d.read(cx).closed)
}

/// The window the guards ask in (the main window, set by [`install`]).
struct GuardWindow(AnyWindowHandle);

impl Global for GuardWindow {}

/// The main window, while it is open (also while it is being updated, when
/// `app::main_window` can't see it).
pub fn guard_window(cx: &App) -> Option<AnyWindowHandle> {
    let window = cx.try_global::<GuardWindow>()?.0;
    cx.windows()
        .iter()
        .any(|w| w.window_id() == window.window_id())
        .then_some(window)
}

/// Shows `kind` in the main window (brought to the front) and resolves
/// with the answer: Cancel without a main window, or when another prompt
/// replaces it.
pub fn ask(kind: GuardKind, cx: &mut App) -> Task<GuardAnswer> {
    let (reply, answer) = oneshot::channel();
    // Not while a window (maybe this one) or the model that asks is being
    // updated; through the untyped handle, so the root view isn't leased
    // (the dialog layer lives in it).
    cx.defer(move |cx| match guard_window(cx) {
        Some(window) => {
            window
                .update(cx, |_, window, cx| open(kind, reply, window, cx))
                .ok();
        }
        None => drop(reply),
    });
    cx.foreground_executor()
        .spawn(async move { answer.await.unwrap_or(GuardAnswer::Cancel) })
}

fn open(
    mut kind: GuardKind,
    reply: oneshot::Sender<GuardAnswer>,
    window: &mut Window,
    cx: &mut App,
) {
    if let (GuardKind::CloseTx { open, .. }, Some(state)) = (&mut kind, AppState::try_global(cx)) {
        let conns = state.connections.read(cx);
        for row in open {
            if let Some(c) = conns.get(&row.connection_id) {
                row.connection = c.config.name.clone();
            }
        }
    }
    if let Some(prev) = active(cx) {
        prev.update(cx, |d, cx| d.answer(GuardAnswer::Cancel, window, cx));
    }
    crate::app::activate_window(window);
    let width = kind.width();
    let view = cx.new(|cx| GuardDialog::new(kind, reply, cx));
    cx.set_global(ActiveGuard(Some(view.downgrade())));
    let first = Cell::new(true);
    let content = view;
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            let handle = content.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        }
        let on_close = content.downgrade();
        dialog_surface(dialog, cx)
            .w(px(width))
            .close_button(false)
            .overlay_closable(false)
            .on_close(move |_, _, cx| {
                if let Some(view) = on_close.upgrade() {
                    view.update(cx, |d, cx| d.dismissed(cx));
                }
            })
            .child(content.clone())
    });
}

// ── The guards ────────────────────────────────────────────────────────────

/// "Run a destructive statement?" for `risks` on session `key`: resolves
/// true to run.
pub fn ask_run(key: &DbKey, risks: Vec<Risk>, production: bool, cx: &mut App) -> Task<bool> {
    let connection = AppState::try_global(cx)
        .and_then(|s| s.connections.read(cx).get(&key.connection_id).cloned())
        .map(Box::new);
    let connection_name = connection
        .as_ref()
        .map_or_else(|| key.connection_id.clone(), |c| c.config.name.clone());
    let production = production
        || connection
            .as_ref()
            .is_some_and(|c| is_production(env_tag(c.config.environment)));
    let task = ask(
        GuardKind::Run {
            connection,
            connection_name,
            database: key.database.clone(),
            risks,
            production,
        },
        cx,
    );
    cx.foreground_executor()
        .spawn(async move { task.await == GuardAnswer::Confirm })
}

/// The console's `ConsoleEvent::ConfirmRun`: asks, then executes `sql` if confirmed.
pub fn confirm_run(
    console: Entity<Console>,
    sql: String,
    risks: Vec<Risk>,
    production: bool,
    cx: &mut App,
) {
    let key = console.read(cx).key.clone();
    let task = ask_run(&key, risks, production, cx);
    cx.spawn(async move |cx| {
        if task.await {
            console.update(cx, |c, cx| c.execute(sql, cx));
        }
    })
    .detach();
}

/// Resolves true to go ahead with closing the sessions of `connection_id`
/// (all connections when None): no transaction is open, or the user accepts
/// that it is rolled back (`confirmCloseTx`).
pub fn confirm_close_tx(
    action: CloseAction,
    connection_id: Option<&str>,
    cx: &mut App,
) -> Task<bool> {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return Task::ready(true);
    };
    let open = state.transactions.read(cx).open_transactions(connection_id);
    if open.is_empty() {
        return Task::ready(true);
    }
    // Names are looked up when shown: the Connections model may be the one asking.
    let open = open
        .into_iter()
        .map(|o| OpenTxRow {
            connection: o.connection_id.clone(),
            connection_id: o.connection_id,
            database: o.database,
            state: o.state,
        })
        .collect();
    let task = ask(GuardKind::CloseTx { action, open }, cx);
    cx.foreground_executor()
        .spawn(async move { task.await == GuardAnswer::Confirm })
}

/// Leaving Manual mode with a transaction open: commit or roll it back
/// first (`chooseEndTx`); None: cancelled.
pub fn choose_end_tx(database: &str, state: TxState, cx: &mut App) -> Task<Option<EndTxChoice>> {
    let task = ask(
        GuardKind::EndTx {
            database: database.to_owned(),
            state,
        },
        cx,
    );
    cx.foreground_executor().spawn(async move {
        match task.await {
            GuardAnswer::Commit => Some(EndTxChoice::Commit),
            GuardAnswer::Rollback => Some(EndTxChoice::Rollback),
            GuardAnswer::Cancel | GuardAnswer::Confirm => None,
        }
    })
}

/// Installs the guards of the main window: the Connections model asks
/// before disconnecting / reconnecting with a transaction open, and closing
/// the window (it is the last one: the app quits) asks like a quit.
pub fn install(window: &mut Window, cx: &mut App) {
    if let Some(state) = AppState::try_global(cx).cloned() {
        state.connections.update(cx, |c, _| {
            c.set_close_guard(Some(std::rc::Rc::new(|action, id: &str, cx: &mut App| {
                confirm_close_tx(action, Some(id), cx)
            })));
        });
    }
    cx.set_global(GuardWindow(window.window_handle()));
    window.on_window_should_close(cx, |_, cx| crate::app::main_window_should_close(cx));
}
