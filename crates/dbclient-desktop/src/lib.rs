//! Database client desktop app, Tauri Rust side.
//!
//! Postgres only for now: saved connections (`connections`), sessions,
//! introspection and query execution (`pg`), exposed to the React frontend
//! in `ui/` as Tauri commands (see `ui/src/lib/backend.ts`).

mod connections;
mod error;
mod pg;
mod tx;

use connections::{ConnConfig, SavedConnection};
use error::{CmdError, CmdResult};
use pg::{ConnectInfo, Pg, QueryResult, Schema};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use tx::TxState;

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

/// Auto (`manual: false`) or Manual transaction mode for one session;
/// returns its transaction state.
#[tauri::command]
async fn set_tx_mode(
    pg: State<'_, Pg>,
    id: String,
    database: String,
    manual: bool,
) -> CmdResult<TxState> {
    pg.set_tx_mode(&id, &database, manual).await
}

/// Quits once the UI has dealt with open transactions (see `run`).
#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// When the UI was last asked to confirm a quit (ms since the epoch).
static QUIT_ASKED: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
            set_tx_mode,
            quit
        ])
        .build(tauri::generate_context!())
        .expect("error while running the app")
        .run(|app, event| {
            // ⌘Q / the Quit menu: hold the exit and let the UI warn about
            // open transactions; it calls `quit` (an exit with a code, not
            // held). Quitting again within 3 s goes through, so a stuck UI
            // can't keep the app open. Without the window (closed): exit.
            if let tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } = &event
            {
                let Some(w) = app.get_webview_window("main") else {
                    return;
                };
                let now = now_ms();
                if now.saturating_sub(QUIT_ASKED.swap(now, Ordering::Relaxed)) > 3000 {
                    api.prevent_exit();
                    let _ = w.emit("quit-requested", ());
                }
            }
        });
}
