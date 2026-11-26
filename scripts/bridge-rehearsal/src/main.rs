//! Bridge rehearsal: the Tauri app's own updater (tauri-plugin-updater 2.14.1,
//! the version in the Tauri app's Cargo.lock) checks a latest.json, downloads
//! and verifies the GPUI .app.tar.gz, and installs it over a Tauri Boa.app,
//! exactly as an installed Tauri build would ("Restart to update" then runs
//! `open -n` on the bundle). Runs on Tauri's mock runtime: no window.
//!
//! bridge-rehearsal <endpoint> <pubkey-base64> <path/to/Boa.app> <current-version>

use tauri_plugin_updater::UpdaterExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, endpoint, pubkey, bundle, current] = args.as_slice() else {
        return Err("usage: bridge-rehearsal <endpoint> <pubkey> <Boa.app> <current-version>".into());
    };
    let mut ctx = tauri::test::mock_context(tauri::test::noop_assets());
    ctx.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": pubkey,
            "endpoints": [endpoint],
            // The rehearsal feed is http://127.0.0.1; the real one is https.
            "dangerousInsecureTransportProtocol": true,
        }),
    );
    ctx.package_info_mut().version = current.parse()?;
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(ctx)?;
    let exe = std::path::Path::new(bundle.as_str())
        .join("Contents/MacOS/boa-desktop");
    let updater = app
        .updater_builder()
        .executable_path(&exe)
        .build()?;
    let Some(update) = updater.check().await? else {
        println!("tauri updater: no update (current {current})");
        return Ok(());
    };
    println!(
        "tauri updater: {} -> {} from {}",
        update.current_version, update.version, update.download_url
    );
    let bytes = update.download(|_, _| {}, || {}).await?;
    println!("tauri updater: downloaded and verified {} bytes", bytes.len());
    update.install(bytes)?;
    println!("tauri updater: installed over {}", app_path(&exe).display());
    Ok(())
}

fn app_path(exe: &std::path::Path) -> std::path::PathBuf {
    exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).unwrap().to_path_buf()
}
