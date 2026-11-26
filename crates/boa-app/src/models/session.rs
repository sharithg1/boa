//! [`Session`]: `<config dir>/session.json` (state/session.ts, `boa.session.v1`),
//! the one writer of that file.
//!
//! At launch ([`Session::start`], from `AppState::init`, before the main
//! window opens) it reads the last session, pruned to the saved connections
//! (`boa_core::state::session::parse_session`), and restores it: each
//! database's tabs (consoles with their SQL, table tabs with their filter /
//! sort, preview flags), the active tab, the selected database, MRU / seen
//! and the panel flags. Results are not kept: no statement is re-run (a
//! table tab reads its first page once its connection is back, as any table
//! tab does). [`Session::reconnect`] then reopens the connections that were
//! open at quit (`connections_to_restore`: the selected one first, which may
//! ask for a password; the others quietly), retrying while the server can't
//! be reached (`RETRY_DELAYS_MS`).
//!
//! From then on any change of [`Workspaces`], [`Layout`] or [`Connections`]
//! (the `wanted` set), and every edit of a console's text
//! ([`Session::touch`]), saves the session [`SAVE_DELAY`] after the last
//! change (session.ts `SAVE_DELAY_MS`; at most [`MAX_SAVE_DELAY`] while
//! changes keep coming), only when the JSON differs from what was last
//! written, and once more on quit. The file is exactly what the Tauri app
//! wrote to localStorage (`JSON.stringify(pick(state))`): the same fields in
//! the same order; like the Tauri app it keeps no other keys.
//!
//! [`import_local_storage`] is the one-time import of the Tauri app's
//! WebView localStorage (prefs, session, CSV options) that runs before any
//! of these files is read.
//!
//! [`Workspaces`]: super::workspaces::Workspaces
//! [`Layout`]: super::layout::Layout
//! [`Connections`]: super::connections::Connections

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use boa_core::state::local_storage::{self, ImportReport};
use boa_core::state::session::{self as core, connections_to_restore};
use gpui::{App, Context, Subscription, Task};

use super::catalogs::DbKey;
use super::layout::SavedPanels;
use crate::app_state::{self, AppState};

/// File name of the session in the config dir.
pub const SESSION_FILE: &str = core::FILE;

/// How long after the last change session.json is written (session.ts `SAVE_DELAY_MS`).
pub const SAVE_DELAY: Duration = Duration::from_millis(core::SAVE_DELAY_MS);

/// A save waits at most about this long while changes keep coming (a
/// streaming result notifying all the time must not starve it).
pub const MAX_SAVE_DELAY: Duration = Duration::from_secs(2);

/// Where the one-time localStorage import looks for WebKit's data when the
/// config dir is overridden (`BOA_CONFIG_DIR`): a home dir holding
/// `Library/WebKit/dev.boa.desktop/…` (migration checks on a copy). Unset:
/// no import, so dev runs and screenshots never pull in the user's data.
pub const IMPORT_HOME_ENV: &str = "BOA_IMPORT_HOME";

