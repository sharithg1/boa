//! The views, one module per React component of the Tauri app. They own no
//! app state (that's [`crate::models`], reached through
//! [`crate::app_state::AppState`]); they observe the models and render.
//!
//! - [`root`]: the main window (App.tsx): layout, panel resizing, the
//!   global shortcuts.
//! - [`title_bar`] (TitleBar.tsx: the database chip and the tabs),
//!   [`sidebar`] (Sidebar.tsx), [`workspace`] (Workspace.tsx: the console
//!   or table tab), [`right_panel`] (RightPanel.tsx), [`bottom_panel`]
//!   (BottomPanel.tsx: result tabs and the Output log), [`status_bar`]
//!   (StatusBar.tsx), [`panel_resize`] (PanelResizeHandle.tsx), [`toasts`]
//!   (Toasts.tsx), [`settings`] (SettingsView.tsx), [`command_palette`]
//!   (CommandPalette.tsx), [`about`]
//!   (AboutWindow.tsx), [`chrome`] (chrome.tsx, IconButton.tsx,
//!   StatusDot.tsx).
//! - Console pieces: [`statement_chooser`] (StatementChooser.tsx),
//!   [`completion_popup`] (CompletionPopup.tsx), [`run_progress`]
//!   (RunProgress.tsx), [`pager`] (Pager.tsx), [`tx_controls`] (TxControls.tsx).
//! - [`guard_dialogs`] (GuardDialogs.tsx): the guard rails' confirmations.

pub mod about;
pub mod bottom_panel;
pub mod checkbox;
pub mod chrome;
pub mod command_palette;
pub mod completion_popup;
pub mod conn_tag;
pub mod connection_dialog;
pub mod csv_export;
#[cfg(feature = "dev-hooks")]
pub mod dev;
#[cfg(feature = "dev-hooks")]
pub mod dev_input;
pub mod guard_dialogs;
pub mod json_viewer;
pub mod pager;
pub mod panel_resize;
pub mod password_prompt;
pub mod query_history;
pub mod right_panel;
pub mod root;
pub mod run_progress;
pub mod scope_picker;
pub mod settings;
pub mod sidebar;
pub mod statement_chooser;
pub mod status_bar;
pub mod table_edits;
pub mod table_filter_bar;
pub mod title_bar;
pub mod toasts;
pub mod tx_controls;
pub mod value_editor;
pub mod workspace;

use boa_core::connections::{Environment, SavedConnection};
use boa_core::grid::conn_tag::Environment as EnvTag;
use gpui::{App, Div, Hsla, SharedString};

use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;
use crate::models::toasts::NewToast;

/// Toast key of the "not yet" notices, so they replace each other.
pub const NOT_YET_TOAST: &str = "not-yet";

/// Says that `what` arrives in a later phase (stub action handlers).
pub fn not_yet(what: impl Into<SharedString>, cx: &mut App) {
    let what = what.into();
    tracing::info!("not yet implemented: {what}");
    let Some(state) = AppState::try_global(cx) else {
        return;
    };
    let toasts = state.toasts.clone();
    toasts.update(cx, |toasts, cx| {
        toasts.push(
            NewToast::info(format!("{what} isn't available yet")).key(NOT_YET_TOAST),
            cx,
        );
    });
}

/// The selected database (`Workspaces::active`) and its connection, as the
/// chrome shows them.
#[derive(Debug, Clone)]
pub struct ActiveDb {
    pub connection_id: SharedString,
    /// The connection's name.
    pub connection: SharedString,
    pub database: SharedString,
    pub status: ConnState,
    pub saved: Option<SavedConnection>,
}

impl ActiveDb {
    /// The selected database, if any.
    pub fn get(cx: &App) -> Option<Self> {
        let state = AppState::try_global(cx)?;
        let key = state.workspaces.read(cx).active.clone()?;
        Some(Self::of(&key, cx))
    }

    /// `key` as the chrome shows it.
    pub fn of(key: &DbKey, cx: &App) -> Self {
        let conns = AppState::try_global(cx).map(|s| s.connections.read(cx));
        let saved = conns.and_then(|c| c.get(&key.connection_id).cloned());
        Self {
            connection_id: key.connection_id.clone().into(),
            connection: saved
                .as_ref()
                .map_or_else(|| key.connection_id.clone(), |s| s.config.name.clone())
                .into(),
            database: key.database.clone().into(),
            status: conns
                .map(|c| c.state_of(&key.connection_id))
                .unwrap_or_default(),
            saved,
        }
    }

    pub fn key(&self) -> DbKey {
        DbKey {
            connection_id: self.connection_id.to_string(),
            database: self.database.to_string(),
        }
    }

    pub fn dot(&self) -> chrome::DotKind {
        match self.status {
            ConnState::Connecting | ConnState::Reconnecting { .. } => chrome::DotKind::Wait,
            ConnState::Connected { .. } => chrome::DotKind::Ok,
            ConnState::Failed { .. } => chrome::DotKind::Err,
            ConnState::Disconnected => chrome::DotKind::Disc,
        }
    }

    /// StatusDot `DOT_LABEL`.
    pub fn dot_label(&self) -> &'static str {
        match self.status {
            ConnState::Connecting => "Connecting",
            ConnState::Reconnecting { .. } => "Reconnecting",
            ConnState::Connected { .. } => "Connected",
            ConnState::Failed { .. } => "Failed",
            ConnState::Disconnected => "Disconnected",
        }
    }

    pub fn server_version(&self) -> Option<&str> {
        match &self.status {
            ConnState::Connected { server_version, .. } => Some(server_version),
            _ => None,
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.status, ConnState::Connected { .. })
    }

    /// The connection's colour (connTag.ts `connectionColor`), if it has one.
    pub fn color(&self) -> Option<Hsla> {
        connection_tint(self.saved.as_ref())
    }
}

/// connTag.ts's view of a saved connection's environment tag.
pub fn env_tag(environment: Option<Environment>) -> Option<EnvTag> {
    Some(match environment? {
        Environment::Development => EnvTag::Development,
        Environment::Test => EnvTag::Test,
        Environment::Staging => EnvTag::Staging,
        Environment::Production => EnvTag::Production,
    })
}

/// A saved connection's colour (its own, else its environment's).
pub fn connection_tint(saved: Option<&SavedConnection>) -> Option<Hsla> {
    conn_tag::connection_color(&saved?.config)
}

/// ConnectionTag.tsx: the environment badge (`PROD`) in the connection
/// colour, plus a lock when read-only; None when it has neither.
pub fn connection_tag(saved: Option<&SavedConnection>, cx: &App) -> Option<Div> {
    conn_tag::connection_tag(&saved?.config, cx)
}
