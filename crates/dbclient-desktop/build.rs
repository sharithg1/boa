//! Tauri build step: generates the app context and the ACL permissions
//! (`allow-<command>`) for every custom command, which
//! `capabilities/default.json` grants to the main window.

/// Must list every `#[tauri::command]` registered in `src/lib.rs`.
const COMMANDS: &[&str] = &[
    "app_info",
    "list_connections",
    "save_connection",
    "delete_connection",
    "test_connection",
    "connect",
    "disconnect",
    "list_schemas",
    "introspect",
    "run_query",
    "cancel_query",
    "set_tx_mode",
    "quit",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri-build failed");
}
