//! Database client desktop app, Tauri Rust side.
//!
//! Postgres only for now: saved connections (`connections`), sessions,
//! introspection and query execution (`pg`), exposed to the React frontend
//! in `ui/` as Tauri commands (see `ui/src/lib/backend.ts`).

mod connections;
mod error;
mod pg;

use connections::{ConnConfig, SavedConnection};
use error::{CmdError, CmdResult};
use pg::{ConnectInfo, Pg, QueryResult, Schema};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

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
    connections::load(&config_dir(&app)?)?
        .into_iter()
        .map(|config| {
            // A keychain that can't be read counts as "no saved password".
            let has_password = connections::password(&config.id).ok().flatten().is_some();
            Ok(SavedConnection {
                config,
                has_password,
            })
        })
        .collect()
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
    if save_password {
        if let Some(p) = password.as_deref() {
            connections::set_password(&config.id, p)?;
        }
    } else {
        connections::delete_password(&config.id)?;
    }
    connections::upsert(&config_dir(&app)?, config)
}

#[tauri::command]
async fn delete_connection(app: AppHandle, pg: State<'_, Pg>, id: String) -> CmdResult<()> {
    pg.disconnect(&id).await;
    connections::remove(&config_dir(&app)?, &id)
}

/// Connects with unsaved settings; returns the server version.
#[tauri::command]
async fn test_connection(config: ConnConfig, password: Option<String>) -> CmdResult<String> {
    let password = match password {
        Some(p) => Some(p),
        None => connections::password(&config.id).ok().flatten(),
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
    let config = connections::get(&config_dir(&app)?, &id)?
        .ok_or_else(|| CmdError::msg("Unknown connection"))?;
    let password = match password {
        Some(p) => Some(p),
        None => connections::password(&id)?,
    };
    pg.connect(config, password).await
}

#[tauri::command]
async fn disconnect(pg: State<'_, Pg>, id: String) -> CmdResult<()> {
    pg.disconnect(&id).await;
    Ok(())
}

#[tauri::command]
async fn introspect(pg: State<'_, Pg>, id: String, database: String) -> CmdResult<Vec<Schema>> {
    pg.introspect(&id, &database).await
}

#[tauri::command]
async fn run_query(
    pg: State<'_, Pg>,
    id: String,
    database: String,
    sql: String,
    limit: usize,
) -> CmdResult<QueryResult> {
    pg.query(&id, &database, &sql, limit.clamp(1, 100_000))
        .await
}

#[tauri::command]
async fn cancel_query(pg: State<'_, Pg>, id: String, database: String) -> CmdResult<()> {
    pg.cancel(&id, &database).await
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
            introspect,
            run_query,
            cancel_query
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
