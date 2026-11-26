//! [`Exports`]: CSV exports (state/csvExport.ts; drawn by views::csv_export
//! like CsvExport.tsx): the delimiter / header options (persisted in
//! `csv-export.json`, the Tauri app's `boa.csvExport` JSON), the exports in
//! flight with their progress, cancel, and how each one ends.
//!
//! [`Exports::export`] (the Export menu, the explorer's "Export to CSV…"):
//! a read-only (pageable) source connects first, then the save panel asks
//! for a file (`cx.prompt_for_new_path`, the name from
//! `csv_file_name`), then [`Exports::export_to`] writes it:
//! - read-only statements and tables: `Backend::export_csv` re-runs the
//!   statement as a streamed `COPY … TO STDOUT` into a temporary file that is
//!   renamed into place (removed on failure or cancel). The backend runs it in
//!   its own `begin … commit`, or under a savepoint that is always rolled back
//!   when the session is inside a user transaction (`pg::export`). Shown as a
//!   job (status bar, result toolbar) with progress every 250 ms; small
//!   exports send none;
//! - statements that change data (`INSERT … RETURNING`) are never re-run:
//!   their loaded rows are written (`to_csv`, temp file + rename).
//!
//! Every export ends with an Output log line (also recorded in the query
//! history, as `logOutput` does) and a toast: "Exported N rows" with
//! **Reveal in Finder** (10 s), "Export cancelled" (57014), or "Export failed".
//!
//! Quitting while exports run cancels them and removes their hidden
//! temporary files (`.<name>.<pid>-<n>.part` next to the destination), so
//! no partial file stays behind ([`Exports::quit`], on `cx.on_app_quit`).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use boa_core::backend::CsvExport;
use boa_core::error::CmdError;
use boa_core::grid::csv::{CsvDelimiter, CsvOptions, to_csv};
use boa_core::model::{QueryResult, Value};
use boa_core::pg::{self, CancelHandle, ExportResult};
use boa_core::state::csv_export::{
    self as core, CANCEL_FAILED_TITLE, CANCELLED, DONE_TOAST_MS, ExportJob, ExportJobs,
    ExportRoute, ExportSource, FAILED_TITLE, REVEAL_LABEL, ToastText,
};
use boa_core::state::history::OutputEntry;
use gpui::{App, AppContext as _, ClipboardItem, Context, Subscription, Task};

use super::catalogs::DbKey;
use super::toasts::NewToast;
use super::workspaces::now_ms;
use crate::app_state::AppState;
use crate::grid::error::RunError;

/// CSV exports: options and the jobs in flight.
#[derive(Debug, Default)]
pub struct Exports {
    /// Where `csv-export.json` lives (None: not persisted, tests).
    config_dir: Option<PathBuf>,
    options: CsvOptions,
    jobs: ExportJobs,
    /// The full path of each running job.
    paths: HashMap<u64, PathBuf>,
    /// The streaming tasks of running jobs.
    tasks: HashMap<u64, Task<()>>,
    /// Cancels each running job (also while it waits for its session).
    cancels: HashMap<u64, CancelHandle>,
    /// The folder the save panel opened in last (session only).
    last_dir: Option<PathBuf>,
    /// The quit hook, registered with the first streamed export.
    on_quit: Option<Subscription>,
}

/// How long the quit hook waits for a cancelled export to stop writing
/// before it looks for its temporary file again (GPUI gives quit hooks
/// 200 ms in all).
const QUIT_SETTLE: Duration = Duration::from_millis(120);

impl Exports {
    /// Default options, not persisted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `<config_dir>/csv-export.json` (missing or corrupt: the defaults)
    /// and saves option changes back to it.
    pub fn load(config_dir: &Path) -> Self {
        Self {
            config_dir: Some(config_dir.to_owned()),
            options: core::load(config_dir),
            ..Self::default()
        }
    }

    // ── Options ───────────────────────────────────────────────────────────

    pub fn options(&self) -> CsvOptions {
        self.options
    }

    /// Sets the options and saves them (`setCsvOptions`; a write error keeps
    /// the in-memory value).
    pub fn set_options(&mut self, options: CsvOptions, cx: &mut Context<Self>) {
        if self.options == options {
            return;
        }
        self.options = options;
        if let Some(dir) = &self.config_dir
            && let Err(e) = core::save(dir, options)
        {
            tracing::warn!(error = %e, "saving {}", core::FILE);
        }
        cx.notify();
    }

