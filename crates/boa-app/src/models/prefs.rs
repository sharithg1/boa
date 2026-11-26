//! [`Prefs`]: the app's preferences as an entity. Owns a
//! [`boa_core::state::prefs::Prefs`] (state/prefs.ts: defaults, clamps,
//! `applyOrder`, reset), saves it to `<config dir>/prefs.json` shortly after
//! each change ([`SAVE_DELAY`]; at once on quit), and emits
//! [`PrefsChanged`] (the TS `onPrefsChange` listeners) after every change.
//!
//! Read with `prefs.read(cx).get()`; change with [`Prefs::update`] (the TS
//! `setPrefs(patch)`) or [`Prefs::reset`] (`resetPrefs()`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use boa_core::state::prefs as core;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Task};

pub use core::{RightTab, Scope};

/// How long after the last change prefs.json is written.
pub const SAVE_DELAY: Duration = Duration::from_millis(250);

/// Emitted after every [`Prefs::update`] / [`Prefs::reset`], with the values
/// before the change (the new ones are in the entity).
#[derive(Debug, Clone)]
pub struct PrefsChanged {
    pub prev: core::Prefs,
}

/// The preferences entity.
#[derive(Debug)]
pub struct Prefs {
    prefs: core::Prefs,
    /// Where prefs.json lives; None keeps them in memory only.
    dir: Option<PathBuf>,
    save: Option<Task<()>>,
}

impl EventEmitter<PrefsChanged> for Prefs {}

impl Prefs {
    /// Loads `<dir>/prefs.json` (defaults when missing or corrupt) and saves
    /// changes back there. Also writes a pending change when the app quits.
    pub fn load(dir: &Path, cx: &mut Context<Self>) -> Self {
        Self::watch_quit(cx);
        Self {
            prefs: core::load(dir),
            dir: Some(dir.to_owned()),
            save: None,
        }
    }

    /// Prefs that are never written (tests, previews).
    pub fn in_memory(prefs: core::Prefs) -> Self {
        Self {
            prefs,
            dir: None,
            save: None,
        }
    }

    /// Creates the entity from `dir` (see [`Prefs::load`]).
    pub fn new_entity(dir: &Path, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::load(dir, cx))
    }

    fn watch_quit(cx: &mut Context<Self>) {
        cx.on_app_quit(|this, _| {
            this.flush();
            async {}
        })
        .detach();
    }

    /// The current values.
    pub fn get(&self) -> &core::Prefs {
        &self.prefs
    }

    /// Where prefs.json is written, if anywhere.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// `setPrefs`: changes the prefs, emits [`PrefsChanged`], schedules a save.
    /// Nothing happens when `f` leaves them as they were.
    pub fn update(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut core::Prefs)) {
        let prev = self.prefs.clone();
        f(&mut self.prefs);
        if self.prefs == prev {
            return;
        }
        self.changed(prev, cx);
    }

    /// `resetPrefs`: the defaults, keeping the layout (sizes, explorer state).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        let prev = self.prefs.clone();
        self.prefs = prev.reset();
        if self.prefs != prev {
            self.changed(prev, cx);
        }
    }

    fn changed(&mut self, prev: core::Prefs, cx: &mut Context<Self>) {
        cx.emit(PrefsChanged { prev });
        cx.notify();
        if self.dir.is_none() {
            return;
        }
        self.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update(cx, |this, _| this.flush()).ok();
        }));
    }

    /// Whether a change is waiting to be written.
    pub fn save_pending(&self) -> bool {
        self.save.is_some()
    }

    /// Writes a pending change now (quitting). Errors are logged: the
    /// in-memory values stay in use, as the TS did when storage failed.
    pub fn flush(&mut self) {
        if self.save.take().is_none() {
            return;
        }
        let Some(dir) = &self.dir else { return };
        if let Err(err) = self.prefs.save(dir) {
            tracing::warn!(dir = %dir.display(), error = %err, "saving prefs.json");
        }
    }
}
