//! Tauri build step: generates the app context and the ACL permissions
//! (`allow-<command>`) for every custom command, which
//! `capabilities/default.json` grants to the main window.

/// Must list every `#[tauri::command]` registered in `src/lib.rs` (and `src/updater.rs`).
const COMMANDS: &[&str] = &[
    // src/updater.rs
    "app_info",
    "check_for_updates",
    "update_status",
    "install_update",
    // src/lib.rs
    "list_connections",
    "save_connection",
    "delete_connection",
    "test_connection",
    "connect",
    "ping",
    "disconnect",
    "list_schemas",
    "introspect",
    "table_ddl",
    "run_query",
    "cancel_query",
    "export_csv",
    "write_csv_file",
    "reveal_path",
    "apply_edits",
    "history_list",
    "history_append",
    "history_delete",
    "history_clear",
    "set_tx_mode",
    "quit",
];

/// `git` output, trimmed; None on any failure (no git, not a checkout).
fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// The commit (and its date) shown in the About window: `$BOA_COMMIT`
/// (release CI sets it, so a cached build script reruns per commit), else
/// git's HEAD.
fn build_commit() {
    println!("cargo:rerun-if-env-changed=BOA_COMMIT");
    let commit = std::env::var("BOA_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .unwrap_or_default();
    let date = if commit.is_empty() {
        String::new()
    } else {
        git(&["show", "-s", "--format=%cI", &commit]).unwrap_or_default()
    };
    println!("cargo:rustc-env=BOA_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=BOA_BUILD_DATE={date}");
}

fn main() {
    build_commit();
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri-build failed");
}
