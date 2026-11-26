//! [\`Updates\`]: update checks, the download and "Restart to update"
//! (src/updater.rs, lib/updates.ts, GuardDialogs.tsx \`restartToUpdate\`) on
//! boa-core's [\`Updater\`] state machine and the I/O in [\`crate::updater\`].
//!
//! - **Check** ([\`Updates::check\`]): GET \`latest.json\` and pick this Mac's
//!   download ([\`updater::check\`]); a newer release is then **downloaded**
//!   and **verified** against the embedded public key ([\`updater::download\`],
//!   progress in [\`Updates::download_progress\`]). Only a verified download
//!   becomes "ready": the About window and the status bar pill offer
//!   "Restart to update".
//! - **Restart** ([\`Updates::restart_to_update\`]): writes the session, asks
//!   about open transactions in the main window like a quit
//!   ([\`guard_dialogs::confirm_close_tx\`] with [\`CloseAction::Update\`]),
//!   rolls them back, swaps the \`.app\` bundle ([\`updater::install\`]),
//!   starts the relaunch helper ([\`updater::schedule_relaunch\`]) and quits.
//!   A failed install shows "Could not install the update" and can be
//!   retried (the download is kept).
//!
//! **Where it looks** ([\`endpoint_for\`]): \`BOA_UPDATE_ENDPOINT\` if set, else
//! the release feed ([\`UPDATE_ENDPOINT\`]) in release builds. Debug builds
//! without \`BOA_UPDATE_ENDPOINT\` never touch the network: a check reports
//! [\`NOT_AVAILABLE\`]. Periodic checks ([\`Updates::start_periodic_checks\`])
//! run in release builds only, unless \`BOA_NO_UPDATE_CHECK\` is set.
//!
//! [\`Updater\`]: boa_core::state::updates::Updater
//! [\`guard_dialogs::confirm_close_tx\`]: crate::views::guard_dialogs::confirm_close_tx

use std::sync::Arc;
use std::time::Duration;

use boa_core::state::guards::CloseAction;
use boa_core::state::updates::{
    self as core, AvailableUpdate, CheckStart, INSTALL_FAILED_TITLE, UpdateEvent, UpdatePhase,
    Updater,
};
use futures::StreamExt as _;
use gpui::{AppContext as _, Context, Task};

use crate::app_state::AppState;
use crate::updater::{self, UPDATE_ENDPOINT, VerifiedUpdate};

pub use crate::updater::ENDPOINT_ENV;

/// What a check reports in a debug build without [\`ENDPOINT_ENV\`].
pub const NOT_AVAILABLE: &str =
    "update checks are off in development builds (set BOA_UPDATE_ENDPOINT)";

/// Installs a verified update over the running bundle and starts the
/// relaunch helper (blocking; runs on a background thread). The app quits
/// once it returns \`Ok\`. Default: [\`install_and_schedule_relaunch\`].
pub type Installer = Arc<dyn Fn(&VerifiedUpdate) -> Result<(), String> + Send + Sync>;

/// A pretend download (tests, \`BOA_DEV_UPDATE=ready\`): "ready" without
/// anything to install.
#[cfg(any(test, feature = "dev-hooks"))]
pub type Downloader =
    std::rc::Rc<dyn Fn(&AvailableUpdate, &mut gpui::App) -> Task<Result<(), String>>>;

/// The feed to check: \`env\` (\`BOA_UPDATE_ENDPOINT\`) when set and not blank;
/// else the release feed in release builds and nothing in debug builds.
pub fn endpoint_for(debug_build: bool, env: Option<String>) -> Option<String> {
    env.map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .or_else(|| (!debug_build).then(|| UPDATE_ENDPOINT.to_owned()))
}

/// This build's feed ([\`endpoint_for\`] with this process's environment).
pub fn default_endpoint() -> Option<String> {
    endpoint_for(cfg!(debug_assertions), std::env::var(ENDPOINT_ENV).ok())
}