    pub fn set_delimiter(&mut self, delimiter: CsvDelimiter, cx: &mut Context<Self>) {
        let options = CsvOptions {
            delimiter,
            ..self.options
        };
        self.set_options(options, cx);
    }

    pub fn set_header(&mut self, header: bool, cx: &mut Context<Self>) {
        let options = CsvOptions {
            header,
            ..self.options
        };
        self.set_options(options, cx);
    }

    // ── Jobs ──────────────────────────────────────────────────────────────

    /// The running exports, in start order (the status bar shows the first).
    pub fn jobs(&self) -> &[ExportJob] {
        self.jobs.all()
    }

    /// The running export started by `owner` (a console's tab id or a
    /// `tableKey`), shown as "Exporting…" in its toolbar.
    pub fn job_of(&self, owner: &str) -> Option<&ExportJob> {
        self.jobs.all().iter().find(|j| j.owner == owner)
    }

    /// The job list itself, for view tests.
    #[cfg(test)]
    pub(crate) fn jobs_mut(&mut self) -> &mut ExportJobs {
        &mut self.jobs
    }

    /// The full path a running job writes to.
    pub fn path_of(&self, id: u64) -> Option<&Path> {
        self.paths.get(&id).map(PathBuf::as_path)
    }

    /// The folder the save panel opens in: the last one used, else
    /// ~/Downloads, else the home folder.
    pub fn save_dir(&self) -> PathBuf {
        if let Some(dir) = self.last_dir.as_ref().filter(|d| d.is_dir()) {
            return dir.clone();
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        home.as_ref()
            .map(|h| h.join("Downloads"))
            .filter(|d| d.is_dir())
            .or(home)
            .unwrap_or_else(std::env::temp_dir)
    }

    /// `exportCsv`: connects first when the source is re-run, asks for a
    /// file (the save panel), then exports to it ([`Exports::export_to`]).
    /// `owner` ties the running export to a result tab or a table.
    pub fn export(&mut self, src: ExportSource, owner: impl Into<String>, cx: &mut Context<Self>) {
        let owner = owner.into();
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let connect = src.needs_connection().then(|| {
            state
                .connections
                .update(cx, |c, cx| c.ensure_connected(&src.connection_id, cx))
        });
        cx.spawn(async move |this, cx| {
            if let Some(connect) = connect {
                let _ = connect.await;
            }
            let Ok(Some(prompt)) = this.update(cx, |this, cx| this.ask_path(&src, cx)) else {
                return;
            };
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(e)) => {
                    state.toasts.update(cx, |t, cx| {
                        t.report_error(FAILED_TITLE, e.to_string(), cx);
                    });
                    return;
                }
            };
            this.update(cx, |this, cx| {
                this.last_dir = path.parent().map(Path::to_owned);
                this.export_to(src, owner, path, cx);
            })
            .ok();
        })
        .detach();
    }

    /// After connecting: "not connected" toast, or the save panel.
    fn ask_path(
        &mut self,
        src: &ExportSource,
        cx: &mut Context<Self>,
    ) -> Option<futures::channel::oneshot::Receiver<anyhow::Result<Option<PathBuf>>>> {
        let state = AppState::try_global(cx).cloned()?;
        if src.needs_connection() {
            let conns = state.connections.read(cx);
            if !conns.is_connected(&src.connection_id) {
                let name = conns.get(&src.connection_id).map(|c| c.config.name.clone());
                state.toasts.update(cx, |t, cx| {
                    t.report_error(FAILED_TITLE, core::not_connected(name.as_deref()), cx);
                });
                return None;
            }
        }
        let name = src.file_name(self.options);
        Some(cx.prompt_for_new_path(&self.save_dir(), Some(&name)))
    }

    /// Exports `src` to `path` without asking: streamed by the backend when
    /// it is read-only, else its loaded rows.
    pub fn export_to(
        &mut self,
        src: ExportSource,
        owner: impl Into<String>,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        match src.route() {
            ExportRoute::Stream => self.stream(src, owner.into(), path, cx),
            ExportRoute::LoadedRows => self.write_loaded(src, path, cx),
        }
    }

    fn stream(&mut self, src: ExportSource, owner: String, path: PathBuf, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let id = self.jobs.start(&owner, &src, &path.to_string_lossy());
        self.paths.insert(id, path.clone());
        if self.on_quit.is_none() {
            self.on_quit = Some(cx.on_app_quit(|this, cx| this.quit(cx)));
        }
        let at = now_ms();
        let CsvExport { result, progress } = state.backend.export_csv(
            &src.connection_id,
            &src.database,
            &src.sql,
            path.clone(),
            pg_options(self.options),
        );
        if let Some(handle) = result.cancel_handle() {
            self.cancels.insert(id, handle);
        }
        let task = cx.spawn(async move |this, cx| {
            // The channel closes when the export ends.
            while let Ok(p) = progress.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if this.jobs.progress(id, p.rows, p.bytes) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = result.await;
            this.update(cx, |this, cx| {
                this.finish_stream(id, src, path, at, result, cx)
            })
            .ok();
        });
        self.tasks.insert(id, task);
        cx.notify();
    }

    fn finish_stream(
        &mut self,
        id: u64,
        src: ExportSource,
        path: PathBuf,
        at: f64,
        result: Result<ExportResult, CmdError>,
        cx: &mut Context<Self>,
    ) {
        self.jobs.finish(id);
        self.paths.remove(&id);
        self.cancels.remove(&id);
        if let Some(task) = self.tasks.remove(&id) {
            // We are inside it: let it end on its own.
            task.detach();
        }
        cx.notify();
        let shown = path.to_string_lossy().into_owned();
        match result {
            Ok(res) => {
                let mut entry = base_entry(&src, at);
                entry.ms = res.ms as f64;
                entry.note = Some(core::streamed_note(res.rows, res.bytes, res.ms, &shown));
                log(&src.connection_id, entry, cx);
                done(path, core::streamed_toast(res.rows, res.bytes, &shown), cx);
            }
            Err(e) => failed(&src, at, &e, cx),
        }
    }

    fn write_loaded(&mut self, src: ExportSource, path: PathBuf, cx: &mut Context<Self>) {
        let rows = src.rows.clone().unwrap_or_default();
        let columns = src.columns.clone().unwrap_or_default();
        let csv = to_csv(&columns, &rows, self.options);
        let at = now_ms();
        let dest = path.clone();
        // Not on the backend's runtime: no foreign wake-ups (deterministic tests).
        let write =
            cx.background_spawn(async move { pg::write_file_atomic(&dest, csv.as_bytes()) });
        cx.spawn(async move |_, cx| {
            let result = write.await;
            cx.update(|cx| {
                let shown = path.to_string_lossy().into_owned();
                match result {
                    Ok(()) => {
                        let n = rows.len() as u64;
                        let mut entry = base_entry(&src, at);
                        entry.ms = (now_ms() - at).max(0.);
                        entry.note = Some(core::loaded_rows_note(n, &shown));
                        log(&src.connection_id, entry, cx);
                        done(path, core::loaded_rows_toast(n, src.truncated, &shown), cx);
                    }
                    Err(e) => failed(&src, at, &e, cx),
                }
            });
        })
        .detach();
    }

    /// Quitting: cancels every running export and removes its temporary
    /// file now and once more after [`QUIT_SETTLE`] (a backend task that had
    /// not created it yet may still do so). The destination is never touched:
    /// an export that finished first was renamed into place.
    pub fn quit(&mut self, cx: &mut Context<Self>) -> impl Future<Output = ()> + use<> {
        let ids: Vec<u64> = self.jobs.all().iter().map(|j| j.id).collect();
        for &id in &ids {
            self.cancel(id, cx);
        }
        let paths: Vec<PathBuf> = ids
            .iter()
            .filter_map(|id| self.paths.get(id).cloned())
            .collect();
        remove_part_files(&paths);
        let timer = (!paths.is_empty()).then(|| cx.background_executor().timer(QUIT_SETTLE));
        async move {
            if let Some(timer) = timer {
                timer.await;
                remove_part_files(&paths);
            }
        }
    }

    /// `cancelExport`: cancels the job's export (a server-side cancel while it
    /// runs; while it waits for its session behind another statement, it just
    /// gives up); it then fails with 57014 ("Export cancelled").
    pub fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(job) = self.jobs.get(id) else {
            return;
        };
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let cancel = match self.cancels.get(&id) {
            Some(handle) => state.backend.cancel(handle),
            None => state
                .backend
                .cancel_query(&job.connection_id, &job.database),
        };
        cx.spawn(async move |_, cx| {
            if let Err(e) = cancel.await {
                state.toasts.update(cx, |t, cx| {
                    t.report_error(CANCEL_FAILED_TITLE, e.message, cx);
                });
            }
        })
        .detach();
    }
}