/// The session file's owner.
pub struct Session {
    /// Where session.json lives; None: not persisted.
    dir: Option<PathBuf>,
    started: bool,
    /// What was restored, until [`Session::reconnect`] used it.
    restored: Option<core::Session>,
    /// The panel flags of the file at launch.
    saved_panels: SavedPanels,
    /// Save `saved_panels` instead of the layout (a scripted layout).
    keep_saved_panels: bool,
    /// The JSON last read or written: an unchanged session isn't rewritten.
    last_written: Option<String>,
    save: Option<Task<()>>,
    pending_since: Option<Instant>,
    reconnect: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("dir", &self.dir)
            .field("started", &self.started)
            .field("save_pending", &self.save.is_some())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A session saved in `<dir>/session.json` once [`Session::start`]ed.
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: Some(dir.to_owned()),
            started: false,
            restored: None,
            saved_panels: SavedPanels::default(),
            keep_saved_panels: false,
            last_written: None,
            save: None,
            pending_since: None,
            reconnect: None,
            _subscriptions: Vec::new(),
        }
    }

    /// `<dir>/session.json`.
    pub fn path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(SESSION_FILE))
    }

    /// Restores the last session into the models (needs [`AppState`], with
    /// the saved connections loaded) and starts saving changes. Once.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        self.started = true;
        if let Some(dir) = self.dir.clone() {
            let raw = local_storage::read_config_file(&dir, SESSION_FILE);
            let ids: Vec<String> = state
                .connections
                .read(cx)
                .saved
                .iter()
                .map(|c| c.config.id.clone())
                .collect();
            if let Some(session) =
                core::parse_session(raw.as_deref(), ids.iter().map(String::as_str))
            {
                self.saved_panels = SavedPanels {
                    sidebar_open: session.sidebar_open,
                    right_panel_open: session.right_panel_open,
                    bottom_open: session.bottom_open,
                };
                let panels = self.saved_panels;
                state
                    .layout
                    .update(cx, |l, cx| l.restore_panels(panels, cx));
                state.workspaces.update(cx, |w, cx| w.restore(&session, cx));
                let tabs: usize = session.workspaces.values().map(|w| w.tabs.len()).sum();
                tracing::info!(
                    databases = session.workspaces.len(),
                    tabs,
                    mru = session.mru.len(),
                    reconnect = connections_to_restore(&session).len(),
                    "session restored"
                );
                self.restored = Some(session);
            }
            self.last_written = raw;
        }
        self._subscriptions = vec![
            cx.observe(&state.workspaces, |this, _, cx| this.schedule_save(cx)),
            cx.observe(&state.layout, |this, _, cx| this.schedule_save(cx)),
            cx.observe(&state.connections, |this, _, cx| this.schedule_save(cx)),
        ];
        cx.on_app_quit(|this, cx| {
            this.flush(cx);
            async {}
        })
        .detach();
    }

    /// The session that was restored at launch, if any (until reconnected).
    pub fn restored(&self) -> Option<&core::Session> {
        self.restored.as_ref()
    }

    /// Reopens the connections that were open at the last quit (session.ts
    /// `startSession`): the selected database's connection first (it may
    /// ask for a password; once back, its database is loaded), then the
    /// others quietly; each retried while unreachable. Once.
    pub fn reconnect(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.restored.take() else {
            return;
        };
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let ids = connections_to_restore(&session);
        if ids.is_empty() {
            return;
        }
        let active = session.active.clone();
        let selected = active.as_ref().map(|a| a.connection_id.as_str());
        let task = state
            .connections
            .update(cx, |c, cx| c.restore(ids, selected, cx));
        if let (Some(task), Some(active)) = (task, active) {
            let catalogs = state.catalogs.clone();
            self.reconnect = Some(cx.spawn(async move |_, cx| {
                if task.await {
                    let key = DbKey::new(active.connection_id, active.database);
                    catalogs.update(cx, |c, cx| c.load(key, false, cx));
                }
            }));
        }
    }

    /// A console's text changed (the editor doesn't notify the models on
    /// every keystroke): save soon.
    pub fn touch(&mut self, cx: &mut Context<Self>) {
        self.schedule_save(cx);
    }

    /// Keeps the panel flags as they were at launch in what is saved (a
    /// scripted layout of a dev run is not the user's).
    pub fn keep_saved_panels(&mut self) {
        self.keep_saved_panels = true;
    }

    /// Whether a save is waiting for its delay.
    pub fn save_pending(&self) -> bool {
        self.save.is_some()
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.dir.is_none() || !self.started {
            return;
        }
        let now = cx.background_executor().now();
        if self.save.is_some()
            && self
                .pending_since
                .is_some_and(|since| now.saturating_duration_since(since) >= MAX_SAVE_DELAY)
        {
            // Changes keep coming: let the pending save run.
            return;
        }
        if self.save.is_none() {
            self.pending_since = Some(now);
        }
        self.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update(cx, |this, cx| this.flush(cx)).ok();
        }));
    }

    /// Writes a pending save now (session.ts `flushSession`; also on quit).
    pub fn flush(&mut self, cx: &App) {
        if self.save.take().is_none() {
            return;
        }
        self.pending_since = None;
        self.write(cx);
    }

    /// The session as it would be saved now.
    pub fn snapshot(&self, cx: &App) -> core::Session {
        let Some(state) = AppState::try_global(cx) else {
            return core::Session::default();
        };
        let mut s = state.workspaces.read(cx).to_session(cx);
        s.wanted = Some(
            state
                .connections
                .read(cx)
                .wanted_ids()
                .into_iter()
                .collect(),
        );
        let layout = state.layout.read(cx).panels();
        let saved = self.saved_panels;
        let pick = |kept: Option<bool>, now: Option<bool>| {
            if self.keep_saved_panels {
                kept.or(now)
            } else {
                now
            }
        };
        s.sidebar_open = pick(saved.sidebar_open, layout.sidebar_open);
        s.right_panel_open = pick(saved.right_panel_open, layout.right_panel_open);
        s.bottom_open = pick(saved.bottom_open, layout.bottom_open);
        s
    }

    /// Writes session.json now if it changed; true when written.
    pub fn write(&mut self, cx: &App) -> bool {
        let Some(dir) = self.dir.clone() else {
            return false;
        };
        let json = self.snapshot(cx).to_json();
        if self.last_written.as_deref() == Some(json.as_str()) {
            return false;
        }
        match local_storage::write_config_file(&dir, SESSION_FILE, &json) {
            Ok(()) => {
                self.last_written = Some(json);
                true
            }
            Err(err) => {
                tracing::warn!(dir = %dir.display(), error = %err, "saving session.json");
                false
            }
        }
    }
}

// ── The one-time localStorage import ──────────────────────────────────────

/// Where the startup import looks for the Tauri app's WebKit data: `$HOME`
/// with the real config dir; with `BOA_CONFIG_DIR` set only
/// `$BOA_IMPORT_HOME` (else no import).
pub fn import_home() -> Option<PathBuf> {
    let nonempty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if nonempty(app_state::CONFIG_DIR_ENV).is_some() {
        return nonempty(IMPORT_HOME_ENV).map(PathBuf::from);
    }
    nonempty("HOME").map(PathBuf::from)
}

/// Imports the Tauri app's localStorage (`boa.prefs.v2`, `boa.session.v1`,
/// `boa.csvExport`) into `config_dir` once
/// (`boa_core::state::local_storage::import_once`; marker
/// `.localstorage-imported`). Call before the models read the files. Never
/// fails or panics; logs what it did (file names and counts only).
pub fn import_local_storage(config_dir: &Path, home: &Path) -> Option<ImportReport> {
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        local_storage::import_once(config_dir, home)
    }));
    match run {
        Ok(Some(report)) => {
            let skipped: Vec<String> = report
                .skipped
                .iter()
                .map(|(file, why)| format!("{file}: {why}"))
                .collect();
            tracing::info!(
                imported = ?report.imported,
                ?skipped,
                found = report.source.is_some(),
                "localStorage import"
            );
            Some(report)
        }
        Ok(None) => None,
        Err(_) => {
            tracing::error!("localStorage import panicked; skipped");
            None
        }
    }
}

#[cfg(test)]
mod tests;
