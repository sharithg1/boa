//! [`AppState`]: the app-wide models, as a GPUI global, so any view reaches
//! them: `AppState::global(cx).prefs.clone()`, then `cx.observe(&prefs, …)`.
//!
//! The config dir is the Tauri app's (`~/Library/Application Support/dev.boa.desktop/`)
//! unless `BOA_CONFIG_DIR=<dir>` is set: development runs, screenshots and
//! tests must use it so they never touch the user's data.

use std::path::{Path, PathBuf};

use boa_core::backend::Backend;
use gpui::{App, AppContext as _, Entity, Global};

use crate::models::catalogs::Catalogs;
use crate::models::connections::{Connections, ConnectionsEvent};
use crate::models::exports::Exports;
use crate::models::history::History;
use crate::models::layout::Layout;
use crate::models::prefs::Prefs;
use crate::models::session::Session;
use crate::models::toasts::Toasts;
use crate::models::transactions::Transactions;
use crate::models::updates::Updates;
use crate::models::workspaces::Workspaces;

/// Overrides the config dir for the whole app (dev runs, screenshots, tests).
pub const CONFIG_DIR_ENV: &str = "BOA_CONFIG_DIR";

/// The config dir to use: `$BOA_CONFIG_DIR` when set and not empty, else the
/// Tauri app's ([`boa_core::config::config_dir`]). None without a home dir.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os(CONFIG_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(boa_core::config::config_dir)
}

/// The app-wide models.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The backend, on [`AppState::config_dir`].
    pub backend: Backend,
    pub config_dir: PathBuf,
    pub prefs: Entity<Prefs>,
    pub layout: Entity<Layout>,
    pub toasts: Entity<Toasts>,
    pub connections: Entity<Connections>,
    pub catalogs: Entity<Catalogs>,
    pub workspaces: Entity<Workspaces>,
    pub transactions: Entity<Transactions>,
    pub history: Entity<History>,
    pub exports: Entity<Exports>,
    pub session: Entity<Session>,
    pub updates: Entity<Updates>,
}

impl Global for AppState {}

impl AppState {
    /// Starts the backend on `config_dir`, loads prefs.json and the saved
    /// connections, creates the models, installs the global and restores
    /// the last session (session.json; `Session::reconnect` reopens its
    /// connections).
    /// `window_width` picks the right panel's default (open from 1100 px).
    pub fn init(config_dir: &Path, window_width: f32, cx: &mut App) -> anyhow::Result<Self> {
        let backend = Backend::with_config_dir(config_dir)
            .map_err(|e| anyhow::anyhow!("starting the backend: {}", e.message))?;
        let prefs = cx.new(|cx| Prefs::load(config_dir, cx));
        let toasts = cx.new(|_| Toasts::new());
        let connections = cx.new(|cx| {
            Connections::with_backend(
                backend.clone(),
                Some(toasts.clone()),
                Some(prefs.clone()),
                cx,
            )
        });
        let state = Self {
            backend: backend.clone(),
            config_dir: config_dir.to_owned(),
            prefs,
            layout: cx.new(|_| Layout::new(window_width)),
            toasts,
            connections,
            catalogs: cx.new(|_| Catalogs::new()),
            workspaces: cx.new(|_| Workspaces::new()),
            transactions: cx.new(|_| Transactions::new()),
            history: cx.new(|cx| History::with_dir(config_dir, cx)),
            exports: cx.new(|_| Exports::load(config_dir)),
            session: cx.new(|_| Session::new(config_dir)),
            updates: cx.new(|_| Updates::new()),
        };
        cx.set_global(state.clone());
        state.catalogs.update(cx, |c, cx| c.watch(cx));
        state.history.update(cx, |h, cx| h.watch(cx));
        let connections = state.connections.clone();
        state
            .transactions
            .update(cx, |t, cx| t.watch(&connections, cx));
        // A deleted connection takes its tabs with it (store.ts
        // `deleteConnection`; Catalogs drops its catalogs by itself).
        let workspaces = state.workspaces.clone();
        cx.subscribe(&state.connections, move |_, event, cx| {
            if let ConnectionsEvent::Deleted { id } = event {
                workspaces.update(cx, |w, cx| w.forget_connection(id, cx));
            }
        })
        .detach();
        // The last session (tabs, SQL, filters, panels), before any window.
        state.session.update(cx, |s, cx| s.start(cx));
        Ok(state)
    }

    /// The global. Panics if [`AppState::init`] has not run.
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// The global, if [`AppState::init`] has run.
    pub fn try_global(cx: &App) -> Option<&Self> {
        cx.try_global::<Self>()
    }
}