/// The temporary files this process's exports to `dest` write
/// (`boa_core::pg`'s `.<name>.<pid>-<nanos>.part` next to it).
pub fn part_files(dest: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name()) else {
        return Vec::new();
    };
    let prefix = format!(".{}.{}-", name.to_string_lossy(), std::process::id());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with(&prefix)
                && n.ends_with(".part")
                && n[prefix.len()..n.len() - ".part".len()]
                    .bytes()
                    .all(|b| b.is_ascii_digit())
        })
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

fn remove_part_files(dests: &[PathBuf]) {
    for dest in dests {
        for part in part_files(dest) {
            match std::fs::remove_file(&part) {
                Ok(()) => tracing::info!("removed {} on quit", part.display()),
                Err(e) => tracing::warn!(error = %e, "removing {}", part.display()),
            }
        }
    }
}

/// The backend's COPY options.
pub fn pg_options(options: CsvOptions) -> pg::CsvOptions {
    pg::CsvOptions {
        delimiter: options.delimiter.as_char(),
        header: options.header,
    }
}

fn base_entry(src: &ExportSource, at: f64) -> OutputEntry {
    OutputEntry {
        sql: src.sql.clone(),
        database: src.database.clone(),
        at,
        source: Some(src.label.clone()),
        ..OutputEntry::default()
    }
}

