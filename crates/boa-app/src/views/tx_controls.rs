//! TxControls.tsx: the transaction controls of a console's toolbar, after
//! DataGrip's "Tx: Auto / Manual" switch: the session's mode (a menu), its
//! state as the server reported it, and Commit / Rollback while in Manual
//! mode or while a transaction is open (also after a typed BEGIN in Auto).
//! The mode belongs to the session, so every console on the database shows
//! the same. Also the status bar's indicator ([`tx_status`]) and the
//! ⌘⇧↵ / ⌘⇧⌫ shortcuts ([`shortcut`], GuardDialogs.tsx `useTxShortcuts`).

use boa_core::state::guards::{EndTxChoice, tx_shortcut_applies};
use gpui::prelude::*;
use gpui::{App, Div, Entity, Task, div, px};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};

use super::chrome::{IconName, icon, tooltip};
use super::guard_dialogs::choose_end_tx;
use super::workspace::{tool_button, tool_menu_trigger};
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::console::Console;
use crate::models::transactions::{TxMode, TxState};
use crate::theme::{self, ActivePalette as _};

/// The commit shortcut, as the tooltips show it.
pub const COMMIT_LABEL: &str = "Commit (⌘⇧↵)";
/// The rollback shortcut, as the tooltips show it.
pub const ROLLBACK_LABEL: &str = "Rollback (⌘⇧⌫)";

/// "Auto" / "Manual".
pub fn mode_label(mode: TxMode) -> &'static str {
    match mode {
        TxMode::Auto => "Auto",
        TxMode::Manual => "Manual",
    }
}

/// The database's active tab, when it is a console.
pub fn active_console(key: &DbKey, cx: &App) -> Option<Entity<Console>> {
    let state = AppState::try_global(cx)?;
    let ws = state.workspaces.read(cx);
    ws.workspace(key)?.active()?.console().cloned()
}

/// Ends the session's transaction (`endTransaction`): `COMMIT` / `ROLLBACK`
/// run as a statement of the database's active console. Resolves true when
/// the session is idle afterwards.
pub fn end_transaction(key: &DbKey, commit: bool, cx: &mut App) -> Task<bool> {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return Task::ready(false);
    };
    let console = active_console(key, cx);
    state
        .transactions
        .update(cx, |t, cx| t.end(key, commit, console, cx))
}

/// Switches the session's mode (`switchTxMode`); leaving Manual with a
/// transaction open asks to end it first ("Switch to auto-commit?").
pub fn switch_mode(key: DbKey, mode: TxMode, cx: &mut App) -> Task<()> {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return Task::ready(());
    };
    let current = state.transactions.read(cx).get(&key);
    if current.mode == mode {
        return Task::ready(());
    }
    let ask = (mode == TxMode::Auto && current.state != TxState::Idle)
        .then(|| choose_end_tx(&key.database, current.state, cx));
    cx.spawn(async move |cx| {
        if let Some(ask) = ask {
            let Some(choice) = ask.await else {
                return;
            };
            let commit = choice == EndTxChoice::Commit;
            cx.update(|cx| end_transaction(&key, commit, cx)).await;
            let idle = cx.update(|cx| state.transactions.read(cx).state(&key) == TxState::Idle);
            if !idle {
                return;
            }
        }
        let set = cx.update(|cx| {
            state
                .transactions
                .update(cx, |t, cx| t.set_mode(&key, mode, cx))
        });
        if let Err(e) = set.await {
            cx.update(|cx| {
                state.toasts.update(cx, |t, cx| {
                    t.report_error("Could not change the transaction mode", e.message, cx);
                })
            });
        }
    })
}

/// ⌘⇧↵ (commit) / ⌘⇧⌫ (rollback) in the main window: ends the selected
/// database's transaction when one is open (an aborted one can only be
/// rolled back). False when it doesn't apply (the key goes on).
pub fn shortcut(commit: bool, cx: &mut App) -> bool {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return false;
    };
    let Some(key) = state.workspaces.read(cx).active.clone() else {
        return false;
    };
    let tx = state.transactions.read(cx).state(&key);
    if !tx_shortcut_applies(tx, commit) {
        return false;
    }
    end_transaction(&key, commit, cx).detach();
    true
}

