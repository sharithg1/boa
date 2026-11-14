//! Database client desktop app, Tauri Rust side.
//!
//! Postgres only for now: saved connections (`connections`), sessions,
//! introspection and query execution (`pg`), exposed to the React frontend
//! in `ui/` as Tauri commands (see `ui/src/lib/backend.ts`).

mod connections;
mod edits;
mod error;
mod pg;

use connections::{ConnConfig, SavedConnection};
use error::{CmdError, CmdResult};
use pg::{ConnectInfo, CsvOptions, ExportResult, Pg, QueryResult, Schema};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

/// Version info for the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    }
}

fn config_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    app.path()
        .app_config_dir()
        .map_err(|e| CmdError::msg(e.to_string()))
}

#[tauri::command]
fn list_connections(app: AppHandle) -> CmdResult<Vec<SavedConnection>> {
    connections::list(&config_dir(&app)?)
}

/// Saves a connection. `password`: `Some` stores it (when `save_password`),
/// `None` keeps the stored one; `save_password: false` forgets it.
#[tauri::command]
fn save_connection(
    app: AppHandle,
    config: ConnConfig,
    password: Option<String>,
    save_password: bool,
) -> CmdResult<()> {
    connections::upsert(&config_dir(&app)?, config, password, save_password)
}

#[tauri::command]
async fn delete_connection(app: AppHandle, pg: State<'_, Pg>, id: String) -> CmdResult<()> {
    pg.disconnect(&id).await;
    connections::remove(&config_dir(&app)?, &id)
}

/// Connects with unsaved settings; returns the server version.
#[tauri::command]
async fn test_connection(
    app: AppHandle,
    config: ConnConfig,
    password: Option<String>,
) -> CmdResult<String> {
    let password = match password {
        Some(p) => Some(p),
        None => connections::password(&config_dir(&app)?, &config.id)?,
    };
    Pg::test(&config, password.as_deref()).await
}

/// Connects a saved connection, with `password` or else the stored one.
#[tauri::command]
async fn connect(
    app: AppHandle,
    pg: State<'_, Pg>,
    id: String,
    password: Option<String>,
) -> CmdResult<ConnectInfo> {
    let (config, stored) = connections::get(&config_dir(&app)?, &id)?
        .ok_or_else(|| CmdError::msg("Unknown connection"))?;
    let password = password.or(stored);
    pg.connect(config, password).await
}

#[tauri::command]
async fn disconnect(pg: State<'_, Pg>, id: String) -> CmdResult<()> {
    pg.disconnect(&id).await;
    Ok(())
}

#[tauri::command]
async fn list_schemas(pg: State<'_, Pg>, id: String, database: String) -> CmdResult<Vec<String>> {
    pg.list_schemas(&id, &database).await
}

/// Tables and columns of `schemas` (the ones picked in the explorer).
#[tauri::command]
async fn introspect(
    pg: State<'_, Pg>,
    id: String,
    database: String,
    schemas: Vec<String>,
) -> CmdResult<Vec<Schema>> {
    pg.introspect(&id, &database, &schemas).await
}

#[tauri::command]
async fn run_query(
    pg: State<'_, Pg>,
    id: String,
    database: String,
    sql: String,
    limit: usize,
    offset: Option<usize>,
) -> CmdResult<QueryResult> {
    pg.query(
        &id,
        &database,
        &sql,
        limit.clamp(1, 100_000),
        offset.unwrap_or(0),
    )
    .await
}

#[tauri::command]
async fn cancel_query(pg: State<'_, Pg>, id: String, database: String) -> CmdResult<()> {
    pg.cancel(&id, &database).await
}

/// Streams the whole result of a read-only statement to `path` as CSV,
/// emitting `csv-export-progress` (`{ exportId, rows, bytes }`) while it
/// runs. Cancelled with `cancel_query` on the same database.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn export_csv(
    app: AppHandle,
    pg: State<'_, Pg>,
    id: String,
    database: String,
    sql: String,
    path: PathBuf,
    options: CsvOptions,
    export_id: String,
) -> CmdResult<ExportResult> {
    pg.export_csv(&id, &database, &sql, &path, options, |p| {
        let _ = app.emit(
            "csv-export-progress",
            serde_json::json!({ "exportId": export_id, "rows": p.rows, "bytes": p.bytes }),
        );
    })
    .await
}

/// Writes CSV built by the UI (rows of a result that must not be re-run).
#[tauri::command]
fn write_csv_file(path: PathBuf, contents: String) -> CmdResult<()> {
    pg::write_file_atomic(&path, contents.as_bytes())
}

/// Shows the file in Finder (Explorer, or the folder elsewhere).
#[tauri::command]
fn reveal_path(path: PathBuf) -> CmdResult<()> {
    let mut cmd;
    if cfg!(target_os = "macos") {
        cmd = std::process::Command::new("open");
        cmd.arg("-R").arg(&path);
    } else if cfg!(target_os = "windows") {
        cmd = std::process::Command::new("explorer");
        cmd.arg(format!("/select,{}", path.display()));
    } else {
        cmd = std::process::Command::new("xdg-open");
        cmd.arg(path.parent().unwrap_or(&path));
    }
    cmd.spawn()?;
    Ok(())
}

/// Runs the data editor's generated statements in one transaction (or a
/// savepoint inside the user's); reports the failing one and rolls back.
#[tauri::command]
async fn apply_edits(
    pg: State<'_, Pg>,
    id: String,
    database: String,
    statements: Vec<edits::EditStatement>,
) -> CmdResult<edits::ApplyOutcome> {
    pg.apply_edits(&id, &database, &statements).await
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("DBCLIENT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

/// Run the app (blocks until exit).
///
/// # Panics
/// If Tauri fails to start.
pub fn run() {
    init_logging();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Pg::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_connections,
            save_connection,
            delete_connection,
            test_connection,
            connect,
            disconnect,
            list_schemas,
            introspect,
            run_query,
            cancel_query,
            export_csv,
            write_csv_file,
            reveal_path,
            apply_edits
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