/// `logOutput`: an Output log line, recorded in the history too.
fn log(connection_id: &str, entry: OutputEntry, cx: &mut App) {
    if let Some(state) = AppState::try_global(cx).cloned() {
        state
            .workspaces
            .update(cx, |w, cx| w.log(connection_id, entry, None, true, cx));
    }
}

/// The success toast, with **Reveal in Finder**.
fn done(path: PathBuf, text: ToastText, cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let backend = state.backend.clone();
    let toast = NewToast::success(text.title)
        .body(text.body)
        .action(REVEAL_LABEL, move |_, _| {
            // Fire and forget (errors ignored, as the Tauri app does).
            drop(backend.reveal_path(path.clone()));
        });
    state.toasts.update(cx, |t, cx| {
        t.push_for(toast, Some(Duration::from_millis(DONE_TOAST_MS)), cx);
    });
}

/// A failed or cancelled export: its Output log line and toast.
fn failed(src: &ExportSource, at: f64, e: &CmdError, cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let mut entry = base_entry(src, at);
    entry.ms = (now_ms() - at).max(0.);
    let toast = if core::is_cancelled(e.code.as_deref()) {
        entry.note = Some(CANCELLED.to_owned());
        NewToast::info(CANCELLED)
    } else {
        let err = RunError::from_cmd(e, &src.sql);
        entry.error = Some(core::failed_note(&err.message));
        entry.detail = err.detail;
        NewToast::error(FAILED_TITLE).body(err.message)
    };
    log(&src.connection_id, entry, cx);
    state.toasts.update(cx, |t, cx| {
        t.push(toast, cx);
    });
}

// ── Sources ───────────────────────────────────────────────────────────────

/// A table tab's source (Workspace.tsx): the whole table with the tab's
/// WHERE / ORDER BY (`sql`), plus the loaded page.
pub fn table_tab_source(
    key: &DbKey,
    schema: &str,
    table: &str,
    sql: &str,
    page: Option<&QueryResult>,
) -> ExportSource {
    let mut src = core::table_source(&key.connection_id, &key.database, schema, table);
    if !sql.trim().is_empty() {
        src.sql = sql.to_owned();
    }
    match page {
        Some(page) => with_rows(src, page),
        None => src,
    }
}

/// Adds the loaded page's columns and rows.
pub fn with_rows(mut src: ExportSource, page: &QueryResult) -> ExportSource {
    src.columns = Some(page.columns.iter().map(|c| c.name.clone()).collect());
    src.rows = Some(page.rows.clone());
    src
}

/// `exportTable`: the explorer's "Export to CSV…" on a table or view; runs
/// as the table's `tableKey`, like its table tab.
pub fn export_table(key: &DbKey, schema: &str, table: &str, cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let src = core::table_source(&key.connection_id, &key.database, schema, table);
    let owner = core::table_owner(&key.connection_id, &key.database, schema, table);
    state.exports.update(cx, |e, cx| e.export(src, owner, cx));
}

/// `copyCsv`: the loaded rows to the clipboard as CSV, with the export options.
pub fn copy_csv(columns: &[String], rows: &[Vec<Value>], cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let options = state.exports.read(cx).options();
    cx.write_to_clipboard(ClipboardItem::new_string(to_csv(columns, rows, options)));
    state.toasts.update(cx, |t, cx| {
        t.push(NewToast::info(core::copied_title(rows.len() as u64)), cx);
    });
}

#[cfg(test)]
mod tests;