/// TxBadge: "Transaction open" while one is open; "Transaction aborted"
/// (25P02) until it is rolled back; nothing when idle.
pub fn tx_badge(state: TxState, cx: &App) -> Option<gpui::Stateful<Div>> {
    if state == TxState::Idle {
        return None;
    }
    let p = cx.palette();
    let failed = state == TxState::Failed;
    let (color, text) = if failed {
        (p.destructive, p.status_err_text)
    } else {
        (p.status_wait, p.status_wait)
    };
    Some(
        div()
            .id("tx-badge")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(px(20.))
            .px(px(8.))
            .rounded_full()
            .bg(color.opacity(0.15))
            .text_color(text)
            .text_size(px(11.))
            .tooltip(tooltip(if failed {
                "The transaction failed: every statement is refused (SQLSTATE 25P02) until it is rolled back"
            } else {
                "A transaction is open: commit or roll it back"
            }))
            .child(if failed {
                icon(IconName::CircleAlert, 12., text).into_any_element()
            } else {
                div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(p.status_wait)
                    .into_any_element()
            })
            .child(if failed {
                "Transaction aborted"
            } else {
                "Transaction open"
            }),
    )
}

/// The console toolbar's controls for session `key`: the mode menu,
/// Commit / Rollback (Manual, or a transaction open), the state badge.
/// `running`: the console runs a statement.
pub fn render(key: &DbKey, connected: bool, running: bool, cx: &App) -> Vec<gpui::AnyElement> {
    let Some(state) = AppState::try_global(cx) else {
        return Vec::new();
    };
    let p = cx.palette();
    let transactions = state.transactions.read(cx);
    let session = transactions.get(key);
    let busy = running || transactions.is_busy(key);
    let (mode, tx) = (session.mode, session.state);
    let menu_key = key.clone();
    let mut out = vec![
        tool_menu_trigger(
            "console-tx",
            format!("Tx: {}", mode_label(mode)),
            false,
            connected,
            cx,
        )
        .tooltip("Transaction mode")
        .dropdown_menu(move |menu, _, _| {
            let item = |label: &'static str, m: TxMode| {
                let key = menu_key.clone();
                PopupMenuItem::new(label)
                    .checked(mode == m)
                    .on_click(move |_, _, cx| switch_mode(key.clone(), m, cx).detach())
            };
            menu.min_w(px(176.))
                .item(item("Auto-commit", TxMode::Auto))
                .item(item("Manual", TxMode::Manual))
        })
        .into_any_element(),
    ];
    if mode == TxMode::Manual || tx != TxState::Idle {
        let can_commit = connected && !busy && tx == TxState::Active;
        let can_rollback = connected && !busy && tx != TxState::Idle;
        let (commit_key, rollback_key) = (key.clone(), key.clone());
        out.push(
            tool_button(
                "console-commit",
                IconName::Check,
                COMMIT_LABEL,
                can_commit,
                None,
                cx,
            )
            .on_click(move |_, _, cx| {
                if can_commit {
                    end_transaction(&commit_key, true, cx).detach();
                }
            })
            .into_any_element(),
        );
        out.push(
            tool_button(
                "console-rollback",
                IconName::Undo2,
                ROLLBACK_LABEL,
                can_rollback,
                (tx == TxState::Failed).then_some(p.status_err_text),
                cx,
            )
            .on_click(move |_, _, cx| {
                if can_rollback {
                    end_transaction(&rollback_key, false, cx).detach();
                }
            })
            .into_any_element(),
        );
    }
    out.extend(tx_badge(tx, cx).map(|b| b.ml(px(4.)).into_any_element()));
    out
}

/// TxStatus: the selected database's transaction in the status bar, unless
/// it is in Auto mode and idle.
pub fn tx_status(cx: &App) -> Option<Div> {
    let state = AppState::try_global(cx)?;
    let key = state.workspaces.read(cx).active.clone()?;
    let session = state.transactions.read(cx).get(&key);
    if session.state == TxState::Idle && session.mode == TxMode::Auto {
        return None;
    }
    let p = cx.palette();
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .text_size(theme::text::XS)
            .text_color(p.dim)
            .child(format!("Tx: {}", mode_label(session.mode)))
            .children(tx_badge(session.state, cx)),
    )
}

