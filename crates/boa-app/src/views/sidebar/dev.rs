//! Development hooks of the explorer (feature `dev-hooks`, implied by
//! `screenshot`), read once when the explorer first renders. Never in release
//! builds.
//!
//! - `BOA_DEV_EXPLORER=connect`: connect every saved connection through the
//!   backend and mark it connected in the Connections model (until that
//!   model connects by itself).
//! - `BOA_DEV_EXPAND=id;id;…`: expand these tree nodes (Sidebar node ids).
//! - `BOA_DEV_ACTIVE_TABLE=conn/db/schema/table`: select that database with
//!   that table as its active tab (for the right panel's Structure tab).
//! - `BOA_DEV_SCOPE_PICKER=key`: open the scope picker of that `scopes` key
//!   (a connection id, or `conn/db`) once its badge shows.
//! - `BOA_DEV_CONTEXT_MENU=node id`: right-click that row once it shows.
//! - `BOA_DEV_FOCUS=node id`: focus that row once it shows.

use std::cell::Cell;

use gpui::{
    Context, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, Window, point, px,
};

use super::Sidebar;
use super::tree::Meta;
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

thread_local! {
    static STARTED: Cell<bool> = const { Cell::new(false) };
    static PICKER_DONE: Cell<bool> = const { Cell::new(false) };
    static MENU_DONE: Cell<bool> = const { Cell::new(false) };
    static FOCUS_DONE: Cell<bool> = const { Cell::new(false) };
}

fn start(cx: &mut Context<Sidebar>) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    if let Some(ids) = env("BOA_DEV_EXPAND") {
        state.prefs.update(cx, |p, cx| {
            p.update(cx, |p| {
                for id in ids.split(';') {
                    p.set_expanded(id.trim(), true);
                }
            })
        });
    }
    if let Some(path) = env("BOA_DEV_ACTIVE_TABLE") {
        let parts: Vec<&str> = path.splitn(4, '/').collect();
        if let [conn, db, schema, table] = parts[..] {
            let key = DbKey::new(conn, db);
            state.workspaces.update(cx, |w, cx| {
                w.open_table(key.clone(), schema, table, true, cx);
                w.active = Some(key);
                cx.notify();
            });
        }
    }
    if env("BOA_DEV_EXPLORER").as_deref() == Some("connect") {
        let backend = state.backend.clone();
        let connections = state.connections.clone();
        cx.spawn(async move |_, cx| {
            let Ok(saved) = backend.list_connections().await else {
                return;
            };
            let ids: Vec<String> = saved.iter().map(|c| c.config.id.clone()).collect();
            connections.update(cx, |c, cx| {
                if c.saved.is_empty() {
                    c.saved = saved;
                }
                cx.notify();
            });
            for id in ids {
                let result = backend.connect(&id, None).await;
                connections.update(cx, |c, cx| {
                    let state = match result {
                        Ok(info) => ConnState::Connected {
                            server_version: info.server_version,
                            databases: info.databases,
                        },
                        Err(e) => ConnState::Failed { message: e.message },
                    };
                    c.state.insert(id.clone(), state);
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

/// Called at the start of every explorer render.
pub(super) fn on_render(this: &mut Sidebar, window: &mut Window, cx: &mut Context<Sidebar>) {
    if !STARTED.replace(true) {
        start(cx);
    }
    if !PICKER_DONE.get()
        && let Some(key) = env("BOA_DEV_SCOPE_PICKER")
    {
        let found = this
            .nodes
            .iter()
            .enumerate()
            .find_map(|(ix, n)| match &n.meta {
                Meta::Scope(b) if b.key == key => Some((ix, b.clone())),
                Meta::Database { badge: Some(b), .. } if b.key == key => Some((ix, b.clone())),
                _ => None,
            });
        if let Some((ix, badge)) = found {
            PICKER_DONE.set(true);
            let y = 40. + 32. + 6. + ix as f32 * 26. + 20.;
            let entity = cx.entity();
            window.defer(cx, move |window, cx| {
                entity.update(cx, |s, cx| {
                    s.open_picker(&badge, point(px(150.), px(y)), window, cx)
                });
            });
        }
    }
    if !FOCUS_DONE.get()
        && let Some(id) = env("BOA_DEV_FOCUS")
        && this.nodes.iter().any(|n| n.id == id)
    {
        FOCUS_DONE.set(true);
        this.focused = Some(id);
    }
    if !MENU_DONE.get()
        && let Some(id) = env("BOA_DEV_CONTEXT_MENU")
        && let Some(ix) = this.nodes.iter().position(|n| n.id == id)
    {
        MENU_DONE.set(true);
        let depth = this.nodes[ix].depth as f32;
        let position = point(
            px(6. + 6. + depth * 16. + 50.),
            px(40. + 32. + 6. + ix as f32 * 26. + 13.),
        );
        window.on_next_frame(move |window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Right,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Right,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        });
    }
}