/// The real install: [\`updater::install\`] over [\`updater::current_bundle\`],
/// then [\`updater::schedule_relaunch\`]. Errors read like the Tauri app's
/// (\`install failed: …\`).
pub fn install_and_schedule_relaunch(update: &VerifiedUpdate) -> Result<(), String> {
    let app = updater::current_bundle().map_err(|e| format!("install failed: {e}"))?;
    updater::install(update, &app).map_err(|e| format!("install failed: {e}"))?;
    updater::schedule_relaunch(&app)
        .map_err(|e| format!("the update is installed, but Boa could not relaunch: {e}"))
}

/// The updater entity.
pub struct Updates {
    updater: Updater,
    version: String,
    endpoint: Option<String>,
    arch: String,
    pubkey: String,
    /// Bytes received and the expected total, while downloading.
    progress: Option<(u64, Option<u64>)>,
    /// The verified download "Restart to update" installs.
    verified: Option<Arc<VerifiedUpdate>>,
    installer: Installer,
    /// "Restart to update" is asking / installing.
    restarting: bool,
    /// ... and it came from the status bar pill (it says "Restarting…").
    pill_restarting: bool,
    #[cfg(any(test, feature = "dev-hooks"))]
    downloader: Option<Downloader>,
    check: Option<Task<()>>,
    restart: Option<Task<()>>,
    periodic: Option<Task<()>>,
}

impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("status", &self.status())
            .field("endpoint", &self.endpoint)
            .field("progress", &self.progress)
            .field("restarting", &self.restarting)
            .finish_non_exhaustive()
    }
}

impl Default for Updates {
    fn default() -> Self {
        Self::new()
    }
}

impl Updates {
    /// This build's updater ([\`default_endpoint\`], the embedded key).
    pub fn new() -> Self {
        Self::with(crate::app::VERSION, default_endpoint())
    }

    /// An updater for \`version\` checking \`endpoint\` (tests).
    pub fn with(version: &str, endpoint: Option<String>) -> Self {
        Self {
            updater: Updater::new(version),
            version: version.to_owned(),
            endpoint,
            arch: updater::current_arch().to_owned(),
            pubkey: updater::embedded_pubkey().to_owned(),
            progress: None,
            verified: None,
            installer: Arc::new(install_and_schedule_relaunch),
            restarting: false,
            pill_restarting: false,
            #[cfg(any(test, feature = "dev-hooks"))]
            downloader: None,
            check: None,
            restart: None,
            periodic: None,
        }
    }

    /// The last update event (\`update_status\`), if a check ran.
    pub fn status(&self) -> Option<&UpdateEvent> {
        self.updater.last()
    }

    /// The downloaded version waiting for "Restart to update".
    pub fn ready_version(&self) -> Option<&str> {
        self.updater.ready()
    }

    /// Bytes received and the expected total (when the server sent one)
    /// while an update downloads.
    pub fn download_progress(&self) -> Option<(u64, Option<u64>)> {
        self.progress
    }

    /// "Restart to update" is under way (asking, installing or quitting).
    pub fn restarting(&self) -> bool {
        self.restarting
    }

    /// The pill's own "Restarting…" (StatusBar.tsx keeps it per pill).
    pub fn pill_restarting(&self) -> bool {
        self.pill_restarting
    }

    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    pub fn set_endpoint(&mut self, endpoint: Option<String>) {
        self.endpoint = endpoint;
    }

    /// The architecture whose download is picked (default: this Mac's).
    pub fn set_arch(&mut self, arch: impl Into<String>) {
        self.arch = arch.into();
    }

    /// The minisign public key downloads must be signed with (default:
    /// [\`updater::embedded_pubkey\`]; tests use a throwaway key).
    pub fn set_pubkey(&mut self, pubkey: impl Into<String>) {
        self.pubkey = pubkey.into();
    }

    /// Replaces the install step (tests, the dev rehearsal's logging).
    pub fn set_installer(&mut self, installer: Option<Installer>) {
        self.installer = installer.unwrap_or_else(|| Arc::new(install_and_schedule_relaunch));
    }