/// Dev hooks (feature `dev-hooks`, never in release builds), for screenshots:
/// `BOA_DEV_TX=open|menu|confirm|endtx|quit` with `BOA_DEV_LIVE=1` (the
/// workspace's scripted session). Once its console is up: `open` switches
/// to Manual and inserts into `boa_txg.shot` (created if missing), so a
/// transaction is open; `menu` then clicks the "Tx:" button at
/// `BOA_DEV_TX_CLICK=x,y` (window points); `endtx` switches back to Auto
/// (the prompt); `quit` asks to quit; `confirm` runs a DELETE without WHERE.
#[cfg(feature = "dev-hooks")]
pub mod dev {
    use std::time::Duration;

    use gpui::{
        App, AsyncApp, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, point,
        px,
    };

    use super::{TxMode, TxState, switch_mode};
    use crate::app_state::AppState;
    use crate::models::catalogs::DbKey;
    use crate::models::console::Console;

    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }

    /// Starts the scripted transaction, if `BOA_DEV_TX` is set.
    pub fn apply(cx: &mut App) {
        let Some(mode) = env("BOA_DEV_TX") else {
            return;
        };
        cx.spawn(async move |cx| {
            if let Err(err) = run(&mode, cx).await {
                tracing::error!("BOA_DEV_TX: {err}");
            }
        })
        .detach();
    }

    async fn pause(cx: &mut AsyncApp, ms: u64) {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
    }

    /// The selected database's console, once its connection is up.
    async fn console(cx: &mut AsyncApp) -> Result<(DbKey, gpui::Entity<Console>), String> {
        for _ in 0..200 {
            let found = cx.update(|cx| {
                let state = AppState::global(cx);
                let key = state.workspaces.read(cx).active.clone()?;
                if !state.connections.read(cx).is_connected(&key.connection_id) {
                    return None;
                }
                let console = state.workspaces.read(cx).active_tab()?.console()?.clone();
                Some((key, console))
            });
            if let Some(found) = found {
                return Ok(found);
            }
            pause(cx, 100).await;
        }
        Err("no connected console (BOA_DEV_LIVE=1?)".into())
    }

    async fn run_sql(cx: &mut AsyncApp, console: &gpui::Entity<Console>, sql: &str) {
        let sql = sql.to_owned();
        console.update(cx, |c, cx| c.run(sql, cx));
        for _ in 0..100 {
            pause(cx, 50).await;
            if console.read_with(cx, |c, _| c.running.is_none()) {
                break;
            }
        }
    }

    async fn run(mode: &str, cx: &mut AsyncApp) -> Result<(), String> {
        let (key, console) = console(cx).await?;
        let backend = cx.update(|cx| AppState::global(cx).backend.clone());
        backend
            .run_query(
                &key.connection_id,
                &key.database,
                "create schema if not exists boa_txg; \
                 create table if not exists boa_txg.shot (id int)",
                10,
                0,
            )
            .await
            .map_err(|e| e.message)?;
        if mode == "confirm" {
            run_sql(cx, &console, "delete from boa_txg.shot").await;
            return Ok(());
        }
        cx.update(|cx| switch_mode(key.clone(), TxMode::Manual, cx))
            .await;
        run_sql(cx, &console, "insert into boa_txg.shot values (1)").await;
        let state = cx.update(|cx| AppState::global(cx).transactions.read(cx).state(&key));
        if state != TxState::Active {
            return Err(format!("the insert left the session {state:?}"));
        }
        match mode {
            "endtx" => {
                cx.update(|cx| switch_mode(key, TxMode::Auto, cx)).detach();
            }
            "quit" => cx.update(crate::app::request_quit),
            "menu" => {
                let at = env("BOA_DEV_TX_CLICK")
                    .and_then(|v| {
                        let (x, y) = v.split_once(',')?;
                        Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
                    })
                    .ok_or("BOA_DEV_TX_CLICK=x,y")?;
                pause(cx, 300).await;
                let window = cx
                    .update(|cx| crate::views::guard_dialogs::guard_window(cx))
                    .ok_or("no main window")?;
                let position = point(px(at.0), px(at.1));
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_event(
                            PlatformInput::MouseDown(MouseDownEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                                first_mouse: false,
                            }),
                            cx,
                        );
                        window.dispatch_event(
                            PlatformInput::MouseUp(MouseUpEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers::default(),
                                click_count: 1,
                            }),
                            cx,
                        );
                    })
                    .map_err(|e| e.to_string())?;
            }
            _ => {}
        }
        Ok(())
    }
}
