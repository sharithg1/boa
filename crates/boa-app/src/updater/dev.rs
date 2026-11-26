//! \`BOA_DEV_UPDATE=1\` (feature \`dev-hooks\`): the update rehearsal driver.
//! At launch the app's [\`Updates\`] model checks its endpoint (point
//! \`BOA_UPDATE_ENDPOINT\` at a local server), and when a newer version is
//! offered downloads and verifies it, then runs "Restart to update" itself
//! ([\`Updates::restart_to_update\`]: the open-transaction guard, the install
//! over the running bundle, the relaunch, the quit): the same path as the
//! status bar pill, without the click. With nothing newer (or on any error)
//! it logs and quits. \`BOA_DEV_UPDATE_LOG=/abs/file\` appends one line per
//! step there (the relaunched instance inherits it through \`open --env\`).
//! See docs/RELEASING.md "Rehearsing an update locally".
//! (\`BOA_DEV_UPDATE=ready\` is views/dev.rs's pretend download instead.)
//!
//! [\`Updates\`]: crate::models::updates::Updates
//! [\`Updates::restart_to_update\`]: crate::models::updates::Updates::restart_to_update

use std::cell::Cell;
use std::io::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use boa_core::state::updates::UpdatePhase;
use gpui::App;

use crate::app_state::AppState;

fn log_line(msg: &str) {
    tracing::info!(target: "boa_app::updater", "{msg}");
    if let Some(path) = std::env::var_os("BOA_DEV_UPDATE_LOG") {
        let line = format!(
            "[pid {} v{}] {msg}\n",
            std::process::id(),
            super::current_version()
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// The model's install step, logging each part.
fn install_logged(verified: &super::VerifiedUpdate) -> Result<(), String> {
    log_line(&format!(
        "installing the verified {} ({} bytes)",
        verified.version(),
        verified.bytes().len()
    ));
    let fail = |e: String| {
        log_line(&format!("install failed: {e}"));
        e
    };
    let app = super::current_bundle().map_err(|e| fail(e.to_string()))?;
    super::install(verified, &app).map_err(|e| fail(e.to_string()))?;
    log_line(&format!(
        "installed {} at {}",
        verified.version(),
        app.display()
    ));
    super::schedule_relaunch(&app).map_err(|e| fail(format!("relaunch failed: {e}")))?;
    log_line("relaunch scheduled; quitting");
    Ok(())
}

/// Starts the rehearsal when \`BOA_DEV_UPDATE\` is set (call once at launch,
/// after the app state exists).
pub fn apply(cx: &mut App) {
    if std::env::var("BOA_DEV_UPDATE").map_or(true, |v| v.is_empty() || v == "ready") {
        return;
    }
    let Some(updates) = AppState::try_global(cx).map(|s| s.updates.clone()) else {
        log_line("no app state; quitting");
        cx.quit();
        return;
    };
    log_line(&format!(
        "running {} from {}; config dir {}; key {}; checking {}",
        super::current_version(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        crate::app_state::config_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        if super::pubkey_is_production() {
            "production"
        } else {
            "OVERRIDDEN (BOA_UPDATER_PUBKEY)"
        },
        updates.read(cx).endpoint().unwrap_or("nothing"),
    ));
    updates.update(cx, |u, _| u.set_installer(Some(Arc::new(install_logged))));
    let downloading = Rc::new(Cell::new(false));
    let restarted = Rc::new(Cell::new(false));
    cx.observe(&updates, move |updates, cx| {
        let u = updates.read(cx);
        let Some(status) = u.status().cloned() else {
            return;
        };
        let version = status.version.clone().unwrap_or_default();
        match status.phase {
            UpdatePhase::Checking => {}
            UpdatePhase::Downloading => {
                if !downloading.replace(true) {
                    log_line(&format!("update {version} available; downloading"));
                }
            }
            UpdatePhase::Ready if !restarted.get() => {
                restarted.set(true);
                log_line(&format!("update {version} ready; restarting to update"));
                updates.update(cx, |u, cx| u.restart_to_update(false, cx));
            }
            UpdatePhase::Ready => {
                if !u.restarting() {
                    // The install failed (logged by install_logged).
                    cx.quit();
                }
            }
            UpdatePhase::UpToDate => {
                log_line("up to date");
                cx.quit();
            }
            UpdatePhase::Error => {
                let message = status.message.unwrap_or_default();
                if downloading.get() {
                    log_line(&format!("download rejected: {message}"));
                } else {
                    log_line(&format!("check failed: {message}"));
                }
                cx.quit();
            }
        }
    })
    .detach();
    updates.update(cx, |u, cx| u.check(true, cx));
}