    /// Plugs in a pretend download (tests, \`BOA_DEV_UPDATE=ready\`).
    #[cfg(any(test, feature = "dev-hooks"))]
    pub fn set_downloader(&mut self, downloader: Option<Downloader>) {
        self.downloader = downloader;
    }

    /// Checks for a newer release and downloads it (\`check_and_download\`;
    /// \`manual\`: from the menu or the About window). A check or download
    /// already running is left to finish; an update already downloaded is
    /// reported again.
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        match self.updater.begin_check(manual) {
            CheckStart::Busy => return,
            CheckStart::Ready(_) => {
                cx.notify();
                return;
            }
            CheckStart::Check(_) => cx.notify(),
        }
        let Some(endpoint) = self.endpoint.clone() else {
            self.updater.checked(Err(NOT_AVAILABLE.into()));
            cx.notify();
            return;
        };
        let (version, arch) = (self.version.clone(), self.arch.clone());
        let fetch = cx.background_spawn(async move { updater::check(&endpoint, &version, &arch) });
        self.check = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| this.checked(result, cx)).ok();
        }));
    }

    /// The check's outcome: up to date, an error, or a download to start.
    fn checked(
        &mut self,
        result: Result<Option<AvailableUpdate>, updater::Error>,
        cx: &mut Context<Self>,
    ) {
        self.check = None;
        let update = match result {
            Ok(Some(update)) => update,
            Ok(None) => {
                self.updater.checked(Ok(None));
                cx.notify();
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "update check failed");
                self.updater.checked(Err(e.to_string()));
                cx.notify();
                return;
            }
        };
        tracing::info!(version = %update.version, "update available; downloading");
        self.updater.checked(Ok(Some(&update)));
        self.progress = Some((0, None));
        cx.notify();
        let version = update.version.clone();
        #[cfg(any(test, feature = "dev-hooks"))]
        if let Some(download) = self.downloader.clone() {
            let download = download(&update, cx);
            self.check = Some(cx.spawn(async move |this, cx| {
                let result = download.await;
                this.update(cx, |this, cx| {
                    this.downloaded(&version, result.map(|()| None), cx)
                })
                .ok();
            }));
            return;
        }
        let (progress_tx, mut progress_rx) = futures::channel::mpsc::unbounded();
        let pubkey = self.pubkey.clone();
        let download = cx.background_spawn(async move {
            updater::download(&update, &pubkey, |received, total| {
                let _ = progress_tx.unbounded_send((received, total));
            })
        });
        self.check = Some(cx.spawn(async move |this, cx| {
            while let Some(progress) = progress_rx.next().await {
                let alive = this.update(cx, |this, cx| this.set_progress(progress, cx));
                if alive.is_err() {
                    return;
                }
            }
            let result = download
                .await
                .map(|verified| Some(Arc::new(verified)))
                .map_err(|e| e.to_string());
            this.update(cx, |this, cx| this.downloaded(&version, result, cx))
                .ok();
        }));
    }

    fn set_progress(&mut self, progress: (u64, Option<u64>), cx: &mut Context<Self>) {
        let percent = |p: Option<(u64, Option<u64>)>| {
            p.and_then(|(got, total)| total.filter(|t| *t > 0).map(|t| got * 100 / t))
        };
        let changed = percent(self.progress) != percent(Some(progress));
        self.progress = Some(progress);
        if changed {
            cx.notify();
        }
    }

    /// The download's outcome: verified (\`Some\`; \`None\`: a pretend
    /// download), or why not (a bad signature, a network error…).
    fn downloaded(
        &mut self,
        version: &str,
        result: Result<Option<Arc<VerifiedUpdate>>, String>,
        cx: &mut Context<Self>,
    ) {
        self.check = None;
        self.progress = None;
        match result {
            Ok(verified) => {
                tracing::info!(%version, "update downloaded and verified");
                self.verified = verified;
                self.updater.downloaded(version, Ok(()));
            }
            Err(message) => {
                tracing::warn!(%version, error = %message, "update download failed");
                self.updater.downloaded(version, Err(message));
            }
        }
        cx.notify();
    }

    /// Release builds: a first check after [\`core::FIRST_CHECK_MS\`], then
    /// every [\`core::CHECK_EVERY_MS\`] (\`spawn_periodic_checks\`). Nothing in
    /// debug builds or with \`BOA_NO_UPDATE_CHECK\` set.
    pub fn start_periodic_checks(&mut self, cx: &mut Context<Self>) {
        let enabled = core::periodic_checks_enabled(
            cfg!(debug_assertions),
            std::env::var_os(core::NO_UPDATE_CHECK_ENV).is_some(),
        );
        if !enabled || self.endpoint.is_none() || self.periodic.is_some() {
            return;
        }
        self.periodic = Some(cx.spawn(async move |this, cx| {
            let mut wait = Duration::from_millis(core::FIRST_CHECK_MS);
            loop {
                cx.background_executor().timer(wait).await;
                if this.update(cx, |this, cx| this.check(false, cx)).is_err() {
                    return;
                }
                wait = Duration::from_millis(core::CHECK_EVERY_MS);
            }
        }));
    }

    /// "Restart to update" (the status bar pill, \`from_pill\`, or the About
    /// window): writes the session, asks about open transactions like a quit
    /// ("Restart to update with an open transaction?") and rolls them back,
    /// installs the download, starts the relaunch helper and quits.
    /// Cancelled: nothing happens. A failed install shows an error toast
    /// and keeps the download for another try.
    pub fn restart_to_update(&mut self, from_pill: bool, cx: &mut Context<Self>) {
        if self.restarting || self.ready_version().is_none() {
            return;
        }
        self.restarting = true;
        self.pill_restarting = from_pill;
        cx.notify();
        let state = AppState::try_global(cx).cloned();
        if let Some(state) = &state {
            state.session.update(cx, |s, cx| s.flush(cx));
        }
        let ask = crate::views::guard_dialogs::confirm_close_tx(CloseAction::Update, None, cx);
        self.restart = Some(cx.spawn(async move |this, cx| {
            if !ask.await {
                this.update(cx, |this, cx| this.restart_done(None, cx)).ok();
                return;
            }
            let rollback = cx.update(crate::app::roll_back_all);
            rollback.await;
            let Ok(job) = this.update(cx, |this, _| {
                (
                    this.verified.clone(),
                    this.installer.clone(),
                    this.ready_version().map(str::to_owned),
                )
            }) else {
                return;
            };
            let (verified, installer, version) = job;
            let result = match verified {
                Some(verified) => {
                    tracing::info!(version = verified.version(), "installing update");
                    cx.background_spawn(async move { installer(&verified) })
                        .await
                }
                None => Err("no update has been downloaded".to_owned()),
            };
            match result {
                Ok(()) => {
                    tracing::info!(?version, "update installed; quitting to relaunch");
                    cx.update(crate::app::quit_now);
                }
                Err(message) => {
                    tracing::warn!(error = %message, "update install failed");
                    this.update(cx, |this, cx| this.restart_done(Some(message), cx))
                        .ok();
                }
            }
        }));
    }

    /// Cancelled (\`None\`) or failed: "Restart to update" works again.
    fn restart_done(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.restarting = false;
        self.pill_restarting = false;
        self.restart = None;
        if let Some(message) = error
            && let Some(state) = AppState::try_global(cx)
        {
            let toasts = state.toasts.clone();
            toasts.update(cx, |t, cx| {
                t.report_error(INSTALL_FAILED_TITLE, message, cx);
            });
        }
        cx.notify();
    }
}

/// The About window / pill phase of an event.
pub fn is_ready(status: Option<&UpdateEvent>) -> bool {
    status.is_some_and(|s| s.phase == UpdatePhase::Ready)
}

#[cfg(test)]
mod tests;
